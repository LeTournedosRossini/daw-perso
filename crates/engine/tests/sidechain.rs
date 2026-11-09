//! Chaîne latérale des effets : le signal d'un autre nœud (avant son fader) alimente le second port d'entrée de l'effet.

use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, GraphError, Insert, InsertFactory, NodeParams, Pool, TrackFormat};

const SR: u32 = 48_000;

/// Effet de test à chaîne latérale : coupe l'entrée à tout échantillon où le signal latéral dépasse 0,5 (un « ducker » parfait).
struct Ducker {
    sc_l: Vec<f32>,
    len: usize,
}

impl Insert for Ducker {
    fn latency(&self) -> u32 {
        0
    }
    fn has_sidechain(&self) -> bool {
        true
    }
    fn set_sidechain(&mut self, l: &[f32], _r: &[f32]) {
        self.sc_l[..l.len()].copy_from_slice(l);
        self.len = l.len();
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        for j in 0..in_l.len() {
            let ducked = j < self.len && self.sc_l[j].abs() > 0.5;
            out_l[j] = if ducked { 0.0 } else { in_l[j] };
            out_r[j] = if ducked { 0.0 } else { in_r[j] };
        }
        self.len = 0;
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn ducker() -> InsertFactory {
    Arc::new(|_, max_block, _| Ok(Box::new(Ducker { sc_l: vec![0.0; max_block], len: 0 }) as Box<dyn Insert>))
}

/// Retard pur annoncé comme latence.
struct Delay {
    ring: Vec<f32>,
    pos: usize,
}

impl Insert for Delay {
    fn latency(&self) -> u32 {
        self.ring.len() as u32
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        for j in 0..in_l.len() {
            out_l[j] = self.ring[self.pos];
            out_r[j] = self.ring[self.pos];
            self.ring[self.pos] = 0.5 * (in_l[j] + in_r[j]);
            self.pos = (self.pos + 1) % self.ring.len();
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {
        self.ring.fill(0.0);
    }
}

fn delay(n: usize) -> InsertFactory {
    Arc::new(move |_, _, _| Ok(Box::new(Delay { ring: vec![0.0; n], pos: 0 }) as Box<dyn Insert>))
}

fn clip(samples: Vec<f32>) -> Clip {
    Clip::whole(AudioData::new(SR, vec![samples.clone(), samples]).unwrap(), 0)
}

/// A joue une impulsion de 100 échantillons (1,0 de 1 000 à 1 099), muette dans le mixage (sortie dans un bus muet) ; B joue 0,5 constant
/// et a un ducker dont la chaîne latérale est A. Rend (graphe, A, B).
fn rig(a_latency: usize, b_latency_before_ducker: usize) -> (GraphDesc, NodeId, NodeId) {
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    let b = g.add_track("B", TrackFormat::Stereo);
    let trash = g.add_bus("poubelle");
    g.set_params(trash, NodeParams { mute: true, ..NodeParams::default() });
    g.set_output(a, trash);
    let mut pulse = vec![0.0f32; 4_000];
    pulse[1_000..1_100].fill(1.0);
    g.add_clip(a, clip(pulse));
    g.add_clip(b, clip(vec![0.5f32; 4_000]));
    if a_latency > 0 {
        g.add_insert(a, "retard A", delay(a_latency)).unwrap();
    }
    if b_latency_before_ducker > 0 {
        g.add_insert(b, "retard B", delay(b_latency_before_ducker)).unwrap();
    }
    let d = g.add_insert(b, "ducker", ducker()).unwrap();
    g.node_mut(b).unwrap().inserts.iter_mut().find(|i| i.id == d).unwrap().sidechain = Some(a);
    (g, a, b)
}

fn render(g: &GraphDesc, block: usize, pool: Option<Pool>) -> Vec<f32> {
    let (mut engine, mut handle) = Engine::new(SR, 256);
    if let Some(p) = pool {
        engine.set_pool(p);
    }
    handle.load_graph(g).unwrap();
    engine.process(&mut [0.0; 8], &mut [0.0; 8]);
    handle.send(Command::Play);
    let mut out = Vec::new();
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    while out.len() < 3_000 {
        engine.process(&mut l, &mut r);
        out.extend_from_slice(&l);
    }
    out
}

#[test]
fn the_side_chain_ducks_the_signal_at_the_exact_samples_whatever_the_block_size() {
    let (g, _, _) = rig(0, 0);
    for block in [1usize, 64, 100, 333] {
        let out = render(&g, block, None);
        assert!(out[..1_000].iter().all(|&v| (v - 0.5).abs() < 1e-6), "avant l'impulsion, bloc {block}");
        assert!(out[1_000..1_100].iter().all(|&v| v == 0.0), "pendant l'impulsion, bloc {block}");
        assert!(out[1_100..3_000].iter().all(|&v| (v - 0.5).abs() < 1e-6), "après l'impulsion, bloc {block}");
    }
}

#[test]
fn the_side_chain_is_aligned_with_the_main_signal_at_the_effect() {
    // B retarde son signal de 100 avant le ducker : la chaîne latérale (A, sans latence) est retardée d'autant pour rester alignée. Le signal
    // de B n'existe qu'à partir de l'échantillon 100 (le retard) et le trou suit l'impulsion décalée de 100.
    let (g, _, _) = rig(0, 100);
    let out = render(&g, 64, None);
    assert!(out[..100].iter().all(|&v| v == 0.0), "le retard pur n'a encore rien à rendre");
    assert!(out[100..1_100].iter().all(|&v| (v - 0.5).abs() < 1e-6), "le signal de B passe jusqu'à l'impulsion décalée");
    assert!(out[1_100..1_200].iter().all(|&v| v == 0.0), "le trou suit le signal de B (retardé de 100)");
    assert!(out[1_200..3_000].iter().all(|&v| (v - 0.5).abs() < 1e-6));
}

#[test]
fn a_source_slower_than_the_effect_cannot_be_advanced_so_the_gap_comes_late_by_the_difference() {
    // A a 100 échantillons de latence : son signal arrive 100 en retard au ducker de B (pas de latence). On ne peut pas l'avancer : le
    // trou arrive 100 échantillons après l'endroit où il aurait dû être (la compensation de latence retarde aussi toute la sortie de 100).
    let (g, _, _) = rig(100, 0);
    let out = render(&g, 64, None);
    assert!(out[..100].iter().all(|&v| v == 0.0), "toute la sortie est retardée de 100 par la compensation de A");
    assert!(out[100..1_200].iter().all(|&v| (v - 0.5).abs() < 1e-6));
    assert!(out[1_200..1_300].iter().all(|&v| v == 0.0), "trou à 1 000 + 100 (latence de A) + 100 (compensation) ; sans retard il serait à 1 100");
    assert!(out[1_300..3_000].iter().all(|&v| (v - 0.5).abs() < 1e-6));
}

#[test]
fn the_result_is_the_same_bit_for_bit_with_several_threads() {
    let (g, _, _) = rig(0, 0);
    let one = render(&g, 64, None);
    for threads in [2usize, 3] {
        assert_eq!(render(&g, 64, Some(Pool::new(threads, None))), one, "{threads} fils");
    }
}

#[test]
fn invalid_side_chains_and_loops_are_refused() {
    let (mut g, a, b) = rig(0, 0);
    // Chaîne latérale vers la sortie principale ou vers soi-même.
    let id = g.nodes[b.0 as usize].inserts.last().unwrap().id;
    g.node_mut(b).unwrap().inserts.last_mut().unwrap().sidechain = Some(NodeId::MASTER);
    assert!(matches!(g.validate(), Err(GraphError::BadSidechain { .. })));
    g.node_mut(b).unwrap().inserts.last_mut().unwrap().sidechain = Some(b);
    assert!(matches!(g.validate(), Err(GraphError::BadSidechain { node, insert }) if node == b && insert == id));
    // Une boucle : A dépend de B (chaîne latérale) alors que B dépend de A.
    g.node_mut(b).unwrap().inserts.last_mut().unwrap().sidechain = Some(a);
    assert!(g.validate().is_ok());
    let d = g.add_insert(a, "ducker 2", ducker()).unwrap();
    g.node_mut(a).unwrap().inserts.iter_mut().find(|i| i.id == d).unwrap().sidechain = Some(b);
    assert!(matches!(g.validate(), Err(GraphError::Cycle(_))));
    // Une source inconnue.
    g.node_mut(a).unwrap().inserts.iter_mut().find(|i| i.id == d).unwrap().sidechain = Some(NodeId(99));
    assert!(g.validate().is_err());
}

#[test]
fn the_source_signal_is_taken_before_its_fader_so_muting_it_does_not_silence_the_side_chain() {
    let (mut g, a, _) = rig(0, 0);
    g.set_params(a, NodeParams { mute: true, gain: 0.0, ..NodeParams::default() });
    let out = render(&g, 64, None);
    assert!(out[1_000..1_100].iter().all(|&v| v == 0.0), "le déclencheur fonctionne même fader à zéro et piste coupée");
}
