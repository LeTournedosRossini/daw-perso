use std::collections::HashMap;
use std::sync::Arc;

use daw_core_types::time::{Frac, Meter, Tempo};
use daw_core_types::NodeId;
use daw_engine::{AudioData, AutoCurve, AutoTarget, Clip, GraphDesc, MidiClip, Note, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_session::project::{AudioLoader, ProjectError, CURRENT_VERSION};
use daw_session::{MClip, MLane, MNote, MPoint, Session, SessionError};

const SR: u32 = 48_000;

fn f(n: u64, d: u64) -> Frac {
    Frac::new(n, d).unwrap()
}

fn note(start: Frac, len: Frac, key: u8) -> MNote {
    MNote { start, len, key, velocity: 0.8, channel: 0, voice: 1 }
}

fn session() -> (Session, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("Synthé", TrackFormat::Stereo);
    (Session::new(g), t)
}

/// Notes du moteur du premier clip de notes de la piste : (début absolu, durée, touche).
fn engine_notes(s: &Session, t: NodeId) -> Vec<(u64, u64, u8)> {
    let c = &s.graph().nodes[t.0 as usize].midi_clips[0];
    c.notes.iter().map(|n| (c.start + n.start, n.len, n.key)).collect()
}

#[test]
fn musical_notes_become_exact_samples_and_follow_the_tempo() {
    let (mut s, t) = session();
    // 120 bpm : une ronde = 96 000 échantillons. Noires aux temps 0, 1, 2 du clip qui commence à la mesure 1.
    let mut c = MClip::new(f(1, 1), f(1, 1));
    for k in 0..3 {
        c.notes.push(note(f(k, 4), f(1, 8), 60 + k as u8));
    }
    let id = s.add_midi_clip(t, c).unwrap();
    assert_eq!(engine_notes(&s, t), vec![(96_000, 12_000, 60), (120_000, 12_000, 61), (144_000, 12_000, 62)]);
    assert_eq!(s.graph().nodes[1].midi_clips[0].len, 96_000);
    // Tempo divisé par deux : tout est deux fois plus long.
    s.set_tempo(Frac::ZERO, Tempo::bpm(60).unwrap()).unwrap();
    assert_eq!(engine_notes(&s, t), vec![(192_000, 24_000, 60), (240_000, 24_000, 61), (288_000, 24_000, 62)]);
    // Changement de tempo au milieu du clip : la 3e note (position 1 + 1/2) est après le changement à 1 + 1/4.
    s.set_tempo(f(5, 4), Tempo::bpm(120).unwrap()).unwrap();
    let n = engine_notes(&s, t);
    assert_eq!(n[0].0, 192_000);
    assert_eq!(n[1].0, 192_000 + 48_000); // 1 + 1/4 : début du nouveau tempo
    assert_eq!(n[2].0, 192_000 + 48_000 + 24_000);
    // Undo : on revient exactement.
    s.undo();
    assert_eq!(engine_notes(&s, t)[2].0, 288_000);
    s.undo();
    assert_eq!(engine_notes(&s, t)[2].0, 144_000);
    s.redo();
    s.redo();
    assert_eq!(engine_notes(&s, t)[2].0, 192_000 + 48_000 + 24_000);
    assert_eq!(s.midi_clip(id).unwrap().notes.len(), 3, "le temps musical n'a pas bougé");
}

#[test]
fn piano_roll_editing_is_undoable_and_keeps_notes_sorted() {
    let (mut s, t) = session();
    let id = s.add_midi_clip(t, MClip::new(Frac::ZERO, f(1, 1))).unwrap();
    s.add_note(id, note(f(1, 2), f(1, 8), 64)).unwrap();
    s.add_note(id, note(f(1, 4), f(1, 8), 60)).unwrap();
    let i = s.add_note(id, note(f(1, 4), f(1, 8), 55)).unwrap();
    assert_eq!(i, 0); // même début : la touche la plus grave d'abord
    let keys: Vec<u8> = s.midi_clip(id).unwrap().notes.iter().map(|n| n.key).collect();
    assert_eq!(keys, vec![55, 60, 64]);

    let before = s.midi_clip(id).unwrap().clone();
    let j = s.move_note(id, 0, f(3, 4), 72).unwrap();
    assert_eq!(j, 2);
    assert_eq!(s.midi_clip(id).unwrap().notes[2].key, 72);
    let k = s.resize_note(id, 0, f(1, 16)).unwrap();
    assert_eq!(s.midi_clip(id).unwrap().notes[k].len, f(1, 16));
    s.remove_note(id, 1).unwrap();
    assert_eq!(s.midi_clip(id).unwrap().notes.len(), 2);
    for _ in 0..3 {
        assert!(s.undo());
    }
    assert_eq!(s.midi_clip(id).unwrap(), &before);
    assert_eq!(s.undo_label(), Some("Ajouter une note"));
    // Le moteur voit la même chose que le modèle.
    assert_eq!(engine_notes(&s, t).len(), 3);
    for _ in 0..3 {
        s.redo();
    }
    assert_eq!(s.midi_clip(id).unwrap().notes.len(), 2);
}

#[test]
fn quantize_snaps_starts_on_the_absolute_grid_and_can_be_undone() {
    let (mut s, t) = session();
    // Clip décalé d'un triple-croche : la grille suit la ligne de temps, pas le clip.
    let id = s.add_midi_clip(t, MClip::new(f(1, 32), f(1, 1))).unwrap();
    s.add_note(id, note(f(1, 8), f(1, 16), 60)).unwrap(); // absolu 5/32
    s.add_note(id, note(f(1, 3), f(1, 16), 62)).unwrap(); // absolu 1/3 + 1/32
    let before = s.midi_clip(id).unwrap().clone();
    s.quantize_notes(id, f(1, 16)).unwrap();
    let c = s.midi_clip(id).unwrap();
    // 5/32 → 1/8 (2,5 doubles → moitié vers le haut = 3/16) ; vérifions les positions absolues obtenues.
    for n in &c.notes {
        let abs = c.start.checked_add(n.start).unwrap();
        assert_eq!(abs.snap_to(f(1, 16)), Some(abs), "{abs:?} n'est pas sur la grille");
        assert_eq!(n.len, f(1, 16), "les durées sont conservées");
    }
    s.undo();
    assert_eq!(s.midi_clip(id).unwrap(), &before);
}

#[test]
fn invalid_music_is_refused_without_history() {
    let (mut s, t) = session();
    let id = s.add_midi_clip(t, MClip::new(Frac::ZERO, f(1, 1))).unwrap();
    let depth = s.undo_label().map(str::to_owned);
    let mut bad_key = note(f(1, 4), f(1, 8), 200);
    assert_eq!(s.add_note(id, bad_key).err(), Some(SessionError::BadMidi));
    bad_key.key = 60;
    bad_key.velocity = 0.0;
    assert_eq!(s.add_note(id, bad_key).err(), Some(SessionError::BadMidi));
    assert_eq!(s.add_note(id, note(f(1, 1), f(1, 8), 60)).err(), Some(SessionError::BadMidi)); // début hors du clip
    assert_eq!(s.add_note(id, note(f(1, 4), Frac::ZERO, 60)).err(), Some(SessionError::BadMidi));
    assert_eq!(s.remove_note(id, 3).err(), Some(SessionError::BadNoteIndex(3)));
    assert_eq!(s.add_midi_clip(t, MClip::new(Frac::ZERO, Frac::ZERO)).err(), Some(SessionError::BadMidi));
    assert_eq!(s.add_midi_clip(NodeId(9), MClip::new(Frac::ZERO, f(1, 1))).err(), Some(SessionError::UnknownNode(NodeId(9))));
    assert_eq!(s.remove_midi_clip(daw_session::MidiClipId(99)).err(), Some(SessionError::UnknownMidiClip(daw_session::MidiClipId(99))));
    assert_eq!(s.undo_label().map(str::to_owned), depth);
    // Dépassement d'entier : refusé et sans trace.
    let huge = Frac::new(u64::MAX / 2, 1).unwrap();
    assert_eq!(s.move_midi_clip(id, huge).err(), Some(SessionError::TimeOverflow));
    assert_eq!(s.midi_clip(id).unwrap().start, Frac::ZERO);
    assert_eq!(engine_notes_len(&s, t), 0);
    assert_eq!(s.undo_label().map(str::to_owned), depth);
}

fn engine_notes_len(s: &Session, t: NodeId) -> usize {
    s.graph().nodes[t.0 as usize].midi_clips[0].notes.len()
}

#[test]
fn automation_in_musical_time_renders_and_follows_the_tempo() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![vec![1.0; 200_000], vec![1.0; 200_000]]).unwrap(), 0));
    let mut s = Session::new(g);
    let lane = MLane {
        target: AutoTarget::Gain,
        points: vec![MPoint { pos: Frac::ZERO, value: 0.0, curve: AutoCurve::Linear }, MPoint { pos: f(1, 1), value: 1.0, curve: AutoCurve::Linear }], mode: daw_engine::AutoMode::Read,
    };
    s.set_lane(t, lane.clone()).unwrap();
    let pts = |s: &Session| s.graph().nodes[1].automation[0].points.iter().map(|p| p.time).collect::<Vec<_>>();
    assert_eq!(pts(&s), vec![0, 96_000]);
    let out = render(&RenderSpec::new(SR, 100_000, s.graph().clone())).unwrap().left;
    assert_eq!(out[48_000], 0.5);
    s.set_tempo(Frac::ZERO, Tempo::bpm(240).unwrap()).unwrap(); // deux fois plus vite
    assert_eq!(pts(&s), vec![0, 48_000]);
    let out = render(&RenderSpec::new(SR, 100_000, s.graph().clone())).unwrap().left;
    assert_eq!(out[24_000], 0.5);
    assert_eq!(out[60_000], 1.0);
    // Remplacer puis supprimer la courbe, avec annulation à la même place.
    s.remove_lane(t, AutoTarget::Gain).unwrap();
    assert!(s.graph().nodes[1].automation.is_empty());
    s.undo();
    assert_eq!(s.lane(t, AutoTarget::Gain), Some(&lane));
    assert_eq!(s.set_lane(t, MLane { target: AutoTarget::Pan, points: vec![], mode: daw_engine::AutoMode::Read }).err(), Some(SessionError::BadLane));
    assert_eq!(
        s.set_lane(t, MLane { target: AutoTarget::Pan, points: vec![MPoint { pos: f(1, 2), value: 0.0, curve: AutoCurve::Linear }, MPoint { pos: f(1, 4), value: 1.0, curve: AutoCurve::Step }], mode: daw_engine::AutoMode::Read }).err(),
        Some(SessionError::BadLane)
    );
}

