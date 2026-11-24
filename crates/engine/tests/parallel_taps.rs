//! Envois pris à un point de la chaîne d'effets d'une piste (piste parallèle) : avant les effets, après l'un d'eux, avant ou après le fader.

use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, GraphError, Insert, InsertFactory, NodeParams, SendDesc, SendTap, TrackFormat};

const SR: u32 = 48_000;

/// Multiplie le signal par `k` et le retarde de `lat` échantillons (annoncés).
struct Scale {
    k: f32,
    lat: usize,
    ring_l: Vec<f32>,
    ring_r: Vec<f32>,
    pos: usize,
}

impl Insert for Scale {
    fn latency(&self) -> u32 {
        self.lat as u32
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        for j in 0..in_l.len() {
            if self.lat == 0 {
                out_l[j] = in_l[j] * self.k;
                out_r[j] = in_r[j] * self.k;
            } else {
                out_l[j] = self.ring_l[self.pos] * self.k;
                out_r[j] = self.ring_r[self.pos] * self.k;
                self.ring_l[self.pos] = in_l[j];
                self.ring_r[self.pos] = in_r[j];
                self.pos = (self.pos + 1) % self.lat;
            }
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {
        self.ring_l.fill(0.0);
        self.ring_r.fill(0.0);
    }
}

fn scale(k: f32, lat: usize) -> InsertFactory {
    Arc::new(move |_, _, _| Ok(Box::new(Scale { k, lat, ring_l: vec![0.0; lat], ring_r: vec![0.0; lat], pos: 0 }) as Box<dyn Insert>))
}

fn constant(v: f32) -> Clip {
    Clip::whole(AudioData::new(SR, vec![vec![v; 30_000], vec![v; 30_000]]).unwrap(), 0)
}

/// Piste T (clip constant 0,1, effets A ×2 puis B ×3), bus parallèle P vers la sortie principale, envoi T → P avec `tap`.
fn rig(tap: SendTap, pre: bool, follow: bool) -> (GraphDesc, NodeId, NodeId, u32, u32) {
    let mut g = GraphDesc::new();
    let t = g.add_track("T", TrackFormat::Stereo);
    let p = g.add_bus("P");
    g.add_clip(t, constant(0.1));
    let a = g.add_insert(t, "A", scale(2.0, 0)).unwrap();
    let b = g.add_insert(t, "B", scale(3.0, 0)).unwrap();
    g.node_mut(t).unwrap().sends.push(SendDesc { to: p, gain: 1.0, pre, mute: false, solo: false, tap, follow });
    (g, t, p, a, b)
}

fn run(g: &GraphDesc, commands: &[Command]) -> f32 {
    let (mut engine, mut handle) = Engine::new(SR, 256);
    handle.load_graph(g).unwrap();
    engine.process(&mut [0.0; 8], &mut [0.0; 8]);
    handle.send(Command::Play);
    for c in commands {
        handle.send(*c);
    }
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..100 {
        engine.process(&mut l, &mut r);
    }
    l[63]
}

fn near(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-5, "{a} au lieu de {b}");
}

#[test]
fn the_parallel_track_takes_the_signal_where_asked() {
    // Chaîne : brut 0,1 → A : 0,2 → B : 0,6 ; fader de T à 1.
    let (g, ..) = rig(SendTap::Auto, true, false);
    near(run(&g, &[]), 0.6 + 0.6); // après tous les effets, avant le fader
    let (g, ..) = rig(SendTap::Input, false, false);
    near(run(&g, &[]), 0.6 + 0.1); // avant les effets
    // Après A seulement : l'identifiant du premier effet est lu dans un graphe identique.
    let (_, _, _, a, _) = rig(SendTap::Input, false, false);
    let (g, ..) = rig(SendTap::AfterInsert(a), false, false);
    near(run(&g, &[]), 0.6 + 0.2); // après A seulement
}

#[test]
fn pre_fader_and_post_fader_differ_by_the_fader() {
    let (g, t, ..) = rig(SendTap::Auto, true, false);
    let half = Command::SetGain { node: t.0, gain: 0.5 };
    near(run(&g, &[half]), 0.3 + 0.6); // le parent est à moitié, la parallèle avant le fader garde tout
    let (g, t, ..) = rig(SendTap::Auto, false, false);
    near(run(&g, &[Command::SetGain { node: t.0, gain: 0.5 }]), 0.3 + 0.3); // après le fader : suit
}

#[test]
fn a_following_parallel_track_is_silenced_with_its_parent() {
    let mute = |t: NodeId| Command::SetMute { node: t.0, mute: true };
    let (g, t, ..) = rig(SendTap::Auto, true, false);
    near(run(&g, &[mute(t)]), 0.6); // un envoi avant fader ordinaire ignore le muet
    let (g, t, ..) = rig(SendTap::Auto, true, true);
    near(run(&g, &[mute(t)]), 0.0); // une parallèle suit
    let (g, t, ..) = rig(SendTap::Input, false, true);
    near(run(&g, &[mute(t)]), 0.0);
}

#[test]
fn a_tap_before_a_slow_effect_stays_aligned_with_the_main_path() {
    let mut g = GraphDesc::new();
    let t = g.add_track("T", TrackFormat::Stereo);
    let p = g.add_bus("P");
    let mut impulse = vec![0.0f32; 4_000];
    impulse[0] = 1.0;
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![impulse.clone(), impulse]).unwrap(), 0));
    g.add_insert(t, "lent", scale(1.0, 100)).unwrap();
    g.node_mut(t).unwrap().sends.push(SendDesc { to: p, gain: 1.0, pre: false, mute: false, solo: false, tap: SendTap::Input, follow: true });
    let (mut engine, mut handle) = Engine::new(SR, 256);
    handle.load_graph(&g).unwrap();
    assert_eq!(handle.latency_samples(), 100);
    engine.process(&mut [0.0; 8], &mut [0.0; 8]);
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 400], vec![0.0f32; 400]);
    engine.process(&mut l, &mut r);
    // Les deux chemins (effet lent et prise avant l'effet) arrivent ensemble, à l'échantillon 100 : 1 + 1.
    for (i, v) in l.iter().enumerate() {
        near(*v, if i == 100 { 2.0 } else { 0.0 });
    }
}

#[test]
fn a_tap_after_an_effect_the_node_does_not_have_is_refused() {
    let (mut g, t, p, ..) = rig(SendTap::Input, false, false);
    g.node_mut(t).unwrap().sends[0].tap = SendTap::AfterInsert(9_999);
    assert_eq!(g.validate(), Err(GraphError::BadSendTap { node: t, to: p }));
    let _ = NodeParams::default();
}
