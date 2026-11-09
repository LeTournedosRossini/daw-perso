//! Envois : plusieurs destinations (bus, sortie principale), niveau, avant / après fader, annulation, projet.

use daw_core_types::NodeId;
use daw_engine::{GraphDesc, TrackFormat};
use daw_session::Session;

fn setup() -> (Session, NodeId, NodeId, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let b1 = g.add_bus("Bus 1");
    let b2 = g.add_bus("Bus 2");
    (Session::new(g), t, b1, b2)
}

fn sends(s: &Session, n: NodeId) -> Vec<(u32, f32, bool)> {
    s.graph().nodes[n.0 as usize].sends.iter().map(|x| (x.to.0, x.gain, x.pre)).collect()
}

#[test]
fn a_track_can_send_to_two_buses_and_to_the_main_output_at_once_and_each_step_is_undoable() {
    let (mut s, t, b1, b2) = setup();
    s.add_send(t, b1).unwrap();
    s.add_send(t, NodeId::MASTER).unwrap();
    s.add_send(t, b2).unwrap();
    assert_eq!(sends(&s, t), vec![(b1.0, 1.0, false), (0, 1.0, false), (b2.0, 1.0, false)]);
    assert_eq!(s.undo_label(), Some("Ajouter un envoi"));
    // Niveau d'un envoi : hors historique ; avant le fader : annulable.
    assert!(s.set_send_gain_untracked(t, NodeId::MASTER, 0.3));
    assert!(!s.set_send_gain_untracked(t, NodeId(99), 0.3));
    s.set_send_pre(t, b1, true).unwrap();
    assert_eq!(s.undo_label(), Some("Envoi avant le fader"));
    s.undo();
    assert_eq!(sends(&s, t)[0], (b1.0, 1.0, false));
    assert_eq!(sends(&s, t)[1], (0, 0.3, false), "le niveau posé hors historique reste");
    s.remove_send(t, NodeId::MASTER).unwrap();
    assert_eq!(sends(&s, t).len(), 2);
    s.undo();
    assert_eq!(sends(&s, t)[1], (0, 0.3, false), "l'envoi supprimé revient à sa place avec son niveau");
}

#[test]
fn invalid_sends_are_refused_without_trace() {
    let (mut s, t, b1, _) = setup();
    s.add_send(t, b1).unwrap();
    let depth = s.undo_label().map(str::to_owned);
    assert!(s.add_send(t, b1).is_err(), "deux envois vers la même destination");
    assert!(s.add_send(t, t).is_err(), "vers soi-même");
    assert!(s.add_send(b1, b1).is_err());
    assert!(s.add_send(NodeId::MASTER, b1).is_err(), "la sortie principale n'envoie nulle part");
    assert!(s.add_send(t, NodeId(50)).is_err());
    assert!(s.remove_send(t, NodeId::MASTER).is_err(), "pas d'envoi à retirer");
    assert!(s.set_send_pre(t, NodeId::MASTER, true).is_err());
    assert_eq!(s.undo_label().map(str::to_owned), depth);
    assert_eq!(sends(&s, t).len(), 1);
}

#[test]
fn a_loop_through_sends_is_refused() {
    let (mut s, _, b1, b2) = setup();
    s.add_send(b1, b2).unwrap();
    assert!(s.add_send(b2, b1).is_err(), "Bus 2 → Bus 1 fermerait la boucle");
}

#[test]
fn sends_survive_the_deletion_and_restoration_of_their_destination_and_a_save_reload() {
    let (mut s, t, b1, _) = setup();
    s.add_send(t, b1).unwrap();
    s.add_send(t, NodeId::MASTER).unwrap();
    s.set_send_pre(t, NodeId::MASTER, true).unwrap();
    s.set_send_gain_untracked(t, NodeId::MASTER, 0.25);
    let rate = s.sample_rate();
    let (json, _) = s.to_json(rate).unwrap();
    assert!(json.contains("\"pre\": true"));
    let (back, _) = Session::from_json(&json, &NoAudio).unwrap();
    assert_eq!(sends(&back, t), vec![(b1.0, 1.0, false), (0, 0.25, true)]);
    // Supprimer le bus retire son envoi ; annuler le rend avec ses réglages.
    s.remove_node(b1, &mut keep).unwrap();
    assert_eq!(sends(&s, t), vec![(0, 0.25, true)]);
    s.undo();
    assert_eq!(sends(&s, t), vec![(b1.0, 1.0, false), (0, 0.25, true)]);
}

#[test]
fn a_version_7_project_loads_with_sends_after_the_fader() {
    let (mut s, t, b1, _) = setup();
    s.add_send(t, b1).unwrap();
    let rate = s.sample_rate();
    let (json, _) = s.to_json(rate).unwrap();
    // Un fichier de la version 7 : pas de champ `pre`, numéro de version 7.
    let old = json.replace("\"version\": 19", "\"version\": 7").replace(",\n          \"pre\": false", "").replace(",\n        \"pre\": false", "");
    let mut value: serde_json::Value = serde_json::from_str(&old).unwrap();
    for n in value["nodes"].as_array_mut().unwrap() {
        for send in n["sends"].as_array_mut().unwrap() {
            send.as_object_mut().unwrap().remove("pre");
        }
    }
    let (back, _) = Session::from_json(&value.to_string(), &NoAudio).unwrap();
    assert_eq!(sends(&back, t), vec![(b1.0, 1.0, false)]);
}

fn keep(_: &daw_engine::InsertDesc) -> daw_engine::InsertFactory {
    std::sync::Arc::new(|_, _, _| Err("pas d'effet dans ce test".to_owned()))
}

struct NoAudio;

impl daw_session::project::AudioLoader for NoAudio {
    fn load(&self, path: &str) -> Result<std::sync::Arc<daw_engine::AudioData>, String> {
        Err(format!("pas d'audio : {path}"))
    }
}
