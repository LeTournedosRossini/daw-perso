//! Voies d'automation : modes, niveaux d'envoi, courbes qui suivent la disparition et le retour de leur cible, projet.

use daw_core_types::time::Frac;
use daw_core_types::NodeId;
use daw_engine::{AutoCurve, AutoMode, AutoTarget, GraphDesc, TrackFormat};
use daw_session::{MLane, MPoint, Session};

fn f(n: u64, d: u64) -> Frac {
    Frac::new(n, d).unwrap()
}

fn keep(_: &daw_engine::InsertDesc) -> daw_engine::InsertFactory {
    std::sync::Arc::new(|_, _, _| Err("pas d'effet dans ce test".to_owned()))
}

fn lane(target: AutoTarget, mode: AutoMode) -> MLane {
    MLane { target, points: vec![MPoint { pos: Frac::ZERO, value: 0.5, curve: AutoCurve::Linear }, MPoint { pos: f(1, 1), value: 1.0, curve: AutoCurve::Linear }], mode }
}

fn setup() -> (Session, NodeId, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let b = g.add_bus("Bus");
    (Session::new(g), t, b)
}

struct NoAudio;

impl daw_session::project::AudioLoader for NoAudio {
    fn load(&self, path: &str) -> Result<std::sync::Arc<daw_engine::AudioData>, String> {
        Err(format!("pas d'audio : {path}"))
    }
}

#[test]
fn the_mode_travels_with_the_lane_to_the_engine_and_changing_it_is_one_undo_step() {
    let (mut s, t, _) = setup();
    s.set_lane(t, lane(AutoTarget::Gain, AutoMode::Read)).unwrap();
    assert_eq!(s.graph().nodes[t.0 as usize].automation[0].mode, AutoMode::Read);
    s.set_lane_as("Mode d'automation", t, lane(AutoTarget::Gain, AutoMode::Trim)).unwrap();
    assert_eq!(s.graph().nodes[t.0 as usize].automation[0].mode, AutoMode::Trim);
    assert_eq!(s.undo_label(), Some("Mode d'automation"));
    s.undo();
    assert_eq!(s.graph().nodes[t.0 as usize].automation[0].mode, AutoMode::Read);
}

#[test]
fn a_send_curve_follows_its_send_away_and_back_and_the_engine_never_sees_an_orphan() {
    let (mut s, t, b) = setup();
    s.add_send(t, b).unwrap();
    s.set_lane(t, lane(AutoTarget::SendGain { to: b }, AutoMode::Trim)).unwrap();
    assert_eq!(s.graph().nodes[t.0 as usize].automation.len(), 1);
    // Retirer l'envoi : la courbe reste dans la session mais le moteur ne la reçoit plus (le graphe reste valide).
    s.remove_send(t, b).unwrap();
    assert!(s.graph().nodes[t.0 as usize].automation.is_empty());
    assert!(s.graph().validate().is_ok());
    assert!(s.lane(t, AutoTarget::SendGain { to: b }).is_some(), "la courbe est conservée");
    // Annuler : l'envoi et sa courbe reviennent.
    s.undo();
    assert_eq!(s.graph().nodes[t.0 as usize].automation.len(), 1);
    assert_eq!(s.graph().nodes[t.0 as usize].automation[0].mode, AutoMode::Trim);
}

#[test]
fn a_send_curve_survives_the_deletion_and_restoration_of_its_destination_with_renumbering() {
    let (mut s, t, b) = setup();
    let c = {
        // Un deuxième bus après le premier : sa suppression ne doit pas toucher à la courbe vers le premier ; celle du premier,
        // si on le supprime, doit revenir à l'annulation avec le bon numéro.
        let mut g = s.graph().clone();
        g.add_bus("Bus 2");
        s = Session::new(g);
        NodeId(3)
    };
    s.add_send(t, b).unwrap();
    s.add_send(t, c).unwrap();
    s.set_lane(t, lane(AutoTarget::SendGain { to: b }, AutoMode::Read)).unwrap();
    s.set_lane(t, lane(AutoTarget::SendGain { to: c }, AutoMode::Trim)).unwrap();
    // Supprimer le premier bus : sa courbe disparaît, celle du second est renumérotée (Bus 2 devient le nœud 2).
    s.remove_node(b, &mut keep).unwrap();
    let auto = &s.graph().nodes[t.0 as usize].automation;
    assert_eq!(auto.len(), 1);
    assert_eq!(auto[0].target, AutoTarget::SendGain { to: NodeId(2) });
    assert!(s.graph().validate().is_ok());
    // Annuler : les deux courbes reviennent avec leurs numéros d'origine.
    s.undo();
    let mut targets: Vec<_> = s.graph().nodes[t.0 as usize].automation.iter().map(|l| (l.target, l.mode)).collect();
    targets.sort_by_key(|(t, _)| format!("{t:?}"));
    assert_eq!(targets, vec![(AutoTarget::SendGain { to: b }, AutoMode::Read), (AutoTarget::SendGain { to: c }, AutoMode::Trim)]);
    assert!(s.graph().validate().is_ok());
}

#[test]
fn send_curves_and_modes_are_saved_and_a_version_8_project_reads_as_read_mode() {
    let (mut s, t, b) = setup();
    s.add_send(t, b).unwrap();
    s.set_lane(t, lane(AutoTarget::SendGain { to: b }, AutoMode::Trim)).unwrap();
    s.set_lane(t, lane(AutoTarget::Gain, AutoMode::Off)).unwrap();
    let rate = s.sample_rate();
    let (json, _) = s.to_json(rate).unwrap();
    assert!(json.contains("\"mode\": \"trim\"") && json.contains("send:2"), "{json}");
    let (back, _) = Session::from_json(&json, &NoAudio).unwrap();
    assert_eq!(back.lane(t, AutoTarget::SendGain { to: b }).unwrap().mode, AutoMode::Trim);
    assert_eq!(back.lane(t, AutoTarget::Gain).unwrap().mode, AutoMode::Off);
    // Un fichier de la version 8 : pas de `mode` dans les courbes.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["version"] = serde_json::Value::from(8);
    for n in v["nodes"].as_array_mut().unwrap() {
        for l in n["automation"].as_array_mut().unwrap() {
            l.as_object_mut().unwrap().remove("mode");
        }
    }
    let (old, _) = Session::from_json(&v.to_string(), &NoAudio).unwrap();
    assert_eq!(old.lane(t, AutoTarget::Gain).unwrap().mode, AutoMode::Read);
}
