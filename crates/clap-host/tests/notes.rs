//! Notes : du clip MIDI du moteur jusqu'au plugin CLAP, à l'échantillon près. Plugin de test d'abord, puis
//! les instruments de Jean (éditeur « Le Tournedos Rossini » uniquement).

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, default_search_dirs, find_plugins, Bundle, Plugin};
use daw_engine::{Command, GraphDesc, MidiClip, MidiMsg, Note, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_test_plugin::entry;

const SR: u32 = 48_000;
const ID: &str = "com.daw-perso.test-gain";

fn test_bundle() -> Arc<Bundle> {
    Arc::new(Bundle::from_static_entry(entry()).unwrap())
}

fn note(start: u64, len: u64, key: u8, velocity: f32) -> Note {
    Note { start, len, key, velocity, channel: 0 }
}

#[test]
fn host_detects_the_note_input_port() {
    let b = test_bundle();
    assert_eq!(Plugin::instantiate(&b, ID).unwrap().note_input_ports(), 1);
}

#[test]
fn midi_clip_notes_reach_the_plugin_at_the_exact_sample() {
    let mut g = GraphDesc::new();
    let t = g.add_track("synthé", TrackFormat::Stereo);
    g.add_insert(t, "test", clap_insert_factory(test_bundle(), ID.into())).unwrap();
    // Touche 127 : continu = vélocité. Clip posé à 100 ; première note à 150 pour 200 échantillons (jusqu'à 350),
    // seconde sur la même touche à 200 pour 100 échantillons (jusqu'à 300).
    g.add_midi_clip(t, MidiClip::new(100, 1_000, Arc::new(vec![note(50, 200, 127, 1.0), note(100, 100, 127, 0.5)])));
    // Le continu du plugin de test n'est pas retardé : on regarde la sortie brute (sans rognage de la latence).
    let mut spec = RenderSpec::new(SR, 600, g);
    spec.compensate_latency = false;
    let out = render(&spec).unwrap();
    for (i, v) in out.left.iter().enumerate() {
        let expected = match i {
            150..=199 => 1.0,
            200..=299 => 0.5, // la seconde note remplace la première sur la même touche
            _ => 0.0,
        };
        assert!((v - expected).abs() < 1e-6, "échantillon {i} : {v} au lieu de {expected}");
    }
}

#[test]
fn live_notes_and_block_sizes_do_not_change_the_result() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_insert(t, "test", clap_insert_factory(test_bundle(), ID.into())).unwrap();
    g.add_midi_clip(t, MidiClip::new(0, 2_000, Arc::new(vec![note(30, 400, 100, 0.8), note(700, 50, 64, 1.0), note(701, 300, 20, 0.3)])));
    let mut spec = RenderSpec::new(SR, 2_000, g);
    spec.compensate_latency = false;
    spec.events.push((900, Command::Midi { node: t.0, msg: MidiMsg::NoteOn { channel: 0, key: 127, velocity: 0.25 } }));
    spec.events.push((1_100, Command::Midi { node: t.0, msg: MidiMsg::NoteOff { channel: 0, key: 127, velocity: 0.0 } }));
    let reference = render(&spec).unwrap();
    assert!(reference.peak() > 0.5);
    for (block, engine_block) in [(1, 1), (7, 64), (64, 16), (333, 256), (4096, 4096)] {
        let mut s = spec.clone();
        s.block = block;
        s.engine_block = engine_block;
        assert_eq!(render(&s).unwrap(), reference, "bloc {block}/{engine_block}");
    }
    // La note jouée en direct sort au bon instant (le bloc est coupé à la commande) : +0,25 à 900, retiré à 1 100.
    // (La note de clip de la touche 20 est tenue de 701 à 1 001 : elle ajoute 0,3 × 20 / 127 pendant ce temps.)
    assert!((reference.left[900] - reference.left[899] - 0.25).abs() < 1e-6, "{} puis {}", reference.left[899], reference.left[900]);
    assert!((reference.left[1_100] - reference.left[1_099] + 0.25).abs() < 1e-6);
    assert!(reference.left[1_100].abs() < 1e-6, "plus rien de tenu à 1 100");
}

#[test]
fn loop_wrap_releases_notes_that_would_hang() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_insert(t, "test", clap_insert_factory(test_bundle(), ID.into())).unwrap();
    // Une note de 0 à 1000, mais la boucle s'arrête à 400 : sans coupure, le continu resterait bloqué.
    g.add_midi_clip(t, MidiClip::new(0, 2_000, Arc::new(vec![note(100, 900, 127, 1.0)])));
    let mut spec = RenderSpec::new(SR, 1_000, g);
    spec.compensate_latency = false;
    spec.events.push((0, Command::SetLoop(Some((0, 400)))));
    let out = render(&spec).unwrap();
    // 1er tour : note de 100 à 400 ; au retour (400 = 0) : coupée, puis rejouée à 100 (soit 500 dans la sortie).
    let held = |i: usize| out.left[i].abs() > 0.5;
    // Tour 1 : 0..400, note tenue de 100 à 399. Retour de boucle : coupée à 400, rejouée à 500. Tour 2 : 400..800.
    assert!(!held(99) && held(100) && held(399), "premier tour");
    assert!(!held(400) && !held(499), "coupée au retour de boucle (sinon elle resterait bloquée)");
    assert!(held(500) && held(799), "rejouée au tour suivant");
    assert!(!held(800), "coupée de nouveau");
}

