//! La carte de structure (intro, couplet, refrain...) : édition par le protocole, annulation, projet enregistré et rouvert.

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

fn structure(ev: &[Event]) -> Option<Vec<(String, [u64; 2], [u64; 2], Option<String>)>> {
    ev.iter().rev().find_map(|e| {
        if let Event::Chords { structure, .. } = e {
            Some(structure.iter().map(|s| (s.name.clone(), s.at, s.dur, s.color.clone())).collect())
        } else {
            None
        }
    })
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

const TWO: &str = r##"{"type":"set_structure","sections":[
  {"at":[4,1],"dur":[8,1],"name":"Refrain","color":"#e5303f"},
  {"at":[0,1],"dur":[4,1],"name":"Intro"}]}"##;

#[test]
fn sections_are_set_sorted_undone_and_saved_with_the_project() {
    let dir = std::env::temp_dir().join(format!("daw-structure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = host();
    assert_eq!(structure(&h.command_json(r#"{"type":"get_project"}"#)).unwrap().len(), 0);

    let ev = h.command_json(TWO);
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let list = structure(&ev).expect("événement chords avec la structure");
    assert_eq!(list.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["Intro", "Refrain"], "triées par début");
    assert_eq!(list[1].3.as_deref(), Some("#e5303f"));

    // Une seule action annulable, puis rétablie.
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(structure(&ev).unwrap().len(), 0);
    let ev = h.command_json(r#"{"type":"redo"}"#);
    assert_eq!(structure(&ev).unwrap().len(), 2);

    // Enregistré puis rouvert dans un autre hôte.
    let path = dir.join("essai.daw");
    let ev = h.command_json(&format!(r#"{{"type":"save_project","path":{}}}"#, serde_json::to_string(&path.to_string_lossy()).unwrap()));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let mut h2 = host();
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{}}}"#, serde_json::to_string(&path.to_string_lossy()).unwrap()));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let list = structure(&ev).expect("structure à l'ouverture");
    assert_eq!(list, vec![("Intro".into(), [0, 1], [4, 1], None), ("Refrain".into(), [4, 1], [8, 1], Some("#e5303f".into()))]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_structures_are_refused_without_a_trace() {
    let mut h = host();
    h.command_json(TWO);
    for bad in [
        r#"{"type":"set_structure","sections":[{"at":[0,1],"dur":[4,1],"name":"A"},{"at":[2,1],"dur":[4,1],"name":"B"}]}"#, // chevauchement
        r#"{"type":"set_structure","sections":[{"at":[0,1],"dur":[0,1],"name":"A"}]}"#,                                          // durée nulle
        r#"{"type":"set_structure","sections":[{"at":[0,1],"dur":[1,1],"name":"  "}]}"#,                                         // nom vide
        r##"{"type":"set_structure","sections":[{"at":[0,1],"dur":[1,1],"name":"A","color":"rouge"}]}"##,                          // couleur
        r#"{"type":"set_structure","sections":[{"at":[0,0],"dur":[1,1],"name":"A"}]}"#,                                          // dénominateur nul
    ] {
        let ev = h.command_json(bad);
        assert!(!errors(&ev).is_empty(), "devrait être refusé : {bad}");
    }
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(structure(&ev).unwrap().len(), 2, "la carte est intacte");
}

#[test]
fn a_section_category_is_kept_and_unknown_ones_are_refused_without_trace() {
    let dir = std::env::temp_dir().join(format!("daw-structure-cat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = host();
    let ev = h.command_json(r##"{"type":"set_structure","sections":[
      {"at":[0,1],"dur":[4,1],"name":"Couplet 1","category":"verse"},
      {"at":[4,1],"dur":[4,1],"name":"Refrain"}]}"##);
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let cats = |ev: &[Event]| -> Vec<Option<String>> { ev.iter().rev().find_map(|e| if let Event::Chords { structure, .. } = e { Some(structure.iter().map(|s| s.category.clone()).collect()) } else { None }).unwrap() };
    assert_eq!(cats(&ev), [Some("verse".to_string()), None]);
    // Catégorie inconnue : refus, rien ne change.
    let bad = h.command_json(r##"{"type":"set_structure","sections":[{"at":[0,1],"dur":[4,1],"name":"X","category":"couplet"}]}"##);
    assert!(!errors(&bad).is_empty());
    assert_eq!(cats(&h.command_json(r#"{"type":"get_project"}"#)), [Some("verse".to_string()), None]);
    // Enregistrée, la catégorie revient à la réouverture.
    let path = dir.join("cat.daw");
    let p = serde_json::to_string(&path.to_string_lossy()).unwrap();
    assert!(errors(&h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#))).is_empty());
    let mut h2 = host();
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    assert_eq!(cats(&h2.command_json(r#"{"type":"get_project"}"#)), [Some("verse".to_string()), None]);
}
