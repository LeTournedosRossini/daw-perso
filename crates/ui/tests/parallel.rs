//! Pistes parallèles : un bus qui prend le signal d'une piste à un point de sa chaîne et se somme avec elle, par le protocole.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::Engine;
use daw_test_plugin::{entry, PLUGIN_ID};
use daw_ui::demo::demo_session;
use daw_ui::host::{PluginRef, PluginResolver};
use daw_ui::protocol::{NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn resolver() -> PluginResolver {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let pref = PluginRef { clap_id: PLUGIN_ID.to_str().unwrap().into(), vendor: "daw-perso".into(), name: "Gain de test".into(), file: "test.clap".into() };
    Box::new(move |e| (e.clap_id == PLUGIN_ID.to_str().unwrap()).then(|| (clap_insert_factory(Arc::clone(&bundle), e.clap_id.clone()), pref.clone())))
}

fn host() -> Host {
    let (engine, handle) = Engine::new(SR, 64);
    std::mem::forget(engine);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_plugin_resolver(resolver());
    h
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de nœud « {name} »"))
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

fn undo_label(ev: &[Event]) -> Option<String> {
    ev.iter().rev().find_map(|e| if let Event::History { undo_label, .. } = e { Some(undo_label.clone()) } else { None }).flatten()
}

fn guitar(h: &mut Host) -> u32 {
    node(&h.command_json(r#"{"type":"get_project"}"#), "Guitare").id
}

#[test]
fn a_parallel_track_is_a_bus_fed_from_its_parent_and_summed_where_the_parent_goes() {
    let mut h = host();
    let g = guitar(&mut h);
    let before = project(&h.command_json(r#"{"type":"get_project"}"#)).nodes.len();
    let ev = h.command_json(&format!(r#"{{"type":"add_parallel","node":{g},"tap":"input"}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(project(&ev).nodes.len(), before + 1);
    let par = node(&ev, "Guitare ∥");
    let parent = node(&ev, "Guitare");
    assert_eq!(par.output, parent.output, "elle se somme là où va le parent");
    let send = parent.sends.iter().find(|s| s.to == par.id).expect("envoi du parent vers la parallèle");
    assert_eq!((send.tap.as_str(), send.follow, send.tap_insert), ("input", true, None));
    assert_eq!(undo_label(&ev).as_deref(), Some("Piste parallèle"), "une seule action");
    // Annuler retire la piste et l'envoi d'un coup.
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(project(&ev).nodes.len(), before);
    assert!(node(&ev, "Guitare").sends.is_empty());
}

#[test]
fn the_tap_point_can_be_changed_and_bad_requests_are_refused_without_a_trace() {
    let mut h = host();
    let g = guitar(&mut h);
    let ev = h.command_json(&format!(r#"{{"type":"add_parallel","node":{g},"tap":"pre_fader","name":"Écrasée"}}"#));
    let par = node(&ev, "Écrasée").id;
    assert_eq!(node(&ev, "Guitare").sends.iter().find(|s| s.to == par).unwrap().tap, "pre_fader");
    let ev = h.command_json(&format!(r#"{{"type":"set_send_tap","node":{g},"to":{par},"tap":"post_fader"}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(node(&ev, "Guitare").sends.iter().find(|s| s.to == par).unwrap().tap, "post_fader");
    assert_eq!(undo_label(&ev).as_deref(), Some("Point de captation d'un envoi"));

    let n = project(&h.command_json(r#"{"type":"get_project"}"#)).nodes.len();
    for bad in [
        format!(r#"{{"type":"add_parallel","node":{g},"tap":"nulle part"}}"#),
        format!(r#"{{"type":"add_parallel","node":{g},"tap":"after_insert"}}"#),
        format!(r#"{{"type":"add_parallel","node":{g},"tap":"input","insert":3}}"#),
        r#"{"type":"add_parallel","node":999,"tap":"input"}"#.to_owned(),
        r#"{"type":"add_parallel","node":0,"tap":"input"}"#.to_owned(), // la sortie principale n'a pas d'envois
        format!(r#"{{"type":"add_parallel","node":{g},"tap":"after_insert","insert":4242}}"#), // pas d'effet de ce nom
    ] {
        assert!(!errors(&h.command_json(&bad)).is_empty(), "devait être refusée : {bad}");
    }
    assert_eq!(project(&h.command_json(r#"{"type":"get_project"}"#)).nodes.len(), n, "rien n'a été créé");
}

#[test]
fn a_tap_after_a_plugin_follows_it_through_saving_and_is_protected_from_its_removal() {
    let dir = std::env::temp_dir().join(format!("daw-parallel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut a = host();
    let g = guitar(&mut a);
    let add = |h: &mut Host| h.command_json(&format!(r#"{{"type":"add_plugin","node":{g},"clap_id":"com.daw-perso.test-gain","file":"test.clap"}}"#));
    let first = node(&add(&mut a), "Guitare").inserts[0].id;
    let _second = node(&add(&mut a), "Guitare").inserts[1].id;
    let ev = a.command_json(&format!(r#"{{"type":"add_parallel","node":{g},"tap":"after_insert","insert":{first}}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let par = node(&ev, "Guitare ∥").id;
    assert_eq!(node(&ev, "Guitare").sends.iter().find(|s| s.to == par).unwrap().tap_insert, Some(first));

    // On ne retire pas un plugin dont une parallèle dépend : message clair, rien ne change.
    let ev = a.command_json(&format!(r#"{{"type":"remove_plugin","insert":{first}}}"#));
    let e = errors(&ev);
    assert!(e.iter().any(|m| m.contains("piste parallèle")), "{e:?}");
    assert_eq!(node(&a.command_json(r#"{"type":"get_project"}"#), "Guitare").inserts.len(), 2);

    // Enregistrer puis rouvrir : les identifiants des plugins changent, le point de captation retrouve son plugin par sa place.
    a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    let mut b = host();
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let par = node(&ev, "Guitare ∥").id;
    let chain = &node(&ev, "Guitare").inserts;
    let send = node(&ev, "Guitare").sends.iter().find(|s| s.to == par).unwrap();
    assert_eq!(send.tap, "after_insert");
    assert_eq!(send.tap_insert, Some(chain[0].id), "après le premier plugin de la chaîne, avec son nouvel identifiant");
    assert!(send.follow);
    let _ = std::fs::remove_dir_all(&dir);
}
