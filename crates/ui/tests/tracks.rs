//! Pistes et bus : ajouter, supprimer, renommer, colorer, rediriger, réordonner — par le protocole, avec annulation.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::{Engine, GraphDesc, InsertFactory, TrackFormat};
use daw_session::Session;
use daw_test_plugin::{entry, PLUGIN_ID};
use daw_ui::demo::demo_session;
use daw_ui::host::{PluginRef, ProjectMeta};
use daw_ui::protocol::{NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn demo() -> Host {
    let (_, handle) = Engine::new(SR, 256);
    let (session, meta) = demo_session(SR);
    Host::new(handle, session, meta).unwrap()
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de nœud « {name} »"))
}

/// Noms dans l'ordre d'affichage (la sortie principale est en tête de liste dans le protocole).
fn names(ev: &[Event]) -> Vec<&str> {
    project(ev).nodes.iter().map(|n| n.name.as_str()).collect()
}

fn undo_label(ev: &[Event]) -> Option<String> {
    ev.iter().rev().find_map(|e| if let Event::History { undo_label, .. } = e { Some(undo_label.clone()) } else { None }).flatten()
}

fn error(ev: &[Event]) -> String {
    ev.iter().find_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).expect("une erreur")
}

fn get(host: &mut Host) -> Vec<Event> {
    host.command_json(r#"{"type":"get_project"}"#)
}

#[test]
fn adding_tracks_and_buses_names_them_and_is_undoable() {
    let mut h = demo();
    let before = get(&mut h);
    let ev = h.command_json(r#"{"type":"add_track","kind":"mono"}"#);
    let t = node(&ev, "Piste 1");
    assert_eq!((t.kind.as_str(), t.output, t.clips.len()), ("mono", Some(0), 0));
    assert!(t.color.starts_with('#') && t.id == 6);
    assert_eq!(undo_label(&ev).as_deref(), Some("Ajouter une piste"));
    let ev = h.command_json(r#"{"type":"add_track","kind":"stereo"}"#);
    assert!(names(&ev).contains(&"Piste 2"), "le plus petit numéro libre");
    let ev = h.command_json(r#"{"type":"add_track","kind":"bus","name":"  Réverbe  "}"#);
    assert_eq!((node(&ev, "Réverbe").kind.as_str(), names(&ev).last().copied()), ("bus", Some("Réverbe")));
    for _ in 0..3 {
        h.command_json(r#"{"type":"undo"}"#);
    }
    assert_eq!(project(&get(&mut h)), project(&before));
    // Un nom mauvais ou un genre inconnu : refusé, rien n'est inscrit.
    assert!(error(&h.command_json(r#"{"type":"add_track","kind":"guitare"}"#)).contains("inconnu"));
    assert!(error(&h.command_json(r#"{"type":"add_track","kind":"mono","name":"   "}"#)).contains("1 à 60"));
}

#[test]
fn deleting_a_track_renumbers_and_undo_restores_everything() {
    let mut h = demo();
    let before = get(&mut h);
    // Basse (nœud 2) : les suivantes sont renumérotées, les clips gardent leur identifiant.
    let ev = h.command_json(r#"{"type":"delete_track","node":2}"#);
    let ids: Vec<(u32, &str)> = project(&ev).nodes.iter().map(|n| (n.id, n.name.as_str())).collect();
    assert_eq!(ids, [(0, "Master"), (1, "Batterie"), (2, "Claviers"), (3, "Guitare"), (4, "Voix")]);
    let riff = node(&ev, "Guitare").clips.iter().map(|c| c.id()).collect::<Vec<_>>();
    assert_eq!(riff, [3, 4], "les clips de la Guitare gardent leurs identifiants");
    assert_eq!(undo_label(&ev).as_deref(), Some("Supprimer une piste"));
    h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(project(&get(&mut h)), project(&before), "tout est revenu, numéros compris");
    h.command_json(r#"{"type":"redo"}"#);
    assert_eq!(project(&get(&mut h)).nodes.len(), 5);
    // La sortie principale ne se supprime pas ; un nœud inconnu non plus.
    assert!(error(&h.command_json(r#"{"type":"delete_track","node":0}"#)).contains("sortie principale"));
    assert!(error(&h.command_json(r#"{"type":"delete_track","node":99}"#)).contains("inconnu"));
}

#[test]
fn routing_a_track_to_a_bus_is_shown_checked_and_survives_deleting_the_bus() {
    let mut h = demo();
    let ev = h.command_json(r#"{"type":"add_track","kind":"bus","name":"Bus A"}"#);
    let bus = node(&ev, "Bus A").id;
    let ev = h.command_json(&format!(r#"{{"type":"set_track_output","node":1,"output":{bus}}}"#));
    assert_eq!(node(&ev, "Batterie").output, Some(bus));
    assert_eq!(undo_label(&ev).as_deref(), Some("Sortie d'une piste"));
    // Une piste n'est pas une destination ; un bus ne peut pas se diriger vers lui-même ; la boucle est refusée.
    assert!(error(&h.command_json(r#"{"type":"set_track_output","node":1,"output":2}"#)).contains("sortie refusée"));
    assert!(error(&h.command_json(&format!(r#"{{"type":"set_track_output","node":{bus},"output":{bus}}}"#))).contains("sortie refusée"));
    let ev = h.command_json(r#"{"type":"add_track","kind":"bus","name":"Bus B"}"#);
    let bus_b = node(&ev, "Bus B").id;
    h.command_json(&format!(r#"{{"type":"set_track_output","node":{bus_b},"output":{bus}}}"#));
    assert!(error(&h.command_json(&format!(r#"{{"type":"set_track_output","node":{bus},"output":{bus_b}}}"#))).contains("sortie refusée"), "boucle A → B → A");
    // Supprimer Bus A renvoie la Batterie (et Bus B) vers la sortie principale ; annuler rebranche.
    let ev = h.command_json(&format!(r#"{{"type":"delete_track","node":{bus}}}"#));
    assert_eq!((node(&ev, "Batterie").output, node(&ev, "Bus B").output), (Some(0), Some(0)));
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!((node(&ev, "Batterie").output, node(&ev, "Bus B").output), (Some(bus), Some(bus)));
    assert!(error(&h.command_json(r#"{"type":"set_track_output","node":0,"output":1}"#)).contains("sortie principale"));
}

#[test]
fn rename_color_and_move_are_undoable_and_validated() {
    let mut h = demo();
    let ev = h.command_json(r#"{"type":"rename_track","node":2,"name":"  Basse électrique "}"#);
    assert_eq!(node(&ev, "Basse électrique").id, 2);
    let ev = h.command_json(r##"{"type":"set_track_color","node":2,"color":"#AABBCC"}"##);
    assert_eq!(node(&ev, "Basse électrique").color, "#aabbcc");
    // Voix (5) juste avant Batterie (1), puis à la fin.
    let ev = h.command_json(r#"{"type":"move_track","node":5,"before":1}"#);
    assert_eq!(names(&ev), ["Master", "Voix", "Batterie", "Basse électrique", "Claviers", "Guitare"]);
    assert_eq!(undo_label(&ev).as_deref(), Some("Déplacer une piste"));
    let ev = h.command_json(r#"{"type":"move_track","node":5}"#);
    assert_eq!(names(&ev).last(), Some(&"Voix"));
    for _ in 0..4 {
        h.command_json(r#"{"type":"undo"}"#);
    }
    let ev = get(&mut h);
    assert_eq!(names(&ev), ["Master", "Batterie", "Basse", "Claviers", "Guitare", "Voix"]);
    assert_eq!(node(&ev, "Basse").color, "#f5a524");
    for bad in [
        r#"{"type":"rename_track","node":2,"name":""}"#,
        r#"{"type":"rename_track","node":0,"name":"x"}"#,
        r#"{"type":"set_track_color","node":2,"color":"rouge"}"#,
        r##"{"type":"set_track_color","node":2,"color":"#12345"}"##,
        r#"{"type":"move_track","node":0}"#,
        r#"{"type":"move_track","node":2,"before":99}"#,
    ] {
        let ev = h.command_json(bad);
        assert!(!error(&ev).is_empty(), "{bad}");
    }
    assert_eq!(undo_label(&get(&mut h)), None, "rien n'a été inscrit");
}

#[test]
fn structure_changes_mark_the_project_unsaved_end_a_fade_preview_and_disarm_tracks() {
    let mut h = demo();
    h.command_json(r#"{"type":"set_node","node":1,"arm":true}"#);
    assert!(project(&get(&mut h)).nodes.iter().any(|n| n.arm));
    h.command_json(r#"{"type":"preview_fade","clip_out":3,"clip_in":4,"margin_ms":0,"solo":true}"#);
    let ev = h.command_json(r#"{"type":"add_track","kind":"stereo"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Preview { active: false })), "l'aperçu est arrêté d'abord");
    assert!(!project(&ev).nodes.iter().any(|n| n.arm), "les armements tenaient à des numéros : défaits");
    assert!(!project(&ev).nodes.iter().any(|n| n.solo), "les solos de l'aperçu sont rendus");
    assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: true, .. })));
}

#[test]
fn colors_and_order_are_saved_with_the_project() {
    let dir = std::env::temp_dir().join(format!("daw-pistes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut a = demo();
    a.command_json(r#"{"type":"add_track","kind":"bus","name":"Bus A"}"#);
    a.command_json(r#"{"type":"set_track_output","node":1,"output":6}"#);
    a.command_json(r##"{"type":"set_track_color","node":3,"color":"#010203"}"##);
    a.command_json(r#"{"type":"move_track","node":5,"before":1}"#);
    a.command_json(r#"{"type":"delete_track","node":2}"#);
    a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    let mut b = demo();
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    let want = get(&mut a);
    // Les identifiants de clips sont réattribués à l'ouverture : on compare tout le reste.
    let summary = |p: &ProjectInfo| p.nodes.iter().map(|n| (n.id, n.name.clone(), n.kind.clone(), n.color.clone(), n.output, n.gain.to_bits(), n.clips.iter().map(|c| match c {
        daw_ui::protocol::ClipInfo::Audio { name, start, len, .. } | daw_ui::protocol::ClipInfo::Midi { name, start, len, .. } => (name.clone(), *start, *len),
    }).collect::<Vec<_>>())).collect::<Vec<_>>();
    assert_eq!(summary(project(&ev)), summary(project(&want)), "ordre, couleurs, sorties, noms : identiques après réouverture");
    assert_eq!(names(&ev), ["Master", "Voix", "Batterie", "Claviers", "Guitare", "Bus A"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Projet à une piste avec le plugin de test dont l'état initial ajoute 100 échantillons de latence.
fn plugin_host() -> Host {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let plain = clap_insert_factory(bundle, PLUGIN_ID.to_str().unwrap().into());
    let mut state = b"DTP2".to_vec();
    state.extend_from_slice(&0.25f64.to_le_bytes());
    state.extend_from_slice(&100u32.to_le_bytes());
    let with_state: InsertFactory = Arc::new(move |sr, mb, _| plain(sr, mb, Some(&state)));
    let mut g = GraphDesc::new();
    let first = g.add_track("Autre", TrackFormat::Stereo);
    let synth = g.add_track("Synthé", TrackFormat::Stereo);
    let id = g.add_insert(synth, "Gain de test", with_state).unwrap();
    let mut meta = ProjectMeta::default();
    meta.plugins.insert(id, PluginRef { clap_id: PLUGIN_ID.to_str().unwrap().into(), vendor: String::new(), name: "Gain de test".into(), file: String::new() });
    let _ = first;
    let (_, handle) = Engine::new(SR, 256);
    Host::new(handle, Session::new_at(g, SR), meta).unwrap()
}

#[test]
fn deleting_a_track_with_a_plugin_and_undoing_brings_the_plugin_back_with_its_state() {
    let mut h = plugin_host();
    assert_eq!(project(&get(&mut h)).latency_samples, 164);
    let ev = h.command_json(r#"{"type":"delete_track","node":2}"#);
    assert_eq!(project(&ev).latency_samples, 0, "le plugin est parti avec sa piste");
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(project(&ev).latency_samples, 164, "revenu avec son état (la latence en dépend)");
    // La piste restaurée garde son plugin : l'enregistrement le retrouve.
    let dir = std::env::temp_dir().join(format!("daw-pistes-plugin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ev = h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    assert!(!ev.iter().any(|e| matches!(e, Event::File { warnings, .. } if !warnings.is_empty())), "{ev:?}");
    let text = std::fs::read_to_string(dir.join("P.daw/projet.json")).unwrap();
    assert!(text.contains("com.daw-perso.test-gain"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn polarity_is_set_shown_marks_the_project_unsaved_and_survives_save_and_reopen() {
    let dir = std::env::temp_dir().join(format!("daw-polarite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut a = demo();
    assert!(!node(&get(&mut a), "Batterie").invert);
    a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    let ev = a.command_json(r#"{"type":"set_node","node":1,"invert":true}"#);
    assert!(node(&ev, "Batterie").invert);
    assert!(!node(&ev, "Basse").invert, "seule la piste visée");
    assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: true, .. })), "la polarité compte comme modification");
    assert_eq!(undo_label(&get(&mut a)), None, "hors historique, comme le mixage");
    a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    let mut b = demo();
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    assert!(node(&ev, "Batterie").invert && !node(&ev, "Voix").invert);
    // Supprimer une piste autre que la Batterie ne déplace pas la polarité sur la mauvaise piste.
    let ev = b.command_json(r#"{"type":"delete_track","node":2}"#);
    assert!(node(&ev, "Batterie").invert);
    let ev = b.command_json(r#"{"type":"undo"}"#);
    assert!(node(&ev, "Batterie").invert && !node(&ev, "Basse").invert);
    std::fs::remove_dir_all(&dir).unwrap();
}
