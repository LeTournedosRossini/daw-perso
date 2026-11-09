//! Métronome, pré-compte et prises qui commencent à une position précise.

use daw_engine::{AudioData, Click, ClickTrack, Clip, Command, CountIn, Engine, GraphDesc, MidiMsg, RecordSource, TrackFormat};

const SR: u32 = 48_000;

/// Formes d'onde simples et reconnaissables : le clic normal vaut `[1, 0,5, 0,25]`, l'accentué `[2, 1]`.
fn track(clicks: &[(u64, bool)]) -> ClickTrack {
    ClickTrack { clicks: clicks.iter().map(|&(pos, accent)| Click { pos, accent }).collect(), normal: vec![1.0, 0.5, 0.25], accent: vec![2.0, 1.0] }
}

fn engine_with(clicks: &[(u64, bool)]) -> (Engine, daw_engine::EngineHandle) {
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&GraphDesc::new()).unwrap();
    handle.set_click_track(track(clicks)).unwrap();
    handle.send(Command::SetMetronome { enabled: true, gain: 1.0 });
    engine.process(&mut [0.0; 8], &mut [0.0; 8]); // prend en compte les messages
    (engine, handle)
}

fn render(engine: &mut Engine, frames: usize, call: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let mut done = 0;
    while done < frames {
        let n = call.min(frames - done);
        let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
        engine.process(&mut l, &mut r);
        assert_eq!(l, r, "le clic est identique à gauche et à droite");
        out.extend(l);
        done += n;
    }
    out
}

/// La sortie attendue : chaque clic de `clicks` y pose sa forme d'onde, multipliée par `gain`.
fn expected(clicks: &[(u64, bool)], from: u64, frames: usize, gain: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    for &(pos, accent) in clicks {
        let wave: &[f32] = if accent { &[2.0, 1.0] } else { &[1.0, 0.5, 0.25] };
        for (k, s) in wave.iter().enumerate() {
            let at = pos as i64 + k as i64 - from as i64;
            if (0..frames as i64).contains(&at) {
                out[at as usize] += s * gain;
            }
        }
    }
    out
}

const CLICKS: [(u64, bool); 5] = [(100, true), (480, false), (481, false), (960, false), (1_439, true)];

#[test]
fn clicks_land_on_their_exact_sample_whatever_the_call_size() {
    for call in [1usize, 7, 64, 100, 333, 2_000] {
        let (mut engine, mut handle) = engine_with(&CLICKS);
        handle.send(Command::Play);
        let out = render(&mut engine, 2_000, call);
        assert_eq!(out, expected(&CLICKS, 0, 2_000, 1.0), "appels de {call}");
    }
}

#[test]
fn nothing_sounds_when_off_stopped_or_before_the_position_and_the_volume_applies() {
    let (mut engine, mut handle) = engine_with(&CLICKS);
    assert!(render(&mut engine, 2_000, 64).iter().all(|&v| v == 0.0), "arrêté : pas de clic");
    handle.send(Command::Play);
    handle.send(Command::SetMetronome { enabled: false, gain: 1.0 });
    assert!(render(&mut engine, 2_000, 64).iter().all(|&v| v == 0.0), "éteint : pas de clic");
    // Rallumé à mi-volume, tête déplacée à 450 : seuls les temps suivants sonnent, à leur place.
    handle.send(Command::Seek(450));
    handle.send(Command::SetMetronome { enabled: true, gain: 0.5 });
    let out = render(&mut engine, 1_200, 64);
    assert_eq!(out, expected(&CLICKS, 450, 1_200, 0.5));
}

#[test]
fn a_loop_replays_the_clicks_of_its_range() {
    let (mut engine, mut handle) = engine_with(&[(100, true), (300, false)]);
    handle.send(Command::SetLoop(Some((0, 500))));
    handle.send(Command::Play);
    let out = render(&mut engine, 1_500, 64);
    let one = expected(&[(100, true), (300, false)], 0, 500, 1.0);
    let three: Vec<f32> = one.iter().chain(&one).chain(&one).copied().collect();
    assert_eq!(out, three, "trois tours de boucle, les mêmes clics à chaque tour");
}

fn clip_engine(clicks: &[(u64, bool)]) -> (Engine, daw_engine::EngineHandle, u32) {
    let data = AudioData::new(SR, vec![vec![0.5f32; 20_000], vec![0.5f32; 20_000]]).unwrap();
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(data, 0));
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.set_click_track(track(clicks)).unwrap();
    handle.send(Command::SetMetronome { enabled: false, gain: 1.0 });
    engine.process(&mut [0.0; 8], &mut [0.0; 8]);
    (engine, handle, t.0)
}

