//! Tempo et chiffrage modifiables : carte des tempos, notes MIDI qui suivent, annulation, refus.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{ClipInfo, ProjectInfo};
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

/// Longueur (échantillons) du clip de notes de la démonstration : 8 mesures.
fn arpeggio_len(p: &ProjectInfo) -> u64 {
    p.nodes.iter().flat_map(|n| &n.clips).find_map(|c| if let ClipInfo::Midi { len, .. } = c { Some(*len) } else { None }).unwrap()
}

#[test]
fn changing_the_tempo_moves_the_notes_not_the_audio_and_is_undoable() {
    let mut h = host();
    let before = h.command_json(r#"{"type":"get_project"}"#);
    let audio_before: Vec<(u64, u64)> = project(&before).nodes.iter().flat_map(|n| &n.clips).filter_map(|c| if let ClipInfo::Audio { start, len, .. } = c { Some((*start, *len)) } else { None }).collect();
    assert_eq!(arpeggio_len(project(&before)), 768_000, "8 mesures à 120 bpm");
    let ev = h.command_json(r#"{"type":"set_tempo","bpm":60,"at_bar":1}"#);
    let p = project(&ev);
    assert_eq!((p.tempo_map[0].num, p.tempo_map[0].den), (60, 1));
    assert_eq!(arpeggio_len(p), 1_536_000, "deux fois plus lent : le clip de notes dure deux fois plus longtemps");
    let audio_after: Vec<(u64, u64)> = p.nodes.iter().flat_map(|n| &n.clips).filter_map(|c| if let ClipInfo::Audio { start, len, .. } = c { Some((*start, *len)) } else { None }).collect();
    assert_eq!(audio_after, audio_before, "les clips audio ne bougent pas");
    assert_eq!(undo_label(&ev).as_deref(), Some("Changer le tempo"));
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(arpeggio_len(project(&ev)), 768_000);
    assert_eq!(project(&ev).tempo_map[0].num, 120);
    // Un tempo à décimales reste exact : 133,333 = 133333/1000.
    let ev = h.command_json(r#"{"type":"set_tempo","bpm":133.333,"at_bar":1}"#);
    assert_eq!((project(&ev).tempo_map[0].num, project(&ev).tempo_map[0].den), (133_333, 1_000));
}

#[test]
fn a_tempo_change_from_a_later_bar_adds_a_step_at_that_bar() {
    let mut h = host();
    let ev = h.command_json(r#"{"type":"set_tempo","bpm":90,"at_bar":5}"#);
    let map = &project(&ev).tempo_map;
    assert_eq!(map.len(), 2);
    assert_eq!((map[0].num, map[1].num), (120, 90));
    // Mesure 5 = 4 mesures de 4/4 = 4 rondes : position [4, 1] (fraction réduite).
    assert_eq!(map[1].pos, [4, 1]);
    // Un changement à la mesure 1 ne touche que le premier palier.
    let ev = h.command_json(r#"{"type":"set_tempo","bpm":100,"at_bar":1}"#);
    assert_eq!((project(&ev).tempo_map[0].num, project(&ev).tempo_map[1].num), (100, 90));
}

#[test]
fn meters_change_from_a_bar_and_are_undoable() {
    let mut h = host();
    let ev = h.command_json(r#"{"type":"set_meter","num":3,"den":4,"at_bar":1}"#);
    assert_eq!((project(&ev).meters[0].num, project(&ev).meters[0].den), (3, 4));
    assert_eq!(undo_label(&ev).as_deref(), Some("Changer le chiffrage"));
    let ev = h.command_json(r#"{"type":"set_meter","num":7,"den":8,"at_bar":3}"#);
    assert_eq!(project(&ev).meters.len(), 2);
    assert_eq!((project(&ev).meters[1].bar, project(&ev).meters[1].num, project(&ev).meters[1].den), (2, 7, 8));
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(project(&ev).meters.len(), 1);
}

#[test]
fn bad_tempo_and_meter_requests_are_refused_without_trace() {
    let mut h = host();
    for (cmd, expect) in [
        (r#"{"type":"set_tempo","bpm":10,"at_bar":1}"#, "entre 20 et 400"),
        (r#"{"type":"set_tempo","bpm":500,"at_bar":1}"#, "entre 20 et 400"),
        (r#"{"type":"set_tempo","bpm":120,"at_bar":0}"#, "at_bar"),
        (r#"{"type":"set_meter","num":0,"den":4,"at_bar":1}"#, "num de 1 à 32"),
        (r#"{"type":"set_meter","num":4,"den":6,"at_bar":1}"#, "den parmi"),
        (r#"{"type":"set_meter","num":4,"den":4,"at_bar":0}"#, "at_bar"),
    ] {
        assert!(error(&h.command_json(cmd)).contains(expect), "{cmd}");
    }
    assert_eq!(undo_label(&h.command_json(r#"{"type":"get_project"}"#)), None);
    // Le même tempo que l'actuel : rien n'est inscrit dans l'historique.
    h.command_json(r#"{"type":"set_tempo","bpm":120,"at_bar":1}"#);
    assert_eq!(undo_label(&h.command_json(r#"{"type":"get_project"}"#)), None);
}

#[test]
fn a_tempo_or_meter_change_can_be_removed_and_the_removal_undone() {
    let mut h = host();
    h.command_json(r#"{"type":"set_tempo","bpm":90,"at_bar":5}"#);
    let ev = h.command_json(r#"{"type":"set_meter","num":3,"den":4,"at_bar":5}"#);
    let p = project(&ev);
    assert_eq!((p.tempo_map.len(), p.meters.len()), (2, 2));
    let pos = p.tempo_map[1].pos;
    let ev = h.command_json(&format!(r#"{{"type":"remove_tempo","pos":[{},{}]}}"#, pos[0], pos[1]));
    assert_eq!(project(&ev).tempo_map.len(), 1);
    assert_eq!(undo_label(&ev).as_deref(), Some("Supprimer un changement de tempo"));
    let ev = h.command_json(r#"{"type":"remove_meter","at_bar":5}"#);
    assert_eq!(project(&ev).meters.len(), 1);
    // Tout annuler : les deux marqueurs reviennent.
    h.command_json(r#"{"type":"undo"}"#);
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!((project(&ev).tempo_map.len(), project(&ev).meters.len()), (2, 2));
    // Refus : le premier, une position sans changement.
    assert!(!error(&h.command_json(r#"{"type":"remove_tempo","pos":[0,1]}"#)).is_empty());
    assert!(!error(&h.command_json(r#"{"type":"remove_tempo","pos":[7,16]}"#)).is_empty());
    assert!(!error(&h.command_json(r#"{"type":"remove_meter","at_bar":1}"#)).is_empty());
    assert!(!error(&h.command_json(r#"{"type":"remove_meter","at_bar":3}"#)).is_empty());
}