#[test]
fn engine_clips_already_in_the_graph_are_converted_exactly() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let notes = vec![Note { start: 10, len: 2_000, key: 60, velocity: 0.5, channel: 1 }, Note { start: 7_777, len: 333, key: 61, velocity: 1.0, channel: 0 }];
    g.add_midi_clip(t, MidiClip::new(1_234, 50_000, Arc::new(notes)));
    let before = g.nodes[1].midi_clips[0].clone();
    let s = Session::new(g);
    let after = &s.graph().nodes[1].midi_clips[0];
    assert_eq!((after.start, after.len), (before.start, before.len));
    assert_eq!(after.notes, before.notes);
    assert_eq!(s.midi_clip_ids(t).len(), 1);
}

struct Mem(HashMap<String, Arc<AudioData>>);
impl AudioLoader for Mem {
    fn load(&self, path: &str) -> Result<Arc<AudioData>, String> {
        self.0.get(path).cloned().ok_or_else(|| format!("introuvable : {path}"))
    }
}

#[test]
fn version_2_project_round_trips_with_tempo_notes_and_automation() {
    let (mut s, t) = session();
    s.set_tempo(f(1, 1), Tempo::new(400, 3).unwrap()).unwrap();
    s.set_meter(2, Meter::new(7, 8).unwrap()).unwrap();
    let id = s.add_midi_clip(t, MClip::new(f(1, 2), f(3, 2))).unwrap();
    s.add_note(id, note(f(1, 12), f(1, 12), 64)).unwrap(); // croche de triolet : exact
    s.add_note(id, note(f(1, 3), f(1, 6), 67)).unwrap();
    s.set_lane(t, MLane { target: AutoTarget::Pan, points: vec![MPoint { pos: Frac::ZERO, value: -1.0, curve: AutoCurve::Step }, MPoint { pos: f(7, 4), value: 0.25, curve: AutoCurve::Linear }], mode: daw_engine::AutoMode::Read }).unwrap();
    let (json, warnings) = s.to_json(SR).unwrap();
    assert!(warnings.is_empty());
    assert!(json.contains("\"version\": 20") && CURRENT_VERSION == 20);
    let (s2, rate) = Session::from_json(&json, &Mem(HashMap::new())).unwrap();
    assert_eq!(rate, SR);
    assert_eq!(s2.tempo_map(), s.tempo_map());
    assert_eq!(s2.meter_map(), s.meter_map());
    let id2 = s2.midi_clip_ids(t)[0];
    assert_eq!(s2.midi_clip(id2), s.midi_clip(id));
    assert_eq!(s2.lane(t, AutoTarget::Pan), s.lane(t, AutoTarget::Pan));
    assert_eq!(engine_notes(&s2, t), engine_notes(&s, t));
    assert_eq!(s2.to_json(SR).unwrap().0, json);
    assert!(!s2.can_undo());
}

