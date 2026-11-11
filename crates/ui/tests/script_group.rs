//! Scripts : `begin_group` / `end_group` / `abort_group` font d'une suite de commandes une seule action annulable.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

fn host() -> (Engine, Host) {
    let (engine, handle) = Engine::new(48_000, 64);
    let (session, meta) = demo_session(48_000);
    (engine, Host::new(handle, session, meta).unwrap())
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap()
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

fn history(ev: &[Event]) -> (bool, Option<String>) {
    ev.iter().rev().find_map(|e| if let Event::History { can_undo, undo_label, .. } = e { Some((*can_undo, undo_label.clone())) } else { None }).expect("événement history")
}

fn names(h: &mut Host) -> Vec<String> {
    project(&h.command_json(r#"{"type":"get_project"}"#)).nodes.iter().map(|n| n.name.clone()).collect()
}

#[test]
fn a_script_is_one_undo_step_whatever_it_did() {
    let (_e, mut h) = host();
    let before = names(&mut h);
    let p = h.command_json(r#"{"type":"get_project"}"#);
    let (drums, bass) = (node(&p, "Batterie").id, node(&p, "Basse").id);
    assert!(errors(&h.command_json(r#"{"type":"begin_group","label":"Mon script"}"#)).is_empty());
    h.command_json(&format!(r#"{{"type":"rename_track","node":{drums},"name":"Kick"}}"#));
    h.command_json(&format!(r#"{{"type":"rename_track","node":{bass},"name":"Sub"}}"#));
    h.command_json(r#"{"type":"add_track","kind":"bus","name":"Bus script"}"#);
    let ev = h.command_json(r#"{"type":"end_group"}"#);
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    assert_eq!(history(&ev), (true, Some("Mon script".into())));
    let after = names(&mut h);
    assert!(after.contains(&"Kick".to_string()) && after.contains(&"Sub".to_string()) && after.contains(&"Bus script".to_string()));
    // Un seul Ctrl+Z défait tout le script.
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(project(&ev).nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>(), before);
    // Rétablir le refait d'un coup.
    let ev = h.command_json(r#"{"type":"redo"}"#);
    assert!(project(&ev).nodes.iter().any(|n| n.name == "Bus script") && node(&ev, "Kick").id == drums);
}

#[test]
fn an_aborted_script_leaves_no_trace_and_no_redo() {
    let (_e, mut h) = host();
    let before = names(&mut h);
    let p = h.command_json(r#"{"type":"get_project"}"#);
    let drums = node(&p, "Batterie").id;
    h.command_json(r#"{"type":"begin_group","label":"Raté"}"#);
    h.command_json(&format!(r#"{{"type":"rename_track","node":{drums},"name":"Kick"}}"#));
    h.command_json(r#"{"type":"add_track","kind":"bus","name":"Bus script"}"#);
    let ev = h.command_json(r#"{"type":"abort_group"}"#);
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    assert_eq!(project(&ev).nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>(), before);
    assert_eq!(errors(&h.command_json(r#"{"type":"redo"}"#)), vec!["rien à rétablir".to_string()]);
    assert_eq!(errors(&h.command_json(r#"{"type":"undo"}"#)), vec!["rien à annuler".to_string()]);
}

#[test]
fn a_script_that_changes_nothing_leaves_the_history_alone() {
    let (_e, mut h) = host();
    h.command_json(r#"{"type":"begin_group","label":"Vide"}"#);
    let ev = h.command_json(r#"{"type":"end_group"}"#);
    assert!(errors(&ev).is_empty());
    assert_eq!(history(&ev).0, false);
}

#[test]
fn groups_cannot_be_nested_nor_mixed_with_undo_or_other_projects() {
    let (_e, mut h) = host();
    assert!(errors(&h.command_json(r#"{"type":"end_group"}"#)).len() == 1, "aucun script en cours");
    assert!(errors(&h.command_json(r#"{"type":"abort_group"}"#)).len() == 1);
    h.command_json(r#"{"type":"begin_group","label":"A"}"#);
    assert_eq!(errors(&h.command_json(r#"{"type":"begin_group","label":"B"}"#)).len(), 1);
    for c in [r#"{"type":"undo"}"#, r#"{"type":"redo"}"#, r#"{"type":"new_project"}"#] {
        assert_eq!(errors(&h.command_json(c)).len(), 1, "{c}");
    }
    h.command_json(r#"{"type":"end_group"}"#);
    // Libellé vide ou trop long : refusé sans ouvrir de groupe.
    assert_eq!(errors(&h.command_json(r#"{"type":"begin_group","label":"  "}"#)).len(), 1);
    assert!(errors(&h.command_json(r#"{"type":"undo"}"#)).iter().all(|m| m.contains("rien")), "plus de groupe ouvert");
}
