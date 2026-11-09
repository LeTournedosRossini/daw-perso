use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{GraphDesc, InsertDesc, InsertFactory, TrackFormat};
use daw_session::{Session, SessionError};

fn factory(tag: &'static str) -> InsertFactory {
    Arc::new(move |_, _, _| Err(tag.to_owned()))
}

fn keep(i: &InsertDesc) -> InsertFactory {
    Arc::clone(&i.factory)
}

fn setup() -> (Session, NodeId, NodeId) {
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    let b = g.add_track("B", TrackFormat::Stereo);
    (Session::new(g), a, b)
}

fn chain(s: &Session, n: NodeId) -> Vec<(u32, String, bool)> {
    s.graph().nodes[n.0 as usize].inserts.iter().map(|i| (i.id, i.name.clone(), i.bypass)).collect()
}

#[test]
fn adding_effects_appends_them_with_fresh_ids_and_is_undoable() {
    let (mut s, a, _) = setup();
    let x = s.add_effect(a, "Synthé", factory("x")).unwrap();
    let y = s.add_effect(a, "Compresseur", factory("y")).unwrap();
    assert_ne!(x, y);
    assert_eq!(chain(&s, a), [(x, "Synthé".into(), false), (y, "Compresseur".into(), false)]);
    assert_eq!(s.undo_label(), Some("Ajouter un plugin"));
    s.undo();
    s.undo();
    assert!(chain(&s, a).is_empty());
    // Rétablir remet les mêmes effets avec les mêmes identifiants ; un nouvel effet ne réutilise jamais un identifiant.
    assert!(s.redo() && s.redo());
    assert_eq!(chain(&s, a).iter().map(|c| c.0).collect::<Vec<_>>(), [x, y]);
    s.undo();
    s.undo();
    let z = s.add_effect(a, "Autre", factory("z")).unwrap();
    assert!(z != x && z != y, "identifiants jamais réutilisés");
    assert!(matches!(s.add_effect(NodeId(42), "x", factory("x")), Err(SessionError::UnknownNode(_))));
}

#[test]
fn removing_an_effect_restores_it_in_place_with_the_factory_the_caller_chose() {
    let (mut s, a, _) = setup();
    let x = s.add_effect(a, "X", factory("x")).unwrap();
    let y = s.add_effect(a, "Y", factory("y")).unwrap();
    let z = s.add_effect(a, "Z", factory("z")).unwrap();
    s.set_effect_bypass_untracked(y, true);
    let with_state = factory("avec état");
    let mut seen = Vec::new();
    s.remove_effect(y, &mut |i: &InsertDesc| {
        seen.push(i.id);
        Arc::clone(&with_state)
    })
    .unwrap();
    assert_eq!(seen, vec![y]);
    assert_eq!(chain(&s, a).iter().map(|c| c.0).collect::<Vec<_>>(), [x, z]);
    s.undo();
    assert_eq!(chain(&s, a), [(x, "X".into(), false), (y, "Y".into(), true), (z, "Z".into(), false)], "même place, même identifiant, contournement conservé");
    assert!(Arc::ptr_eq(&s.graph().nodes[a.0 as usize].inserts[1].factory, &with_state));
    assert_eq!(s.remove_effect(999, &mut keep).unwrap_err(), SessionError::UnknownInsert(999));
}

#[test]
fn moving_an_effect_reorders_the_chain_and_is_undoable() {
    let (mut s, a, _) = setup();
    let x = s.add_effect(a, "X", factory("x")).unwrap();
    let y = s.add_effect(a, "Y", factory("y")).unwrap();
    let z = s.add_effect(a, "Z", factory("z")).unwrap();
    let ids = |s: &Session| chain(s, a).iter().map(|c| c.0).collect::<Vec<_>>();
    s.move_effect(z, Some(x)).unwrap();
    assert_eq!(ids(&s), [z, x, y]);
    s.move_effect(z, None).unwrap();
    assert_eq!(ids(&s), [x, y, z]);
    s.move_effect(x, Some(z)).unwrap();
    assert_eq!(ids(&s), [y, x, z]);
    assert_eq!(s.undo_label(), Some("Déplacer un plugin"));
    for _ in 0..3 {
        s.undo();
    }
    assert_eq!(ids(&s), [x, y, z]);
    // Sans changement : rien dans l'historique.
    let label = s.undo_label().map(str::to_owned);
    s.move_effect(y, Some(z)).unwrap();
    s.move_effect(y, Some(y)).unwrap();
    assert_eq!(s.undo_label().map(str::to_owned), label);
    assert_eq!(s.move_effect(y, Some(999)).unwrap_err(), SessionError::UnknownInsert(999));
}

#[test]
fn bypass_is_not_in_the_history_and_survives_undo_of_other_edits() {
    let (mut s, a, _) = setup();
    let x = s.add_effect(a, "X", factory("x")).unwrap();
    s.rename_node(a, "Autre nom").unwrap();
    assert!(s.set_effect_bypass_untracked(x, true));
    assert!(!s.set_effect_bypass_untracked(999, true));
    s.undo(); // annule le renommage
    assert!(chain(&s, a)[0].2, "le contournement n'a pas été touché");
    assert_eq!(s.undo_label(), Some("Ajouter un plugin"), "rien n'a été inscrit pour le contournement");
}

#[test]
fn a_deleted_track_brings_its_effects_back_with_their_bypass() {
    let (mut s, a, b) = setup();
    let x = s.add_effect(b, "X", factory("x")).unwrap();
    s.set_effect_bypass_untracked(x, true);
    s.remove_node(b, &mut keep).unwrap();
    assert!(s.locate_insert(x).is_none());
    s.undo();
    assert_eq!(s.locate_insert(x), Some((b.0 as usize, 0)));
    assert!(chain(&s, b)[0].2);
    let _ = a;
}
