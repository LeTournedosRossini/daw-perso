//! Prises avec fenêtre (punch-in / punch-out) dans le vrai moteur : début et fin exacts à l'échantillon près quelle que soit la
//! taille des appels, la prise se déclare terminée toute seule à sa fin, le transport continue.

use daw_engine::{Command, Engine, GraphDesc, MidiMsg, RecordSource, TrackFormat};

const SR: u32 = 48_000;

/// L'entrée vaut l'indice absolu de l'échantillon (de la lecture) : on reconnaît chaque échantillon de la prise.
fn run(call: usize, start: u64, end: u64) -> (Vec<f32>, u64, u64, bool) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo).0;
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::SetInput { node: t, armed: true, monitor: false, source: RecordSource::Left });
    let mut rx = handle.start_record_window(t, RecordSource::Left, 2.0, Some(start), Some(end)).unwrap();
    handle.send(Command::Seek(400));
    engine.process(&mut [0.0; 4], &mut [0.0; 4]);
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; call], vec![0.0f32; call]);
    let mut pos = 400u64;
    for _ in 0..(3_000 / call + 2) {
        let input: Vec<f32> = (0..call).map(|k| (pos + k as u64) as f32).collect();
        engine.process_inputs(&[&input], &mut l, &mut r);
        pos += call as u64;
    }
    let closed = rx.status.closed.load(std::sync::atomic::Ordering::Acquire);
    let n = rx.consumer.slots();
    let chunk = rx.consumer.read_chunk(n).unwrap();
    let (a, b) = chunk.as_slices();
    let data = [a, b].concat();
    chunk.commit_all();
    (data, rx.status.start_pos.load(std::sync::atomic::Ordering::Relaxed), handle.position(), closed)
}

#[test]
fn an_audio_take_covers_exactly_the_window_whatever_the_call_size_and_closes_itself() {
    for call in [1usize, 7, 64, 100, 333] {
        let (data, start_pos, position, closed) = run(call, 1_000, 2_037);
        assert_eq!(start_pos, 1_000, "appels de {call}");
        assert_eq!(data.len(), 1_037, "appels de {call}");
        assert!(data.iter().enumerate().all(|(k, v)| *v == (1_000 + k) as f32), "appels de {call}");
        assert!(closed, "terminée toute seule, appels de {call}");
        assert!(position > 2_037, "la lecture continue après la fin de la prise");
    }
}

#[test]
fn a_midi_take_stops_at_its_end_and_only_keeps_what_falls_inside() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo).0;
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    let mut rx = handle.start_midi_record_window(t, 64, Some(1_000), Some(2_000)).unwrap();
    handle.send(Command::Seek(900));
    engine.process(&mut [0.0; 4], &mut [0.0; 4]);
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let mut pos = 900u64;
    let mut sent = 0;
    while pos < 3_000 {
        // Une note tous les 100 échantillons, envoyée au début du bloc qui la contient (livrée à l'échantillon du bloc).
        if (pos..pos + 64).any(|p| p % 100 == 0 && p >= 900) && sent < 30 {
            handle.send(Command::Midi { node: t, msg: MidiMsg::NoteOn { channel: 0, key: 60 + sent as u8 % 12, velocity: 0.8 } });
            sent += 1;
        }
        engine.process(&mut l, &mut r);
        pos += 64;
    }
    assert!(rx.is_closed(), "terminée toute seule à la fin de la fenêtre");
    assert_eq!((rx.start_pos(), rx.stop_pos()), (1_000, 2_000));
    let events = rx.take_events();
    assert!(!events.is_empty());
    assert!(events.iter().all(|e| e.pos >= 1_000 && e.pos < 2_000), "{:?}", events.iter().map(|e| e.pos).collect::<Vec<_>>());
}

#[test]
fn midi_input_events_after_the_end_of_the_window_are_not_recorded() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo).0;
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    let mut input = handle.take_midi_input().expect("entrée MIDI");
    let mut rx = handle.start_midi_record_window(t, 64, Some(2_000), Some(4_000)).unwrap();
    handle.send(Command::Seek(2_000));
    engine.process(&mut [0.0; 4], &mut [0.0; 4]);
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..8 {
        engine.process(&mut l, &mut r);
    }
    input.push(t, MidiMsg::NoteOn { channel: 0, key: 60, velocity: 0.8 }, 0);
    for _ in 0..40 {
        engine.process(&mut l, &mut r);
    }
    input.push(t, MidiMsg::NoteOff { channel: 0, key: 60, velocity: 0.0 }, 0);
    input.push(t, MidiMsg::NoteOn { channel: 0, key: 62, velocity: 0.8 }, 0);
    for _ in 0..4 {
        engine.process(&mut l, &mut r);
    }
    assert!(rx.is_closed());
    let events = rx.take_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(events[0].msg, MidiMsg::NoteOn { key: 60, .. }));
}
