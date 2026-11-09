use daw_core_types::NodeId;
use daw_engine::{GraphDesc, MidiMsg, RecordedMidi, TrackFormat};
use daw_session::{MidiTakeData, Session};

fn setup() -> (Session, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    (Session::new(g), t)
}

fn pass(start: u64, stop: u64, notes: &[(u64, u8)]) -> MidiTakeData {
    let mut events = Vec::new();
    for &(at, key) in notes {
        events.push(RecordedMidi { pos: at, msg: MidiMsg::NoteOn { channel: 0, key, velocity: 0.8 } });
        events.push(RecordedMidi { pos: at + 2_000, msg: MidiMsg::NoteOff { channel: 0, key, velocity: 0.0 } });
    }
    MidiTakeData { start_pos: start, stop_pos: stop, events }
}

fn flags(s: &Session, t: NodeId) -> Vec<(bool, u32)> {
    s.midi_clip_ids(t).iter().map(|&id| s.midi_clip(id).map(|c| (c.muted, c.take_group)).unwrap()).collect()
}

fn playing_notes(s: &Session, t: NodeId) -> usize {
    s.graph().nodes[t.0 as usize].midi_clips.iter().filter(|c| !c.muted).map(|c| c.notes.len()).sum()
}

#[test]
fn passes_become_containers_of_one_group_the_last_active_and_empty_passes_make_none() {
    let (mut s, t) = setup();
    let passes = [pass(0, 96_000, &[(1_000, 60)]), pass(0, 96_000, &[]), pass(0, 96_000, &[(3_000, 62), (5_000, 64)])];
    let ids = s.add_recorded_passes(t, &passes, 0).unwrap();
    assert_eq!(ids.len(), 2, "le passage vide ne crée pas de conteneur");
    assert_eq!(flags(&s, t), vec![(true, 1), (false, 1)]);
    assert_eq!(playing_notes(&s, t), 2, "seul le dernier conteneur joue");
    assert_eq!(s.undo_label(), Some("Enregistrer"));
    assert!(s.undo());
    assert!(s.midi_clip_ids(t).is_empty(), "une seule action annulable");
}

#[test]
fn a_single_non_empty_pass_is_an_ordinary_clip_and_all_empty_creates_nothing() {
    let (mut s, t) = setup();
    assert!(s.add_recorded_passes(t, &[pass(0, 1_000, &[]), pass(0, 1_000, &[])], 0).unwrap().is_empty());
    s.add_recorded_passes(t, &[pass(0, 96_000, &[]), pass(0, 96_000, &[(1_000, 60)])], 0).unwrap();
    assert_eq!(flags(&s, t), vec![(false, 0)]);
}

#[test]
fn switching_the_active_midi_container_is_one_undo_step_and_the_project_keeps_it() {
    let (mut s, t) = setup();
    let ids = s.add_recorded_passes(t, &[pass(0, 96_000, &[(1_000, 60)]), pass(0, 96_000, &[(3_000, 62), (5_000, 64)])], 0).unwrap();
    assert_eq!(flags(&s, t), vec![(true, 1), (false, 1)]);
    s.set_active_midi_take(ids[0]).unwrap();
    assert_eq!(flags(&s, t), vec![(false, 1), (true, 1)]);
    assert_eq!(playing_notes(&s, t), 1);
    assert_eq!(s.undo_label(), Some("Changer de prise"));
    assert!(s.undo());
    assert_eq!(flags(&s, t), vec![(true, 1), (false, 1)]);
    let (json, _) = s.to_json(48_000).unwrap();
    assert!(json.contains("\"muted\": true") && json.contains("\"take_group\": 1"));
}
