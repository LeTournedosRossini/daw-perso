//! Enregistrement et écoute : entrée simulée → prises, monitoring, position, indépendance vis-à-vis du mixage.

mod common;
use common::*;
use daw_engine::{Command, Engine, GraphDesc, RecordSource, TrackFormat};
use daw_offline_render::{render, RecordJob, RenderSpec};

const N: u64 = 6_000;

fn input(seed: u32) -> (Vec<f32>, Vec<f32>) {
    (noise(seed, N as usize), noise(seed + 1, N as usize))
}

fn one_track() -> (GraphDesc, u32) {
    let mut g = GraphDesc::new();
    let t = g.add_track("voix", TrackFormat::Stereo);
    (g, t.0)
}

fn job(node: u32, source: RecordSource, start: u64, stop: u64, monitor: bool) -> RecordJob {
    RecordJob { node, source, start, stop, monitor }
}

#[test]
fn take_contains_exactly_the_input_between_start_and_stop() {
    let (l, r) = input(1);
    for (source, expect) in [
        (RecordSource::Left, l[1_000..3_000].to_vec()),
        (RecordSource::Right, r[1_000..3_000].to_vec()),
        (RecordSource::Stereo, l[1_000..3_000].iter().zip(&r[1_000..3_000]).flat_map(|(a, b)| [*a, *b]).collect()),
    ] {
        let (g, t) = one_track();
        let mut s = RenderSpec::new(SR, N, g);
        s.input = Some((l.clone(), r.clone()));
        s.records = vec![job(t, source, 1_000, 3_000, false)];
        let out = render(&s).unwrap();
        let take = &out.takes[0];
        assert_eq!(take.data, expect, "{source:?}");
        assert_eq!(take.start_pos, 1_000, "le transport joue depuis 0 : position = échantillon du rendu");
        assert_eq!(take.overruns, 0);
    }
}

#[test]
fn take_is_the_same_for_any_block_size() {
    let (l, r) = input(2);
    let mut reference = None;
    for (block, engine_block) in [(6_000, 6_000), (1, 1), (7, 64), (333, 256), (64, 16)] {
        let (g, t) = one_track();
        let mut s = RenderSpec::new(SR, N, g);
        s.block = block;
        s.engine_block = engine_block;
        s.input = Some((l.clone(), r.clone()));
        s.records = vec![job(t, RecordSource::Stereo, 777, 4_321, false)];
        let take = render(&s).unwrap().takes.remove(0);
        match &reference {
            None => reference = Some(take),
            Some(r0) => assert_eq!(&take, r0, "bloc {block}/{engine_block}"),
        }
    }
}

#[test]
fn recording_works_while_the_transport_is_stopped() {
    let (l, r) = input(3);
    let (g, t) = one_track();
    let mut s = RenderSpec::new(SR, N, g);
    s.events = vec![(2_000, Command::Seek(10_000)), (2_000, Command::Play)];
    s.input = Some((l.clone(), r));
    s.records = vec![job(t, RecordSource::Left, 500, 3_000, false)];
    let take = render(&s).unwrap().takes.remove(0);
    // Pas de lecture avant 2 000 : la tête reste à 0, puis saute à 10 000 et avance. La prise commence à l'arrêt.
    assert_eq!(take.start_pos, 0);
    assert_eq!(take.data, l[500..3_000].to_vec(), "toute la prise, avant comme après le démarrage de la lecture");
}

#[test]
fn recording_is_dry_whatever_the_fader_the_inserts_or_the_mute() {
    let (l, r) = input(4);
    let (mut g, t) = one_track();
    let id = daw_core_types::NodeId(t);
    g.add_insert(id, "retard", delay_factory(50)).unwrap();
    g.node_mut(id).unwrap().params.gain = 0.1;
    g.node_mut(id).unwrap().params.mute = true;
    let mut s = RenderSpec::new(SR, N, g);
    s.compensate_latency = false;
    s.input = Some((l.clone(), r));
    s.records = vec![job(t, RecordSource::Left, 100, 2_100, true)];
    let out = render(&s).unwrap();
    assert_eq!(out.takes[0].data, l[100..2_100].to_vec());
    assert!(out.left.iter().all(|&v| v == 0.0), "piste en sourdine : l'écoute aussi");
}

#[test]
fn monitoring_passes_the_input_through_the_track_only_while_armed() {
    let (l, r) = input(5);
    let (g, t) = one_track();
    let mut s = RenderSpec::new(SR, N, g);
    s.compensate_latency = false;
    s.input = Some((l.clone(), r.clone()));
    s.records = vec![job(t, RecordSource::Stereo, 1_000, 3_000, true)];
    let out = render(&s).unwrap();
    assert!(out.left[..1_000].iter().all(|&v| v == 0.0), "avant l'armement : silence");
    assert_eq!(&out.left[1_000..3_000], &l[1_000..3_000]);
    assert_eq!(&out.right[1_000..3_000], &r[1_000..3_000]);
    assert!(out.left[3_000..].iter().all(|&v| v == 0.0), "après : silence");
}

