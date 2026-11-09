use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, FadeShape, GraphDesc, NodeParams, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_session::{Session, SessionError};

const SR: u32 = 48_000;

fn data(seed: u32) -> Arc<AudioData> {
    let mut s = seed;
    let mut v = || {
        (0..2_000)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / 16_777_216.0 - 0.5
            })
            .collect::<Vec<f32>>()
    };
    AudioData::new(SR, vec![v(), v()]).unwrap()
}

fn setup() -> (Session, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    (Session::new(g), t)
}

fn hash(s: &Session) -> u64 {
    render(&RenderSpec::new(SR, 5_000, s.graph().clone())).unwrap().hash()
}

#[test]
fn every_action_is_undone_and_redone_bit_for_bit() {
    let (mut s, t) = setup();
    let a = s.add_clip(t, Clip::whole(data(1), 0)).unwrap();
    let b = s.add_clip(t, Clip::whole(data(2), 1_500)).unwrap();
    let mut states = vec![hash(&s)];
    s.move_clip(b, 1_700).unwrap();
    states.push(hash(&s));
    s.trim_end(a, 1_800).unwrap();
    states.push(hash(&s));
    s.trim_start(a, 100).unwrap();
    states.push(hash(&s));
    s.split(a, 900).unwrap();
    states.push(hash(&s));
    s.crossfade(a, b, FadeShape::EqualPower).unwrap_err(); // `a` finit avant `b` : refusé, rien n'est inscrit
    s.set_params(t, NodeParams { gain: 0.5, ..Default::default() }).unwrap();
    states.push(hash(&s));
    s.remove_clip(b).unwrap();
    states.push(hash(&s));

    for k in (0..states.len() - 1).rev() {
        assert!(s.undo());
        assert_eq!(hash(&s), states[k], "annulation vers l'état {k}");
    }
    assert_eq!(s.undo_label(), Some("Ajouter un clip")); // les deux ajouts initiaux restent annulables
    for (k, expected) in states.iter().enumerate().skip(1) {
        assert!(s.redo());
        assert_eq!(hash(&s), *expected, "rétablissement vers l'état {k}");
    }
    assert!(!s.redo());
}

#[test]
fn split_is_one_undo_step_and_ids_are_stable() {
    let (mut s, t) = setup();
    let a = s.add_clip(t, Clip::whole(data(3), 100)).unwrap();
    let before = hash(&s);
    let (l, r) = s.split(a, 700).unwrap();
    assert_eq!(l, a);
    assert_eq!(s.clip_ids(t), &[l, r]);
    assert_eq!(hash(&s), before, "les deux moitiés rejouent l'original");
    assert_eq!(s.undo_label(), Some("Découper"));
    assert!(s.undo());
    assert_eq!(s.clip_ids(t), &[a]);
    assert!(s.clip(r).is_none());
    assert!(s.redo());
    assert_eq!(s.clip_ids(t), &[l, r]);
    assert_eq!(s.clip(r).unwrap().start, 700);
}

#[test]
fn a_new_action_clears_the_redo_stack() {
    let (mut s, t) = setup();
    let a = s.add_clip(t, Clip::whole(data(4), 0)).unwrap();
    s.move_clip(a, 10).unwrap();
    s.undo();
    assert!(s.can_redo());
    s.move_clip(a, 20).unwrap();
    assert!(!s.can_redo());
    assert_eq!(s.clip(a).unwrap().start, 20);
}

#[test]
fn removed_clip_returns_at_its_place_with_its_id() {
    let (mut s, t) = setup();
    let ids: Vec<_> = (0..3).map(|k| s.add_clip(t, Clip::whole(data(10 + k), k as u64 * 100)).unwrap()).collect();
    s.remove_clip(ids[1]).unwrap();
    assert_eq!(s.clip_ids(t), &[ids[0], ids[2]]);
    s.undo();
    assert_eq!(s.clip_ids(t), &ids[..]);
    assert_eq!(s.clip(ids[1]).unwrap().start, 100);
}

#[test]
fn crossfade_is_a_single_undoable_action() {
    let (mut s, t) = setup();
    let a = s.add_clip(t, Clip::whole(data(5), 0)).unwrap();
    let b = s.add_clip(t, Clip::whole(data(6), 1_500)).unwrap();
    let before = hash(&s);
    s.crossfade(a, b, FadeShape::Linear).unwrap();
    assert_ne!(hash(&s), before);
    assert_eq!((s.clip(a).unwrap().fade_out.len, s.clip(b).unwrap().fade_in.len), (500, 500));
    s.undo();
    assert_eq!(hash(&s), before);
}

#[test]
fn a_failing_group_leaves_no_trace() {
    let (mut s, t) = setup();
    let a = s.add_clip(t, Clip::whole(data(7), 0)).unwrap();
    let before = hash(&s);
    let depth = s.can_undo();
    let r: Result<(), SessionError> = s.group("raté", |s| {
        s.move_clip(a, 50)?;
        s.trim_end(a, 5_000_000)?; // plus de données : échec
        Ok(())
    });
    assert!(matches!(r, Err(SessionError::Edit(_))));
    assert_eq!(hash(&s), before);
    assert_eq!(s.clip(a).unwrap().start, 0);
    assert_eq!(s.undo_label(), Some("Ajouter un clip"));
    assert!(depth);
}

#[test]
fn bad_requests_are_refused_without_history() {
    let (mut s, t) = setup();
    let a = s.add_clip(t, Clip::whole(data(8), 0)).unwrap();
    assert!(s.split(a, 0).is_err());
    assert!(s.trim_start(a, 5_000).is_err());
    assert_eq!(s.remove_clip(daw_session::ClipId(999)), Err(SessionError::UnknownClip(daw_session::ClipId(999))));
    assert!(s.add_clip(NodeId(77), Clip::whole(data(9), 0)).is_err());
    assert_eq!(s.undo_label(), Some("Ajouter un clip"));
}

#[test]
fn existing_clips_get_ids_and_a_user_group_is_one_step() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(data(1), 0));
    g.add_clip(t, Clip::whole(data(2), 100));
    let mut s = Session::new(g);
    let ids = s.clip_ids(t).to_vec();
    assert_eq!(ids.len(), 2);
    s.group("Tout décaler", |s| {
        for &id in &ids {
            let st = s.clip(id).unwrap().start;
            s.move_clip(id, st + 10)?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!((s.clip(ids[0]).unwrap().start, s.clip(ids[1]).unwrap().start), (10, 110));
    s.undo();
    assert_eq!((s.clip(ids[0]).unwrap().start, s.clip(ids[1]).unwrap().start), (0, 100));
    assert!(!s.can_undo());
}
