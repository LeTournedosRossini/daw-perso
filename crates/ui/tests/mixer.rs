//! Mixeur : envois multiples (bus et sortie principale), niveau par envoi, avant / après fader, projet enregistré.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{NodeInfo, ProjectInfo, SendInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn rig() -> (Engine, Host) {
    let (engine, handle) = Engine::new(SR, 256);
    let (session, meta) = demo_session(SR);
    (engine, Host::new(handle, session, meta).unwrap())
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

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de nœud « {name} »"))
}

fn sends(ev: &[Event], name: &str) -> Vec<(u32, f32, bool)> {
    node(ev, name).sends.iter().map(|s: &SendInfo| (s.to, s.gain, s.pre)).collect()
}

/// Énergie de la sortie principale sur 10 blocs de lecture (après avoir laissé la tête sur un passage du morceau).
fn energy(engine: &mut Engine, host: &mut Host) -> f32 {
    host.command_json(r#"{"type":"seek","pos":96000}"#);
    host.command_json(r#"{"type":"transport","action":"play"}"#);
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let mut e = 0.0f32;
    for _ in 0..40 {
        engine.process(&mut l, &mut r);
    }
    for _ in 0..10 {
        engine.process(&mut l, &mut r);
        e += l.iter().chain(&r).map(|v| v * v).sum::<f32>();
    }
    host.command_json(r#"{"type":"transport","action":"stop"}"#);
    e
}

#[test]
fn sends_to_a_bus_and_to_the_main_output_at_once_with_levels_and_pre_fader_all_through_the_protocol() {
    let (_, mut h) = rig();
    h.command_json(r#"{"type":"add_track","kind":"bus","name":"Réverbe"}"#);
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let bus = node(&ev, "Réverbe").id;
    let batt = node(&ev, "Batterie").id;
    h.command_json(&format!(r#"{{"type":"add_send","node":{batt},"to":{bus}}}"#));
    let ev = h.command_json(&format!(r#"{{"type":"add_send","node":{batt},"to":0}}"#));
    assert_eq!(sends(&ev, "Batterie"), vec![(bus, 1.0, false), (0, 1.0, false)]);
    assert_eq!(undo_label(&ev).as_deref(), Some("Ajouter un envoi"));
    // Niveau : hors historique (le dernier libellé reste « Ajouter un envoi »), mais la modification est signalée.
    let ev = h.command_json(&format!(r#"{{"type":"set_send","node":{batt},"to":0,"gain":0.5}}"#));
    assert_eq!(sends(&ev, "Batterie")[1], (0, 0.5, false));
    assert_eq!(undo_label(&h.command_json(r#"{"type":"get_project"}"#)).as_deref(), Some("Ajouter un envoi"), "le niveau n'est pas dans l'historique");
    let ev2 = h.command_json(r#"{"type":"get_project"}"#);
    assert!(ev2.iter().any(|e| matches!(e, Event::File { dirty: true, .. })), "projet modifié");
    // Avant le fader : annulable.
    let ev = h.command_json(&format!(r#"{{"type":"set_send","node":{batt},"to":{bus},"pre":true}}"#));
    assert_eq!(sends(&ev, "Batterie")[0], (bus, 1.0, true));
    assert_eq!(undo_label(&ev).as_deref(), Some("Envoi avant le fader"));
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert!(!ev.is_empty());
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(sends(&ev, "Batterie")[0], (bus, 1.0, false));
    // Retirer, puis annuler.
    let ev = h.command_json(&format!(r#"{{"type":"remove_send","node":{batt},"to":0}}"#));
    assert_eq!(sends(&ev, "Batterie").len(), 1);
    let _ = h.command_json(r#"{"type":"undo"}"#);
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(sends(&ev, "Batterie"), vec![(bus, 1.0, false), (0, 0.5, false)]);
}

#[test]
fn the_engine_hears_the_send_to_the_main_output_and_its_level() {
    let (mut engine, mut h) = rig();
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let batt = node(&ev, "Batterie").id;
    let dry = energy(&mut engine, &mut h);
    h.command_json(&format!(r#"{{"type":"add_send","node":{batt},"to":0}}"#));
    let wet = energy(&mut engine, &mut h);
    assert!(wet > dry * 1.2, "l'envoi vers la sortie principale s'ajoute : {dry} → {wet}");
    h.command_json(&format!(r#"{{"type":"set_send","node":{batt},"to":0,"gain":0}}"#));
    let silent = energy(&mut engine, &mut h);
    assert!((silent - dry).abs() < dry * 0.02, "niveau 0 : retour au son sans envoi ({dry} → {silent})");
}

#[test]
fn invalid_send_commands_are_refused_without_trace() {
    let (_, mut h) = rig();
    h.command_json(r#"{"type":"add_track","kind":"bus"}"#);
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let (bus, batt, basse) = (node(&ev, "Bus 1").id, node(&ev, "Batterie").id, node(&ev, "Basse").id);
    h.command_json(&format!(r#"{{"type":"add_send","node":{batt},"to":{bus}}}"#));
    let label = undo_label(&h.command_json(r#"{"type":"get_project"}"#));
    for bad in [
        format!(r#"{{"type":"add_send","node":{batt},"to":{bus}}}"#),   // doublon
        format!(r#"{{"type":"add_send","node":{batt},"to":{basse}}}"#), // vers une piste
        format!(r#"{{"type":"add_send","node":{bus},"to":{bus}}}"#),    // vers soi-même
        r#"{"type":"add_send","node":0,"to":1}"#.to_string(),          // la sortie principale n'envoie nulle part
        format!(r#"{{"type":"remove_send","node":{batt},"to":0}}"#),    // pas d'envoi à retirer
        format!(r#"{{"type":"set_send","node":{batt},"to":0,"gain":1}}"#),
        format!(r#"{{"type":"set_send","node":{batt},"to":{bus},"gain":5}}"#),
        format!(r#"{{"type":"set_send","node":{batt},"to":{bus}}}"#),
        r#"{"type":"add_send","node":99,"to":0}"#.to_string(),
    ] {
        assert!(!error(&h.command_json(&bad)).is_empty(), "{bad}");
    }
    assert_eq!(undo_label(&h.command_json(r#"{"type":"get_project"}"#)), label);
    // Boucle : un envoi du bus vers la piste n'existe pas ; bus → bus fermant une boucle est refusé.
    h.command_json(r#"{"type":"add_track","kind":"bus","name":"B2"}"#);
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let b2 = node(&ev, "B2").id;
    h.command_json(&format!(r#"{{"type":"add_send","node":{bus},"to":{b2}}}"#));
    assert!(!error(&h.command_json(&format!(r#"{{"type":"add_send","node":{b2},"to":{bus}}}"#))).is_empty());
}

#[test]
fn sends_are_saved_with_the_project_and_survive_the_deletion_of_a_destination_and_its_undo() {
    let dir = std::env::temp_dir().join(format!("daw-mixer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (_, mut a) = rig();
    a.command_json(r#"{"type":"add_track","kind":"bus","name":"Réverbe"}"#);
    let ev = a.command_json(r#"{"type":"get_project"}"#);
    let (bus, batt) = (node(&ev, "Réverbe").id, node(&ev, "Batterie").id);
    a.command_json(&format!(r#"{{"type":"add_send","node":{batt},"to":{bus}}}"#));
    a.command_json(&format!(r#"{{"type":"add_send","node":{batt},"to":0}}"#));
    a.command_json(&format!(r#"{{"type":"set_send","node":{batt},"to":0,"gain":0.25,"pre":true}}"#));
    let path = dir.join("P");
    a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, path.to_string_lossy()));
    let (_, mut b) = rig();
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    let (bus2, batt2) = (node(&ev, "Réverbe").id, node(&ev, "Batterie").id);
    assert_eq!(sends(&ev, "Batterie"), vec![(bus2, 1.0, false), (0, 0.25, true)]);
    // Supprimer le bus retire l'envoi ; annuler le rend avec ses réglages.
    let ev = b.command_json(&format!(r#"{{"type":"delete_track","node":{bus2}}}"#));
    assert_eq!(sends(&ev, "Batterie"), vec![(0, 0.25, true)]);
    b.command_json(r#"{"type":"undo"}"#);
    let ev = b.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(sends(&ev, "Batterie"), vec![(node(&ev, "Réverbe").id, 1.0, false), (0, 0.25, true)]);
    let _ = (batt, batt2);
    let _ = std::fs::remove_dir_all(&dir);
}
