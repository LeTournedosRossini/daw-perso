//! Retours de monitoring dans la session : sortie physique d'un bus, annulation, projet.

use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, GraphDesc, PhysOut, TrackFormat};
use daw_session::project::AudioLoader;
use daw_session::{Session, SessionError};

struct NoAudio;
impl AudioLoader for NoAudio {
    fn load(&self, _: &str) -> Result<Arc<AudioData>, String> {
        Err("pas d'audio".into())
    }
}

fn session() -> (Session, NodeId, NodeId, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let a = g.add_return("Retour Marie");
    let b = g.add_return("Retour Paul");
    (Session::new(g), t, a, b)
}

const MARIE: PhysOut = PhysOut { first: 2, stereo: true, mono: false };
const PAUL: PhysOut = PhysOut { first: 4, stereo: false, mono: true };

#[test]
fn a_bus_gets_a_physical_output_in_one_undo_step_and_gives_it_back() {
    let (mut s, _, a, b) = session();
    s.set_node_phys(a, Some(MARIE)).unwrap();
    s.set_node_phys(b, Some(PAUL)).unwrap();
    assert_eq!(s.graph().nodes[a.0 as usize].phys, Some(MARIE));
    assert_eq!(s.undo_label(), Some("Sortie d'un retour"));
    assert!(s.undo());
    assert_eq!(s.graph().nodes[b.0 as usize].phys, None);
    assert!(s.redo());
    assert_eq!(s.graph().nodes[b.0 as usize].phys, Some(PAUL));
    s.set_node_phys(b, None).unwrap();
    assert_eq!(s.undo_label(), Some("Retirer la sortie d'un retour"));
    // La même valeur n'inscrit rien.
    let before = s.undo_label().map(str::to_owned);
    s.set_node_phys(a, Some(MARIE)).unwrap();
    assert_eq!(s.undo_label().map(str::to_owned), before);
}

#[test]
fn refused_without_a_trace_a_track_the_main_channels_a_taken_channel() {
    let (mut s, t, a, b) = session();
    s.set_node_phys(a, Some(MARIE)).unwrap();
    let label = s.undo_label().map(str::to_owned);
    for (node, phys) in [
        (t, PAUL),                                                      // une piste ne sort pas sur la carte
        (b, PhysOut { first: 0, stereo: true, mono: false }),           // la sortie principale
        (b, PhysOut { first: 3, stereo: false, mono: true }),           // canal pris par la paire 2-3
        (b, PhysOut { first: 31, stereo: true, mono: false }),          // au-delà du maximum
    ] {
        assert_eq!(s.set_node_phys(node, Some(phys)), Err(SessionError::BadPhysOut(node)));
    }
    assert_eq!(s.undo_label().map(str::to_owned), label, "aucune trace");
    assert_eq!(s.graph().nodes[b.0 as usize].phys, None);
    assert_eq!(s.set_node_phys(NodeId(99), Some(PAUL)), Err(SessionError::UnknownNode(NodeId(99))));
}

#[test]
fn the_output_is_kept_in_the_project_and_follows_the_bus_when_it_is_deleted_and_restored() {
    let (mut s, _, a, b) = session();
    s.set_node_phys(a, Some(MARIE)).unwrap();
    s.set_node_phys(b, Some(PAUL)).unwrap();
    let (json, _) = s.to_json(48_000).unwrap();
    assert!(json.contains("\"phys\"") && json.contains("\"stereo\": false"));
    let (back, _) = Session::from_json(&json, &NoAudio).unwrap();
    assert_eq!(back.graph().nodes[a.0 as usize].phys, Some(MARIE));
    assert_eq!(back.graph().nodes[b.0 as usize].phys, Some(PAUL));
    assert_eq!(back.to_json(48_000).unwrap().0, json, "réécriture identique");
    // Supprimer puis annuler : la sortie revient avec le bus.
    s.remove_node(a, &mut |d| d.factory.clone()).unwrap();
    assert!(s.undo());
    assert_eq!(s.graph().nodes[a.0 as usize].phys, Some(MARIE));
    // Un projet sans retour n'a pas la clé.
    let (plain, ..) = session();
    assert!(!plain.to_json(48_000).unwrap().0.contains("phys"));
}