const V1_PROJECT: &str = r#"{
  "version": 1,
  "sample_rate": 48000,
  "pdc": true,
  "assets": [],
  "nodes": [
    {"kind": "master", "name": "Master", "gain": 1.0, "pan": 0.0, "mute": false, "solo": false, "output": null, "sends": [], "clips": [], "midi_clips": []},
    {"kind": "stereo", "name": "Synthé", "gain": 0.5, "pan": 0.0, "mute": false, "solo": false, "output": 0, "sends": [], "clips": [],
     "midi_clips": [{"start": 96000, "len": 48000, "notes": [
        {"start": 0, "len": 12000, "key": 60, "velocity": 0.5, "channel": 0},
        {"start": 24000, "len": 8000, "key": 64, "velocity": 1.0, "channel": 3}]}]}
  ]
}"#;

#[test]
fn version_1_project_is_migrated_to_musical_time() {
    let (s, rate) = Session::from_json(V1_PROJECT, &Mem(HashMap::new())).unwrap();
    assert_eq!(rate, 48_000);
    let t = NodeId(1);
    let id = s.midi_clip_ids(t)[0];
    let c = s.midi_clip(id).unwrap();
    // 120 bpm à 48 kHz : une ronde = 96 000 échantillons.
    assert_eq!((c.start, c.len), (f(1, 1), f(1, 2)));
    assert_eq!((c.notes[0].start, c.notes[0].len, c.notes[1].start, c.notes[1].len), (Frac::ZERO, f(1, 8), f(1, 4), f(1, 12)));
    // Même son qu'avant en échantillons.
    let m = &s.graph().nodes[1].midi_clips[0];
    assert_eq!((m.start, m.len), (96_000, 48_000));
    assert_eq!((m.notes[0].len, m.notes[1].start, m.notes[1].len), (12_000, 24_000, 8_000));
    assert_eq!(s.tempo_map().changes().len(), 1);
    // Une réécriture sort dans la dernière version.
    assert!(s.to_json(rate).unwrap().0.contains("\"version\": 20"));
    // Un v1 abîmé échoue proprement à la migration.
    let broken = V1_PROJECT.replace("\"midi_clips\": [{\"start\"", "\"midi_clips\": [{\"startX\"");
    assert!(matches!(Session::from_json(&broken, &Mem(HashMap::new())).err(), Some(ProjectError::Migration { from: 1, .. }) | Some(ProjectError::Parse(_))));
}

// ---- enregistrement MIDI ----------------------------------------------------------------------------------------

use daw_engine::{MidiMsg, RecordedMidi};
use daw_session::MidiTakeData;

fn ev(pos: u64, msg: MidiMsg) -> RecordedMidi {
    RecordedMidi { pos, msg }
}
fn on(key: u8, vel: f32) -> MidiMsg {
    MidiMsg::NoteOn { channel: 0, key, velocity: vel }
}
fn off(key: u8) -> MidiMsg {
    MidiMsg::NoteOff { channel: 0, key, velocity: 0.0 }
}

