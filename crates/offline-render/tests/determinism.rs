//! Déterminisme : le résultat ne dépend ni de la taille de bloc ni du nombre d'exécutions.

mod common;
use common::*;
use daw_core_types::NodeId;
use daw_engine::{Command, GraphDesc, NodeParams, TrackFormat};
use daw_offline_render::{render, RenderSpec};

/// Projet de référence : 3 pistes (mono/stéréo), 2 bus chaînés, un départ d'effet, du mouvement de paramètres,
/// une boucle. N'utilise que des opérations arithmétiques exactes (+, ×, ÷, sqrt) : l'empreinte est la même partout.
fn reference_project() -> RenderSpec {
    let frames = 30_000;
    let mut g = GraphDesc::new();
    let drums = g.add_track("batterie", TrackFormat::Stereo);
    let bass = g.add_track("basse", TrackFormat::Mono);
    let gtr = g.add_track("guitare", TrackFormat::Mono);
    let sub = g.add_bus("sous-groupe");
    let fx = g.add_bus("effet");
    g.set_output(drums, sub);
    g.set_output(bass, sub);
    g.set_output(sub, NodeId::MASTER);
    g.set_output(fx, NodeId::MASTER);
    g.add_send(gtr, fx, 0.4);
    g.add_send(drums, fx, 0.25);
    g.add_clip(drums, clip(&stereo(noise(100, 20_000), noise(101, 20_000)), 0));
    g.add_clip(bass, clip(&mono(noise(102, 25_000)), 1_500));
    g.add_clip(gtr, clip(&mono(noise(103, 12_000)), 5_000));
    g.add_clip(gtr, clip(&mono(noise(104, 9_000)), 14_321));
    g.set_params(bass, NodeParams { gain: 0.8, pan: -0.3, ..Default::default() });
    g.set_params(gtr, NodeParams { gain: 0.6, pan: 0.45, ..Default::default() });
    g.node_mut(sub).unwrap().params.gain = 0.9;

    let mut spec = RenderSpec::new(SR, frames, g);
    spec.events.extend([
        (0, Command::SetLoop(Some((2_000, 18_000)))),
        (3_000, Command::SetGain { node: bass.0, gain: 0.25 }),
        (4_321, Command::SetPan { node: gtr.0, pan: -0.8 }),
        (7_777, Command::SetSendGain { node: gtr.0, send: 0, gain: 0.9 }),
        (9_001, Command::SetMute { node: drums.0, mute: true }),
        (12_345, Command::SetMute { node: drums.0, mute: false }),
        (15_000, Command::SetSolo { node: gtr.0, solo: true }),
        (20_000, Command::SetSolo { node: gtr.0, solo: false }),
        (22_222, Command::Seek(100)),
        (25_000, Command::SetLoop(None)),
    ]);
    spec
}

#[test]
fn identical_runs_are_bit_identical() {
    let spec = reference_project();
    assert_eq!(render(&spec).unwrap(), render(&spec).unwrap());
}

#[test]
fn result_does_not_depend_on_block_sizes() {
    let reference = render(&reference_project()).unwrap();
    assert!(reference.peak() > 0.1, "le projet de référence doit produire du signal");
    for block in [1usize, 7, 64, 333, 1024, 4096, 50_000] {
        for engine_block in [1usize, 64, 256, 4096] {
            let mut spec = reference_project();
            spec.block = block;
            spec.engine_block = engine_block;
            let out = render(&spec).unwrap();
            assert_eq!(out.hash(), reference.hash(), "bloc {block}, bloc moteur {engine_block}");
            assert_eq!(out, reference);
        }
    }
}

/// Empreinte de référence ("golden"). Si elle change, le son rendu a changé : vérifier que c'est voulu
/// avant de la mettre à jour.
const GOLDEN_HASH: u64 = 0xe3e2010b698a3c6c;

#[test]
fn reference_project_matches_golden_hash() {
    let h = render(&reference_project()).unwrap().hash();
    if GOLDEN_HASH == 0 {
        panic!("empreinte de référence non renseignée : {h:#018x}");
    }
    assert_eq!(h, GOLDEN_HASH, "le rendu de référence a changé : {h:#018x}");
}
