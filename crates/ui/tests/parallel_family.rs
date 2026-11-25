//! Pistes parallèles rattachées à leur parente : lien, ordre d'affichage, déplacement en bloc, suppression en cascade, enregistrement.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> Host {
    let (engine, handle) = Engine::new(SR, 64);
    std::mem::forget(engine);
    let (session, meta) = demo_session(SR);
    Host::new(handle, session, meta).unwrap()
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de nœud « {name} »"))
}

fn names(ev: &[Event]) -> Vec<String> {
    project(ev).nodes.iter().map(|n| n.name.clone()).collect()
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

fn get(h: &mut Host) -> Vec<Event> {
    h.command_json(r#"{"type":"get_project"}"#)
}

fn add_par(h: &mut Host, node_id: u32, name: &str) -> Vec<Event> {
    h.command_json(&format!(r#"{{"type":"add_parallel","node":{node_id},"tap":"input","name":{name:?}}}"#))
}

#[test]
fn a_parallel_knows_its_parent_and_sits_right_after_its_family() {
    let mut h = host();
    let ev = get(&mut h);
    let g = node(&ev, "Guitare").id;
    let ev = add_par(&mut h, g, "P1");
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let ev = add_par(&mut h, g, "P2");
    let (g, p1, p2) = (node(&ev, "Guitare").id, node(&ev, "P1").id, node(&ev, "P2").id);
    assert_eq!(node(&ev, "Guitare").parent, None);
    assert_eq!((node(&ev, "P1").parent, node(&ev, "P2").parent), (Some(g), Some(g)));
    let order = names(&ev);
    let at = order.iter().position(|n| n == "Guitare").unwrap();
    assert_eq!(&order[at..at + 3], ["Guitare", "P1", "P2"], "{order:?}");
    // Une parallèle de parallèle vient après sa famille.
    let ev = add_par(&mut h, p1, "P1a");
    let order = names(&ev);
    let at = order.iter().position(|n| n == "Guitare").unwrap();
    assert_eq!(&order[at..at + 4], ["Guitare", "P1", "P1a", "P2"], "{order:?}");
    assert_eq!(node(&ev, "P1a").parent, Some(p1));
    let _ = p2;
}

#[test]
fn moving_the_parent_moves_the_family_and_a_parallel_cannot_move_alone() {
    let mut h = host();
    let g = node(&get(&mut h), "Guitare").id;
    add_par(&mut h, g, "P1");
    let ev = add_par(&mut h, g, "P2");
    let basse = node(&ev, "Basse").id;
    let p1 = node(&ev, "P1").id;
    // Seule : refusée, rien ne change.
    let before = names(&get(&mut h));
    let ev = h.command_json(&format!(r#"{{"type":"move_track","node":{p1},"before":{basse}}}"#));
    assert!(!errors(&ev).is_empty());
    assert_eq!(names(&get(&mut h)), before);
    // La parente emmène ses parallèles ; viser une parallèle revient à viser sa famille (avant la parente).
    let ev = h.command_json(&format!(r#"{{"type":"move_track","node":{basse},"before":{p1}}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let order = names(&ev);
    let (b, gi) = (order.iter().position(|n| n == "Basse").unwrap(), order.iter().position(|n| n == "Guitare").unwrap());
    assert_eq!(b + 1, gi, "avant la famille entière : {order:?}");
    let ev = h.command_json(&format!(r#"{{"type":"move_track","node":{g},"before":null}}"#));
    let order = names(&ev);
    let n = order.len();
    assert_eq!(&order[n - 3..], ["Guitare", "P1", "P2"], "{order:?}");
}

#[test]
fn deleting_the_parent_deletes_its_parallels_and_one_undo_brings_everything_back() {
    let mut h = host();
    let g = node(&get(&mut h), "Guitare").id;
    add_par(&mut h, g, "P1");
    let ev = add_par(&mut h, g, "P2");
    let p1 = node(&ev, "P1").id;
    add_par(&mut h, p1, "P1a");
    let full = names(&get(&mut h));
    let ev = h.command_json(&format!(r#"{{"type":"delete_track","node":{g}}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let after = names(&ev);
    assert_eq!(after.len(), full.len() - 4, "{after:?}");
    assert!(!after.iter().any(|n| n.starts_with('P') && n.len() <= 3 || n == "Guitare"));
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(names(&ev), full, "un seul pas d'annulation remet toute la famille à sa place");
    let (g, p1) = (node(&ev, "Guitare").id, node(&ev, "P1").id);
    assert_eq!(node(&ev, "P1").parent, Some(g));
    assert_eq!(node(&ev, "P2").parent, Some(g));
    assert_eq!(node(&ev, "P1a").parent, Some(p1));
    // Supprimer seulement une parallèle retire sa propre descendance, pas sa parente.
    let ev = h.command_json(&format!(r#"{{"type":"delete_track","node":{p1}}}"#));
    let after = names(&ev);
    assert_eq!(after.len(), full.len() - 2, "{after:?}");
    assert!(after.contains(&"Guitare".to_string()) && after.contains(&"P2".to_string()));
}

#[test]
fn the_parent_link_survives_saving_and_reopening() {
    let dir = std::env::temp_dir().join(format!("daw-family-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut a = host();
    let g = node(&get(&mut a), "Guitare").id;
    add_par(&mut a, g, "P1");
    let ev = add_par(&mut a, g, "P2");
    let p2 = node(&ev, "P2").id;
    add_par(&mut a, p2, "P2a");
    let order = names(&get(&mut a));
    a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    let mut b = host();
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(names(&ev), order);
    let (g, p2) = (node(&ev, "Guitare").id, node(&ev, "P2").id);
    assert_eq!((node(&ev, "P1").parent, node(&ev, "P2").parent, node(&ev, "P2a").parent), (Some(g), Some(g), Some(p2)));
    assert_eq!(node(&ev, "Guitare").parent, None);
    let _ = std::fs::remove_dir_all(&dir);
}
