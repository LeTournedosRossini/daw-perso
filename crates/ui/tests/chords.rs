//! La piste d'accords dans l'hôte : import, édition, export, annulation, projet.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

const CARTE: &str = r#"{ "format": "flibuste-carte", "version": 1, "title": "Essai", "qualities": { "m7": [0, 3, 7, 10] },
  "chords": [ { "at": [0, 1], "dur": [1, 1], "root": "D", "quality": "m7", "bass": null },
              { "at": [1, 1], "dur": [1, 1], "root": "Q", "quality": "m7" } ],
  "scales": [ { "at": [0, 1], "dur": [2, 1], "root": "D", "scale": "dorian", "intervals": [0, 2, 3, 5, 7, 9, 10] } ] }"#;

fn host() -> Host {
    let (_engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    std::mem::forget(_engine);
    Host::new(handle, session, meta).unwrap()
}

fn chords(ev: &[Event]) -> Option<(u32, serde_json::Value, Vec<String>)> {
    ev.iter().rev().find_map(|e| if let Event::Chords { revision, track, skipped, .. } = e { Some((*revision, track.clone(), skipped.clone())) } else { None })
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

#[test]
fn chords_are_imported_edited_exported_undone_and_saved_with_the_project() {
    let dir = std::env::temp_dir().join(format!("daw-chords-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = host();

    // Au démarrage la page reçoit une piste vide.
    let (rev0, track, _) = chords(&h.command_json(r#"{"type":"get_project"}"#)).expect("événement chords");
    assert_eq!(track["chords"].as_array().unwrap().len(), 0);

    // Import : l'accord à fondamentale illisible est ignoré et dit.
    let file = dir.join("essai.json");
    std::fs::write(&file, CARTE).unwrap();
    let ev = h.command_json(&format!(r#"{{"type":"import_chords","path":{:?}}}"#, file.to_string_lossy()));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let (rev1, track, skipped) = chords(&ev).unwrap();
    assert_ne!(rev1, rev0);
    assert_eq!(track["chords"].as_array().unwrap().len(), 1);
    assert_eq!(track["scales"][0]["intervals"], serde_json::json!([0, 2, 3, 5, 7, 9, 10]));
    assert!(skipped.iter().any(|s| s.contains("fondamentale")), "{skipped:?}");
    assert!(ev.iter().any(|e| matches!(e, Event::History { undo_label: Some(l), .. } if l == "Importer une carte")));

    // Édition de la page : remplace toute la piste, une action annulable ; une piste invalide est refusée sans trace.
    let mut edited = track.clone();
    edited["chords"].as_array_mut().unwrap().push(serde_json::json!({ "at": [1, 1], "dur": [1, 2], "root": "G", "quality": "m7", "bass": "D" }));
    let ev = h.command_json(&serde_json::json!({ "type": "set_chords", "track": edited, "label": "Ajouter un accord" }).to_string());
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(chords(&ev).unwrap().1["chords"].as_array().unwrap().len(), 2);
    let mut bad = edited.clone();
    bad["chords"][1]["dur"] = serde_json::json!([0, 1]);
    let ev = h.command_json(&serde_json::json!({ "type": "set_chords", "track": bad }).to_string());
    assert_eq!(errors(&ev).len(), 1, "une durée nulle est refusée");
    assert!(chords(&h.command_json(r#"{"type":"get_chords"}"#)).unwrap().1["chords"].as_array().unwrap().len() == 2, "rien n'a changé");

    // Une qualité que le dictionnaire ne connaît pas est refusée en entier (jamais de perte silencieuse) ; un alias est accepté et ramené à son identifiant.
    let mut unknown = edited.clone();
    unknown["chords"][1]["quality"] = serde_json::json!("nimportequoi");
    let ev = h.command_json(&serde_json::json!({ "type": "set_chords", "track": unknown }).to_string());
    assert!(errors(&ev).iter().any(|m| m.contains("qualité")), "{ev:?}");
    let mut alias = edited.clone();
    alias["chords"][1]["quality"] = serde_json::json!("maj7");
    let ev = h.command_json(&serde_json::json!({ "type": "set_chords", "track": alias, "label": "Alias" }).to_string());
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(chords(&ev).unwrap().1["chords"][1]["quality"], "M7", "maj7 devient M7");
    h.command_json(r#"{"type":"undo"}"#);

    // Le catalogue pour la saisie est envoyé avec le projet : 31 qualités, des alias, 72 gammes.
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let cat = ev.iter().find_map(|e| if let Event::ChordCatalog { qualities, aliases, scales } = e { Some((qualities.len(), aliases.len(), scales.len())) } else { None });
    assert_eq!(cat, Some((31, 42, 72)));

    // Export puis réimport dans un autre hôte : même piste.
    let out = dir.join("sortie.json");
    h.command_json(&format!(r#"{{"type":"export_chords","path":{:?}}}"#, out.to_string_lossy()));
    let mut h2 = host();
    let ev = h2.command_json(&format!(r#"{{"type":"import_chords","path":{:?}}}"#, out.to_string_lossy()));
    assert!(chords(&ev).unwrap().2.is_empty(), "rien d'ignoré dans un fichier que nous avons écrit");
    assert_eq!(chords(&ev).unwrap().1["chords"], chords(&h.command_json(r#"{"type":"get_chords"}"#)).unwrap().1["chords"]);

    // Annuler ramène l'import seul ; annuler encore, la piste vide.
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(chords(&ev).unwrap().1["chords"].as_array().unwrap().len(), 1);
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(chords(&ev).unwrap().1["chords"].as_array().unwrap().len(), 0);
    h.command_json(r#"{"type":"redo"}"#);
    h.command_json(r#"{"type":"redo"}"#);

    // Projet : enregistré, rouvert, la piste est là.
    let p = dir.join("Chants");
    let ev = h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, p.to_string_lossy()));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let mut h3 = host();
    let ev = h3.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, p.with_extension("daw").to_string_lossy()));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let (_, track, _) = chords(&ev).expect("la page est prévenue à l'ouverture");
    assert_eq!(track["chords"].as_array().unwrap().len(), 2);
    assert_eq!(track["scales"].as_array().unwrap().len(), 1);

    // Fichier absent ou illisible : erreur claire, rien ne change.
    let ev = h3.command_json(r#"{"type":"import_chords","path":"/n/existe/pas.json"}"#);
    assert_eq!(errors(&ev).len(), 1);
    std::fs::write(&file, "{ pas du json").unwrap();
    let ev = h3.command_json(&format!(r#"{{"type":"import_chords","path":{:?}}}"#, file.to_string_lossy()));
    assert_eq!(errors(&ev).len(), 1);
    assert_eq!(chords(&h3.command_json(r#"{"type":"get_chords"}"#)).unwrap().1["chords"].as_array().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_key_the_page_can_place_is_accepted_saved_and_read_back_with_its_fifths() {
    // Les 30 tonalités de la voie « Tonalités » : même forme que l'entrée écrite par la page (`root`, `mode`, `fifths`).
    let majors = ["Cb", "Gb", "Db", "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#"];
    let minors = ["Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#", "G#", "D#", "A#"];
    let mut keys = Vec::new();
    for (i, (maj, min)) in majors.iter().zip(minors.iter()).enumerate() {
        let fifths = i as i64 - 7;
        keys.push(serde_json::json!({ "at": [i * 2, 1], "root": maj, "mode": "major", "fifths": fifths }));
        keys.push(serde_json::json!({ "at": [i * 2 + 1, 1], "root": min, "mode": "minor", "fifths": fifths }));
    }
    let track = serde_json::json!({ "format": "flibuste-carte", "version": 1, "chords": [], "scales": [], "patterns": [], "measures": [], "qualities": {}, "keys": keys });
    let mut h = host();
    let ev = h.command_json(&serde_json::json!({ "type": "set_chords", "track": track, "label": "Ajouter une tonalité" }).to_string());
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let (_, back, skipped) = chords(&ev).unwrap();
    assert!(skipped.is_empty(), "{skipped:?}");
    let list = back["keys"].as_array().unwrap();
    assert_eq!(list.len(), 30);
    // Les quintes reviennent exactement ; le mode aussi ; l'ordre est celui du temps.
    for (i, k) in list.iter().enumerate() {
        assert_eq!(k["fifths"].as_i64().unwrap(), (i / 2) as i64 - 7, "{k}");
        assert_eq!(k["mode"], if i % 2 == 0 { "major" } else { "minor" });
    }
    // Une armure hors de −7 à 7 est refusée en entier.
    let bad = serde_json::json!({ "format": "flibuste-carte", "version": 1, "chords": [], "scales": [], "patterns": [], "measures": [], "qualities": {}, "keys": [{ "at": [0, 1], "mode": "major", "fifths": 9 }] });
    let ev = h.command_json(&serde_json::json!({ "type": "set_chords", "track": bad }).to_string());
    let after = chords(&ev);
    assert!(!errors(&ev).is_empty() || after.is_some_and(|(_, t, _)| t["keys"].as_array().unwrap().len() == 30), "une armure de 9 quintes ne doit rien changer");
}