#[test]
fn a_count_in_parks_the_playhead_plays_its_clicks_then_starts_playback_exactly_at_its_end() {
    let (mut engine, mut handle, _) = clip_engine(&[]);
    handle.send(Command::Seek(1_000));
    handle.start_count_in(CountIn { clicks: vec![(0, true), (100, false), (200, false)], frames: 300 }).unwrap();
    // Même si le métronome est éteint, le pré-compte se fait entendre (il faut un son pour les formes d'onde : on en a posé un).
    let out = render(&mut engine, 600, 64);
    let clicks: Vec<(usize, f32)> = out.iter().enumerate().filter(|(_, &v)| v != 0.0 && v != 0.5).map(|(i, &v)| (i, v)).collect();
    assert_eq!(&out[..300], &expected(&[(0, true), (100, false), (200, false)], 0, 300, 1.0)[..], "le clip ne joue pas pendant le pré-compte : seulement les clics");
    assert!(clicks.iter().all(|(i, _)| *i < 300));
    // À partir de l'échantillon 300 (pas un multiple du bloc de 64) : lecture du clip, à la position 1 000.
    assert!(out[300..].iter().all(|&v| (v - 0.5).abs() < 1e-6), "la lecture démarre exactement à la fin du pré-compte");
    assert_eq!(handle.position(), 1_000 + 300, "la tête était immobile à 1 000 pendant le pré-compte");
}

#[test]
fn stop_cancels_a_count_in() {
    let (mut engine, mut handle, _) = clip_engine(&[]);
    handle.start_count_in(CountIn { clicks: vec![(0, true)], frames: 1_000 }).unwrap();
    render(&mut engine, 200, 64);
    handle.send(Command::Stop);
    let out = render(&mut engine, 1_500, 64);
    assert!(out.iter().all(|&v| v == 0.0), "annulé : ni clic ni lecture");
    assert_eq!(handle.position(), 0);
}

fn drain(rx: &mut daw_engine::RecordReceiver) -> Vec<f32> {
    let n = rx.consumer.slots();
    let chunk = rx.consumer.read_chunk(n).unwrap();
    let (a, b) = chunk.as_slices();
    let v = [a, b].concat();
    chunk.commit_all();
    v
}

#[test]
fn a_take_starting_at_a_position_ignores_the_pre_roll_and_the_count_in() {
    // Pré-roll de 300 échantillons : la lecture commence à 700, la prise à 1 000.
    let (mut engine, mut handle, t) = clip_engine(&[]);
    let mut rx = handle.start_record_at(t, RecordSource::Channel(0), 4.0, Some(1_000)).unwrap();
    handle.send(Command::Seek(700));
    handle.start_count_in(CountIn { clicks: vec![(0, true)], frames: 130 }).unwrap(); // pré-compte avant le pré-roll
    let ramp = |from: usize, n: usize| -> Vec<f32> { (0..n).map(|k| (from + k) as f32 / 10_000.0).collect() };
    let mut fed = 0;
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    while fed < 130 + 300 + 500 {
        let input = ramp(fed, 64);
        engine.process_inputs(&[&input], &mut l, &mut r);
        fed += 64;
    }
    // Pré-compte : 130 échantillons ; puis lecture de 700 à 1 000 (300 échantillons) ; la prise commence donc à l'échantillon
    // 430 du flux d'entrée.
    assert_eq!(rx.status.start_pos.load(std::sync::atomic::Ordering::Relaxed), 1_000);
    let got = drain(&mut rx);
    assert!(!got.is_empty());
    for (k, v) in got.iter().enumerate().take(50) {
        assert!((v - (430 + k) as f32 / 10_000.0).abs() < 1e-6, "échantillon {k} de la prise");
    }
}

#[test]
fn a_midi_take_with_a_start_position_keeps_only_what_is_played_from_there() {
    let (mut engine, mut handle, t) = clip_engine(&[]);
    let mut midi = handle.take_midi_input().unwrap();
    let mut rx = handle.start_midi_record_at(t, 1_000, Some(640)).unwrap();
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for block in 0..20u32 {
        if block == 3 {
            midi.push(t, MidiMsg::NoteOn { channel: 0, key: 60, velocity: 0.5 }, 0); // avant la position
        }
        if block == 12 {
            midi.push(t, MidiMsg::NoteOn { channel: 0, key: 62, velocity: 0.5 }, 0); // après
        }
        engine.process(&mut l, &mut r);
    }
    assert_eq!(rx.start_pos(), 640);
    let events = rx.take_events();
    assert_eq!(events.iter().map(|e| e.pos).collect::<Vec<_>>(), vec![12 * 64], "seule la note jouée après la position est gardée");
}
