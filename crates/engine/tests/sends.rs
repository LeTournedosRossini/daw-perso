//! Envois : plusieurs destinations (bus et sortie principale), niveau par envoi, avant ou après le fader.

use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, NodeParams, TrackFormat};

const SR: u32 = 48_000;

/// Une piste stéréo qui joue 0,5 constant, avec un fader à 0,5 : sortie 0,25 par la sortie ordinaire.
fn graph() -> (GraphDesc, NodeId, NodeId) {
    let data = AudioData::new(SR, vec![vec![0.5f32; 4_000], vec![0.5f32; 4_000]]).unwrap();
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let bus = g.add_bus("bus");
    g.add_clip(t, Clip::whole(data, 0));
    g.set_params(t, NodeParams { gain: 0.5, ..NodeParams::default() });
    (g, t, bus)
}

/// Joue un bloc après avoir laissé les rampes de lissage se stabiliser ; rend la valeur de sortie (gauche) à la fin.
fn level(g: &GraphDesc, tweak: impl FnOnce(&mut daw_engine::EngineHandle)) -> f32 {
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(g).unwrap();
    engine.process(&mut [0.0; 8], &mut [0.0; 8]);
    handle.send(Command::Play);
    tweak(&mut handle);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..40 {
        engine.process(&mut l, &mut r);
    }
    assert_eq!(l, r);
    l[63]
}

#[test]
fn a_send_to_the_main_output_adds_to_the_track_own_output_with_its_own_level() {
    let (mut g, t, _) = graph();
    assert!((level(&g, |_| {}) - 0.25).abs() < 1e-6, "sans envoi : 0,25 par la sortie ordinaire");
    g.add_send(t, NodeId::MASTER, 1.0);
    assert!((level(&g, |_| {}) - 0.5).abs() < 1e-6, "+ envoi après le fader (0,25) vers la sortie principale");
    g.node_mut(t).unwrap().sends[0].gain = 0.5;
    assert!((level(&g, |_| {}) - 0.375).abs() < 1e-6, "envoi à mi-niveau : 0,25 + 0,125");
    g.validate().expect("l'envoi vers la sortie principale est valide");
}

#[test]
fn several_destinations_at_once_a_bus_and_the_main_output_each_with_their_level() {
    let (mut g, t, bus) = graph();
    g.set_output(t, NodeId::MASTER);
    g.add_send(t, bus, 1.0);
    g.add_send(t, NodeId::MASTER, 0.5);
    // Le bus renvoie vers la sortie principale (gain 1) : 0,25 (direct) + 0,25 (par le bus) + 0,125 (envoi direct à mi-niveau).
    let v = level(&g, |_| {});
    assert!((v - 0.625).abs() < 1e-6, "{v}");
}

#[test]
fn the_send_level_can_be_changed_live() {
    let (mut g, t, _) = graph();
    g.add_send(t, NodeId::MASTER, 0.0);
    let v = level(&g, |h| {
        h.send(Command::SetSendGain { node: t.0, send: 0, gain: 1.0 });
    });
    assert!((v - 0.5).abs() < 1e-6, "{v}");
}

#[test]
fn a_pre_fader_send_ignores_the_fader_and_the_mute_while_a_post_fader_send_follows_them() {
    let (mut g, t, _) = graph();
    g.add_send(t, NodeId::MASTER, 1.0);
    g.node_mut(t).unwrap().sends[0].pre = true;
    // Avant le fader : l'envoi vaut 0,5 (le signal tel qu'il sort des effets), plus 0,25 par la sortie ordinaire.
    assert!((level(&g, |_| {}) - 0.75).abs() < 1e-6);
    // Piste muette : la sortie ordinaire se tait, l'envoi avant fader reste.
    g.set_params(t, NodeParams { gain: 0.5, mute: true, ..NodeParams::default() });
    assert!((level(&g, |_| {}) - 0.5).abs() < 1e-6);
    // Même cas après le fader : tout se tait.
    g.node_mut(t).unwrap().sends[0].pre = false;
    assert!(level(&g, |_| {}).abs() < 1e-6);
}

#[test]
fn sends_to_a_track_or_twice_to_the_same_destination_are_still_refused() {
    let (mut g, t, bus) = graph();
    let t2 = g.add_track("t2", TrackFormat::Stereo);
    let mut h = g.clone();
    h.add_send(t, t2, 1.0);
    assert!(h.validate().is_err());
    let mut h = g.clone();
    h.add_send(t, bus, 1.0);
    h.add_send(t, bus, 1.0);
    assert!(h.validate().is_err());
    let mut h = g.clone();
    h.add_send(t, NodeId::MASTER, 1.0);
    h.add_send(t, NodeId::MASTER, 1.0);
    assert!(h.validate().is_err());
    // Le nœud principal ne peut pas avoir d'envoi.
    let mut h = g.clone();
    h.add_send(NodeId::MASTER, bus, 1.0);
    assert!(h.validate().is_err());
}

