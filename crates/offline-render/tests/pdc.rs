//! Compensation de latence (PDC) et effets insérés.

mod common;
use common::*;
use daw_core_types::NodeId;
use daw_engine::{Command, Engine, GraphDesc, GraphError, Insert, LoadError, TrackFormat};
use daw_offline_render::{render, RenderSpec, Rendered};

const LEN: usize = 600;

fn peaks(r: &Rendered) -> Vec<(usize, f32)> {
    r.left.iter().enumerate().filter(|(_, v)| v.abs() > 1e-6).map(|(i, v)| (i, *v)).collect()
}

fn spec(g: GraphDesc, compensate: bool) -> RenderSpec {
    let mut s = RenderSpec::new(SR, LEN as u64, g);
    s.compensate_latency = compensate;
    s
}

/// A : impulsion + effet de 100 échantillons ; B : impulsion seule. Les deux vont à la sortie principale.
fn two_tracks(pdc: bool) -> GraphDesc {
    let mut g = GraphDesc::new();
    g.pdc = pdc;
    let a = impulse_track(&mut g, "a", 0, LEN);
    impulse_track(&mut g, "b", 0, LEN);
    g.add_insert(a, "retard", delay_factory(100)).unwrap();
    g
}

#[test]
fn without_pdc_each_path_keeps_its_own_latency() {
    let out = render(&spec(two_tracks(false), false)).unwrap();
    assert_eq!(peaks(&out), vec![(0, 1.0), (100, 1.0)]);
}

#[test]
fn with_pdc_paths_are_aligned_at_the_total_latency() {
    let out = render(&spec(two_tracks(true), false)).unwrap();
    assert_eq!(peaks(&out), vec![(100, 2.0)], "les deux impulsions tombent ensemble, à la latence totale");
}

#[test]
fn compensated_render_is_aligned_on_the_timeline() {
    let out = render(&spec(two_tracks(true), true)).unwrap();
    assert_eq!(out.left.len(), LEN);
    assert_eq!(peaks(&out), vec![(0, 2.0)]);
}

#[test]
fn reports_total_latency() {
    let g = two_tracks(true);
    assert_eq!(g.compile(SR, 64).unwrap().latency(), 100);
    // Sans effet : 0. Latence = celle du chemin le plus lent, pas la somme.
    assert_eq!(GraphDesc::new().compile(SR, 64).unwrap().latency(), 0);
    let mut h = two_tracks(true);
    let c = h.add_track("c", TrackFormat::Mono);
    h.add_insert(c, "retard 30", delay_factory(30)).unwrap();
    assert_eq!(h.compile(SR, 64).unwrap().latency(), 100);
}

#[test]
fn inserts_in_series_add_up() {
    let mut g = GraphDesc::new();
    let a = impulse_track(&mut g, "a", 0, LEN);
    impulse_track(&mut g, "b", 0, LEN);
    g.add_insert(a, "x", delay_factory(30)).unwrap();
    g.add_insert(a, "y", delay_factory(20)).unwrap();
    assert_eq!(peaks(&render(&spec(g, false)).unwrap()), vec![(50, 2.0)]);
}

#[test]
fn insert_on_a_bus_aligns_with_a_direct_track() {
    let mut g = GraphDesc::new();
    let bus = g.add_bus("bus");
    g.add_insert(bus, "retard", delay_factory(40)).unwrap();
    let a = impulse_track(&mut g, "a", 0, LEN);
    g.set_output(a, bus);
    impulse_track(&mut g, "direct", 0, LEN);
    assert_eq!(peaks(&render(&spec(g, false)).unwrap()), vec![(40, 2.0)]);
}

#[test]
fn sends_are_compensated_too() {
    // A : effet de 64 ; C : sans effet, mais envoie vers un bus « fx » qui a un effet de 30.
    let mut g = GraphDesc::new();
    let a = impulse_track(&mut g, "a", 0, LEN);
    g.add_insert(a, "a", delay_factory(64)).unwrap();
    let c = impulse_track(&mut g, "c", 0, LEN);
    let fx = g.add_bus("fx");
    g.add_insert(fx, "fx", delay_factory(30)).unwrap();
    g.add_send(c, fx, 0.5);
    // Tout doit tomber à la même position (64) : A (1) + C direct (1) + C via fx (0,5).
    assert_eq!(peaks(&render(&spec(g.clone(), false)).unwrap()), vec![(64, 2.5)]);
    assert_eq!(peaks(&render(&spec(g, true)).unwrap()), vec![(0, 2.5)]);
}

#[test]
fn pdc_is_independent_of_block_sizes() {
    let reference = render(&spec(two_tracks(true), true)).unwrap();
    for (block, engine_block) in [(1, 1), (7, 64), (64, 16), (500, 256)] {
        let mut s = spec(two_tracks(true), true);
        s.block = block;
        s.engine_block = engine_block;
        assert_eq!(render(&s).unwrap(), reference, "bloc {block}/{engine_block}");
    }
}

