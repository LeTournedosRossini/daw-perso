//! Banque personnelle de pages de contrôle : définie une fois sur l'ordinateur, elle sert à tous les projets ; la page d'un plugin du projet est
//! résolue d'après les paramètres qu'il expose (identifiant, sinon nom), avec leurs valeurs et leurs textes.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::Engine;
use daw_test_plugin::{entry, PARAM_EXTRA_LATENCY, PARAM_GAIN, PLUGIN_ID};
use daw_ui::demo::demo_session;
use daw_ui::host::{PluginRef, PluginResolver};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;
const ID: &str = "com.daw-perso.test-gain";

fn resolver() -> PluginResolver {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    Box::new(move |e| (e.clap_id == PLUGIN_ID.to_str().unwrap()).then(|| (clap_insert_factory(Arc::clone(&bundle), e.clap_id.clone()), PluginRef { clap_id: ID.into(), vendor: "daw-perso".into(), name: "Gain de test".into(), file: "test.clap".into() })))
}

fn host(bank: Option<&std::path::Path>) -> (Engine, Host) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_plugin_resolver(resolver());
    h.set_control_pages_path(bank.map(|p| p.to_path_buf()));
    (engine, h)
}

fn add_plugin(h: &mut Host) -> u32 {
    let ev = h.command_json(&format!(r#"{{"type":"add_plugin","node":2,"clap_id":"{ID}","file":"test.clap"}}"#));
    ev.iter()
        .find_map(|e| if let Event::Project(p) = e { p.nodes.iter().find(|n| n.name == "Basse").and_then(|n| n.inserts.first().map(|i| i.id)) } else { None })
        .expect("le plugin est ajouté")
}

fn page(h: &mut Host, insert: u32) -> (bool, Vec<(u32, String, String, f64)>, Vec<String>) {
    match h.command_json(&format!(r#"{{"type":"get_insert_page","insert":{insert}}}"#)).remove(0) {
        Event::InsertPage { configured, params, missing, .. } => (configured, params.into_iter().map(|p| (p.id, p.name, p.text, p.value)).collect(), missing),
        e => panic!("{e:?}"),
    }
}

fn bank(h: &mut Host) -> (Vec<(String, usize)>, Option<String>) {
    match h.command_json(r#"{"type":"get_control_pages"}"#).remove(0) {
        Event::ControlPages { entries, error } => (entries.into_iter().map(|e| (e.plugin_id, e.params.len())).collect(), error),
        e => panic!("{e:?}"),
    }
}

#[test]
fn a_page_is_defined_once_resolved_against_the_plugin_and_follows_its_values() {
    let (_e, mut h) = host(None);
    let ins = add_plugin(&mut h);
    assert!(!page(&mut h, ins).0, "pas encore de page pour ce plugin");
    // Une page : le gain (par identifiant), la latence supplémentaire retrouvée par son nom (identifiant changé) et un paramètre qui n'existe pas.
    let ev = h.command_json(&format!(r#"{{"type":"set_control_page","plugin_id":"{ID}","name":"Gain de test","params":[{{"id":{PARAM_GAIN},"name":"Gain"}},{{"id":77,"name":"Latence supplementaire"}},{{"id":99,"name":"Inexistant"}}]}}"#));
    assert!(matches!(&ev[0], Event::ControlPages { entries, .. } if entries.len() == 1));
    let (configured, params, missing) = page(&mut h, ins);
    assert!(configured);
    assert_eq!(params.iter().map(|p| p.1.as_str()).collect::<Vec<_>>(), ["Gain", "Latence supplementaire"], "ordre de la page");
    assert_eq!(params[1].0, PARAM_EXTRA_LATENCY, "retrouvé par son nom");
    assert_eq!(missing, vec!["Inexistant".to_string()]);
    assert!(!params[0].2.is_empty(), "le texte de la valeur est celui du plugin");
    // Une valeur réglée se retrouve dans la page.
    h.command_json(&format!(r#"{{"type":"set_plugin_param","insert":{ins},"param":{PARAM_GAIN},"value":0.5}}"#));
    let (_, params, _) = page(&mut h, ins);
    assert!((params[0].3 - 0.5).abs() < 1e-9, "{params:?}");
    // Une page sans paramètre supprime celle du plugin.
    h.command_json(&format!(r#"{{"type":"set_control_page","plugin_id":"{ID}","name":"","params":[]}}"#));
    assert!(!page(&mut h, ins).0);
}

#[test]
fn invalid_pages_are_refused_without_changing_the_bank() {
    let (_e, mut h) = host(None);
    let many: Vec<String> = (0..17).map(|i| format!(r#"{{"id":{i},"name":"p{i}"}}"#)).collect();
    for body in [
        format!(r#"{{"type":"set_control_page","plugin_id":"p","name":"","params":[{}]}}"#, many.join(",")),
        r#"{"type":"set_control_page","plugin_id":"p","name":"","params":[{"id":1,"name":"a"},{"id":1,"name":"b"}]}"#.to_string(),
        r#"{"type":"set_control_page","plugin_id":"","name":"","params":[{"id":1,"name":"a"}]}"#.to_string(),
    ] {
        let ev = h.command_json(&body);
        assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })), "{body}");
    }
    assert!(bank(&mut h).0.is_empty());
}

#[test]
fn the_bank_lives_in_a_file_of_the_user_and_serves_every_project() {
    let dir = std::env::temp_dir().join(format!("daw-bankhost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("control-pages.json");
    {
        let (_e, mut h) = host(Some(&file));
        h.command_json(&format!(r#"{{"type":"set_control_page","plugin_id":"{ID}","name":"Gain de test","params":[{{"id":{PARAM_GAIN},"name":"Gain"}}]}}"#));
        assert_eq!(bank(&mut h), (vec![(ID.to_string(), 1)], None));
    }
    // Un autre hôte (autre session, autre projet) relit le même fichier.
    let (_e, mut h2) = host(Some(&file));
    assert_eq!(bank(&mut h2).0, vec![(ID.to_string(), 1)]);
    let ins = add_plugin(&mut h2);
    assert!(page(&mut h2, ins).0, "la page de la banque s'applique au plugin d'un autre projet");
    h2.command_json(r#"{"type":"new_project"}"#);
    assert_eq!(bank(&mut h2).0.len(), 1, "un nouveau projet ne vide pas la banque");
    // Un fichier abîmé : banque vide, erreur montrée, fichier intact.
    std::fs::write(&file, "pas du json").unwrap();
    let (_e, mut h3) = host(Some(&file));
    let (entries, error) = bank(&mut h3);
    assert!(entries.is_empty() && error.as_deref().is_some_and(|e| e.contains("illisible")));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "pas du json");
    let _ = std::fs::remove_dir_all(&dir);
}
