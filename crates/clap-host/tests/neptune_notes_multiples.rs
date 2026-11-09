//! Neptune : plusieurs notes (début et fin) à des instants différents dans un même bloc. Le rendu doit être le même quelle que soit
//! la taille de bloc du moteur (référence : bloc de 1 échantillon). Ajouté par la conversation neptune, 2026-11-04.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, default_search_dirs, find_plugins, Bundle};
use daw_engine::{GraphDesc, MidiClip, Note, TrackFormat};
use daw_offline_render::{render, RenderSpec};

const SR: u32 = 48_000;

#[test]
fn neptune_many_notes_same_result_for_every_block_size() {
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(bundle) = Bundle::open(&file) else { continue };
            let bundle = Arc::new(bundle);
            let Ok(list) = bundle.plugins() else { continue };
            for info in list.into_iter().filter(|i| i.vendor == "Le Tournedos Rossini" && ["Neptune", "AnalogSynth", "FM6Op"].contains(&i.name.as_str())) {
                let notes = vec![
                    Note { start: 1_000, len: 3_001, key: 60, velocity: 0.8, channel: 0 },
                    Note { start: 1_037, len: 2_500, key: 64, velocity: 0.6, channel: 0 },
                    Note { start: 1_201, len: 977, key: 67, velocity: 0.9, channel: 0 },
                    Note { start: 2_999, len: 1_113, key: 72, velocity: 0.5, channel: 0 },
                    Note { start: 3_001, len: 4_321, key: 48, velocity: 0.7, channel: 0 },
                    Note { start: 6_400, len: 17, key: 55, velocity: 1.0, channel: 0 },
                ];
                let run = |engine_block: usize| {
                    let mut g = GraphDesc::new();
                    let t = g.add_track("t", TrackFormat::Stereo);
                    g.add_insert(t, &info.name, clap_insert_factory(Arc::clone(&bundle), info.id.clone())).unwrap();
                    g.add_midi_clip(t, MidiClip::new(0, 48_000, Arc::new(notes.clone())));
                    let mut spec = RenderSpec::new(SR, 14_000, g);
                    spec.engine_block = engine_block;
                    render(&spec).unwrap()
                };
                let reference = run(1);
                let level = reference.left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                let mut line = format!("{:<12} niveau {level:.3} ; écart max au bloc de 1 :", info.name);
                for block in [64usize, 256, 1024] {
                    let out = run(block);
                    let diff = reference.left.iter().zip(&out.left).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
                    line.push_str(&format!("  bloc {block} : {diff:.2e}"));
                }
                eprintln!("{line}");
            }
        }
    }
}
