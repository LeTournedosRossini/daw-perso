//! Marqueurs de repère dans la session : tri, identifiants stables, refus sans trace, annulation.

use daw_core_types::time::Frac;
use daw_engine::GraphDesc;
use daw_session::markers::{Marker, MAX_NAME};
use daw_session::{Session, SessionError};

fn at(n: u64, d: u64) -> Frac {
    Frac::new(n, d).unwrap()
}

#[test]
fn markers_are_sorted_ids_are_never_reused_and_bad_lists_leave_no_trace() {
    let mut s = Session::new(GraphDesc::new());
    let a = s.add_marker(at(2, 1), "B", None).unwrap();
    let b = s.add_marker(at(1, 1), "A", Some("#112233".into())).unwrap();
    assert_ne!(a, b);
    assert_eq!(s.markers().iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["A", "B"], "triés par instant");

    // Annuler deux fois puis poser : l'identifiant n'est pas repris.
    assert!(s.undo());
    assert!(s.undo());
    assert!(s.markers().is_empty());
    let c = s.add_marker(at(0, 1), "C", None).unwrap();
    assert!(c > b, "identifiant jamais réutilisé après annulation : {c} > {b}");

    let before = s.markers().to_vec();
    let depth = s.can_undo();
    let bad = |list: Vec<Marker>| matches!(s_set(&list), Err(SessionError::BadMarkers(_)));
    fn s_set(list: &[Marker]) -> Result<(), SessionError> {
        let mut s = Session::new(GraphDesc::new());
        s.set_markers("x", list.to_vec())
    }
    assert!(bad(vec![Marker { id: 1, at: at(0, 1), name: String::new(), color: None }]), "nom vide");
    assert!(bad(vec![Marker { id: 1, at: at(0, 1), name: "   ".into(), color: None }]), "nom d'espaces");
    assert!(bad(vec![Marker { id: 1, at: at(0, 1), name: "x".repeat(MAX_NAME + 1), color: None }]), "nom trop long");
    assert!(bad(vec![Marker { id: 1, at: at(0, 1), name: "a\nb".into(), color: None }]), "caractère de contrôle");
    assert!(bad(vec![Marker { id: 1, at: at(0, 1), name: "a".into(), color: Some("rouge".into()) }]), "couleur");
    assert!(bad(vec![Marker { id: 7, at: at(0, 1), name: "a".into(), color: None }, Marker { id: 7, at: at(1, 1), name: "b".into(), color: None }]), "même identifiant");
    let r = s.set_markers("x", vec![Marker { id: 9, at: at(0, 1), name: String::new(), color: None }]);
    assert!(matches!(r, Err(SessionError::BadMarkers(_))));
    assert_eq!(s.markers(), &before[..], "un refus ne change rien");
    assert_eq!(s.can_undo(), depth, "un refus n'inscrit rien dans l'historique");
}
