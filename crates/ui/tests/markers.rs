//! Repères (marqueurs nommés) : posés à la position de lecture, modifiés, supprimés, annulés, enregistrés et rouverts, et refusés sans trace.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> Host {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    std::mem::forget(engine);
    Host::new(handle, session, meta).unwrap()
}

type Row = (u32, [u64; 2], String, Option<String>);

fn markers(ev: &[Event]) -> Option<Vec<Row>> {
    ev.iter().rev().find_map(|e| if let Event::Chords { markers, .. } = e { Some(markers.iter().map(|m| (m.id, m.at, m.name.clone(), m.color.clone())).collect()) } else { None })
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

#[test]
fn markers_are_added_at_given_places_edited_deleted_undone_and_never_share_an_id() {
    let mut h = host();
    assert_eq!(markers(&h.command_json(r#"{"type":"get_project"}"#)).unwrap(), vec![]);

    let ev = h.command_json(r##"{"type":"add_marker","at":[3,4],"name":"Refrain","color":"#2fb457"}"##);
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let list = markers(&ev).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].1, list[0].2.as_str(), list[0].3.as_deref()), ([3, 4], "Refrain", Some("#2fb457")));
    let first = list[0].0;

    // Sans position : la tête de lecture (au début : 0). Sans nom : « Marqueur N ».
    let ev = h.command_json(r#"{"type":"add_marker"}"#);
    let list = markers(&ev).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!((list[0].1, list[0].2.as_str()), ([0, 1], "Marqueur 2"), "trié par instant : celui du début passe devant");
    assert_ne!(list[0].0, first);

    // Modifier : nom, position, couleur (vide : retirée).
    let ev = h.command_json(&format!(r#"{{"type":"edit_marker","id":{first},"name":"  À refaire ","at":[1,1],"color":""}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let list = markers(&ev).unwrap();
    let m = list.iter().find(|m| m.0 == first).unwrap();
    assert_eq!((m.1, m.2.as_str(), m.3.as_deref()), ([1, 1], "À refaire", None));

    // Supprimer puis annuler : il revient avec le même identifiant ; un marqueur neuf ne le reprend jamais.
    let ev = h.command_json(&format!(r#"{{"type":"delete_markers","ids":[{first}]}}"#));
    assert_eq!(markers(&ev).unwrap().len(), 1);
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert!(markers(&ev).unwrap().iter().any(|m| m.0 == first));
    h.command_json(&format!(r#"{{"type":"delete_markers","ids":[{first}]}}"#));
    let ev = h.command_json(r#"{"type":"add_marker","name":"Autre"}"#);
    assert!(markers(&ev).unwrap().iter().all(|m| m.0 != first), "identifiant jamais réutilisé");

    // Refus sans trace.
    let n = markers(&h.command_json(r#"{"type":"get_project"}"#)).unwrap().len();
    for bad in [
        r#"{"type":"add_marker","at":[1,0]}"#,
        r#"{"type":"add_marker","name":"x","color":"vert"}"#,
        r#"{"type":"edit_marker","id":9999,"name":"x"}"#,
        r#"{"type":"edit_marker","id":9999}"#,
        r#"{"type":"delete_markers","ids":[9999]}"#,
        r#"{"type":"delete_markers","ids":[]}"#,
    ] {
        let ev = h.command_json(bad);
        assert!(!errors(&ev).is_empty(), "{bad} doit être refusée");
    }
    let name_too_long = format!(r#"{{"type":"add_marker","name":"{}"}}"#, "x".repeat(61));
    assert!(!errors(&h.command_json(&name_too_long)).is_empty(), "nom de plus de 60 caractères");
    assert_eq!(markers(&h.command_json(r#"{"type":"get_project"}"#)).unwrap().len(), n, "rien n'a été ajouté par les refus");
}

#[test]
fn a_marker_dropped_while_the_play_head_moves_lands_at_the_play_head_and_follows_the_tempo() {
    let (mut engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.command_json(r#"{"type":"seek","pos":96000}"#); // 2 s à 120 bpm = une ronde
    let (mut l, mut r) = ([0f32; 64], [0f32; 64]);
    engine.process(&mut l, &mut r);
    h.tick();
    let ev = h.command_json(r#"{"type":"add_marker","name":"Ici"}"#);
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(markers(&ev).unwrap()[0].1, [1, 1], "2 s à 120 bpm : une ronde, exactement");
}

#[test]
fn markers_are_saved_with_the_project_and_reopened() {
    let dir = std::env::temp_dir().join(format!("daw-markers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("m.daw");
    let p = serde_json::to_string(&path.to_string_lossy()).unwrap();
    let mut h = host();
    h.command_json(r##"{"type":"add_marker","at":[1,2],"name":"Pont","color":"#112233"}"##);
    h.command_json(r#"{"type":"add_marker","at":[5,3],"name":"Fin"}"#);
    assert!(errors(&h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#))).is_empty());
    let mut h2 = host();
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let list = markers(&ev).unwrap();
    assert_eq!(list.iter().map(|m| (m.1, m.2.as_str(), m.3.as_deref())).collect::<Vec<_>>(), vec![([1, 2], "Pont", Some("#112233")), ([5, 3], "Fin", None)]);
    // Un marqueur neuf ne reprend aucun identifiant du fichier.
    let ev = h2.command_json(r#"{"type":"add_marker","name":"Neuf"}"#);
    let ids: Vec<u32> = markers(&ev).unwrap().iter().map(|m| m.0).collect();
    let mut dedup = ids.clone();
    dedup.sort_unstable();
    dedup.dedup();
    assert_eq!(ids.len(), dedup.len());
    let _ = std::fs::remove_dir_all(&dir);
}
