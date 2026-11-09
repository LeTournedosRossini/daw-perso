//! Mixage : gains, panoramique, mute, solo, bus, envois, placement des clips.

mod common;
use common::*;
use daw_core_types::NodeId;
use daw_engine::{Command, GraphDesc, TrackFormat};
use daw_offline_render::{render, RenderSpec};

const N: usize = 1000;

fn assert_close(a: &[f32], b: &[f32], tol: f32) {
    assert_eq!(a.len(), b.len());
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        assert!((x - y).abs() <= tol, "échantillon {i} : {x} au lieu de {y}");
    }
}

fn scaled(v: &[f32], k: f32) -> Vec<f32> {
    v.iter().map(|x| x * k).collect()
}

#[test]
fn empty_graph_is_silent() {
    let r = render_simple(GraphDesc::new(), 500);
    assert!(r.left.iter().chain(&r.right).all(|&s| s == 0.0));
}

#[test]
fn stereo_track_at_unity_is_bit_exact() {
    let (l, rr) = (noise(1, N), noise(2, N));
    let data = stereo(l.clone(), rr.clone());
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, clip(&data, 0));
    let out = render_simple(g, N as u64);
    assert_eq!(out.left, l);
    assert_eq!(out.right, rr);
}

#[test]
fn track_gain_applies() {
    let x = noise(3, N);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, clip(&stereo(x.clone(), x.clone()), 0));
    g.node_mut(t).unwrap().params.gain = 0.5;
    let out = render_simple(g, N as u64);
    assert_eq!(out.left, scaled(&x, 0.5));
}

#[test]
fn mono_track_is_centered_at_minus_3db_and_hard_pans() {
    let x = noise(4, N);
    for (pan, expect_l, expect_r) in [(0.0, 0.707_106_77, 0.707_106_77), (-1.0, 1.0, 0.0), (1.0, 0.0, 1.0)] {
        let mut g = GraphDesc::new();
        let t = g.add_track("t", TrackFormat::Mono);
        g.add_clip(t, clip(&mono(x.clone()), 0));
        g.node_mut(t).unwrap().params.pan = pan;
        let out = render_simple(g, N as u64);
        assert_close(&out.left, &scaled(&x, expect_l), 1e-6);
        assert_close(&out.right, &scaled(&x, expect_r), 1e-6);
    }
}

#[test]
fn stereo_track_balance_keeps_unity_at_center() {
    let (l, r) = (noise(5, N), noise(6, N));
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, clip(&stereo(l.clone(), r.clone()), 0));
    g.node_mut(t).unwrap().params.pan = 0.5; // atténue la gauche de moitié
    let out = render_simple(g, N as u64);
    assert_close(&out.left, &scaled(&l, 0.5), 1e-7);
    assert_eq!(out.right, r);
}

#[test]
fn mute_silences_a_track() {
    let x = noise(7, N);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Mono);
    g.add_clip(t, clip(&mono(x), 0));
    g.node_mut(t).unwrap().params.mute = true;
    let out = render_simple(g, N as u64);
    assert!(out.left.iter().chain(&out.right).all(|&s| s == 0.0));
}

#[test]
fn solo_keeps_only_soloed_tracks_and_leaves_buses_alone() {
    let (a, b) = (noise(8, N), noise(9, N));
    let mut g = GraphDesc::new();
    let t1 = g.add_track("t1", TrackFormat::Stereo);
    let t2 = g.add_track("t2", TrackFormat::Stereo);
    let bus = g.add_bus("bus");
    g.set_output(t1, bus);
    g.set_output(t2, bus);
    g.add_clip(t1, clip(&stereo(a.clone(), a.clone()), 0));
    g.add_clip(t2, clip(&stereo(b.clone(), b.clone()), 0));
    g.node_mut(t2).unwrap().params.solo = true;
    let out = render_simple(g, N as u64);
    assert_eq!(out.left, b, "seule t2 passe par le bus, qui n'est pas coupé");
}

#[test]
fn bus_sums_its_tracks_and_master_gain_applies() {
    let (a, b) = (noise(10, N), noise(11, N));
    let mut g = GraphDesc::new();
    let t1 = g.add_track("t1", TrackFormat::Stereo);
    let t2 = g.add_track("t2", TrackFormat::Stereo);
    let bus = g.add_bus("bus");
    g.set_output(t1, bus);
    g.set_output(t2, bus);
    g.add_clip(t1, clip(&stereo(a.clone(), a.clone()), 0));
    g.add_clip(t2, clip(&stereo(b.clone(), b.clone()), 0));
    g.node_mut(NodeId::MASTER).unwrap().params.gain = 0.5;
    let out = render_simple(g, N as u64);
    let sum: Vec<f32> = a.iter().zip(&b).map(|(x, y)| (x + y) * 0.5).collect();
    assert_close(&out.left, &sum, 1e-7);
}

#[test]
fn send_is_post_fader_and_adds_to_the_bus() {
    let x = noise(12, N);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let fx = g.add_bus("fx");
    g.add_clip(t, clip(&stereo(x.clone(), x.clone()), 0));
    g.node_mut(t).unwrap().params.gain = 0.5; // post-fader : le départ voit aussi ce gain
    g.add_send(t, fx, 0.5);
    let out = render_simple(g, N as u64);
    // direct : 0,5·x ; via le bus : 0,5·x·0,5 → total 0,75·x
    assert_close(&out.left, &scaled(&x, 0.75), 1e-7);
}