#[test]
fn recorded_take_becomes_a_musical_clip_on_the_tick_grid() {
    let (mut s, t) = session();
    // 120 bpm / 48 kHz : une ronde = 96 000 échantillons = 3 840 ticks, donc 1 tick = 25 échantillons.
    let take = MidiTakeData {
        start_pos: 96_000,
        stop_pos: 192_000,
        events: vec![
            ev(120_000, on(60, 0.5)),  // 1/4 après le début
            ev(132_000, off(60)),      // durée 1/8
            ev(150_000, on(62, 1.0)),  // jamais relâchée : finit à la fin de la prise
            ev(160_000, on(62, 0.25)), // même touche rejouée : termine la précédente
            ev(170_000, off(64)),      // fin sans début : ignorée
        ],
    };
    let id = s.add_recorded_take(t, &take, 0).unwrap().unwrap();
    let c = s.midi_clip(id).unwrap();
    assert_eq!((c.start, c.len), (f(1, 1), f(1, 1)));
    let got: Vec<_> = c.notes.iter().map(|n| (n.start, n.len, n.key, n.velocity)).collect();
    assert_eq!(got, vec![(f(1, 4), f(1, 8), 60, 0.5), (f(9, 16), f(5, 48), 62, 1.0), (f(2, 3), f(1, 3), 62, 0.25)]);
    // Le moteur rejoue exactement les échantillons joués (ils tombent sur la grille de 25).
    assert_eq!(engine_notes(&s, t), vec![(120_000, 12_000, 60), (150_000, 10_000, 62), (160_000, 32_000, 62)]);
    // Une seule action annulable.
    assert_eq!(s.undo_label(), Some("Ajouter un clip de notes"));
    assert!(s.undo());
    assert!(s.midi_clip_ids(t).is_empty());
}

#[test]
fn recording_rounds_to_the_nearest_tick_and_compensates_the_input_latency() {
    let (mut s, t) = session();
    // 1 tick = 25 échantillons. 24 012 / 25 = 960,48 → 960 ; 36 013 / 25 = 1 440,52 → 1 441.
    let take = MidiTakeData { start_pos: 0, stop_pos: 96_000, events: vec![ev(24_012, on(60, 1.0)), ev(36_013, off(60))] };
    let id = s.add_recorded_take(t, &take, 0).unwrap().unwrap();
    let n = s.midi_clip(id).unwrap().notes[0];
    assert_eq!((n.start, n.len), (f(960, 3_840), f(481, 3_840)));
    // Latence d'entrée de 25 échantillons = 1 tick : tout arrive 1 tick plus tôt.
    let id = s.add_recorded_take(t, &take, 25).unwrap().unwrap();
    let n = s.midi_clip(id).unwrap().notes[0];
    assert_eq!((n.start, n.len), (f(959, 3_840), f(481, 3_840)));
}

#[test]
fn empty_or_noteless_takes_create_nothing() {
    let (mut s, t) = session();
    let take = MidiTakeData { start_pos: 0, stop_pos: 1_000, events: vec![] };
    assert_eq!(s.add_recorded_take(t, &take, 0).unwrap(), None);
    let take = MidiTakeData { start_pos: 0, stop_pos: 1_000, events: vec![ev(10, off(60))] };
    assert_eq!(s.add_recorded_take(t, &take, 0).unwrap(), None);
    assert!(!s.can_undo());
    let take = MidiTakeData { start_pos: 0, stop_pos: 1_000, events: vec![ev(10, on(60, 1.0))] };
    assert!(s.add_recorded_take(NodeId(9), &take, 0).is_err());
}

// ---- contrôleurs, molette, pressions ---------------------------------------------------------------------------

use daw_session::{MEvent, MKind};

fn cc(controller: u8, value: u8) -> MidiMsg {
    MidiMsg::ControlChange { channel: 0, controller, value }
}

#[test]
fn recorded_take_keeps_controllers_wheel_and_pressure() {
    let (mut s, t) = session();
    // 1 tick = 25 échantillons. Pédale enfoncée avant la note, molette pendant, pédale relâchée après la fin de la note.
    let take = MidiTakeData {
        start_pos: 0,
        stop_pos: 96_000,
        events: vec![
            ev(10_000, cc(64, 127)),
            ev(12_000, on(60, 0.8)),
            ev(13_000, MidiMsg::PitchBend { channel: 0, value: -3_000 }),
            ev(14_000, MidiMsg::PolyPressure { channel: 0, key: 60, value: 55 }),
            ev(15_000, off(60)),
            ev(20_000, MidiMsg::ChannelPressure { channel: 0, value: 9 }),
            ev(30_000, cc(64, 0)),
        ],
    };
    let id = s.add_recorded_take(t, &take, 0).unwrap().unwrap();
    let c = s.midi_clip(id).unwrap();
    assert_eq!(c.notes.len(), 1);
    let got: Vec<(Frac, MKind)> = c.events.iter().map(|e| (e.start, e.kind)).collect();
    // ticks : 10 000/25 = 400, 13 000/25 = 520, 14 000/25 = 560, 20 000/25 = 800, 30 000/25 = 1 200 (sur 3 840 par ronde)
    assert_eq!(
        got,
        vec![
            (f(400, 3_840), MKind::ControlChange { controller: 64, value: 127 }),
            (f(520, 3_840), MKind::PitchBend { value: -3_000 }),
            (f(560, 3_840), MKind::PolyPressure { key: 60, value: 55 }),
            (f(800, 3_840), MKind::ChannelPressure { value: 9 }),
            (f(1_200, 3_840), MKind::ControlChange { controller: 64, value: 0 }),
        ]
    );
    // Le moteur reçoit les mêmes messages aux mêmes échantillons.
    let ce = &s.graph().nodes[t.0 as usize].midi_clips[0].events;
    let at: Vec<(u64, MidiMsg)> = ce.iter().map(|e| (e.start, e.msg)).collect();
    assert_eq!(at[0], (10_000, cc(64, 127)));
    assert_eq!(at[1], (13_000, MidiMsg::PitchBend { channel: 0, value: -3_000 }));
    assert_eq!(at[4], (30_000, cc(64, 0)));
    // Une prise sans note mais avec un contrôleur crée quand même un clip.
    let only_ctl = MidiTakeData { start_pos: 0, stop_pos: 48_000, events: vec![ev(1_000, cc(1, 90))] };
    let id2 = s.add_recorded_take(t, &only_ctl, 0).unwrap().unwrap();
    assert!(s.midi_clip(id2).unwrap().notes.is_empty() && s.midi_clip(id2).unwrap().events.len() == 1);
}

