//! Une note qui sonne quand on déplace la tête de lecture (`Seek`) ne doit pas rester bloquée dans le plugin : le moteur
//! oublie son suivi des notes au `Seek`, donc si le plugin ne coupe pas ses voix de lui-même, plus rien ne les arrêterait.
//! Le test mesure sur les instruments de Jean (éditeur « Le Tournedos Rossini ») et exige le silence après quelques secondes.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, default_search_dirs, find_plugins, Bundle};
use daw_engine::{Command, Engine, GraphDesc, MidiClip, Note, TrackFormat};

const SR: u32 = 48_000;
const BLOCK: usize = 256;

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// Joue une note longue, applique `then` en plein milieu, laisse tourner 4 s, et rend le niveau des 0,2 dernières secondes.
fn tail_level(bundle: &Arc<Bundle>, id: &str, name: &str, then: impl Fn(&mut daw_engine::EngineHandle)) -> f32 {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_insert(t, name, clap_insert_factory(Arc::clone(bundle), id.to_string())).unwrap();
    g.add_midi_clip(t, MidiClip::new(0, 400_000, Arc::new(vec![Note { start: 0, len: 380_000, key: 57, velocity: 0.8, channel: 0 }])));
    let (mut engine, mut handle) = Engine::new(SR, BLOCK);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let mut run = |engine: &mut Engine, samples: usize, keep: &mut Vec<f32>| {
        for _ in 0..samples / BLOCK {
            engine.process(&mut l, &mut r);
            keep.extend_from_slice(&l);
        }
    };
    let mut sink = Vec::new();
    run(&mut engine, 24_000, &mut sink);
    assert!(rms(&sink[12_000..]) > 1e-3, "{name} : la note devrait sonner avant le déplacement");
    then(&mut handle);
    let mut after = Vec::new();
    run(&mut engine, 4 * SR as usize, &mut after);
    rms(&after[after.len() - 9_600..])
}

#[test]
fn a_seek_while_notes_sound_leaves_no_stuck_voice_in_jeans_instruments() {
    let mut tested = 0;
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(bundle) = Bundle::open(&file) else { continue };
            let bundle = Arc::new(bundle);
            let Ok(list) = bundle.plugins() else { continue };
            for info in list.into_iter().filter(|i| i.vendor == "Le Tournedos Rossini" && ["AnalogSynth", "FM6Op", "Orgamon"].contains(&i.name.as_str())) {
                // Seek vers une zone sans note (après le clip) : tout ce qui sonne encore est une voix bloquée.
                let level = tail_level(&bundle, &info.id, &info.name, |h| {
                    h.send(Command::Seek(900_000));
                });
                eprintln!("{:<12} niveau après Seek puis 4 s : {level:.5}", info.name);
                assert!(level < 1e-4, "{} : voix bloquée après un déplacement (niveau {level})", info.name);
                tested += 1;
            }
        }
    }
    if tested == 0 {
        eprintln!("aucun instrument de Jean trouvé : test ignoré");
    }
}
