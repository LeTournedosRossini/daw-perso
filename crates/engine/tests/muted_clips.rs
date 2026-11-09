//! Clip muet : gardé dans le graphe, jamais joué.

use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, TrackFormat};

const SR: u32 = 48_000;

fn play(muted: bool) -> Vec<f32> {
    let data = AudioData::new(SR, vec![vec![0.5f32; 1_000], vec![0.5f32; 1_000]]).unwrap();
    let mut g = GraphDesc::new();
    let t = g.add_track("T", TrackFormat::Stereo);
    let mut c = Clip::whole(data, 0);
    c.muted = muted;
    g.add_clip(t, c);
    g.validate().unwrap();
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    engine.process(&mut l, &mut r);
    l
}

#[test]
fn a_muted_clip_is_silent_and_a_normal_clip_is_not() {
    assert!(play(false).iter().any(|&v| v.abs() > 0.01), "témoin : le clip normal sonne");
    assert!(play(true).iter().all(|&v| v == 0.0));
}

#[test]
fn halves_of_a_split_keep_the_muted_flag() {
    let data = AudioData::new(SR, vec![vec![0.5f32; 1_000]]).unwrap();
    let mut c = Clip::whole(data, 0);
    c.muted = true;
    let (a, b) = c.split(400).unwrap();
    assert!(a.muted && b.muted);
}
