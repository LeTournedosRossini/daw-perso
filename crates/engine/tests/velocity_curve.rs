//! Courbe de réponse de la vélocité d'une piste : appliquée aux notes des clips et à celles jouées en direct, avant le plugin ;
//! désactivée, les notes passent telles quelles ; la prise MIDI reste brute.

use std::sync::{Arc, Mutex};

use daw_engine::{MidiClip, MidiEvent, MidiMsg, Note};
use daw_engine::{Command, Engine, GraphDesc, Insert, InsertFactory, TrackFormat, VelocityCurve};

const SR: u32 = 48_000;

/// Plugin espion : mémorise la vélocité de chaque début de note reçu.
struct Spy(Arc<Mutex<Vec<f32>>>);

impl Insert for Spy {
    fn latency(&self) -> u32 {
        0
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        out_l.copy_from_slice(in_l);
        out_r.copy_from_slice(in_r);
    }
    fn accepts_notes(&self) -> bool {
        true
    }
    fn process_with_notes(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32], notes: &[MidiEvent]) {
        self.process(in_l, in_r, out_l, out_r);
        for e in notes {
            if let MidiMsg::NoteOn { velocity, .. } = e.msg {
                self.0.lock().unwrap().push(velocity);
            }
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn rig(curve: VelocityCurve) -> (Engine, daw_engine::EngineHandle, u32, Arc<Mutex<Vec<f32>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = Arc::clone(&seen);
    let factory: InsertFactory = Arc::new(move |_, _, _| Ok(Box::new(Spy(Arc::clone(&s2))) as Box<dyn Insert>));
    let mut g = GraphDesc::new();
    let t = g.add_track("clavier", TrackFormat::Stereo);
    g.add_insert(t, "espion", factory).unwrap();
    g.node_mut(t).unwrap().vel_curve = curve;
    let notes = Arc::new(vec![Note { start: 100, len: 50, key: 60, velocity: 1.0 / 127.0, channel: 0 }, Note { start: 300, len: 50, key: 62, velocity: 1.0, channel: 0 }]);
    g.add_midi_clip(t, MidiClip::new(0, 1000, notes));
    let (engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    (engine, handle, t.0, seen)
}

fn run(engine: &mut Engine, blocks: usize) {
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..blocks {
        engine.process(&mut l, &mut r);
    }
}

fn v(x: u8) -> f32 {
    x as f32 / 127.0
}

#[test]
fn disabled_the_notes_reach_the_plugin_unchanged() {
    let (mut engine, mut handle, t, seen) = rig(VelocityCurve::default());
    handle.send(Command::Play);
    handle.send(Command::Midi { node: t, msg: MidiMsg::NoteOn { channel: 0, key: 64, velocity: v(70) } });
    run(&mut engine, 10);
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 3);
    assert!(got.contains(&v(70)) && got.contains(&1.0) && got.contains(&v(1)), "{got:?}");
}

#[test]
fn enabled_the_curve_changes_clip_notes_and_live_notes_before_the_plugin() {
    let curve = VelocityCurve { enabled: true, min: 40, max: 100, ..VelocityCurve::default() };
    let (mut engine, mut handle, t, seen) = rig(curve);
    handle.send(Command::Play);
    handle.send(Command::Midi { node: t, msg: MidiMsg::NoteOn { channel: 0, key: 64, velocity: v(127) } });
    handle.send(Command::Midi { node: t, msg: MidiMsg::NoteOff { channel: 0, key: 64, velocity: v(20) } });
    run(&mut engine, 10);
    let mut got = seen.lock().unwrap().clone();
    got.sort_by(f32::total_cmp);
    assert_eq!(got, vec![v(40), v(100), v(100)], "clip : 1 → 40 et 127 → 100 ; direct : 127 → 100");
}

#[test]
fn a_fixed_velocity_and_a_switch_through_the_graph_work() {
    let (mut engine, mut handle, t, seen) = rig(VelocityCurve { enabled: true, fixed: 90, ..VelocityCurve::default() });
    handle.send(Command::Play);
    handle.send(Command::Midi { node: t, msg: MidiMsg::NoteOn { channel: 0, key: 64, velocity: v(5) } });
    run(&mut engine, 10);
    assert!(seen.lock().unwrap().iter().all(|&x| x == v(90)));
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[test]
fn the_midi_take_stays_raw() {
    let curve = VelocityCurve { enabled: true, min: 40, max: 40, ..VelocityCurve::default() };
    let (mut engine, mut handle, t, seen) = rig(curve);
    let mut rx = handle.start_midi_record(t, 64).unwrap();
    handle.send(Command::Midi { node: t, msg: MidiMsg::NoteOn { channel: 0, key: 64, velocity: v(110) } });
    run(&mut engine, 4);
    handle.stop_midi_record(t);
    run(&mut engine, 2);
    let events = rx.take_events();
    assert!(events.iter().any(|e| matches!(e.msg, MidiMsg::NoteOn { velocity, .. } if velocity == v(110))), "la prise garde 110 : {events:?}");
    assert!(seen.lock().unwrap().contains(&v(40)), "le plugin reçoit 40");
}