#[test]
fn muted_track_also_mutes_its_send() {
    let x = noise(13, N);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let fx = g.add_bus("fx");
    g.add_clip(t, clip(&stereo(x.clone(), x), 0));
    g.add_send(t, fx, 1.0);
    g.node_mut(t).unwrap().params.mute = true;
    assert!(render_simple(g, N as u64).left.iter().all(|&s| s == 0.0));
}

#[test]
fn send_can_feed_a_chain_of_buses() {
    let x = noise(14, N);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let b1 = g.add_bus("b1");
    let b2 = g.add_bus("b2");
    g.set_output(t, b1);
    g.set_output(b1, b2);
    g.node_mut(b1).unwrap().params.gain = 0.5;
    g.node_mut(b2).unwrap().params.gain = 0.5;
    g.add_clip(t, clip(&stereo(x.clone(), x.clone()), 0));
    let out = render_simple(g, N as u64);
    assert_close(&out.left, &scaled(&x, 0.25), 1e-7);
}

#[test]
fn clip_placement_offset_length_and_gain_are_sample_accurate() {
    let mut data = vec![0.0f32; 400];
    for (i, s) in data.iter_mut().enumerate() {
        *s = (i + 1) as f32; // valeurs entières exactes : on repère chaque échantillon
    }
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let d = stereo(data.clone(), data.clone());
    g.add_clip(t, daw_engine::Clip::new(d, 100, 10, 20, 2.0));
    let out = render_simple(g, 200);
    for f in 0..200usize {
        let expected = if (100..120).contains(&f) { 2.0 * data[10 + (f - 100)] } else { 0.0 };
        assert_eq!(out.left[f], expected, "trame {f}");
    }
}

#[test]
fn overlapping_clips_add() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let d = stereo(vec![1.0; 10], vec![1.0; 10]);
    g.add_clip(t, clip(&d, 0));
    g.add_clip(t, clip(&d, 5));
    let out = render_simple(g, 20);
    let expected: Vec<f32> = (0..20).map(|f| if f < 5 { 1.0 } else if f < 10 { 2.0 } else if f < 15 { 1.0 } else { 0.0 }).collect();
    assert_eq!(out.left, expected);
}

#[test]
fn gain_change_ramps_without_a_jump() {
    // Signal continu de 1,0 ; le gain passe de 1 à 0 à la trame 1000.
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, clip(&stereo(vec![1.0; 5000], vec![1.0; 5000]), 0));
    let mut spec = RenderSpec::new(SR, 3000, g);
    spec.events.push((1000, Command::SetGain { node: t.0, gain: 0.0 }));
    let out = render(&spec).unwrap();
    let ramp = (SR / 200) as usize; // 240 échantillons
    assert_eq!(out.left[999], 1.0);
    for f in 1000..1000 + ramp {
        assert!(out.left[f] < out.left[f - 1], "doit décroître à la trame {f}");
        assert!(out.left[f - 1] - out.left[f] < 2.0 / ramp as f32, "saut trop grand à la trame {f}");
    }
    assert_eq!(out.left[1000 + ramp], 0.0);
    assert_eq!(out.left[2999], 0.0);
}

#[test]
fn commands_on_unknown_nodes_are_ignored() {
    let x = noise(15, N);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, clip(&stereo(x.clone(), x.clone()), 0));
    let mut spec = RenderSpec::new(SR, N as u64, g);
    spec.events.extend([
        (10, Command::SetGain { node: 99, gain: 0.0 }),
        (10, Command::SetPan { node: 99, pan: 1.0 }),
        (10, Command::SetSendGain { node: t.0, send: 7, gain: 0.0 }),
        (10, Command::SetGain { node: t.0, gain: f32::NAN }),
    ]);
    assert_eq!(render(&spec).unwrap().left, x);
}

#[test]
fn meters_report_the_post_fader_peak_of_each_node_and_reset_on_read() {
    use daw_engine::{Command, Engine};
    let mut g = GraphDesc::new();
    let t = g.add_track("t", daw_engine::TrackFormat::Stereo);
    let mut x = vec![0.0f32; 1_000];
    x[500] = 0.8;
    g.add_clip(t, clip(&stereo(x.clone(), x), 0));
    g.set_params(t, daw_engine::NodeParams { gain: 0.5, ..Default::default() });
    let (mut engine, mut handle) = Engine::new(SR, 256);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let bank = handle.meters();
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    for _ in 0..3 {
        engine.process(&mut l, &mut r);
    }
    // L'impulsion de 0,8 passe dans le fader à 0,5 (balance centrée : gain 1 de chaque côté) : crête 0,4 sur la piste et au master.
    assert_eq!(bank.take(t.0 as usize), (0.4, 0.4));
    assert_eq!(bank.take(0), (0.4, 0.4));
    assert_eq!(bank.take(t.0 as usize), (0.0, 0.0), "remis à zéro par la lecture");
    handle.send(Command::SetMute { node: t.0, mute: true });
    for _ in 0..10 {
        engine.process(&mut l, &mut r);
    }
    assert_eq!(bank.take(t.0 as usize), (0.0, 0.0));
}
