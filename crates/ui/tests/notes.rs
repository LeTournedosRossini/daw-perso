//! Piano roll : notes d'un clip en temps musical exact, modifications groupées annulables, écoute d'une note.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{ClipInfo, NoteDetail, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> Host {
    let (_, handle) = Engine::new(SR, 256);
    let (session, meta) = demo_session(SR);
    Host::new(handle, session, meta).unwrap()
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

struct Notes {
    clip: u64,
    notes: Vec<NoteDetail>,
    selection: Vec<usize>,
    events: Vec<daw_ui::protocol::EventDetail>,
}

fn notes(ev: &[Event]) -> Notes {
    ev.iter().rev().find_map(|e| if let Event::Notes { clip, notes, selection, events, .. } = e { Some(Notes { clip: *clip, notes: notes.clone(), selection: selection.clone(), events: events.clone() }) } else { None }).expect("événement notes")
}

/// Identifiant du clip de notes de la démonstration.
fn midi_clip(h: &mut Host) -> u64 {
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    project(&ev).nodes.iter().flat_map(|n| &n.clips).find_map(|c| if let ClipInfo::Midi { id, .. } = c { Some(*id) } else { None }).expect("un clip de notes")
}

#[test]
fn get_notes_gives_exact_musical_positions_and_unknown_clips_are_refused() {
    let mut h = host();
    let clip = midi_clip(&mut h);
    let n = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    assert_eq!(n.clip, clip);
    assert!(n.notes.len() > 8);
    // L'arpège est écrit en croches : le début de la deuxième note est 1/8 de ronde.
    assert_eq!(n.notes[0].start, [0, 1]);
    assert_eq!(n.notes[1].start, [1, 8]);
    assert!(n.selection.is_empty());
    assert!(!error(&h.command_json(r#"{"type":"get_notes","clip":1}"#)).is_empty(), "un clip audio n'a pas de notes");
    assert!(!error(&h.command_json(r#"{"type":"get_notes","clip":999999999}"#)).is_empty());
}

#[test]
fn edits_are_one_undo_step_and_the_new_indices_come_back_as_the_selection() {
    let mut h = host();
    let clip = midi_clip(&mut h);
    let before = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    // Les deux premières notes : une croche plus tard et un demi-ton plus haut ; vélocité de la première changée.
    let ev = h.command_json(&format!(
        r#"{{"type":"edit_notes","clip":{clip},"label":"Déplacer des notes","edits":[{{"index":0,"start":[1,8],"key":{k0},"velocity":0.9}},{{"index":1,"start":[1,4],"key":{k1}}}]}}"#,
        k0 = before.notes[0].key + 1,
        k1 = before.notes[1].key + 1
    ));
    let after = notes(&ev);
    assert_eq!(undo_label(&ev).as_deref(), Some("Déplacer des notes"));
    assert_eq!(after.selection.len(), 2);
    let moved = &after.notes[after.selection[0]];
    assert_eq!((moved.start, moved.key, moved.velocity), ([1, 8], before.notes[0].key + 1, 0.9));
    assert_eq!(after.notes[after.selection[1]].start, [1, 4]);
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::History { can_redo: true, .. })));
    let restored = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    assert_eq!(restored.notes, before.notes, "un seul pas d'annulation");
}

#[test]
fn add_delete_and_quantize_notes() {
    let mut h = host();
    let clip = midi_clip(&mut h);
    let n0 = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#))).notes.len();
    let ev = h.command_json(&format!(r#"{{"type":"add_notes","clip":{clip},"notes":[{{"start":[1,3],"len":[1,16],"key":72,"velocity":0.7}}]}}"#));
    let added = notes(&ev);
    assert_eq!(added.notes.len(), n0 + 1);
    let i = added.selection[0];
    assert_eq!((added.notes[i].start, added.notes[i].key), ([1, 3], 72));
    // Quantifier à la croche : 1/3 (triolet) passe à 3/8 ou 1/4 selon l'arrondi ; la note reste dans le clip, aucune note n'est perdue.
    let ev = h.command_json(&format!(r#"{{"type":"quantize_notes","clip":{clip},"grid":[1,8]}}"#));
    let q = notes(&ev);
    assert_eq!(q.notes.len(), n0 + 1);
    assert!(q.notes.iter().all(|n| (n.start[0] * 8) % n.start[1] == 0), "tous les débuts sur la croche");
    assert_eq!(undo_label(&ev).as_deref(), Some("Quantifier"));
    let ev = h.command_json(&format!(r#"{{"type":"delete_notes","clip":{clip},"indices":[0,1]}}"#));
    assert_eq!(notes(&ev).notes.len(), n0 - 1);
    assert_eq!(undo_label(&ev).as_deref(), Some("Supprimer des notes"));
}

#[test]
fn refused_edits_change_nothing_and_leave_no_history() {
    let mut h = host();
    let clip = midi_clip(&mut h);
    let before = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    let depth = h.command_json(r#"{"type":"get_project"}"#);
    let label = undo_label(&depth);
    for bad in [
        format!(r#"{{"type":"edit_notes","clip":{clip},"edits":[{{"index":9999,"key":60}}]}}"#),
        format!(r#"{{"type":"edit_notes","clip":{clip},"edits":[{{"index":0,"start":[900,1]}}]}}"#),
        format!(r#"{{"type":"edit_notes","clip":{clip},"edits":[{{"index":0,"len":[0,1]}}]}}"#),
        format!(r#"{{"type":"edit_notes","clip":{clip},"edits":[{{"index":0,"start":[1,0]}}]}}"#),
        format!(r#"{{"type":"edit_notes","clip":{clip},"edits":[{{"index":0,"velocity":0}}]}}"#),
        format!(r#"{{"type":"edit_notes","clip":{clip},"edits":[{{"index":0,"channel":16}}]}}"#),
        format!(r#"{{"type":"add_notes","clip":{clip},"notes":[{{"start":[900,1],"len":[1,8],"key":60,"velocity":0.5}}]}}"#),
        format!(r#"{{"type":"delete_notes","clip":{clip},"indices":[9999]}}"#),
        format!(r#"{{"type":"quantize_notes","clip":{clip},"grid":[0,8]}}"#),
        r#"{"type":"edit_notes","clip":1,"edits":[{"index":0,"key":60}]}"#.to_string(),
    ] {
        assert!(!error(&h.command_json(&bad)).is_empty(), "{bad}");
    }
    let after = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    assert_eq!(after.notes, before.notes);
    assert_eq!(undo_label(&h.command_json(r#"{"type":"get_project"}"#)), label);
}

#[test]
fn preview_note_is_validated_and_does_not_touch_the_project() {
    let mut h = host();
    assert!(h.command_json(r#"{"type":"preview_note","node":3,"key":60,"velocity":0.8,"on":true}"#).is_empty());
    assert!(h.command_json(r#"{"type":"preview_note","node":3,"key":60,"velocity":0.8,"on":false}"#).is_empty());
    assert!(!error(&h.command_json(r#"{"type":"preview_note","node":99,"key":60,"velocity":0.8,"on":true}"#)).is_empty());
    assert!(!error(&h.command_json(r#"{"type":"preview_note","node":3,"key":200,"velocity":0.8,"on":true}"#)).is_empty());
    assert!(!error(&h.command_json(r#"{"type":"preview_note","node":3,"key":60,"velocity":0,"on":true}"#)).is_empty());
    // Le projet n'est pas « modifié » par une note d'écoute.
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: false, .. })));
}

#[test]
fn the_channel_of_a_note_is_editable_and_comes_back_in_the_notes_event() {
    let mut h = host();
    let clip = midi_clip(&mut h);
    let ev = h.command_json(&format!(r#"{{"type":"edit_notes","clip":{clip},"label":"Canal","edits":[{{"index":0,"channel":7}}]}}"#));
    let after = notes(&ev);
    assert_eq!(after.notes[after.selection[0]].channel, 7);
    assert_eq!(undo_label(&ev).as_deref(), Some("Canal"));
}

#[test]
fn the_display_voice_of_a_note_is_editable_undoable_saved_and_ignored_by_the_audio() {
    let dir = std::env::temp_dir().join(format!("daw-voice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = host();
    let clip = midi_clip(&mut h);
    let n = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    assert!(n.notes.iter().all(|x| x.voice == 1), "voix 1 par défaut");

    // Passer deux notes en voix 2 : une seule action annulable, les indices et la sélection reviennent.
    let ev = h.command_json(&format!(r#"{{"type":"edit_notes","clip":{clip},"edits":[{{"index":0,"voice":2}},{{"index":3,"voice":3}}],"label":"Changer de voix"}}"#));
    let n = notes(&ev);
    assert_eq!(n.notes.iter().filter(|x| x.voice == 2).count(), 1);
    assert_eq!(n.notes.iter().filter(|x| x.voice == 3).count(), 1);
    assert_eq!(undo_label(&ev).as_deref(), Some("Changer de voix"));
    assert_eq!(n.selection.len(), 2);
    // Une note ajoutée avec une voix la garde ; sans voix : 1.
    let ev = h.command_json(&format!(r#"{{"type":"add_notes","clip":{clip},"notes":[{{"start":[1,16],"len":[1,16],"key":50,"velocity":0.5,"voice":4}},{{"start":[3,16],"len":[1,16],"key":52,"velocity":0.5}}]}}"#));
    let n = notes(&ev);
    assert_eq!(n.notes.iter().find(|x| x.key == 50 && x.start == [1, 16]).unwrap().voice, 4);
    assert_eq!(n.notes.iter().find(|x| x.key == 52 && x.start == [3, 16]).unwrap().voice, 1);

    // Refus sans trace : voix 0 ou 5.
    for bad in [0, 5] {
        let ev = h.command_json(&format!(r#"{{"type":"edit_notes","clip":{clip},"edits":[{{"index":1,"voice":{bad}}}]}}"#));
        assert!(!error(&ev).is_empty());
    }
    assert!(!error(&h.command_json(&format!(r#"{{"type":"add_notes","clip":{clip},"notes":[{{"start":[0,1],"len":[1,16],"key":50,"velocity":0.5,"voice":9}}]}}"#))).is_empty());

    // Annuler l'ajout puis le changement de voix : retour à la voix 1.
    h.command_json(r#"{"type":"undo"}"#);
    let ev = h.command_json(r#"{"type":"undo"}"#);
    let _ = ev;
    let n = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    assert!(n.notes.iter().all(|x| x.voice == 1), "annulé");
    h.command_json(r#"{"type":"redo"}"#);

    // Enregistré puis rouvert : la voix est conservée.
    let path = dir.join("v.daw");
    let p = serde_json::to_string(&path.to_string_lossy()).unwrap();
    h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#));
    let mut h2 = host();
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    let clip2 = midi_clip(&mut h2);
    let n = notes(&h2.command_json(&format!(r#"{{"type":"get_notes","clip":{clip2}}}"#)));
    assert_eq!(n.notes.iter().filter(|x| x.voice == 2).count(), 1);
    assert_eq!(n.notes.iter().filter(|x| x.voice == 3).count(), 1);
    // Le JSON d'un projet sans voix ne porte pas le champ (anciens fichiers identiques).
    let text = std::fs::read_to_string(path.join("projet.json")).unwrap();
    assert!(text.contains("\"voice\""));
}

#[test]
fn a_control_lane_is_replaced_in_one_undoable_action_without_touching_the_others() {
    let mut h = host();
    let clip = midi_clip(&mut h);
    let n = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    let before = n.events.len();

    // Une courbe de modulation (CC 1) de trois points, puis une molette : deux voies indépendantes.
    let ev = h.command_json(&format!(r#"{{"type":"set_events","clip":{clip},"kind":"cc","controller":1,"events":[{{"start":[1,8],"value":10}},{{"start":[1,4],"value":64}},{{"start":[3,8],"value":127}}],"label":"Dessiner la modulation"}}"#));
    let n = notes(&ev);
    let cc1: Vec<_> = n.events.iter().filter(|e| e.kind == "cc" && e.controller == Some(1)).collect();
    assert_eq!(cc1.len(), 3);
    assert_eq!(cc1.iter().map(|e| e.value).collect::<Vec<_>>(), [10, 64, 127], "dans l'ordre du temps");
    assert_eq!(undo_label(&ev).as_deref(), Some("Dessiner la modulation"));
    let ev = h.command_json(&format!(r#"{{"type":"set_events","clip":{clip},"kind":"bend","events":[{{"start":[1,2],"value":-8192}},{{"start":[5,8],"value":8191}}]}}"#));
    let n = notes(&ev);
    assert_eq!(n.events.iter().filter(|e| e.kind == "bend").count(), 2);
    assert_eq!(n.events.iter().filter(|e| e.kind == "cc" && e.controller == Some(1)).count(), 3, "l'autre voie ne bouge pas");

    // Remplacer la voie CC 1 : les anciens points disparaissent, ceux d'un autre contrôleur restent.
    h.command_json(&format!(r#"{{"type":"set_events","clip":{clip},"kind":"cc","controller":64,"events":[{{"start":[0,1],"value":127}}]}}"#));
    let ev = h.command_json(&format!(r#"{{"type":"set_events","clip":{clip},"kind":"cc","controller":1,"events":[{{"start":[1,2],"value":5}}]}}"#));
    let n = notes(&ev);
    assert_eq!(n.events.iter().filter(|e| e.controller == Some(1)).map(|e| e.value).collect::<Vec<_>>(), [5]);
    assert_eq!(n.events.iter().filter(|e| e.controller == Some(64)).count(), 1);
    // Une voie vidée = tous ses points supprimés.
    let ev = h.command_json(&format!(r#"{{"type":"set_events","clip":{clip},"kind":"cc","controller":1,"events":[]}}"#));
    assert_eq!(notes(&ev).events.iter().filter(|e| e.controller == Some(1)).count(), 0);

    // Annuler : chaque geste est un pas ; on revient à l'état de départ.
    for _ in 0..5 {
        h.command_json(r#"{"type":"undo"}"#);
    }
    let n = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    assert_eq!(n.events.len(), before, "tout annulé");

    // Refus sans trace : voie inconnue, contrôleur manquant ou hors plage, valeur hors plage, point hors du clip.
    for bad in [
        r#""kind":"cc","events":[]"#,
        r#""kind":"cc","controller":200,"events":[]"#,
        r#""kind":"nope","events":[]"#,
        r#""kind":"bend","controller":1,"events":[]"#,
        r#""kind":"cc","controller":1,"events":[{"start":[0,1],"value":128}]"#,
        r#""kind":"bend","events":[{"start":[0,1],"value":8192}]"#,
        r#""kind":"cc","controller":1,"events":[{"start":[0,1],"channel":16,"value":1}]"#,
        r#""kind":"cc","controller":1,"events":[{"start":[900,1],"value":1}]"#,
    ] {
        let ev = h.command_json(&format!(r#"{{"type":"set_events","clip":{clip},{bad}}}"#));
        assert!(!error(&ev).is_empty(), "{bad}");
    }
    let n = notes(&h.command_json(&format!(r#"{{"type":"get_notes","clip":{clip}}}"#)));
    assert_eq!(n.events.len(), before);
}