#[test]
fn insert_parameters_are_commanded_by_id() {
    let mut g = GraphDesc::new();
    let a = impulse_track(&mut g, "a", 0, LEN);
    let id = g.add_insert(a, "gain", delay_factory(10)).unwrap();
    let mut s = spec(g, false);
    s.events.push((0, Command::SetInsertParam { insert: id, param: 0, value: 0.25 }));
    s.events.push((0, Command::SetInsertParam { insert: 999, param: 0, value: 9.0 })); // inconnu : ignoré
    assert_eq!(peaks(&render(&s).unwrap()), vec![(10, 0.25)]);
}

#[test]
fn factory_failure_is_reported() {
    let mut g = GraphDesc::new();
    let a = g.add_track("a", TrackFormat::Stereo);
    g.add_insert(a, "cassé", std::sync::Arc::new(|_, _, _| Err("plugin introuvable".to_owned()))).unwrap();
    match render(&spec(g, false)) {
        Err(daw_offline_render::RenderError::Graph(GraphError::InsertFailed { node, reason, .. })) => {
            assert_eq!(node, a);
            assert_eq!(reason, "plugin introuvable");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn seek_clears_effect_and_delay_line_memory() {
    let mut s = spec(two_tracks(true), false);
    s.events.push((50, Command::Seek(2_000))); // avant que l'impulsion (retard 100) ne soit sortie
    let out = render(&s).unwrap();
    assert!(peaks(&out).iter().all(|&(i, _)| i < 50), "rien ne doit sortir après le déplacement : {:?}", peaks(&out));
}

// ---- adoption des effets lors d'un changement de graphe ----------------------------------------

#[test]
fn effect_state_survives_a_graph_swap_with_the_same_insert_id() {
    let mut g1 = GraphDesc::new();
    let t = impulse_track(&mut g1, "t", 10, LEN);
    g1.add_insert(t, "retard", delay_factory(100)).unwrap();
    let mut g2 = g1.clone(); // même identifiant d'effet, routage modifié
    g2.node_mut(t).unwrap().params.pan = 0.0;
    g2.add_bus("nouveau bus");

    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g1).unwrap();
    handle.send(Command::Play);
    let (mut out, mut tmp) = (vec![0.0f32; LEN], vec![0.0f32; LEN]);
    engine.process(&mut out[..50], &mut tmp[..50]); // l'impulsion (frame 10) est dans le retard de l'effet
    handle.load_graph(&g2).unwrap();
    engine.process(&mut out[50..], &mut tmp[50..]);
    let hits: Vec<_> = out.iter().enumerate().filter(|(_, v)| v.abs() > 1e-6).collect();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].0, 110, "l'impulsion sort à 10 + 100 malgré l'échange de graphe");
    assert_eq!(handle.collect_garbage(), 2, "le graphe vide initial et g1 reviennent au contrôle");
}

#[test]
fn a_new_graph_without_the_effect_drops_it_on_the_control_side() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Marker;
    impl Insert for Marker {
        fn latency(&self) -> u32 {
            0
        }
        fn process(&mut self, i: &[f32], j: &[f32], o: &mut [f32], p: &mut [f32]) {
            o.copy_from_slice(i);
            p.copy_from_slice(j);
        }
        fn set_param(&mut self, _: u32, _: f64) {}
        fn reset(&mut self) {}
    }
    impl Drop for Marker {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    let mut g1 = GraphDesc::new();
    let t = g1.add_track("t", TrackFormat::Stereo);
    g1.add_insert(t, "marqueur", Arc::new(|_, _, _| Ok(Box::new(Marker) as Box<dyn Insert>))).unwrap();
    let mut g2 = g1.clone();
    g2.node_mut(t).unwrap().inserts.clear();

    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g1).unwrap();
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    engine.process(&mut l, &mut r);
    handle.load_graph(&g2).unwrap();
    engine.process(&mut l, &mut r);
    assert_eq!(DROPS.load(Ordering::SeqCst), 0, "l'effet ne doit pas être libéré par le thread audio");
    assert_eq!(handle.collect_garbage(), 2); // le graphe vide initial et g1
    assert_eq!(DROPS.load(Ordering::SeqCst), 1, "libéré côté contrôle");
}

#[test]
fn reloading_with_a_live_id_does_not_call_the_factory_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_insert(t, "x", Arc::new(move |_, _, _| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(DelayGain::new(5)) as Box<dyn Insert>)
    }))
    .unwrap();
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.load_graph(&g).unwrap();
    handle.load_graph(&g).unwrap();
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    engine.process(&mut l, &mut r);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(handle.latency_samples(), 5);
    assert!(matches!(handle.load_graph(&GraphDesc::new()), Ok(())));
    let _ = LoadError::QueueFull;
    assert_eq!(handle.latency_samples(), 0);
    let _ = NodeId::MASTER;
}
