//! Voies d'automation par le protocole : courbes, modes, niveaux d'envoi, refus sans trace, annulation.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{LaneInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> Host {
    let (_, handle) = Engine::new(SR, 256);
    let (session, meta) = demo_session(SR);
    Host::new(handle, session, meta).unwrap()
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn error(ev: &[Event]) -> String {
    ev.iter().find_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).expect("une erreur")
}

fn undo_label(ev: &[Event]) -> Option<String> {
    ev.iter().rev().find_map(|e| if let Event::History { undo_label, .. } = e { Some(undo_label.clone()) } else { None }).flatten()
}

fn lanes(ev: &[Event], node: u32) -> Vec<LaneInfo> {
    project(ev).nodes.iter().find(|n| n.id == node).unwrap().automation.clone()
}

const POINTS: &str = r#"[{"pos":[0,1],"value":0.5,"curve":"linear"},{"pos":[1,1],"value":1.0,"curve":"step"}]"#;

#[test]
fn lanes_are_set_with_a_mode_changed_and_removed_each_in_one_undo_step() {
    let mut h = host();
    let ev = h.command_json(&format!(r#"{{"type":"set_lane","node":1,"target":{{"kind":"gain"}},"points":{POINTS},"label":"Dessiner la courbe"}}"#));
    let l = lanes(&ev, 1);
    assert_eq!(l.len(), 1);
    assert_eq!((l[0].mode.as_str(), l[0].points.len(), l[0].points[1].curve.as_str()), ("trim", 2, "step"));
    assert_eq!(undo_label(&ev).as_deref(), Some("Dessiner la courbe"));
    let ev = h.command_json(r#"{"type":"set_lane_mode","node":1,"target":{"kind":"gain"},"mode":"trim"}"#);
    assert_eq!(lanes(&ev, 1)[0].mode, "trim");
    assert_eq!(undo_label(&ev).as_deref(), Some("Mode d'automation"));
    // Une courbe neuve est en trim par défaut ; on la passe en lecture puis en désactivée.
    let ev = h.command_json(r#"{"type":"set_lane_mode","node":1,"target":{"kind":"gain"},"mode":"read"}"#);
    assert_eq!(lanes(&ev, 1)[0].mode, "read");
    let ev = h.command_json(r#"{"type":"set_lane_mode","node":1,"target":{"kind":"gain"},"mode":"trim"}"#);
    assert_eq!(lanes(&ev, 1)[0].mode, "trim");
    // Remplacer les points garde le mode de la courbe existante quand `mode` est absent.
    let ev = h.command_json(r#"{"type":"set_lane","node":1,"target":{"kind":"gain"},"points":[{"pos":[0,1],"value":0.8,"curve":"linear"}]}"#);
    assert_eq!((lanes(&ev, 1)[0].mode.as_str(), lanes(&ev, 1)[0].points.len()), ("trim", 1));
    // Sans point : la courbe est supprimée.
    let ev = h.command_json(r#"{"type":"set_lane","node":1,"target":{"kind":"gain"},"points":[]}"#);
    assert!(lanes(&ev, 1).is_empty());
    assert_eq!(undo_label(&ev).as_deref(), Some("Supprimer l'automation"));
    h.command_json(r#"{"type":"undo"}"#);
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(lanes(&ev, 1)[0].points.len(), 1);
}

#[test]
fn a_send_level_curve_follows_the_send_and_a_curve_cannot_be_created_without_it() {
    let mut h = host();
    h.command_json(r#"{"type":"add_track","kind":"bus","name":"Réverbe"}"#);
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let bus = project(&ev).nodes.iter().find(|n| n.name == "Réverbe").unwrap().id;
    // Sans envoi, le moteur refuse la courbe de niveau d'envoi : rien n'est enregistré.
    let bad = h.command_json(&format!(r#"{{"type":"set_lane","node":1,"target":{{"kind":"send","to":{bus}}},"points":{POINTS}}}"#));
    assert!(!error(&bad).is_empty());
    h.command_json(&format!(r#"{{"type":"add_send","node":1,"to":{bus}}}"#));
    let ev = h.command_json(&format!(r#"{{"type":"set_lane","node":1,"target":{{"kind":"send","to":{bus}}},"mode":"trim","points":{POINTS}}}"#));
    assert_eq!(lanes(&ev, 1).len(), 1);
    // Retirer l'envoi : la courbe disparaît de l'affichage (le moteur reste valide) ; annuler la rend.
    let ev = h.command_json(&format!(r#"{{"type":"remove_send","node":1,"to":{bus}}}"#));
    assert!(lanes(&ev, 1).is_empty());
    h.command_json(r#"{"type":"undo"}"#);
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!((lanes(&ev, 1).len(), lanes(&ev, 1)[0].mode.as_str()), (1, "trim"));
}

#[test]
fn invalid_lanes_are_refused_without_trace() {
    let mut h = host();
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let label = undo_label(&ev);
    for bad in [
        r#"{"type":"set_lane","node":1,"target":{"kind":"gain"},"points":[{"pos":[1,1],"value":0.5,"curve":"linear"},{"pos":[1,2],"value":0.5,"curve":"linear"}]}"#, // positions non croissantes
        r#"{"type":"set_lane","node":1,"target":{"kind":"gain"},"points":[{"pos":[0,1],"value":-1,"curve":"linear"}]}"#,                                              // gain négatif
        r#"{"type":"set_lane","node":1,"target":{"kind":"pan"},"points":[{"pos":[0,1],"value":2,"curve":"linear"}]}"#,                                                 // pan hors plage
        r#"{"type":"set_lane","node":1,"target":{"kind":"gain"},"points":[{"pos":[0,0],"value":1,"curve":"linear"}]}"#,                                                // fraction invalide
        r#"{"type":"set_lane","node":1,"target":{"kind":"gain"},"points":[{"pos":[0,1],"value":1,"curve":"cubic"}]}"#,                                                 // courbe inconnue
        r#"{"type":"set_lane","node":1,"target":{"kind":"gain"},"mode":"write","points":[{"pos":[0,1],"value":1,"curve":"linear"}]}"#,                                 // mode inconnu
        r#"{"type":"set_lane","node":99,"target":{"kind":"gain"},"points":[{"pos":[0,1],"value":1,"curve":"linear"}]}"#,                                               // nœud inconnu
        r#"{"type":"set_lane_mode","node":1,"target":{"kind":"gain"},"mode":"trim"}"#,                                                                                  // pas de courbe
        r#"{"type":"set_lane","node":1,"target":{"kind":"insert","insert":777,"param":0},"points":[{"pos":[0,1],"value":1,"curve":"linear"}]}"#,                       // plugin inexistant
    ] {
        assert!(!error(&h.command_json(bad)).is_empty(), "{bad}");
    }
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert!(lanes(&ev, 1).is_empty());
    assert_eq!(undo_label(&ev), label);
}