#[test]
fn armed_track_without_monitoring_makes_no_sound() {
    let (l, r) = input(6);
    let (g, t) = one_track();
    let mut s = RenderSpec::new(SR, N, g);
    s.input = Some((l, r));
    s.records = vec![job(t, RecordSource::Stereo, 0, 4_000, false)];
    let out = render(&s).unwrap();
    assert!(out.left.iter().chain(&out.right).all(|&v| v == 0.0));
    assert_eq!(out.takes[0].data.len(), 8_000);
}

#[test]
fn live_monitoring_is_not_delayed_by_latency_compensation() {
    // B a un effet de 100 échantillons : la compensation retarde de 100 la piste A, sauf si A est écoutée en direct.
    let build = || {
        let mut g = GraphDesc::new();
        let a = g.add_track("a", TrackFormat::Stereo);
        let b = g.add_track("b", TrackFormat::Stereo);
        g.add_insert(b, "retard", delay_factory(100)).unwrap();
        (g, a, b)
    };
    // A écoutée en direct : l'impulsion d'entrée (échantillon 50) ressort à 50.
    let (g, a, _) = build();
    let mut imp = vec![0.0f32; N as usize];
    imp[50] = 1.0;
    let mut s = RenderSpec::new(SR, N, g);
    s.compensate_latency = false;
    s.input = Some((imp.clone(), imp.clone()));
    s.records = vec![job(a.0, RecordSource::Stereo, 0, 1_000, true)];
    let live = render(&s).unwrap();
    let hits: Vec<usize> = live.left.iter().enumerate().filter(|(_, v)| v.abs() > 1e-6).map(|(i, _)| i).collect();
    assert_eq!(hits, vec![50]);

    // La même impulsion lue depuis un clip de A est retardée de 100 par la compensation.
    let (mut g, a, _) = build();
    let mut x = vec![0.0f32; 1_000];
    x[50] = 1.0;
    g.add_clip(a, clip(&stereo(x.clone(), x), 0));
    let mut s = RenderSpec::new(SR, N, g);
    s.compensate_latency = false;
    let played = render(&s).unwrap();
    let hits: Vec<usize> = played.left.iter().enumerate().filter(|(_, v)| v.abs() > 1e-6).map(|(i, _)| i).collect();
    assert_eq!(hits, vec![150]);
}

#[test]
fn a_take_survives_a_graph_swap() {
    let (l, r) = input(7);
    let (g1, t) = one_track();
    let mut g2 = g1.clone();
    g2.add_bus("nouveau bus"); // le routage change en pleine prise
    let (mut engine, mut handle) = Engine::new(SR, 256);
    handle.load_graph(&g1).unwrap();
    handle.send(Command::SetInput { node: t, armed: true, monitor: false, source: RecordSource::Stereo });
    let mut rx = handle.start_record(t, RecordSource::Stereo, 2.0).unwrap();
    let (mut ol, mut or) = (vec![0.0f32; 4_000], vec![0.0f32; 4_000]);
    engine.process_io(&l[..1_500], &r[..1_500], &mut ol[..1_500], &mut or[..1_500]);
    handle.load_graph(&g2).unwrap();
    engine.process_io(&l[1_500..4_000], &r[1_500..4_000], &mut ol[1_500..], &mut or[1_500..]);
    handle.stop_record(t);
    engine.process_io(&[], &[], &mut [], &mut []);
    assert!(rx.status.closed.load(std::sync::atomic::Ordering::Acquire));
    let n = rx.consumer.slots();
    let chunk = rx.consumer.read_chunk(n).unwrap();
    let (a, b) = chunk.as_slices();
    let data = [a, b].concat();
    assert_eq!(data.len(), 8_000, "aucune trame perdue au changement de graphe");
    for i in 0..4_000 {
        assert_eq!((data[2 * i], data[2 * i + 1]), (l[i], r[i]), "trame {i}");
    }
}

#[test]
fn stopping_the_transport_lets_effect_tails_ring_out() {
    // Impulsion jouée à 0 dans un effet de 200 échantillons ; arrêt à 50. La queue doit sortir quand même, à 200.
    let mut g = GraphDesc::new();
    let t = impulse_track(&mut g, "t", 0, 1_000);
    g.add_insert(t, "retard", delay_factory(200)).unwrap();
    let mut s = RenderSpec::new(SR, 600, g);
    s.compensate_latency = false;
    s.events.push((50, Command::Stop));
    let out = render(&s).unwrap();
    let hits: Vec<usize> = out.left.iter().enumerate().filter(|(_, v)| v.abs() > 1e-6).map(|(i, _)| i).collect();
    assert_eq!(hits, vec![200]);
}

#[test]
fn input_commands_on_unknown_or_non_track_nodes_are_ignored() {
    let (l, r) = input(8);
    let (g, _) = one_track();
    let mut s = RenderSpec::new(SR, 2_000, g);
    s.input = Some((l, r));
    s.events.push((0, Command::SetInput { node: 0, armed: true, monitor: true, source: RecordSource::Stereo }));
    s.events.push((0, Command::SetInput { node: 99, armed: true, monitor: true, source: RecordSource::Stereo }));
    s.records = vec![job(77, RecordSource::Left, 0, 100, true)]; // piste inconnue : prise jamais installée
    let out = render(&s).unwrap();
    assert!(out.left.iter().all(|&v| v == 0.0));
    assert!(out.takes[0].data.is_empty());
}