#[test]
fn a_return_is_a_type_of_its_own_a_plain_bus_with_an_output_is_not() {
    let (mut s, _, a, _) = session();
    let ret = s.add_node(daw_session::nodes::NewNode::Return, "Retour Lou", "#aabbcc").unwrap();
    let bus = s.add_node(daw_session::nodes::NewNode::Bus, "Sous-groupe", "#112233").unwrap();
    assert!(s.graph().nodes[ret.0 as usize].monitor && !s.graph().nodes[bus.0 as usize].monitor && s.graph().nodes[a.0 as usize].monitor);
    // Un bus ordinaire peut sortir sur la carte, avec son propre libellé d'historique.
    s.set_node_phys(bus, Some(PhysOut { first: 6, stereo: true, mono: false })).unwrap();
    assert_eq!(s.undo_label(), Some("Sortie d'un bus"));
    // Les canaux pris par un bus le sont aussi pour un retour, et inversement (refus sans trace).
    s.set_node_phys(a, Some(MARIE)).unwrap();
    let depth = s.undo_label().map(str::to_owned);
    assert!(matches!(s.set_node_phys(ret, Some(PhysOut { first: 7, stereo: false, mono: true })), Err(SessionError::BadPhysOut(_))), "pris par le bus");
    assert!(matches!(s.set_node_phys(bus, Some(PhysOut { first: 3, stereo: false, mono: true })), Err(SessionError::BadPhysOut(_))), "pris par le retour");
    assert_eq!(s.undo_label().map(str::to_owned), depth, "refus sans trace");
    // Un retour sans sortie reste valide : il se taira.
    // Seul un retour a ses propres groupes.
    assert!(s.set_return_groups("x", bus, None).is_err());
    assert!(s.set_return_groups("x", ret, None).is_ok());
    // Un retour sans sortie physique est valide (il se taira).
    assert!(s.graph().validate().is_ok());
}

#[test]
fn the_return_type_survives_removal_undo_and_a_save() {
    let (mut s, _, a, _) = session();
    s.set_node_phys(a, Some(MARIE)).unwrap();
    let ret = s.add_node(daw_session::nodes::NewNode::Return, "Sans sortie", "#aabbcc").unwrap();
    let bus = s.add_node(daw_session::nodes::NewNode::Bus, "Bus", "#112233").unwrap();
    s.remove_node(ret, &mut |_| unreachable!("aucun effet")).unwrap();
    assert!(s.undo());
    assert!(s.graph().nodes[ret.0 as usize].monitor, "le retour revient avec son type");
    let (json, _) = s.to_json(48_000).unwrap();
    let back = Session::from_json(&json, &NoAudio).unwrap().0;
    assert!(back.graph().nodes[ret.0 as usize].monitor && back.graph().nodes[a.0 as usize].monitor && !back.graph().nodes[bus.0 as usize].monitor);
    assert_eq!(back.graph().nodes[ret.0 as usize].phys, None);
    assert_eq!(back.to_json(48_000).unwrap().0, json, "réécriture identique");
}

#[test]
fn a_version_20_project_keeps_its_returns_a_bus_with_an_output_was_one() {
    let (mut s, _, a, b) = session();
    s.set_node_phys(a, Some(MARIE)).unwrap();
    let (json, _) = s.to_json(48_000).unwrap();
    // Un fichier de la version 20 : pas de champ `monitor`, un bus avec sortie physique, un sans.
    let v20: String = json
        .lines()
        .filter(|l| !l.contains("\"monitor\": true"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace("\"version\": 21", "\"version\": 20");
    let back = Session::from_json(&v20, &NoAudio).unwrap().0;
    assert!(back.graph().nodes[a.0 as usize].monitor, "avec sortie physique : c'était un retour");
    assert!(!back.graph().nodes[b.0 as usize].monitor, "sans sortie physique : un bus");
}