/// Plugins de Jean seulement : chaque instrument reçoit une note de 0,5 s ; il doit rester muet avant la note,
/// sonner pendant, et rester fini (pas de NaN).
#[test]
fn users_instruments_play_a_midi_clip() {
    let mut tested = 0;
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(bundle) = Bundle::open(&file) else { continue };
            let bundle = Arc::new(bundle);
            let Ok(list) = bundle.plugins() else { continue };
            for info in list.into_iter().filter(|i| i.vendor == "Le Tournedos Rossini" && i.features.iter().any(|f| f == "instrument")) {
                let mut g = GraphDesc::new();
                let t = g.add_track("t", TrackFormat::Stereo);
                g.add_insert(t, &info.name, clap_insert_factory(Arc::clone(&bundle), info.id.clone())).unwrap();
                let start = 4_800u64;
                g.add_midi_clip(t, MidiClip::new(0, 48_000, Arc::new(vec![Note { start, len: 24_000, key: 60, velocity: 0.8, channel: 0 }])));
                let out = render(&RenderSpec::new(SR, 72_000, g)).unwrap();
                assert!(out.left.iter().chain(&out.right).all(|v| v.is_finite()), "{} : valeur non finie", info.name);
                let before = out.left[..start as usize].iter().chain(&out.right[..start as usize]).fold(0.0f32, |m, v| m.max(v.abs()));
                let during = out.left[start as usize..28_800].iter().chain(&out.right[start as usize..28_800]).fold(0.0f32, |m, v| m.max(v.abs()));
                let tail = out.left[60_000..].iter().chain(&out.right[60_000..]).fold(0.0f32, |m, v| m.max(v.abs()));
                eprintln!("{:<10} avant {before:.5}  pendant {during:.4}  queue {tail:.5}", info.name);
                // Neptune applique les débuts de note au début du bloc (voir CLAUDE.md) : on ne vérifie le silence avant la note
                // que pour les instruments qui respectent l'instant de l'événement.
                if info.name != "Neptune" {
                    assert!(before < 1e-4, "{} : du son avant la note ({before})", info.name);
                }
                // Seuls les synthétiseurs jouent seuls : Neptune est un échantillonneur sans échantillon chargé, et Diapason
                // un diapason commandé par un paramètre, pas par les notes.
                if ["AnalogSynth", "FM6Op", "Orgamon"].contains(&info.name.as_str()) {
                    assert!(during > 1e-3, "{} : muet pendant la note ({during})", info.name);
                    assert!(tail < during * 0.5, "{} : la note ne s'éteint pas (queue {tail}, pendant {during})", info.name);
                }
                tested += 1;
            }
        }
    }
    eprintln!("{tested} instrument(s) de Jean testé(s)");
}

/// Instant d'attaque des instruments de Jean selon la taille de bloc : AnalogSynth et FM6Op doivent démarrer au même
/// échantillon quel que soit le bloc (événements honorés à l'échantillon près). Les autres sont seulement rapportés.
#[test]
fn users_instruments_onset_versus_block_size() {
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(bundle) = Bundle::open(&file) else { continue };
            let bundle = Arc::new(bundle);
            let Ok(list) = bundle.plugins() else { continue };
            for info in list.into_iter().filter(|i| i.vendor == "Le Tournedos Rossini" && i.features.iter().any(|f| f == "instrument")) {
                let onsets: Vec<Option<usize>> = [1usize, 64, 256, 1024]
                    .iter()
                    .map(|&engine_block| {
                        let mut g = GraphDesc::new();
                        let t = g.add_track("t", TrackFormat::Stereo);
                        g.add_insert(t, &info.name, clap_insert_factory(Arc::clone(&bundle), info.id.clone())).unwrap();
                        g.add_midi_clip(t, MidiClip::new(0, 48_000, Arc::new(vec![Note { start: 4_800, len: 24_000, key: 60, velocity: 0.8, channel: 0 }])));
                        let mut spec = RenderSpec::new(SR, 12_000, g);
                        spec.engine_block = engine_block;
                        let out = render(&spec).unwrap();
                        out.left.iter().zip(&out.right).position(|(l, r)| l.abs() > 1e-6 || r.abs() > 1e-6)
                    })
                    .collect();
                eprintln!("{:<12} attaque (note à 4800) pour des blocs de 1/64/256/1024 : {onsets:?}", info.name);
                if ["AnalogSynth", "FM6Op"].contains(&info.name.as_str()) {
                    assert!(onsets.iter().all(|o| *o == onsets[0] && o.is_some_and(|i| (4_800..4_900).contains(&i))), "{} : {onsets:?}", info.name);
                }
            }
        }
    }
}
