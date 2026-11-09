use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, EditError, Fade, FadeShape, GraphDesc, TrackFormat};
use daw_session::xfade::{length_bounds, resize, Anchor};
use daw_session::{ClipId, Session, SessionError};

const SR: u32 = 48_000;
const ANCHORS: [Anchor; 3] = [Anchor::Center, Anchor::Start, Anchor::End];

fn data() -> Arc<AudioData> {
    AudioData::new(SR, vec![(0..2_000).map(|i| i as f32).collect(), (0..2_000).map(|i| -(i as f32)).collect()]).unwrap()
}

/// a : début 1000, décalage 500, 600 échantillons (fin 1600, 900 de données après) ;
/// b : début 1400, décalage 300, 500 échantillons (fin 1900, 300 de données avant). Chevauchement : 200.
fn pair() -> (Clip, Clip) {
    let mut a = Clip::new(data(), 1_000, 500, 600, 1.0);
    let mut b = Clip::new(data(), 1_400, 300, 500, 1.0);
    a.fade_out = Fade::curved(200, FadeShape::Linear, 400, 250);
    b.fade_in = Fade::curved(200, FadeShape::EqualPower, -300, 0);
    (a, b)
}

fn setup() -> (Session, NodeId, ClipId, ClipId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let mut s = Session::new(g);
    let (a, b) = pair();
    let ia = s.add_clip(t, a).unwrap();
    let ib = s.add_clip(t, b).unwrap();
    (s, t, ia, ib)
}

#[test]
fn bounds_match_a_brute_force_search_for_every_anchor() {
    let (a, b) = pair();
    for anchor in ANCHORS {
        let (min, max) = length_bounds(&a, &b, anchor).unwrap();
        assert_eq!(min, 1);
        for len in 1..=1_500 {
            let ok = resize(&a, &b, len, anchor).is_ok();
            assert_eq!(ok, len <= max, "{anchor:?} len {len} (max {max})");
        }
    }
}

#[test]
fn known_bounds() {
    let (a, b) = pair();
    // Début fixe (1400) : fin de a au plus min(fin de b 1900, fin de a + données 2500) → 500.
    assert_eq!(length_bounds(&a, &b, Anchor::Start).unwrap().1, 500);
    // Fin fixe (1600) : début de b au plus tôt max(début de a + 1 = 1001, 1400 − 300 = 1100) → 500.
    assert_eq!(length_bounds(&a, &b, Anchor::End).unwrap().1, 500);
    // Centré sur 1500 : bornes 1100 et 1900 → 2 × 400 = 800 d'un côté, mais 2×1900+1−3000 = 801 de l'autre → 800.
    assert_eq!(length_bounds(&a, &b, Anchor::Center).unwrap().1, 800);
}

#[test]
fn content_stays_in_place_and_curves_are_kept() {
    let (a, b) = pair();
    for anchor in ANCHORS {
        for len in [1, 7, 100, 200, 333] {
            let (na, nb) = resize(&a, &b, len, anchor).unwrap();
            assert_eq!(na.start as i64 - na.offset as i64, a.start as i64 - a.offset as i64, "contenu de a déplacé");
            assert_eq!(nb.start as i64 - nb.offset as i64, b.start as i64 - b.offset as i64, "contenu de b déplacé");
            assert_eq!(na.start, a.start);
            assert_eq!(nb.end(), b.end());
            assert_eq!(na.end() - nb.start, len, "zone commune");
            assert_eq!((na.fade_out.len, nb.fade_in.len), (len, len));
            assert_eq!((na.fade_out.shape, na.fade_out.bend, na.fade_out.smooth), (FadeShape::Linear, 400, 250));
            assert_eq!((nb.fade_in.shape, nb.fade_in.bend, nb.fade_in.smooth), (FadeShape::EqualPower, -300, 0));
        }
    }
}

#[test]
fn each_anchor_keeps_its_own_edge() {
    let (a, b) = pair();
    let (na, nb) = resize(&a, &b, 300, Anchor::Start).unwrap();
    assert_eq!((nb.start, na.end()), (1_400, 1_700));
    let (na, nb) = resize(&a, &b, 300, Anchor::End).unwrap();
    assert_eq!((nb.start, na.end()), (1_300, 1_600));
    let (na, nb) = resize(&a, &b, 300, Anchor::Center).unwrap();
    assert_eq!((nb.start, na.end()), (1_350, 1_650));
    // Longueur impaire : le début de b prend l'arrondi inférieur.
    let (na, nb) = resize(&a, &b, 201, Anchor::Center).unwrap();
    assert_eq!((nb.start, na.end()), (1_399, 1_600));
}

#[test]
fn clip_without_a_previous_fade_gets_equal_power() {
    let a = Clip::new(data(), 1_000, 500, 600, 1.0);
    let b = Clip::new(data(), 1_400, 300, 500, 1.0);
    let (na, nb) = resize(&a, &b, 200, Anchor::Center).unwrap();
    assert_eq!((na.fade_out.shape, nb.fade_in.shape), (FadeShape::EqualPower, FadeShape::EqualPower));
}

#[test]
fn refused_without_effect_outside_bounds_or_without_overlap() {
    let (a, b) = pair();
    assert_eq!(resize(&a, &b, 0, Anchor::Center).unwrap_err(), EditError::OutOfRange);
    assert_eq!(resize(&a, &b, 801, Anchor::Center).unwrap_err(), EditError::OutOfRange);
    assert_eq!(resize(&b, &a, 100, Anchor::Center).unwrap_err(), EditError::NoOverlap);
    let far = Clip::new(data(), 5_000, 0, 500, 1.0);
    assert_eq!(length_bounds(&a, &far, Anchor::End).unwrap_err(), EditError::NoOverlap);
}

#[test]
fn session_action_is_one_undo_step() {
    let (mut s, _t, ia, ib) = setup();
    let before = (s.clip(ia).unwrap().clone(), s.clip(ib).unwrap().clone());
    s.set_crossfade_length(ia, ib, 300, Anchor::Center).unwrap();
    let (a, b) = (s.clip(ia).unwrap(), s.clip(ib).unwrap());
    assert_eq!((b.start, a.end()), (1_350, 1_650));
    assert_eq!(s.undo_label().as_deref(), Some("Longueur du fondu enchaîné"));
    s.undo();
    let key = |c: &Clip| (c.start, c.offset, c.len, c.fade_in, c.fade_out);
    assert_eq!((key(s.clip(ia).unwrap()), key(s.clip(ib).unwrap())), (key(&before.0), key(&before.1)));
    assert!(s.redo());
    assert_eq!(s.clip(ib).unwrap().start, 1_350);
    // Refus : rien n'entre dans l'historique.
    s.undo();
    let err = s.set_crossfade_length(ia, ib, 5_000, Anchor::Start).unwrap_err();
    assert!(matches!(err, SessionError::Edit(EditError::OutOfRange)));
    assert!(!s.can_undo() || s.undo_label().as_deref() != Some("Longueur du fondu enchaîné"));
}
