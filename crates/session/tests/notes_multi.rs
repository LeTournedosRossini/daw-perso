//! Modifier, ajouter et supprimer plusieurs notes en une seule action annulable.

use daw_core_types::time::Frac;
use daw_core_types::NodeId;
use daw_engine::{GraphDesc, TrackFormat};
use daw_session::{MClip, MNote, MidiClipId, NoteEdit, Session, SessionError};

fn f(n: u64, d: u64) -> Frac {
    Frac::new(n, d).unwrap()
}

fn note(start: Frac, key: u8) -> MNote {
    MNote { start, len: f(1, 8), key, velocity: 0.5, channel: 0, voice: 1 }
}

fn setup() -> (Session, MidiClipId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("Piano", TrackFormat::Stereo);
    let mut s = Session::new(g);
    let id = s.add_midi_clip(t, MClip::new(f(0, 1), f(1, 1))).unwrap();
    for (st, k) in [(f(0, 1), 60), (f(1, 4), 62), (f(1, 2), 64)] {
        s.add_note(id, note(st, k)).unwrap();
    }
    (s, id)
}

fn keys(s: &Session, id: MidiClipId) -> Vec<(Frac, u8, f32)> {
    s.midi_clip(id).unwrap().notes.iter().map(|n| (n.start, n.key, n.velocity)).collect()
}

#[test]
fn several_notes_move_in_one_undo_step_and_the_new_indices_are_returned() {
    let (mut s, id) = setup();
    let before = keys(&s, id);
    // Le premier et le dernier : décalés d'une croche ; le premier passe donc après le deuxième.
    let edits = [(0, NoteEdit { start: Some(f(3, 8)), key: Some(61), ..NoteEdit::default() }), (2, NoteEdit { start: Some(f(5, 8)), ..NoteEdit::default() })];
    let idx = s.edit_notes(id, "Déplacer des notes", &edits).unwrap();
    let after = keys(&s, id);
    assert_eq!(after, vec![(f(1, 4), 62, 0.5), (f(3, 8), 61, 0.5), (f(5, 8), 64, 0.5)]);
    assert_eq!(idx, vec![1, 2], "où atterrissent les notes 0 et 2");
    assert_eq!(s.undo_label(), Some("Déplacer des notes"));
    s.undo();
    assert_eq!(keys(&s, id), before, "un seul pas d'annulation");
    s.redo();
    assert_eq!(keys(&s, id), after);
}

#[test]
fn velocity_and_length_edits_and_refusals_leave_no_trace() {
    let (mut s, id) = setup();
    s.edit_notes(id, "Vélocité", &[(1, NoteEdit { velocity: Some(0.9), len: Some(f(1, 16)), ..NoteEdit::default() })]).unwrap();
    assert_eq!(s.midi_clip(id).unwrap().notes[1].velocity, 0.9);
    let before = keys(&s, id);
    let depth = s.can_undo();
    for bad in [
        vec![(7, NoteEdit::default())],                                                                  // indice inexistant
        vec![(0, NoteEdit::default()), (0, NoteEdit::default())],                                         // deux fois la même note
        vec![(0, NoteEdit { start: Some(f(2, 1)), ..NoteEdit::default() })],                                // début hors du clip
        vec![(0, NoteEdit { velocity: Some(0.0), ..NoteEdit::default() })],                                 // vélocité nulle
        vec![(0, NoteEdit { key: Some(200), ..NoteEdit::default() })],                                      // touche invalide
    ] {
        let r = s.edit_notes(id, "x", &bad);
        assert!(matches!(r, Err(SessionError::BadNoteIndex(_) | SessionError::BadMidi)), "{r:?}");
    }
    assert_eq!(keys(&s, id), before);
    assert_eq!(s.can_undo(), depth);
    assert_eq!(s.undo_label(), Some("Vélocité"));
}

#[test]
fn several_notes_are_added_and_removed_in_one_step() {
    let (mut s, id) = setup();
    let idx = s.add_notes(id, &[note(f(3, 4), 67), note(f(1, 8), 65)]).unwrap();
    assert_eq!(s.midi_clip(id).unwrap().notes.len(), 5);
    assert_eq!(s.midi_clip(id).unwrap().notes[idx[0]].key, 67);
    assert_eq!(s.midi_clip(id).unwrap().notes[idx[1]].key, 65);
    assert!(s.add_notes(id, &[note(f(5, 1), 60)]).is_err(), "hors du clip");
    assert_eq!(s.midi_clip(id).unwrap().notes.len(), 5);
    s.remove_notes(id, &[0, 2, 0]).unwrap(); // doublon ignoré
    assert_eq!(s.midi_clip(id).unwrap().notes.len(), 3);
    assert!(s.remove_notes(id, &[9]).is_err());
    s.undo();
    assert_eq!(s.midi_clip(id).unwrap().notes.len(), 5);
    s.undo();
    assert_eq!(s.midi_clip(id).unwrap().notes.len(), 3);
    let _ = NodeId(0);
}

#[test]
fn the_channel_of_several_notes_changes_in_one_undo_step_and_an_invalid_channel_is_refused() {
    let (mut s, id) = setup();
    let channels = |s: &Session| s.midi_clip(id).unwrap().notes.iter().map(|n| n.channel).collect::<Vec<_>>();
    assert_eq!(channels(&s), vec![0, 0, 0]);
    let edits = [(0, NoteEdit { channel: Some(5), ..NoteEdit::default() }), (2, NoteEdit { channel: Some(9), ..NoteEdit::default() })];
    s.edit_notes(id, "Canal", &edits).unwrap();
    assert_eq!(channels(&s), vec![5, 0, 9]);
    assert_eq!(s.undo_label(), Some("Canal"));
    s.undo();
    assert_eq!(channels(&s), vec![0, 0, 0], "un seul pas d'annulation");
    let steps = s.undo_label().map(str::to_owned);
    assert!(s.edit_notes(id, "Canal", &[(0, NoteEdit { channel: Some(16), ..NoteEdit::default() })]).is_err());
    assert_eq!(s.undo_label().map(str::to_owned), steps, "refus sans trace");
}
