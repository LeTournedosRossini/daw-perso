//! Notes MIDI dans le moteur : événements datés à l'échantillon, coupure des notes bloquées, validation.

mod common;
use common::*;
use std::sync::{Arc, Mutex};

use daw_engine::{Command, GraphDesc, GraphError, Insert, MidiClip, MidiEvent, MidiMsg, Note, TrackFormat};
use daw_offline_render::{render, RenderSpec};

type Log = Arc<Mutex<Vec<(u64, MidiMsg)>>>;

/// Effet de test qui note les événements reçus (instant absolu = échantillons déjà traités + décalage dans le bloc).
struct Recorder {
    log: Log,
    done: u64,
    plain_calls: Arc<Mutex<u32>>,
}

impl Insert for Recorder {
    fn latency(&self) -> u32 {
        0
    }
    fn accepts_notes(&self) -> bool {
        true
    }
    fn process(&mut self, i: &[f32], j: &[f32], o: &mut [f32], p: &mut [f32]) {
        *self.plain_calls.lock().unwrap() += 1;
        o.copy_from_slice(i);
        p.copy_from_slice(j);
        self.done += i.len() as u64;
    }
    fn process_with_notes(&mut self, i: &[f32], j: &[f32], o: &mut [f32], p: &mut [f32], notes: &[MidiEvent]) {
        let mut last = 0;
        for e in notes {
            assert!(e.time >= last && (e.time as usize) < i.len().max(1), "événements triés et dans le bloc");
            last = e.time;
            self.log.lock().unwrap().push((self.done + e.time as u64, e.msg));
        }
        o.copy_from_slice(i);
        p.copy_from_slice(j);
        self.done += i.len() as u64;
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn recorder_factory(log: &Log, plain: &Arc<Mutex<u32>>) -> daw_engine::InsertFactory {
    let (log, plain) = (Arc::clone(log), Arc::clone(plain));
    Arc::new(move |_, _, _| Ok(Box::new(Recorder { log: Arc::clone(&log), done: 0, plain_calls: Arc::clone(&plain) }) as Box<dyn Insert>))
}

fn note(start: u64, len: u64, key: u8) -> Note {
    Note { start, len, key, velocity: 0.5, channel: 2 }
}

fn setup(clips: Vec<MidiClip>) -> (GraphDesc, Log, Arc<Mutex<u32>>, daw_core_types::NodeId) {
    let log: Log = Arc::default();
    let plain = Arc::new(Mutex::new(0));
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_insert(t, "enregistreur", recorder_factory(&log, &plain)).unwrap();
    for c in clips {
        g.add_midi_clip(t, c);
    }
    (g, log, plain, t)
}

fn kinds(log: &Log) -> Vec<(u64, &'static str, u8)> {
    log.lock().unwrap().iter().map(|(t, m)| (*t, if matches!(m, MidiMsg::NoteOn { .. }) { "on" } else { "off" }, m.key())).collect()
}

#[test]
fn clip_notes_are_delivered_at_absolute_sample_times() {
    let clip = MidiClip::new(1_000, 5_000, Arc::new(vec![note(0, 100, 60), note(250, 500, 64), note(4_900, 400, 67)]));
    let (g, log, plain, _) = setup(vec![clip]);
    render(&RenderSpec::new(SR, 7_000, g)).unwrap();
    assert_eq!(
        kinds(&log),
        vec![(1_000, "on", 60), (1_100, "off", 60), (1_250, "on", 64), (1_750, "off", 64), (5_900, "on", 67), (6_000, "off", 67)],
        "la dernière note est coupée à la fin du clip (6000)"
    );
    assert_eq!(*plain.lock().unwrap(), 0, "un effet qui accepte les notes n'est jamais appelé sans elles");
}

#[test]
fn delivery_is_independent_of_block_sizes() {
    let clip = MidiClip::new(0, 3_000, Arc::new((0..40).map(|k| note(k * 71, 150, 40 + k as u8)).collect()));
    let mut reference = Vec::new();
    for (block, engine_block) in [(3_000, 3_000), (1, 1), (7, 64), (64, 16), (333, 256)] {
        let (g, log, _, _) = setup(vec![clip.clone()]);
        let mut s = RenderSpec::new(SR, 3_000, g);
        s.block = block;
        s.engine_block = engine_block;
        render(&s).unwrap();
        let k = kinds(&log);
        if reference.is_empty() {
            reference = k;
            assert_eq!(reference.len(), 80);
        } else {
            assert_eq!(k, reference, "bloc {block}/{engine_block}");
        }
    }
}

#[test]
fn offs_come_before_ons_at_the_same_instant() {
    let clip = MidiClip::new(0, 1_000, Arc::new(vec![note(0, 100, 60), note(100, 100, 60)]));
    let (g, log, _, _) = setup(vec![clip]);
    render(&RenderSpec::new(SR, 400, g)).unwrap();
    assert_eq!(kinds(&log), vec![(0, "on", 60), (100, "off", 60), (100, "on", 60), (200, "off", 60)]);
}

#[test]
fn seeking_into_a_note_gives_no_note_on_but_the_note_off_still_arrives() {
    let clip = MidiClip::new(0, 1_000, Arc::new(vec![note(100, 600, 60)]));
    let (g, log, _, _) = setup(vec![clip]);
    let mut s = RenderSpec::new(SR, 600, g);
    s.events = vec![(0, Command::Seek(400)), (0, Command::Play)];
    render(&s).unwrap();
    assert_eq!(kinds(&log), vec![(300, "off", 60)], "pas de rattrapage de note : seule la fin (700) est livrée");
}

#[test]
fn loop_wrap_cuts_sounding_notes_before_replaying() {
    let clip = MidiClip::new(0, 2_000, Arc::new(vec![note(100, 900, 60)]));
    let (g, log, _, _) = setup(vec![clip]);
    let mut s = RenderSpec::new(SR, 900, g);
    s.events = vec![(0, Command::SetLoop(Some((0, 400)))), (0, Command::Play)];
    render(&s).unwrap();
    // tour 1 (0-400) : début à 100 ; retour à 400 : coupure à 400 ; tour 2 : début à 500, coupure à 800 ; tour 3 : début à 900.
    assert_eq!(kinds(&log), vec![(100, "on", 60), (400, "off", 60), (500, "on", 60), (800, "off", 60)]);
}

#[test]
fn live_notes_go_to_the_track_at_the_command_instant() {
    let (g, log, _, t) = setup(vec![]);
    let mut s = RenderSpec::new(SR, 500, g);
    s.events.push((123, Command::Midi { node: t.0, msg: MidiMsg::NoteOn { channel: 1, key: 72, velocity: 0.9 } }));
    s.events.push((321, Command::Midi { node: t.0, msg: MidiMsg::NoteOff { channel: 1, key: 72, velocity: 0.0 } }));
    s.events.push((200, Command::Midi { node: 0, msg: MidiMsg::NoteOn { channel: 0, key: 1, velocity: 1.0 } })); // sortie principale : ignorée
    s.events.push((200, Command::Midi { node: 99, msg: MidiMsg::NoteOn { channel: 0, key: 1, velocity: 1.0 } })); // inconnu : ignoré
    render(&s).unwrap();
    assert_eq!(kinds(&log), vec![(123, "on", 72), (321, "off", 72)]);
}

#[test]
fn effects_without_note_input_never_see_notes() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_insert(t, "retard", delay_factory(0)).unwrap();
    g.add_midi_clip(t, MidiClip::new(0, 100, Arc::new(vec![note(0, 50, 60)])));
    // DelayGain n'a pas `accepts_notes` : rendu normal, silencieux.
    let out = render(&RenderSpec::new(SR, 200, g)).unwrap();
    assert!(out.left.iter().all(|&v| v == 0.0));
}

#[test]
fn invalid_notes_are_rejected() {
    for bad in [note(0, 0, 60), note(0, 10, 128), note(100, 10, 60), Note { velocity: 0.0, ..note(0, 10, 60) }, Note { channel: 16, ..note(0, 10, 60) }] {
        let (g, ..) = setup(vec![MidiClip::new(0, 100, Arc::new(vec![bad]))]);
        assert!(matches!(render(&RenderSpec::new(SR, 10, g)), Err(daw_offline_render::RenderError::Graph(GraphError::BadNote(_)))), "{bad:?}");
    }
    let mut g = GraphDesc::new();
    let bus = g.add_bus("bus");
    g.add_midi_clip(bus, MidiClip::new(0, 10, Arc::new(vec![])));
    assert!(matches!(render(&RenderSpec::new(SR, 10, g)), Err(daw_offline_render::RenderError::Graph(GraphError::ClipOnNonTrack(_)))));
}

#[test]
fn stopping_the_transport_releases_sounding_notes() {
    let clip = MidiClip::new(0, 2_000, Arc::new(vec![note(100, 1_500, 60)]));
    let (g, log, _, _) = setup(vec![clip]);
    let mut s = RenderSpec::new(SR, 600, g);
    s.events.push((300, Command::Stop));
    render(&s).unwrap();
    // Le graphe tourne à l'arrêt : la fin de note part au bloc suivant la commande, sinon la note resterait bloquée.
    assert_eq!(kinds(&log), vec![(100, "on", 60), (300, "off", 60)]);
}
