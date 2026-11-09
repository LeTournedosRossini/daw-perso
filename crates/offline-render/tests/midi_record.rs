//! Entrée MIDI en direct et enregistrement des notes jouées.

mod common;

use common::*;
use daw_engine::{Command, GraphDesc, Insert, InsertFactory, MidiMsg, TrackFormat};
use daw_offline_render::{render, MidiRecordJob, RenderSpec};

fn on(key: u8) -> MidiMsg {
    MidiMsg::NoteOn { channel: 0, key, velocity: 0.75 }
}
fn off(key: u8) -> MidiMsg {
    MidiMsg::NoteOff { channel: 0, key, velocity: 0.0 }
}

/// Effet de test : pose 1,0 sur la sortie gauche à chaque début de note.
struct Clicker;
impl Insert for Clicker {
    fn latency(&self) -> u32 {
        0
    }
    fn process(&mut self, _: &[f32], _: &[f32], l: &mut [f32], r: &mut [f32]) {
        l.fill(0.0);
        r.fill(0.0);
    }
    fn accepts_notes(&self) -> bool {
        true
    }
    fn process_with_notes(&mut self, _: &[f32], _: &[f32], l: &mut [f32], r: &mut [f32], notes: &[daw_engine::MidiEvent]) {
        l.fill(0.0);
        r.fill(0.0);
        for e in notes {
            if let MidiMsg::NoteOn { .. } = e.msg {
                l[e.time as usize] = 1.0;
            }
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn setup() -> (GraphDesc, u32) {
    let mut g = GraphDesc::new();
    let t = g.add_track("clavier", TrackFormat::Stereo);
    let f: InsertFactory = std::sync::Arc::new(|_, _, _| Ok(Box::new(Clicker) as Box<dyn Insert>));
    g.add_insert(t, "clicker", f).unwrap();
    (g, t.0)
}

fn spec(g: &GraphDesc, t: u32, block: usize, engine_block: usize) -> RenderSpec {
    let mut s = RenderSpec::new(SR, 3_000, g.clone());
    s.block = block;
    s.engine_block = engine_block;
    s.midi_in = vec![(100, t, on(60)), (1_000, t, off(60)), (1_001, t, on(64)), (1_001, t, off(64)), (2_047, t, on(67)), (2_500, t, off(67))];
    s.midi_records = vec![MidiRecordJob { node: t, start: 50, stop: 2_900 }];
    s
}

#[test]
fn played_notes_are_recorded_and_heard_at_the_exact_sample_for_any_block_size() {
    let (g, t) = setup();
    let reference = render(&spec(&g, t, 4_096, 4_096)).unwrap();
    let take = &reference.midi_takes[0];
    let got: Vec<(u64, MidiMsg)> = take.events.iter().map(|e| (e.pos, e.msg)).collect();
    assert_eq!(got, vec![(100, on(60)), (1_000, off(60)), (1_001, on(64)), (1_001, off(64)), (2_047, on(67)), (2_500, off(67))]);
    assert_eq!((take.start_pos, take.stop_pos, take.overruns), (50, 2_900, 0));
    // L'instrument entend les débuts de note aux mêmes échantillons.
    let heard: Vec<usize> = (0..reference.left.len()).filter(|&i| reference.left[i] != 0.0).collect();
    assert_eq!(heard, vec![100, 1_001, 2_047]);
    for (block, eb) in [(1, 1), (7, 64), (64, 256), (100, 33), (333, 256), (1_024, 1_024)] {
        let r = render(&spec(&g, t, block, eb)).unwrap();
        assert_eq!(r.midi_takes, reference.midi_takes, "prise, blocs {block}/{eb}");
        assert_eq!(r.hash(), reference.hash(), "son, blocs {block}/{eb}");
    }
}

#[test]
fn only_what_is_played_live_is_recorded_not_the_clips() {
    let (mut g, t) = setup();
    g.add_midi_clip(daw_core_types::NodeId(t), daw_engine::MidiClip::new(0, 1_000, std::sync::Arc::new(vec![daw_engine::Note { start: 200, len: 100, key: 50, velocity: 1.0, channel: 0 }])));
    let r = render(&spec(&g, t, 256, 256)).unwrap();
    assert!(r.midi_takes[0].events.iter().all(|e| e.msg.key() != 50));
    let heard: Vec<usize> = (0..r.left.len()).filter(|&i| r.left[i] != 0.0).collect();
    assert_eq!(heard, vec![100, 200, 1_001, 2_047], "le clip joue toujours, et le direct aussi");
}

#[test]
fn no_take_when_nothing_is_armed_and_unknown_tracks_are_ignored() {
    let (g, t) = setup();
    let mut s = spec(&g, t, 256, 256);
    s.midi_records.clear();
    s.midi_in.push((10, 99, on(60))); // piste inconnue
    let r = render(&s).unwrap();
    assert!(r.midi_takes.is_empty());
    let heard: Vec<usize> = (0..r.left.len()).filter(|&i| r.left[i] != 0.0).collect();
    assert_eq!(heard, vec![100, 1_001, 2_047]);
}

#[test]
fn transport_stop_cuts_held_notes_and_the_take_sees_the_note_off() {
    let (g, t) = setup();
    let mut s = RenderSpec::new(SR, 1_000, g);
    s.midi_in = vec![(100, t, on(60))];
    s.midi_records = vec![MidiRecordJob { node: t, start: 0, stop: 900 }];
    s.events.push((500, Command::Stop));
    let r = render(&s).unwrap();
    let ev: Vec<_> = r.midi_takes[0].events.iter().map(|e| (e.pos, e.msg.key(), matches!(e.msg, MidiMsg::NoteOn { .. }))).collect();
    assert_eq!(ev, vec![(100, 60, true), (500, 60, false)]);
}
