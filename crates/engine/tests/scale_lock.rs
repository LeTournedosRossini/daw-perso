//! Verrou de gamme d'une piste : les notes jouées en direct sont ramenées à la gamme ou à l'accord en vigueur de la piste d'accords ;
//! la fin d'une note rejoint la touche réellement émise ; les clips ne sont jamais touchés ; la prise enregistre ce qui a été joué
//! (donc la touche ramenée).

use std::sync::{Arc, Mutex};

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};

use daw_engine::{ChordItem, ChordSnapshot, Command, Engine, GraphDesc, Insert, InsertFactory, MidiClip, MidiEvent, MidiMsg, Note, ScaleLock, TrackFormat};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;
static SERIAL: Mutex<()> = Mutex::new(());

const SR: u32 = 48_000;
/// À 120 bpm une noire dure 24 000 échantillons.
const BEAT: u64 = 24_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Ev {
    On(u8),
    Off(u8),
}

struct Spy(Arc<Mutex<Vec<Ev>>>);

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
            match e.msg {
                MidiMsg::NoteOn { key, .. } => self.0.lock().unwrap().push(Ev::On(key)),
                MidiMsg::NoteOff { key, .. } => self.0.lock().unwrap().push(Ev::Off(key)),
                _ => {}
            }
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn item(start: f64, len: f64, root: u8, mask: u32) -> ChordItem {
    let mut i = ChordItem::new(start, len);
    i.root = root;
    i.mask = mask;
    i
}

/// Do majeur (beats 0 à 8), puis ré majeur (8 à 16) ; accord do majeur (0 à 4), puis accord ré mineur (4 à 8).
fn snapshot() -> Arc<ChordSnapshot> {
    let mut s = ChordSnapshot::default();
    let major = 0b1010_1011_0101;
    s.scales = vec![item(0.0, 8.0, 0, major), item(8.0, 8.0, 2, major)];
    s.max_scale_len = 8.0;
    s.chords = vec![item(0.0, 4.0, 0, 0b1001_0001), item(4.0, 4.0, 2, 0b1000_1001)];
    Arc::new(s)
}

