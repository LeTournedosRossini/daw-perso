//! Le texte des paroles en strophes : édition par le protocole, annulation, refus sans trace, projet enregistré et rouvert.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> Host {
    let (_engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    std::mem::forget(_engine);
    Host::new(handle, session, meta).unwrap()
}

fn strophes(ev: &[Event]) -> Option<Vec<(String, Option<String>, Vec<String>, Option<String>)>> {
    ev.iter().rev().find_map(|e| {
        if let Event::Chords { strophes, .. } = e {
            Some(strophes.iter().map(|s| (s.name.clone(), s.category.clone(), s.lines.clone(), s.link.clone())).collect())
        } else {
            None
        }
    })
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

const TEXT: &str = r##"{"type":"set_strophes","strophes":[
  {"name":"Couplet 1","category":"verse","lines":["Ma maman est la plus belle","Elle chante dans la cuisine"]},
  {"name":"Refrain","category":"chorus","lines":["La la la"],"link":"Refrain"}]}"##;

#[test]
fn strophes_are_set_undone_and_saved_with_the_project() {
    let dir = std::env::temp_dir().join(format!("daw-strophes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = host();
    assert_eq!(strophes(&h.command_json(r#"{"type":"get_project"}"#)).unwrap().len(), 0);

    let ev = h.command_json(TEXT);
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let list = strophes(&ev).expect("événement chords avec les strophes");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].0, "Couplet 1");
    assert_eq!(list[0].2.len(), 2);
    assert_eq!(list[1].3.as_deref(), Some("Refrain"));

    // Un seul pas d'annulation.
    assert_eq!(strophes(&h.command_json(r#"{"type":"undo"}"#)).unwrap().len(), 0);
    assert_eq!(strophes(&h.command_json(r#"{"type":"redo"}"#)).unwrap().len(), 2);

    let path = dir.join("essai.daw");
    let p = serde_json::to_string(&path.to_string_lossy()).unwrap();
    let ev = h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let mut h2 = host();
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let back = strophes(&h2.command_json(r#"{"type":"get_project"}"#)).unwrap();
    assert_eq!(back, list, "relues à l'identique à l'ouverture");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_strophes_are_refused_without_a_trace() {
    let mut h = host();
    h.command_json(TEXT);
    let bad = [
        r#"{"type":"set_strophes","strophes":[{"name":"","lines":[]}]}"#,
        r#"{"type":"set_strophes","strophes":[{"name":"X","category":"inconnue","lines":[]}]}"#,
        r#"{"type":"set_strophes","strophes":[{"name":"X","lines":["deux\nlignes"]}]}"#,
        r#"{"type":"set_strophes","strophes":[{"name":"X","link":"","lines":[]}]}"#,
    ];
    for b in bad {
        let ev = h.command_json(b);
        assert!(!errors(&ev).is_empty(), "devait être refusé : {b}");
    }
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(strophes(&ev).unwrap().len(), 2, "rien n'a changé");
    // L'historique garde un seul pas pour le texte.
    assert_eq!(strophes(&h.command_json(r#"{"type":"undo"}"#)).unwrap().len(), 0);
}
