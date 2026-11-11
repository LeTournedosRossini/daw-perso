//! Sorties auxiliaires d'un plugin multi-sorties : elles suivent le muet et le solo de leur piste (contrairement à un envoi « avant fader »
//! ordinaire, qui les ignore volontairement).

use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, Insert, InsertFactory, NodeParams, TrackFormat};

const SR: u32 = 48_000;

/// Sortie principale constante 0,5 ; port auxiliaire 0 constant 0,25.
struct Multi {
    aux: Vec<f32>,
    n: usize,
}

impl Insert for Multi {
    fn latency(&self) -> u32 {
        0
    }
    fn aux_ports(&self) -> usize {
        1
    }
    fn aux_output(&self, port: usize, n: usize) -> Option<(&[f32], &[f32])> {
        (port == 0).then(|| (&self.aux[..n], &self.aux[..n]))
    }
    fn process(&mut self, in_l: &[f32], _in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        self.n = in_l.len();
        out_l.fill(0.5);
        out_r.fill(0.5);
        self.aux[..self.n].fill(0.25);
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn multi() -> InsertFactory {
    Arc::new(|_, max_block, _| Ok(Box::new(Multi { aux: vec![0.0; max_block], n: 0 }) as Box<dyn Insert>))
}

fn constant(v: f32) -> Clip {
    Clip::whole(AudioData::new(SR, vec![vec![v; 20_000], vec![v; 20_000]]).unwrap(), 0)
}

/// M (plugin multi-sorties, aux 0 → bus X), O (une autre piste, 0,1 constant), X (bus vers la sortie principale).
fn rig() -> (GraphDesc, NodeId, NodeId) {
    let mut g = GraphDesc::new();
    let m = g.add_track("M", TrackFormat::Stereo);
    let o = g.add_track("O", TrackFormat::Stereo);
    let x = g.add_bus("X");
    g.add_clip(m, constant(0.0));
    g.add_clip(o, constant(0.1));
    let ins = g.add_insert(m, "multi", multi()).unwrap();
    g.node_mut(m).unwrap().inserts.iter_mut().find(|i| i.id == ins).unwrap().aux = vec![Some(x)];
    g.set_params(x, NodeParams::default());
    (g, m, o)
}

/// Niveau de la sortie principale après les commandes, une fois les rampes terminées.
fn level(commands: &[Command]) -> f32 {
    let (g, _, _) = rig();
    let (mut engine, mut handle) = Engine::new(SR, 256);
    handle.load_graph(&g).unwrap();
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

#[test]
fn without_mute_or_solo_everything_is_heard() {
    // principale de M 0,5 + sortie auxiliaire 0,25 (par le bus) + O 0,1
    assert!((level(&[]) - 0.85).abs() < 1e-5, "{}", level(&[]));
}

#[test]
fn muting_the_track_mutes_its_auxiliary_outputs_too() {
    let v = level(&[Command::SetMute { node: 1, mute: true }]);
    assert!((v - 0.1).abs() < 1e-5, "seul O doit rester : {v}");
}

#[test]
fn soloing_another_track_silences_the_auxiliary_outputs_of_the_excluded_track() {
    let v = level(&[Command::SetSolo { node: 2, solo: true }]);
    assert!((v - 0.1).abs() < 1e-5, "seul O (en solo) doit rester : {v}");
}

#[test]
fn soloing_the_multi_output_track_keeps_its_outputs_and_excludes_the_others() {
    let v = level(&[Command::SetSolo { node: 1, solo: true }]);
    assert!((v - 0.75).abs() < 1e-5, "M et ses sorties, sans O : {v}");
}

#[test]
fn leaving_the_solo_brings_everything_back() {
    let v = level(&[Command::SetSolo { node: 2, solo: true }, Command::SetSolo { node: 2, solo: false }]);
    assert!((v - 0.85).abs() < 1e-5, "{v}");
}