fn rig(lock: ScaleLock, with_clip: bool) -> (Engine, daw_engine::EngineHandle, u32, Arc<Mutex<Vec<Ev>>>, GraphDesc) {
    let seen = Arc::new(Mutex::new(Vec::with_capacity(8192))); // réservé : le plugin espion ne doit pas allouer dans le callback
    // Sous macOS le premier verrouillage d'un `Mutex` alloue : on le fait ici, hors du callback surveillé.
    drop(seen.lock());
    let s2 = Arc::clone(&seen);
    let factory: InsertFactory = Arc::new(move |_, _, _| Ok(Box::new(Spy(Arc::clone(&s2))) as Box<dyn Insert>));
    let mut g = GraphDesc::new();
    let t = g.add_track("clavier", TrackFormat::Stereo);
    g.add_insert(t, "espion", factory).unwrap();
    g.node_mut(t).unwrap().scale_lock = lock;
    g.chords = snapshot();
    if with_clip {
        let notes = Arc::new(vec![Note { start: 100, len: 50, key: 61, velocity: 0.8, channel: 0 }]);
        g.add_midi_clip(t, MidiClip::new(0, 1000, notes));
    }
    let (engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    (engine, handle, t.0, seen, g)
}

fn run(engine: &mut Engine, blocks: usize) {
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..blocks {
        engine.process(&mut l, &mut r);
    }
}

fn on(key: u8) -> MidiMsg {
    MidiMsg::NoteOn { channel: 0, key, velocity: 0.7 }
}
fn off(key: u8) -> MidiMsg {
    MidiMsg::NoteOff { channel: 0, key, velocity: 0.0 }
}

#[test]
fn off_the_notes_pass_unchanged() {
    let (mut engine, mut handle, t, seen, _) = rig(ScaleLock::Off, false);
    handle.send(Command::Midi { node: t, msg: on(61) });
    handle.send(Command::Midi { node: t, msg: off(61) });
    run(&mut engine, 2);
    assert_eq!(*seen.lock().unwrap(), vec![Ev::On(61), Ev::Off(61)]);
}

#[test]
fn a_live_note_is_brought_to_the_scale_and_its_end_follows_the_sounding_key() {
    let (mut engine, mut handle, t, seen, _) = rig(ScaleLock::Scale, false);
    handle.send(Command::Midi { node: t, msg: on(61) }); // do# → do (égalité : la plus grave)
    run(&mut engine, 2);
    handle.send(Command::Midi { node: t, msg: off(61) });
    run(&mut engine, 2);
    assert_eq!(*seen.lock().unwrap(), vec![Ev::On(60), Ev::Off(60)], "la fin va à la touche réellement émise");
}

#[test]
fn the_scale_in_force_at_the_instant_of_the_note_decides() {
    let (mut engine, mut handle, t, seen, _) = rig(ScaleLock::Scale, false);
    handle.send(Command::Midi { node: t, msg: on(61) });
    run(&mut engine, 2);
    handle.send(Command::Midi { node: t, msg: off(61) });
    handle.send(Command::Seek(9 * BEAT)); // ré majeur : do# est permis
    run(&mut engine, 2);
    handle.send(Command::Midi { node: t, msg: on(61) });
    run(&mut engine, 2);
    handle.send(Command::Seek(20 * BEAT)); // plus aucune gamme : rien n'est contraint
    run(&mut engine, 2);
    handle.send(Command::Midi { node: t, msg: on(66) });
    run(&mut engine, 2);
    let got = seen.lock().unwrap().clone();
    let ons: Vec<u8> = got.iter().filter_map(|e| if let Ev::On(k) = e { Some(*k) } else { None }).collect();
    assert_eq!(ons, vec![60, 61, 66], "{got:?}");
}

#[test]
fn chord_mode_uses_the_chord_notes() {
    let (mut engine, mut handle, t, seen, _) = rig(ScaleLock::Chord, false);
    handle.send(Command::Midi { node: t, msg: on(62) }); // ré sous l'accord do (do mi sol) : 60 et 64 à égalité → 60
    handle.send(Command::Midi { node: t, msg: on(63) }); // ré# → mi
    run(&mut engine, 2);
    handle.send(Command::Seek(5 * BEAT)); // accord ré mineur (ré fa la)
    run(&mut engine, 2);
    handle.send(Command::Midi { node: t, msg: on(61) }); // do# → ré
    run(&mut engine, 2);
    let ons: Vec<u8> = seen.lock().unwrap().iter().filter_map(|e| if let Ev::On(k) = e { Some(*k) } else { None }).collect();
    assert_eq!(ons, vec![60, 64, 62]);
}

#[test]
fn clips_are_never_touched() {
    let (mut engine, mut handle, _, seen, _) = rig(ScaleLock::Scale, true);
    handle.send(Command::Play);
    run(&mut engine, 6);
    assert!(seen.lock().unwrap().contains(&Ev::On(61)), "la note du clip reste à 61 : {:?}", seen.lock().unwrap());
}

#[test]
fn the_end_of_a_note_still_follows_when_the_lock_is_switched_off_meanwhile() {
    let (mut engine, mut handle, t, seen, mut g) = rig(ScaleLock::Scale, false);
    handle.send(Command::Midi { node: t, msg: on(61) });
    run(&mut engine, 2);
    g.nodes[t as usize].scale_lock = ScaleLock::Off;
    handle.load_graph(&g).unwrap();
    run(&mut engine, 2);
    handle.send(Command::Midi { node: t, msg: off(61) });
    run(&mut engine, 2);
    let got = seen.lock().unwrap().clone();
    assert!(got.contains(&Ev::Off(60)) && !got.contains(&Ev::Off(61)), "pas de note bloquée : {got:?}");
}

#[test]
fn the_midi_take_records_the_note_that_was_actually_played() {
    let (mut engine, mut handle, t, _, _) = rig(ScaleLock::Scale, false);
    let mut rx = handle.start_midi_record(t, 64).unwrap();
    handle.send(Command::Midi { node: t, msg: on(61) });
    run(&mut engine, 4);
    handle.stop_midi_record(t);
    run(&mut engine, 2);
    let events = rx.take_events();
    assert!(events.iter().any(|e| matches!(e.msg, MidiMsg::NoteOn { key: 60, .. })), "la prise garde la touche ramenée : {events:?}");
}

#[test]
fn no_allocation_in_the_callback_with_the_lock_notes_and_a_graph_swap() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut engine, mut handle, t, _seen, mut g) = rig(ScaleLock::Scale, true);
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..400u32 {
            if i % 7 == 0 {
                handle.send(Command::Midi { node: t, msg: on(55 + (i % 20) as u8) });
            }
            if i % 7 == 3 {
                handle.send(Command::Midi { node: t, msg: off(55 + ((i - 3) % 20) as u8) });
            }
            if i == 200 {
                handle.send(Command::Seek(9 * BEAT));
            }
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation dans process() avec le verrou de gamme");
    g.nodes[t as usize].scale_lock = ScaleLock::Chord;
    handle.load_graph(&g).unwrap();
    reset_violations();
    {
        let _rt = RtScope::enter();
        for _ in 0..100 {
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation après l'échange de graphe");
}
