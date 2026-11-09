//! Les instruments de Jean réagissent-ils à la molette de hauteur, à la modulation, à la pédale et à la pression ?
//! Le test mesure et rapporte (voir `--nocapture`) ; il n'exige que ce qui est sûr : pas de valeur non finie, et que
//! les messages soient bien livrés sans perdre de note.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, default_search_dirs, find_plugins, Bundle};
use daw_engine::{CtlEvent, GraphDesc, MidiClip, MidiMsg, Note, TrackFormat};
use daw_offline_render::{render, RenderSpec};

const SR: u32 = 48_000;

/// Fréquence dominante estimée par comptage des passages par zéro sur `[a, b[`.
fn zero_cross_hz(x: &[f32], a: usize, b: usize) -> f64 {
    let mut n = 0;
    for i in a + 1..b {
        if (x[i - 1] <= 0.0) != (x[i] <= 0.0) {
            n += 1;
        }
    }
    n as f64 / 2.0 / ((b - a) as f64 / SR as f64)
}

#[test]
fn report_how_jeans_instruments_react_to_controllers() {
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(bundle) = Bundle::open(&file) else { continue };
            let bundle = Arc::new(bundle);
            let Ok(list) = bundle.plugins() else { continue };
            for info in list.into_iter().filter(|i| i.vendor == "Le Tournedos Rossini" && ["AnalogSynth", "FM6Op", "Orgamon"].contains(&i.name.as_str())) {
                let run = |events: Vec<CtlEvent>| {
                    let mut g = GraphDesc::new();
                    let t = g.add_track("t", TrackFormat::Stereo);
                    g.add_insert(t, &info.name, clap_insert_factory(Arc::clone(&bundle), info.id.clone())).unwrap();
                    let mut c = MidiClip::new(0, 40_000, Arc::new(vec![Note { start: 0, len: 30_000, key: 57, velocity: 0.8, channel: 0 }]));
                    c.events = Arc::new(events);
                    g.add_midi_clip(t, c);
                    render(&RenderSpec::new(SR, 30_000, g)).unwrap().left
                };
                let plain = run(vec![]);
                let bent = run(vec![CtlEvent { start: 10_000, msg: MidiMsg::PitchBend { channel: 0, value: 8191 } }]);
                let modul = run(vec![CtlEvent { start: 0, msg: MidiMsg::ControlChange { channel: 0, controller: 1, value: 127 } }]);
                let press = run(vec![CtlEvent { start: 0, msg: MidiMsg::ChannelPressure { channel: 0, value: 127 } }]);
                assert!(plain.iter().chain(&bent).chain(&modul).chain(&press).all(|v| v.is_finite()), "{} : valeur non finie", info.name);
                assert!(plain.iter().fold(0.0f32, |m, v| m.max(v.abs())) > 0.001, "{} : la note ne sonne plus", info.name);
                let diff = |a: &[f32], b: &[f32]| a.iter().zip(b).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
                eprintln!(
                    "{:<12} fréquence avant/après molette max : {:.1} → {:.1} Hz ; écart max : molette {:.3}, modulation {:.3}, pression {:.3}",
                    info.name,
                    zero_cross_hz(&plain, 20_000, 28_000),
                    zero_cross_hz(&bent, 20_000, 28_000),
                    diff(&plain, &bent),
                    diff(&plain, &modul),
                    diff(&plain, &press),
                );
            }
        }
    }
}

/// La pédale de maintien (contrôleur 64) prolonge-t-elle la note après sa fin ? On compare l'énergie après le relâchement
/// de la touche avec et sans pédale enfoncée avant la note.
#[test]
fn report_whether_jeans_instruments_sustain_with_the_pedal() {
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(bundle) = Bundle::open(&file) else { continue };
            let bundle = Arc::new(bundle);
            let Ok(list) = bundle.plugins() else { continue };
            for info in list.into_iter().filter(|i| i.vendor == "Le Tournedos Rossini" && ["AnalogSynth", "FM6Op", "Orgamon", "Diapason", "Neptune"].contains(&i.name.as_str())) {
                let run = |pedal: bool| {
                    let mut g = GraphDesc::new();
                    let t = g.add_track("t", TrackFormat::Stereo);
                    g.add_insert(t, &info.name, clap_insert_factory(Arc::clone(&bundle), info.id.clone())).unwrap();
                    // Note de 8 000 échantillons (0,17 s), puis 40 000 échantillons de silence : la queue d'un synthé sans
                    // pédale s'éteint vite ; avec une pédale prise en compte, la note continue jusqu'au relâchement.
                    let mut c = MidiClip::new(0, 60_000, Arc::new(vec![Note { start: 100, len: 8_000, key: 57, velocity: 0.8, channel: 0 }]));
                    let mut ev = vec![];
                    if pedal {
                        ev.push(CtlEvent { start: 0, msg: MidiMsg::ControlChange { channel: 0, controller: 64, value: 127 } });
                        ev.push(CtlEvent { start: 40_000, msg: MidiMsg::ControlChange { channel: 0, controller: 64, value: 0 } });
                    }
                    c.events = Arc::new(ev);
                    g.add_midi_clip(t, c);
                    render(&RenderSpec::new(SR, 60_000, g)).unwrap().left
                };
                let (without, with) = (run(false), run(true));
                let energy = |x: &[f32], a: usize, b: usize| (x[a..b].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / (b - a) as f64).sqrt();
                eprintln!(
                    "{:<12} niveau RMS entre 0,4 s et 0,8 s (la touche est relâchée à 0,17 s) : sans pédale {:.5}, avec pédale {:.5}",
                    info.name,
                    energy(&without, 19_000, 38_000),
                    energy(&with, 19_000, 38_000)
                );
            }
        }
    }
}
