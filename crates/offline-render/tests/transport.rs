//! Transport : lecture, arrêt, déplacement, boucle.

mod common;
use common::*;
use daw_engine::{Command, GraphDesc, TrackFormat};
use daw_offline_render::{render, RenderSpec};

/// Piste dont l'échantillon `i` vaut `i` : la sortie indique directement la position lue.
fn counter_graph(len: usize) -> GraphDesc {
    let ramp: Vec<f32> = (0..len).map(|i| i as f32).collect();
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, clip(&stereo(ramp.clone(), ramp), 0));
    g
}

#[test]
fn silent_until_play_and_position_holds_while_stopped() {
    let mut spec = RenderSpec::new(SR, 300, counter_graph(1000));
    spec.events = vec![(100, Command::Play), (200, Command::Stop)];
    let out = render(&spec).unwrap();
    for f in 0..300usize {
        let expected = if (100..200).contains(&f) { (f - 100) as f32 } else { 0.0 };
        assert_eq!(out.left[f], expected, "trame {f}");
    }
}

#[test]
fn stop_then_play_resumes_where_it_stopped() {
    let mut spec = RenderSpec::new(SR, 400, counter_graph(1000));
    spec.events = vec![(0, Command::Play), (50, Command::Stop), (200, Command::Play)];
    let out = render(&spec).unwrap();
    assert_eq!(out.left[49], 49.0);
    assert_eq!(out.left[199], 0.0);
    assert_eq!(out.left[200], 50.0);
    assert_eq!(out.left[399], 249.0);
}

#[test]
fn seek_moves_the_playhead() {
    let mut spec = RenderSpec::new(SR, 100, counter_graph(1000));
    spec.events = vec![(0, Command::Seek(500)), (0, Command::Play), (40, Command::Seek(10))];
    let out = render(&spec).unwrap();
    assert_eq!(out.left[0], 500.0);
    assert_eq!(out.left[39], 539.0);
    assert_eq!(out.left[40], 10.0);
    assert_eq!(out.left[99], 69.0);
}

#[test]
fn loop_wraps_sample_accurately() {
    let mut spec = RenderSpec::new(SR, 100, counter_graph(1000));
    spec.events = vec![(0, Command::SetLoop(Some((10, 20)))), (0, Command::Seek(5)), (0, Command::Play)];
    let out = render(&spec).unwrap();
    let expected: Vec<f32> = (0..100).map(|f| if f < 15 { 5.0 + f as f32 } else { 10.0 + ((f - 15) % 10) as f32 }).collect();
    assert_eq!(out.left, expected);
}

#[test]
fn invalid_loop_is_ignored() {
    let mut spec = RenderSpec::new(SR, 50, counter_graph(1000));
    spec.events = vec![(0, Command::SetLoop(Some((20, 20)))), (0, Command::Play)];
    let out = render(&spec).unwrap();
    assert_eq!(out.left[49], 49.0);
}

#[test]
fn clearing_the_loop_lets_playback_continue() {
    let mut spec = RenderSpec::new(SR, 60, counter_graph(1000));
    spec.events = vec![(0, Command::SetLoop(Some((0, 10)))), (0, Command::Play), (25, Command::SetLoop(None))];
    let out = render(&spec).unwrap();
    assert_eq!(out.left[24], 4.0);
    // pas de boucle à partir de 25 : on continue depuis la position courante (5)
    assert_eq!(out.left[25], 5.0);
    assert_eq!(out.left[59], 39.0);
}