#[test]
fn events_can_be_added_and_removed_with_undo_and_follow_the_tempo() {
    let (mut s, t) = session();
    let id = s.add_midi_clip(t, MClip::new(Frac::ZERO, f(1, 1))).unwrap();
    let e1 = MEvent { start: f(1, 2), channel: 0, kind: MKind::PitchBend { value: 100 } };
    let e0 = MEvent { start: f(1, 4), channel: 1, kind: MKind::ControlChange { controller: 1, value: 64 } };
    assert_eq!(s.add_event(id, e1).unwrap(), 0);
    assert_eq!(s.add_event(id, e0).unwrap(), 0); // trié par instant
    assert_eq!(s.midi_clip(id).unwrap().events, vec![e0, e1]);
    let ev_samples = |s: &Session| s.graph().nodes[t.0 as usize].midi_clips[0].events.iter().map(|e| e.start).collect::<Vec<_>>();
    assert_eq!(ev_samples(&s), vec![24_000, 48_000]);
    s.set_tempo(Frac::ZERO, Tempo::bpm(60).unwrap()).unwrap();
    assert_eq!(ev_samples(&s), vec![48_000, 96_000]);
    s.undo();
    s.remove_event(id, 0).unwrap();
    assert_eq!(s.midi_clip(id).unwrap().events, vec![e1]);
    s.undo();
    assert_eq!(s.midi_clip(id).unwrap().events, vec![e0, e1]);
    assert_eq!(s.remove_event(id, 5).err(), Some(SessionError::BadNoteIndex(5)));
    let bad = MEvent { start: f(1, 2), channel: 0, kind: MKind::ControlChange { controller: 200, value: 1 } };
    assert_eq!(s.add_event(id, bad).err(), Some(SessionError::BadMidi));
    let outside = MEvent { start: f(1, 1), channel: 0, kind: MKind::ChannelPressure { value: 1 } };
    assert_eq!(s.add_event(id, outside).err(), Some(SessionError::BadMidi));
}

#[test]
fn version_3_project_round_trips_events() {
    let (mut s, t) = session();
    let id = s.add_midi_clip(t, MClip::new(f(1, 2), f(1, 1))).unwrap();
    for (start, channel, kind) in [
        (f(0, 1), 0, MKind::ControlChange { controller: 64, value: 127 }),
        (f(1, 12), 3, MKind::PitchBend { value: -8_192 }),
        (f(1, 8), 3, MKind::PitchBend { value: 8_191 }),
        (f(1, 6), 15, MKind::ChannelPressure { value: 127 }),
        (f(1, 3), 0, MKind::PolyPressure { key: 61, value: 5 }),
    ] {
        s.add_event(id, MEvent { start, channel, kind }).unwrap();
    }
    let (json, _) = s.to_json(SR).unwrap();
    assert!(json.contains("\"type\": \"pitch_bend\"") && json.contains("\"controller\": 64"));
    let (s2, _) = Session::from_json(&json, &Mem(HashMap::new())).unwrap();
    assert_eq!(s2.midi_clip(s2.midi_clip_ids(t)[0]), s.midi_clip(id));
    assert_eq!(s2.to_json(SR).unwrap().0, json);
    // Événement abîmé : refusé proprement.
    let broken = json.replace("\"pitch_bend\"", "\"zigzag\"");
    assert!(matches!(Session::from_json(&broken, &Mem(HashMap::new())).err(), Some(ProjectError::Parse(_))));
}

const V2_PROJECT: &str = r#"{
  "version": 2,
  "sample_rate": 48000,
  "pdc": true,
  "tempo_map": [{"pos": [0, 1], "num": 90, "den": 1}],
  "meters": [{"bar": 0, "num": 3, "den": 4}],
  "assets": [],
  "nodes": [
    {"kind": "master", "name": "Master", "gain": 1.0, "pan": 0.0, "mute": false, "solo": false, "output": null, "sends": [], "clips": [], "midi_clips": [], "automation": []},
    {"kind": "stereo", "name": "Synthé", "gain": 0.5, "pan": 0.0, "mute": false, "solo": false, "output": 0, "sends": [], "clips": [], "automation": [],
     "midi_clips": [{"start": [1, 4], "len": [1, 2], "notes": [{"start": [0, 1], "len": [1, 8], "key": 60, "velocity": 0.5, "channel": 0}]}]}
  ]
}"#;

