//! Contrôleurs, molette de hauteur et pressions : clips, entrée en direct, prise, nettoyage à l'arrêt.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use daw_engine::{Command, CtlEvent, GraphDesc, GraphError, Insert, InsertFactory, MidiClip, MidiEvent, MidiMsg, Note, TrackFormat};
use daw_offline_render::{render, MidiRecordJob, RenderSpec};

type Log = Arc<Mutex<Vec<(u64, MidiMsg)>>>;

/// Effet de test : consigne tout message reçu avec sa position absolue (il compte les échantillons qu'il traite).
struct Spy {
    log: Log,
    seen: u64,
}
impl Insert for Spy {
    fn latency(&self) -> u32 {
        0
    }
    fn process(&mut self, _: &[f32], _: &[f32], l: &mut [f32], r: &mut [f32]) {
        l.fill(0.0);
        r.fill(0.0);
        self.seen += l.len() as u64;
    }
    fn accepts_notes(&self) -> bool {
        true
    }
    fn process_with_notes(&mut self, i: &[f32], j: &[f32], l: &mut [f32], r: &mut [f32], notes: &[MidiEvent]) {
        let base = self.seen;
        for e in notes {
            self.log.lock().unwrap().push((base + e.time as u64, e.msg));
        }
        self.process(i, j, l, r);
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn setup(clip: Option<MidiClip>) -> (GraphDesc, u32, Log) {
    let log: Log = Default::default();
    let mut g = GraphDesc::new();
    let t = g.add_track("clavier", TrackFormat::Stereo);
    let l2 = log.clone();
    let f: InsertFactory = Arc::new(move |_, _, _| Ok(Box::new(Spy { log: l2.clone(), seen: 0 }) as Box<dyn Insert>));
    g.add_insert(t, "spy", f).unwrap();
    if let Some(c) = clip {
        g.add_midi_clip(t, c);
    }
    (g, t.0, log)
}

fn cc(controller: u8, value: u8) -> MidiMsg {
    MidiMsg::ControlChange { channel: 0, controller, value }
}
fn bend(value: i16) -> MidiMsg {
    MidiMsg::PitchBend { channel: 0, value }
}

fn clip_with_everything() -> MidiClip {
    let mut c = MidiClip::new(1_000, 4_000, Arc::new(vec![Note { start: 500, len: 1_000, key: 60, velocity: 0.8, channel: 0 }]));
    c.events = Arc::new(vec![
        CtlEvent { start: 0, msg: cc(64, 127) },
        CtlEvent { start: 500, msg: bend(4_096) }, // même instant que le début de la note : doit passer avant
        CtlEvent { start: 777, msg: MidiMsg::ChannelPressure { channel: 0, value: 90 } },
        CtlEvent { start: 778, msg: MidiMsg::PolyPressure { channel: 0, key: 60, value: 33 } },
        CtlEvent { start: 1_500, msg: cc(1, 64) }, // même instant que la fin de la note : après la fin
        CtlEvent { start: 3_999, msg: cc(64, 0) },
    ]);
    c
}

fn run(g: &GraphDesc, block: usize, engine_block: usize, frames: u64) -> daw_offline_render::Rendered {
    let mut s = RenderSpec::new(SR, frames, g.clone());
    s.block = block;
    s.engine_block = engine_block;
    render(&s).unwrap()
}

#[test]
fn clip_controllers_are_delivered_at_exact_samples_in_a_stable_order_for_any_block_size() {
    let (g, _, log) = setup(Some(clip_with_everything()));
    run(&g, 4_096, 4_096, 6_000);
    let reference = log.lock().unwrap().clone();
    let on = MidiMsg::NoteOn { channel: 0, key: 60, velocity: 0.8 };
    let off = MidiMsg::NoteOff { channel: 0, key: 60, velocity: 0.0 };
    assert_eq!(
        reference,
        vec![
            (1_000, cc(64, 127)),
            (1_500, bend(4_096)), // avant la note
            (1_500, on),
            (1_777, MidiMsg::ChannelPressure { channel: 0, value: 90 }),
            (1_778, MidiMsg::PolyPressure { channel: 0, key: 60, value: 33 }),
            (2_500, off), // la fin de la note d'abord
            (2_500, cc(1, 64)),
            (4_999, cc(64, 0)),
        ]
    );
    for (b, e) in [(1, 1), (7, 64), (100, 33), (333, 256), (1_024, 1_024)] {
        log.lock().unwrap().clear();
        run(&g, b, e, 6_000);
        assert_eq!(*log.lock().unwrap(), reference, "blocs {b}/{e}");
    }
}

#[test]
fn live_controllers_arrive_at_their_sample_and_are_recorded() {
    let (g, t, log) = setup(None);
    let mut s = RenderSpec::new(SR, 3_000, g);
    s.block = 200;
    s.midi_in = vec![(300, t, cc(7, 100)), (1_001, t, bend(-8192)), (1_002, t, bend(8191)), (1_500, t, MidiMsg::ChannelPressure { channel: 5, value: 12 })];
    s.midi_records = vec![MidiRecordJob { node: t, start: 0, stop: 2_500 }];
    let r = render(&s).unwrap();
    let want = vec![(300, cc(7, 100)), (1_001, bend(-8192)), (1_002, bend(8191)), (1_500, MidiMsg::ChannelPressure { channel: 5, value: 12 })];
    let got = log.lock().unwrap().clone();
    // + le recentrage de la molette quand le transport s'arrête (voir test suivant) : ici le rendu se termine sans arrêt.
    assert_eq!(got, want);
    let rec: Vec<(u64, MidiMsg)> = r.midi_takes[0].events.iter().map(|e| (e.pos, e.msg)).collect();
    assert_eq!(rec, want);
}

#[test]
fn stopping_releases_the_pedal_and_recenters_the_wheel() {
    let (g, t, log) = setup(None);
    let mut s = RenderSpec::new(SR, 1_000, g);
    s.midi_in = vec![(100, t, cc(64, 127)), (110, t, bend(2_000)), (120, t, MidiMsg::NoteOn { channel: 2, key: 60, velocity: 0.5 }), (130, t, MidiMsg::ControlChange { channel: 2, controller: 64, value: 100 })];
    s.events.push((500, Command::Stop));
    render(&s).unwrap();
    let got = log.lock().unwrap().clone();
    let after: Vec<MidiMsg> = got.iter().filter(|(p, _)| *p >= 500).map(|(_, m)| *m).collect();
    assert!(after.contains(&MidiMsg::NoteOff { channel: 2, key: 60, velocity: 0.0 }), "{after:?}");
    assert!(after.contains(&cc(64, 0)), "pédale du canal 0 relâchée : {after:?}");
    assert!(after.contains(&MidiMsg::ControlChange { channel: 2, controller: 64, value: 0 }), "pédale du canal 2 relâchée : {after:?}");
    assert!(after.contains(&bend(0)), "molette recentrée : {after:?}");
    assert_eq!(after.len(), 4, "rien d'autre : {after:?}");
}

#[test]
fn loop_wrap_releases_the_pedal_too() {
    let mut c = MidiClip::new(0, 1_000, Arc::new(vec![]));
    c.events = Arc::new(vec![CtlEvent { start: 100, msg: cc(64, 127) }, CtlEvent { start: 200, msg: bend(-3_000) }]);
    let (g, _, log) = setup(Some(c));
    let mut s = RenderSpec::new(SR, 1_300, g);
    s.events.push((0, Command::SetLoop(Some((0, 1_000)))));
    render(&s).unwrap();
    let got = log.lock().unwrap().clone();
    let at_wrap: Vec<(u64, MidiMsg)> = got.iter().filter(|(p, _)| *p >= 1_000 && *p < 1_100).cloned().collect();
    assert!(at_wrap.contains(&(1_000, cc(64, 0))) && at_wrap.contains(&(1_000, bend(0))), "{at_wrap:?}");
    // Et le tour suivant rejoue les événements.
    assert!(got.contains(&(1_100, cc(64, 127))) && got.contains(&(1_200, bend(-3_000))));
}

#[test]
fn invalid_controller_events_are_refused() {
    let bad = |msg: MidiMsg, start: u64| {
        let mut c = MidiClip::new(0, 1_000, Arc::new(vec![]));
        c.events = Arc::new(vec![CtlEvent { start, msg }]);
        let (g, t, _) = setup(Some(c));
        g.compile(SR, 64).err().map(|e| (e, t))
    };
    for (msg, start) in [
        (cc(200, 0), 0),
        (cc(1, 128), 0),
        (bend(8_192), 0),
        (bend(-8_193), 0),
        (MidiMsg::ChannelPressure { channel: 0, value: 200 }, 0),
        (MidiMsg::ControlChange { channel: 16, controller: 1, value: 1 }, 0),
        (MidiMsg::NoteOn { channel: 0, key: 60, velocity: 1.0 }, 0), // une note n'est pas un événement de contrôle
        (cc(1, 1), 1_000),                                           // hors du clip
    ] {
        let r = bad(msg, start);
        assert!(matches!(r, Some((GraphError::BadNote(_), _))), "{msg:?} @ {start} → {r:?}");
    }
    assert!(bad(cc(1, 127), 999).is_none());
}

// ---- changement de graphe en pleine note ------------------------------------------------------------------------------

fn note_clip(start: u64) -> MidiClip {
    MidiClip::new(start, 30_000, Arc::new(vec![Note { start: 0, len: 10_000, key: 60, velocity: 0.8, channel: 0 }]))
}

fn run_blocks(engine: &mut daw_engine::Engine, samples: usize) {
    let (mut l, mut r) = (vec![0.0f32; 100], vec![0.0f32; 100]);
    for _ in 0..samples / 100 {
        engine.process(&mut l, &mut r);
    }
}

#[test]
fn moving_a_clip_while_its_note_sounds_releases_the_note_but_unrelated_edits_do_not() {
    let (mut g, t, log) = setup(Some(note_clip(1_000)));
    let (mut engine, mut handle) = daw_engine::Engine::new(SR, 100);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    run_blocks(&mut engine, 4_000); // la note a démarré à 1 000 (fin prévue à 11 000)
    let on = MidiMsg::NoteOn { channel: 0, key: 60, velocity: 0.8 };
    let off = MidiMsg::NoteOff { channel: 0, key: 60, velocity: 0.0 };
    assert_eq!(log.lock().unwrap().iter().filter(|(_, m)| *m == on).count(), 1);
    assert!(!log.lock().unwrap().iter().any(|(_, m)| *m == off));

    // Modification étrangère à la piste (une autre piste est ajoutée) : la note continue, sa fin arrive à 11 000.
    let mut unrelated = g.clone();
    unrelated.add_track("autre", TrackFormat::Stereo);
    handle.load_graph(&unrelated).unwrap();
    run_blocks(&mut engine, 3_000); // jusqu'à 7 000
    assert!(!log.lock().unwrap().iter().any(|(_, m)| *m == off), "pas de coupure pour une modification étrangère");
    run_blocks(&mut engine, 5_000); // jusqu'à 12 000 : la fin d'origine est passée
    assert_eq!(log.lock().unwrap().iter().filter(|(p, m)| *m == off && *p == 11_000).count(), 1, "fin d'origine à 11 000");

    // Deuxième note : on déplace le clip pendant qu'elle sonne. Position actuelle : 12 000 ; on rejoue depuis 0.
    log.lock().unwrap().clear();
    handle.send(Command::Seek(0));
    run_blocks(&mut engine, 2_000); // la note démarre à 1 000 et sonne encore à 2 000
    assert!(log.lock().unwrap().iter().any(|(_, m)| *m == on));
    g.nodes[t as usize].midi_clips.clear();
    g.add_midi_clip(daw_core_types::NodeId(t), note_clip(50_000)); // le clip passe loin devant : la fin d'origine n'arrivera pas
    handle.load_graph(&g).unwrap();
    run_blocks(&mut engine, 300);
    assert!(log.lock().unwrap().iter().any(|(_, m)| *m == off), "la note qui sonnait est relâchée au changement de graphe : {:?}", log.lock().unwrap());
    // Et rien d'autre ne reste en suspens plus tard (pas de seconde fin, pas de début parasite).
    let n_off = log.lock().unwrap().iter().filter(|(_, m)| *m == off).count();
    run_blocks(&mut engine, 5_000);
    assert_eq!(log.lock().unwrap().iter().filter(|(_, m)| *m == off).count(), n_off);
}

#[test]
fn removing_the_clip_or_the_sustain_pedal_state_is_released_at_a_graph_change() {
    let mut c = note_clip(0);
    c.events = Arc::new(vec![CtlEvent { start: 100, msg: cc(64, 127) }, CtlEvent { start: 200, msg: bend(1_000) }]);
    let (mut g, t, log) = setup(Some(c));
    let (mut engine, mut handle) = daw_engine::Engine::new(SR, 100);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    run_blocks(&mut engine, 1_000); // note, pédale et molette en cours
    g.nodes[t as usize].midi_clips.clear(); // clip supprimé
    handle.load_graph(&g).unwrap();
    run_blocks(&mut engine, 300);
    let got: Vec<MidiMsg> = log.lock().unwrap().iter().filter(|(p, _)| *p >= 1_000).map(|(_, m)| *m).collect();
    assert!(got.contains(&MidiMsg::NoteOff { channel: 0, key: 60, velocity: 0.0 }), "{got:?}");
    assert!(got.contains(&cc(64, 0)), "pédale relâchée : {got:?}");
    assert!(got.contains(&bend(0)), "molette recentrée : {got:?}");
}