/// Deux pistes (0,25 chacune après fader) qui envoient vers le même **retour de monitoring** (sorties 3-4 de la carte). Leur sortie
/// ordinaire va dans un bus muet : on n'écoute que le retour.
fn two_sources() -> (GraphDesc, NodeId, NodeId) {
    let data = AudioData::new(SR, vec![vec![0.5f32; 4_000], vec![0.5f32; 4_000]]).unwrap();
    let mut g = GraphDesc::new();
    let a = g.add_track("a", TrackFormat::Stereo);
    let b = g.add_track("b", TrackFormat::Stereo);
    let bus = g.add_bus("retour");
    g.node_mut(bus).unwrap().phys = Some(daw_engine::PhysOut { first: 2, stereo: true, mono: false });
    g.node_mut(bus).unwrap().monitor = true;
    let trash = g.add_bus("poubelle");
    g.set_params(trash, NodeParams { mute: true, ..NodeParams::default() });
    for t in [a, b] {
        g.add_clip(t, Clip::whole(data.clone(), 0));
        g.set_params(t, NodeParams { gain: 0.5, ..NodeParams::default() });
        g.set_output(t, trash);
        g.add_send(t, bus, 1.0);
    }
    (g, a, bus)
}

/// Comme `level`, mais lu sur le retour (canal 3 de la carte, première sortie du retour).
fn level_ret(g: &GraphDesc, tweak: impl FnOnce(&mut daw_engine::EngineHandle)) -> f32 {
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(g).unwrap();
    let mut bufs = vec![vec![0.0f32; 64]; 4];
    let run = |engine: &mut Engine, bufs: &mut Vec<Vec<f32>>| {
        let mut refs: Vec<&mut [f32]> = bufs.iter_mut().map(|b| b.as_mut_slice()).collect();
        engine.process_outputs(&[], &mut refs);
    };
    run(&mut engine, &mut bufs);
    handle.send(Command::Play);
    tweak(&mut handle);
    for _ in 0..40 {
        run(&mut engine, &mut bufs);
    }
    bufs[2][63]
}

#[test]
fn a_muted_send_is_silent_in_its_destination_and_comes_back() {
    let (g, a, _) = two_sources();
    assert!((level_ret(&g, |_| {}) - 0.5).abs() < 1e-6, "deux sources à 0,25");
    let v = level_ret(&g, |h| {
        h.send(Command::SetSendMute { node: a.0, send: 0, mute: true });
    });
    assert!((v - 0.25).abs() < 1e-6, "{v}");
    let v = level_ret(&g, |h| {
        h.send(Command::SetSendMute { node: a.0, send: 0, mute: true });
        h.send(Command::SetSendMute { node: a.0, send: 0, mute: false });
    });
    assert!((v - 0.5).abs() < 1e-6, "{v}");
}

#[test]
fn a_solo_send_silences_the_other_sends_to_the_same_destination_only() {
    let (g, a, _) = two_sources();
    let v = level_ret(&g, |h| {
        h.send(Command::SetSendSolo { node: a.0, send: 0, solo: true });
    });
    assert!((v - 0.25).abs() < 1e-6, "seule la source en solo : {v}");
    // Les deux en solo : les deux s'entendent.
    let v = level_ret(&g, |h| {
        h.send(Command::SetSendSolo { node: a.0, send: 0, solo: true });
        h.send(Command::SetSendSolo { node: a.0 + 1, send: 0, solo: true });
    });
    assert!((v - 0.5).abs() < 1e-6, "{v}");
    // Un envoi coupé ET en solo reste coupé.
    let v = level_ret(&g, |h| {
        h.send(Command::SetSendSolo { node: a.0, send: 0, solo: true });
        h.send(Command::SetSendMute { node: a.0, send: 0, mute: true });
    });
    assert!(v.abs() < 1e-6, "{v}");
}

#[test]
fn the_flags_in_the_graph_description_apply_from_the_start() {
    let (mut g, a, _) = two_sources();
    g.node_mut(a).unwrap().sends[0].mute = true;
    assert!((level_ret(&g, |_| {}) - 0.25).abs() < 1e-6);
    g.node_mut(a).unwrap().sends[0].mute = false;
    g.node_mut(a).unwrap().sends[0].solo = true;
    assert!((level_ret(&g, |_| {}) - 0.25).abs() < 1e-6);
}

#[test]
fn mute_and_solo_of_a_send_do_nothing_towards_a_bus_that_is_not_a_return() {
    // Le retour qui devient un bus ordinaire (redevenu un bus ordinaire) : ses envois coupés ou en solo ne changent plus rien au mixage
    // principal, mais les réglages sont conservés (ils reviennent si le bus redevient un retour).
    let (mut g, a, bus) = two_sources();
    g.node_mut(bus).unwrap().phys = None;
    g.node_mut(bus).unwrap().monitor = false;
    g.set_output(bus, NodeId::MASTER);
    let plain = level(&g, |_| {});
    g.node_mut(a).unwrap().sends[0].mute = true;
    assert!((level(&g, |_| {}) - plain).abs() < 1e-6, "un envoi coupé vers un bus ordinaire reste entendu");
    g.node_mut(a).unwrap().sends[0].mute = false;
    g.node_mut(a).unwrap().sends[0].solo = true;
    assert!((level(&g, |_| {}) - plain).abs() < 1e-6, "un solo vers un bus ordinaire ne coupe rien");
    assert!(g.node_mut(a).unwrap().sends[0].solo, "le réglage est conservé");
    let mut ret = g.clone();
    ret.node_mut(bus).unwrap().phys = Some(daw_engine::PhysOut { first: 2, stereo: true, mono: false });
    ret.node_mut(bus).unwrap().monitor = true;
    assert!((level_ret(&ret, |_| {}) - 0.25).abs() < 1e-6, "redevenu un retour : le solo agit de nouveau");
}