#[test]
fn version_2_project_is_migrated_to_version_3_and_keeps_its_tempo() {
    let (s, rate) = Session::from_json(V2_PROJECT, &Mem(HashMap::new())).unwrap();
    assert_eq!(rate, 48_000);
    let t = NodeId(1);
    let c = s.midi_clip(s.midi_clip_ids(t)[0]).unwrap();
    assert_eq!((c.start, c.len, c.notes.len(), c.events.len()), (f(1, 4), f(1, 2), 1, 0));
    assert_eq!(s.tempo_map().changes()[0].1, Tempo::bpm(90).unwrap());
    assert_eq!(s.meter_map().changes()[0].1, Meter::new(3, 4).unwrap());
    assert!(s.to_json(rate).unwrap().0.contains("\"version\": 20"));
}

// ---- clips entre pistes, fenêtre visible, découpe et fondus de vélocité des clips de notes ----------------------------

fn two_tracks() -> (Session, NodeId, NodeId) {
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    let b = g.add_track("B", TrackFormat::Mono);
    g.add_bus("Bus");
    (Session::new(g), a, b)
}

fn notes_at(s: &Session, node: NodeId) -> Vec<(u64, u8, f32)> {
    let c = &s.graph().nodes[node.0 as usize].midi_clips[0];
    c.notes.iter().map(|n| (c.start + n.start, n.key, n.velocity)).collect()
}

#[test]
fn audio_clip_moves_to_another_track_in_one_undo_step_keeping_its_id() {
    let (mut s, a, b) = two_tracks();
    let data = AudioData::new(SR, vec![vec![0.5; 1_000]]).unwrap();
    let id = s.add_clip(a, Clip::whole(data, 100)).unwrap();
    s.move_clip_to(id, b, 5_000).unwrap();
    assert!(s.clip_ids(a).is_empty());
    assert_eq!(s.clip_ids(b), &[id]);
    assert_eq!(s.clip(id).unwrap().start, 5_000);
    assert_eq!(s.undo_label(), Some("Déplacer un clip"));
    assert!(s.undo());
    assert_eq!((s.clip_ids(a), s.clip(id).unwrap().start), (&[id][..], 100));
    assert!(s.clip_ids(b).is_empty());
    assert!(s.redo());
    assert_eq!(s.clip_ids(b), &[id]);
    // Même piste : simple déplacement.
    s.move_clip_to(id, b, 7).unwrap();
    assert_eq!(s.clip(id).unwrap().start, 7);
    // Destinations invalides : sortie principale, bus, nœud inconnu — sans trace.
    let depth = s.undo_label().map(str::to_owned);
    assert_eq!(s.move_clip_to(id, NodeId::MASTER, 0).err(), Some(SessionError::NotATrack(NodeId::MASTER)));
    assert_eq!(s.move_clip_to(id, NodeId(3), 0).err(), Some(SessionError::NotATrack(NodeId(3))));
    assert_eq!(s.move_clip_to(id, NodeId(99), 0).err(), Some(SessionError::UnknownNode(NodeId(99))));
    assert_eq!(s.undo_label().map(str::to_owned), depth);
    assert_eq!(s.clip_ids(b), &[id]);
}

#[test]
fn notes_clip_moves_between_tracks_and_back() {
    let (mut s, a, b) = two_tracks();
    let id = s.add_midi_clip(a, MClip::new(f(1, 4), f(1, 1))).unwrap();
    s.add_note(id, note(f(1, 8), f(1, 8), 60)).unwrap();
    s.move_midi_clip_to(id, b, f(1, 2)).unwrap();
    assert!(s.midi_clip_ids(a).is_empty());
    assert_eq!(s.midi_clip_ids(b), vec![id]);
    assert_eq!(notes_at(&s, b), vec![(48_000 + 12_000, 60, 0.8)]);
    assert!(s.graph().nodes[a.0 as usize].midi_clips.is_empty());
    assert!(s.undo());
    assert_eq!(s.midi_clip_ids(a), vec![id]);
    assert_eq!(notes_at(&s, a), vec![(24_000 + 12_000, 60, 0.8)]);
}

#[test]
fn trimming_and_splitting_a_notes_clip_hide_notes_without_destroying_them() {
    let (mut s, a, _) = two_tracks();
    // Quatre notes aux positions 0, 1/4, 1/2, 3/4 (1/8 de long), clip de 0 à 1 ronde.
    let id = s.add_midi_clip(a, MClip::new(Frac::ZERO, f(1, 1))).unwrap();
    for (k, key) in [60u8, 62, 64, 65].iter().enumerate() {
        s.add_note(id, note(f(k as u64, 4), f(1, 8), *key)).unwrap();
    }
    assert_eq!(notes_at(&s, a).iter().map(|n| n.1).collect::<Vec<_>>(), [60, 62, 64, 65]);
    // Rogner la fin à 1/2 : les deux dernières disparaissent du moteur mais restent dans le clip.
    s.set_midi_window(id, Frac::ZERO, f(1, 2), Frac::ZERO).unwrap();
    assert_eq!(notes_at(&s, a).iter().map(|n| n.1).collect::<Vec<_>>(), [60, 62]);
    assert_eq!(s.midi_clip(id).unwrap().notes.len(), 4);
    // Rogner le début à 1/4 (la fenêtre commence à la matière 1/4) : la note de 1/4 démarre au début du clip.
    s.set_midi_window(id, f(1, 4), f(1, 2), f(1, 4)).unwrap();
    assert_eq!(notes_at(&s, a), vec![(24_000, 62, 0.8), (24_000 + 24_000, 64, 0.8)]);
    // Étendre de nouveau : tout revient.
    s.set_midi_window(id, Frac::ZERO, f(1, 1), Frac::ZERO).unwrap();
    assert_eq!(notes_at(&s, a).len(), 4);
    // Ajouter une note hors de la partie visible est refusé.
    s.set_midi_window(id, f(1, 4), f(1, 2), f(1, 4)).unwrap();
    assert_eq!(s.add_note(id, note(f(0, 1), f(1, 8), 70)).err(), Some(SessionError::BadMidi), "avant la fenêtre");
    assert_eq!(s.add_note(id, note(f(3, 4), f(1, 8), 70)).err(), Some(SessionError::BadMidi), "après la fenêtre");
    assert!(s.add_note(id, note(f(1, 2), f(1, 16), 70)).is_ok());
    s.undo();
    // Découpe à 1/2 de ronde sur la ligne de temps (fenêtre 1/4..3/4) : deux clips non destructifs.
    let before = notes_at(&s, a);
    let (l, r) = s.split_midi_clip(id, f(1, 2)).unwrap();
    assert_eq!(l, id);
    assert_ne!(r, id);
    assert_eq!(s.undo_label(), Some("Découper"));
    let left = s.midi_clip(l).unwrap().clone();
    let right = s.midi_clip(r).unwrap().clone();
    assert_eq!((left.start, left.len, left.offset), (f(1, 4), f(1, 4), f(1, 4)));
    assert_eq!((right.start, right.len, right.offset), (f(1, 2), f(1, 4), f(1, 2)));
    let all: Vec<_> = s.graph().nodes[a.0 as usize].midi_clips.iter().flat_map(|c| c.notes.iter().map(move |n| (c.start + n.start, n.key))).collect();
    assert_eq!(all, vec![(24_000, 62), (48_000, 64)], "chaque note n'est jouée qu'une fois : 62 à gauche (1/4), 64 à droite (1/2)");
    assert_eq!(before.len(), 2);
    // Une découpe hors du clip est refusée sans trace.
    assert_eq!(s.split_midi_clip(l, f(9, 1)).err(), Some(SessionError::Edit(daw_engine::EditError::OutOfRange)));
    assert_eq!(s.split_midi_clip(l, f(1, 4)).err(), Some(SessionError::Edit(daw_engine::EditError::OutOfRange)));
    // Tout annuler d'un coup de découpe.
    assert!(s.undo());
    assert_eq!(s.midi_clip_ids(a), vec![id]);
}

#[test]
fn velocity_fades_scale_the_notes_that_start_inside_them() {
    let (mut s, a, _) = two_tracks();
    let id = s.add_midi_clip(a, MClip::new(Frac::ZERO, f(1, 1))).unwrap();
    for k in 0..8u64 {
        s.add_note(id, MNote { start: f(k, 8), len: f(1, 16), key: 60, velocity: 1.0, channel: 0, voice: 1 }).unwrap();
    }
    s.set_midi_fades(id, Some(f(1, 2)), Some(f(1, 2))).unwrap();
    let v: Vec<f32> = notes_at(&s, a).iter().map(|n| n.2).collect();
    // Fondu d'entrée sur [0, 1/2[ : 0, 1/4, 1/2, 3/4 ; au milieu pleine vélocité ; fondu de sortie sur [1/2, 1[ (reste 1/2 → 0).
    // k = 0 : 0 → plancher 1/127 ; k = 1 : 0,25 ; k = 2 : 0,5 ; k = 3 : 0,75 ; k = 4 : 1 ; fondu de sortie pour k = 5, 6, 7.
    assert!((v[0] - 1.0 / 127.0).abs() < 1e-6, "{v:?}");
    assert!((v[1] - 0.25).abs() < 1e-6 && (v[2] - 0.5).abs() < 1e-6 && (v[3] - 0.75).abs() < 1e-6, "{v:?}");
    assert!((v[4] - 1.0).abs() < 1e-6, "{v:?}");
    assert!((v[5] - 0.75).abs() < 1e-6 && (v[6] - 0.5).abs() < 1e-6 && (v[7] - 0.25).abs() < 1e-6, "{v:?}");
    // Les notes du modèle ne sont pas touchées : seul ce que reçoit le moteur est atténué.
    assert!(s.midi_clip(id).unwrap().notes.iter().all(|n| n.velocity == 1.0));
    // Fondu plus long que le clip : borné. Annulation exacte.
    s.set_midi_fades(id, Some(f(9, 1)), None).unwrap();
    assert_eq!(s.midi_clip(id).unwrap().fade_in, f(1, 1));
    s.undo();
    s.undo();
    assert!(notes_at(&s, a).iter().all(|n| n.2 == 1.0));
}

#[test]
fn version_4_project_keeps_the_window_and_velocity_fades_and_v3_migrates() {
    let (mut s, a, _) = two_tracks();
    let id = s.add_midi_clip(a, MClip::new(f(1, 2), f(1, 1))).unwrap();
    s.add_note(id, note(f(1, 8), f(1, 8), 60)).unwrap();
    s.add_note(id, note(f(1, 4), f(1, 8), 62)).unwrap();
    s.set_midi_window(id, f(1, 2), f(3, 4), f(1, 8)).unwrap();
    s.set_midi_fades(id, Some(f(1, 8)), Some(f(1, 16))).unwrap();
    let (json, _) = s.to_json(SR).unwrap();
    assert!(json.contains("\"offset\"") && json.contains("\"fade_in\""));
    let (s2, _) = Session::from_json(&json, &Mem(HashMap::new())).unwrap();
    assert_eq!(s2.midi_clip(s2.midi_clip_ids(a)[0]), s.midi_clip(id));
    assert_eq!(notes_at(&s2, a), notes_at(&s, a));
    assert_eq!(s2.to_json(SR).unwrap().0, json);
    // Un fichier v3 (sans fenêtre ni fondus) s'ouvre avec des valeurs nulles.
    let v3 = V2_PROJECT.replace("\"version\": 2", "\"version\": 3").replace("\"notes\": [{\"start\": [0, 1]", "\"events\": [], \"notes\": [{\"start\": [0, 1]");
    let (s3, _) = Session::from_json(&v3, &Mem(HashMap::new())).unwrap();
    let c = s3.midi_clip(s3.midi_clip_ids(NodeId(1))[0]).unwrap();
    assert_eq!((c.offset, c.fade_in, c.fade_out, c.notes.len()), (Frac::ZERO, Frac::ZERO, Frac::ZERO, 1));
}

#[test]
fn version_5_fade_shapes_become_curve_settings_in_version_6() {
    let (mut s, a, _) = two_tracks();
    let data = AudioData::new(SR, vec![vec![0.5; 4_000]]).unwrap();
    let id = s.add_clip(a, Clip::whole(data.clone(), 0)).unwrap();
    s.register_asset("demo/a.wav", data);
    let mut c = s.clip(id).unwrap().clone();
    c.fade_in = daw_engine::Fade::curved(500, daw_engine::FadeShape::EqualPower, -250, 400);
    c.fade_out = daw_engine::Fade::curved(900, daw_engine::FadeShape::Linear, 1000, 0);
    s.set_clip(id, c).unwrap();
    let (json, _) = s.to_json(SR).unwrap();
    assert!(json.contains("\"bend\": -250") && json.contains("\"smooth\": 400") && json.contains("\"version\": 20"));
    let loader = |_: &str| -> Result<Arc<AudioData>, String> { Ok(AudioData::new(SR, vec![vec![0.5; 4_000]]).unwrap()) };
    struct One<F>(F);
    impl<F: Fn(&str) -> Result<Arc<AudioData>, String>> AudioLoader for One<F> {
        fn load(&self, p: &str) -> Result<Arc<AudioData>, String> {
            (self.0)(p)
        }
    }
    let (s2, _) = Session::from_json(&json, &One(loader)).unwrap();
    let c2 = s2.clip(s2.clip_ids(a)[0]).unwrap();
    assert_eq!((c2.fade_in, c2.fade_out), (daw_engine::Fade::curved(500, daw_engine::FadeShape::EqualPower, -250, 400), daw_engine::Fade::curved(900, daw_engine::FadeShape::Linear, 1000, 0)));
    assert_eq!(s2.to_json(SR).unwrap().0, json);
    // Un fichier v5 avec les anciennes formes : « s_curve » = adoucissement 1000, « slow » = courbure +1000, « fast » = −1000.
    let make_v5 = |fade_in: &str, fade_out: &str| {
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["version"] = serde_json::json!(5);
        for node in v["nodes"].as_array_mut().unwrap() {
            for clip in node["clips"].as_array_mut().unwrap() {
                for (key, shape) in [("fade_in", fade_in), ("fade_out", fade_out)] {
                    let f = clip[key].as_object_mut().unwrap();
                    f.remove("bend");
                    f.remove("smooth");
                    f.insert("shape".into(), serde_json::json!(shape));
                }
            }
        }
        v.to_string()
    };
    let v5 = make_v5("s_curve", "slow");
    assert!(v5.contains("s_curve") && v5.contains("slow") && !v5.contains("\"bend\""));
    let (s3, _) = Session::from_json(&v5, &One(loader)).unwrap();
    let c3 = s3.clip(s3.clip_ids(a)[0]).unwrap();
    assert_eq!((c3.fade_in.shape, c3.fade_in.bend, c3.fade_in.smooth, c3.fade_in.len), (daw_engine::FadeShape::Linear, 0, 1000, 500));
    assert_eq!((c3.fade_out.shape, c3.fade_out.bend, c3.fade_out.smooth, c3.fade_out.len), (daw_engine::FadeShape::Linear, 1000, 0, 900));
    let (s4, _) = Session::from_json(&make_v5("fast", "equal_power"), &One(loader)).unwrap();
    let c4 = s4.clip(s4.clip_ids(a)[0]).unwrap();
    assert_eq!((c4.fade_in.bend, c4.fade_in.smooth, c4.fade_out.shape, c4.fade_out.bend), (-1000, 0, daw_engine::FadeShape::EqualPower, 0));
}

#[test]
fn the_engine_time_map_follows_tempo_and_meter_changes() {
    let mut g = GraphDesc::new();
    g.add_track("t", TrackFormat::Stereo);
    let mut s = Session::new(g);
    let t = &s.graph().time_map.tempo;
    assert_eq!((t.len(), t[0].bpm), (1, 120.0));
    s.set_tempo(Frac::new(1, 1).unwrap(), Tempo::bpm(60).unwrap()).unwrap(); // à la mesure 2 (une ronde = 4 noires)
    s.set_meter(3, Meter::new(3, 4).unwrap()).unwrap();
    let m = s.graph().time_map.clone();
    assert_eq!(m.tempo.len(), 2);
    assert_eq!((m.tempo[1].start_beat, m.tempo[1].bpm, m.tempo[1].start_sample), (4.0, 60.0, 96_000), "4 noires à 120 bpm = 2 s");
    assert_eq!(m.meters.len(), 2);
    assert_eq!((m.meters[1].start_bar, m.meters[1].num, m.meters[1].den, m.meters[1].start_beat), (4, 3, 4, 12.0), "mesure d'indice 3 = la 4ᵉ (numérotée à partir de 1 pour les plugins), 12 noires plus loin");
    s.undo();
    s.undo();
    assert_eq!(s.graph().time_map.tempo.len(), 1, "l'annulation remet la carte");
}
