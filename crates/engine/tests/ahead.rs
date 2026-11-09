//! Calcul en avance des pistes de lecture (`Engine::enable_ahead`) : après le raccord du démarrage, le son est **identique au bit près**
//! à celui d'un moteur qui calcule tout dans le callback (boucle comprise, déplacements, fader, muet, solo) ; arrêt, nouveau graphe,
//! armement, réglages de plugin, manque de données et arrêt du moteur se passent sans blocage ni allocation dans le fil audio.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};
use daw_engine::{AheadConfig, AudioData, Clip, Command, Engine, EngineHandle, GraphDesc, Insert, InsertFactory, RecordSource, TrackFormat};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;

const SR: u32 = 48_000;
const WAIT: Duration = Duration::from_secs(10);

/// Le compteur du garde-fou est global au processus : les tests passent l'un après l'autre.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn noise(seed: u32, len: usize) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 16_777_216.0 - 0.5
        })
        .collect()
}

/// Filtre à un pôle à mémoire, avec latence et coût réglables : sa sortie dépend de tout ce qu'il a reçu avant.
struct Pole {
    y: [f32; 2],
    a: f32,
    burn: u32,
    ring: Vec<f32>,
    pos: usize,
    sink: f32,
}

impl Insert for Pole {
    fn latency(&self) -> u32 {
        self.ring.len() as u32
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let mut x = self.sink;
        for _ in 0..self.burn {
            x = (x + 0.37).sin();
        }
        self.sink = x * 1e-30;
        for j in 0..in_l.len() {
            self.y[0] += self.a * (in_l[j] - self.y[0]);
            self.y[1] += self.a * (in_r[j] - self.y[1]);
            if self.ring.is_empty() {
                out_l[j] = self.y[0];
            } else {
                out_l[j] = self.ring[self.pos];
                self.ring[self.pos] = self.y[0];
                self.pos = (self.pos + 1) % self.ring.len();
            }
            out_r[j] = self.y[1];
        }
    }
    fn set_param(&mut self, _: u32, value: f64) {
        self.a = (value as f32).clamp(0.001, 1.0);
    }
    fn reset(&mut self) {
        self.y = [0.0; 2];
        self.ring.fill(0.0);
    }
}

fn pole(a: f32, burn: u32, latency: usize) -> InsertFactory {
    Arc::new(move |_, _, _| Ok(Box::new(Pole { y: [0.0; 2], a, burn, ring: vec![0.0; latency], pos: 0, sink: 0.0 }) as Box<dyn Insert>))
}

/// Pistes de lecture avec effets à mémoire, un bus, des envois : de quoi couvrir le routage.
fn graph(tracks: usize, burn: u32) -> GraphDesc {
    let data = AudioData::new(SR, vec![noise(1, SR as usize * 3), noise(2, SR as usize * 3)]).unwrap();
    let mut g = GraphDesc::new();
    let bus = g.add_bus("bus");
    g.add_insert(bus, "bus", pole(0.7, 0, 0));
    for i in 0..tracks {
        let t = g.add_track(&format!("p{i}"), if i % 3 == 0 { TrackFormat::Mono } else { TrackFormat::Stereo });
        g.add_clip(t, Clip::whole(data.clone(), (i * 211) as u64));
        g.set_output(t, bus);
        if i % 2 == 1 {
            g.add_send(t, daw_core_types::NodeId::MASTER, 0.25);
        }
        g.add_insert(t, "pole", pole(0.15 + 0.07 * (i % 6) as f32, burn, if i % 4 == 0 { 9 + i } else { 0 }));
    }
    g
}

fn cfg(ms: u32, threads: usize) -> AheadConfig {
    AheadConfig { lookahead_ms: ms, block: 256, threads, on_start: None }
}

fn engine(ahead: Option<AheadConfig>) -> (Engine, EngineHandle) {
    let (mut e, mut h) = Engine::new(SR, 256);
    if let Some(c) = ahead {
        e.enable_ahead(&mut h, &c);
    }
    (e, h)
}

/// Fait tourner `blocks` blocs de 64 échantillons ; avec l'avance, on attend les fils de rendu avant chaque bloc (un vrai callback ne
/// peut pas attendre : ici on veut comparer des sons, pas mesurer la vitesse). `script(bloc, poignée)` envoie les commandes.
fn stream(e: &mut Engine, h: &mut EngineHandle, ahead: bool, blocks: usize, mut script: impl FnMut(usize, &mut EngineHandle)) -> Vec<f32> {
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let mut out = Vec::with_capacity(blocks * 64);
    for b in 0..blocks {
        script(b, h);
        if ahead && !e.ahead_wait_ready(64, WAIT) {
            panic!("les fils de rendu n'ont pas fourni de données au bloc {b}");
        }
        e.process(&mut l, &mut r);
        out.extend_from_slice(&l);
        if b % 64 == 0 {
            h.collect_garbage();
        }
    }
    out
}

fn assert_same_after(a: &[f32], b: &[f32], skip: &[(usize, usize)], what: &str) {
    assert_eq!(a.len(), b.len());
    let mut compared = 0usize;
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if skip.iter().any(|&(s, e)| i >= s && i < e) {
            continue;
        }
        assert_eq!(x.to_bits(), y.to_bits(), "{what} : échantillon {i} ({x} contre {y})");
        compared += 1;
    }
    assert!(compared > a.len() / 2, "trop peu comparé");
}

#[test]
fn after_the_start_the_sound_is_bit_identical_to_a_live_engine_across_loop_wraps_seeks_and_faders() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let script = |b: usize, h: &mut EngineHandle| {
        match b {
            0 => {
                h.send(Command::SetLoop(Some((0, 40_000))));
                h.send(Command::Play);
            }
            300 => drop(h.send(Command::SetGain { node: 4, gain: 0.4 })),
            500 => drop(h.send(Command::SetMute { node: 6, mute: true })),
            700 => drop(h.send(Command::SetSolo { node: 3, solo: true })),
            900 => drop(h.send(Command::Seek(5_000))),
            1100 => drop(h.send(Command::SetSolo { node: 3, solo: false })),
            1300 => drop(h.send(Command::SetPan { node: 5, pan: -0.6 })),
            _ => {}
        }
    };
    let g = graph(10, 200);
    let (mut live, mut hl) = engine(None);
    hl.load_graph(&g).unwrap();
    let a = stream(&mut live, &mut hl, false, 1700, script);
    let (mut ahead, mut ha) = engine(Some(cfg(50, 2)));
    ha.load_graph(&g).unwrap();
    let b = stream(&mut ahead, &mut ha, true, 1700, script);
    // On ne compare pas les 512 premiers échantillons après un départ ou un déplacement (silence puis fondu d'entrée de la reprise).
    assert_same_after(&a, &b, &[(0, 512), (900 * 64, 900 * 64 + 512)], "avance contre direct");
    let st = ahead.ahead_stats().unwrap();
    assert_eq!(st.underruns.load(Ordering::Relaxed), 0);
    assert_eq!(st.desyncs.load(Ordering::Relaxed), 0);
    assert!(st.produced_blocks.load(Ordering::Relaxed) > 100);
    assert!(a.iter().any(|x| x.abs() > 0.05), "le projet fait du bruit");
}

#[test]
fn stop_cuts_the_playback_tracks_with_a_short_fade_and_play_resumes() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut e, mut h) = engine(Some(cfg(50, 2)));
    h.load_graph(&graph(6, 100)).unwrap();
    let out = stream(&mut e, &mut h, true, 900, |b, h| match b {
        0 => drop(h.send(Command::Play)),
        400 => drop(h.send(Command::Stop)),
        600 => drop(h.send(Command::Play)),
        _ => {}
    });
    let rms = |from: usize, to: usize| (out[from..to].iter().map(|x| x * x).sum::<f32>() / (to - from) as f32).sqrt();
    assert!(rms(300 * 64, 400 * 64) > 0.02, "la lecture sonne avant l'arrêt");
    // Après l'arrêt : quelques blocs de fondu au plus, puis silence.
    assert!(rms(405 * 64, 590 * 64) < 1e-6, "silence après l'arrêt : {}", rms(405 * 64, 590 * 64));
    // La lecture reprend à la position où elle s'était arrêtée, avec du son.
    assert!(rms(700 * 64, 900 * 64) > 0.02, "la lecture reprend : {}", rms(700 * 64, 900 * 64));
    assert!(out.iter().all(|x| x.is_finite()));
}

#[test]
fn a_plugin_setting_on_a_playback_track_is_heard_after_the_lookahead_and_faders_at_once() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // Une piste à un pôle très lent (presque rien ne passe), puis on l'ouvre : le son monte avec le retard de l'avance.
    let mut g = GraphDesc::new();
    let data = AudioData::new(SR, vec![noise(5, SR as usize * 4), noise(6, SR as usize * 4)]).unwrap();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(data, 0));
    let id = g.add_insert(t, "pole", pole(0.002, 0, 0)).unwrap();
    let (mut e, mut h) = engine(Some(cfg(50, 1)));
    h.load_graph(&g).unwrap();
    let mut sent_at = 0usize;
    let out = stream(&mut e, &mut h, true, 1500, |b, h| match b {
        0 => drop(h.send(Command::Play)),
        500 => {
            sent_at = b;
            h.send(Command::SetInsertParam { insert: id, param: 0, value: 1.0 });
        }
        1100 => drop(h.send(Command::SetGain { node: 1, gain: 0.0 })),
        _ => {}
    });
    let level = |from: usize, to: usize| out[from * 64..to * 64].iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    assert!(level(300, 500) < 0.05, "avant : presque rien ne passe ({})", level(300, 500));
    // Le réglage est entendu : dans les 50 ms d'avance + une marge (60 blocs de 64 = 80 ms), le niveau a nettement monté.
    assert!(level(sent_at + 60, sent_at + 200) > 0.2, "le réglage du plugin est entendu ({})", level(sent_at + 60, sent_at + 200));
    // Le fader, lui, agit tout de suite (rampe de 5 ms ≈ 4 blocs).
    assert!(level(1100 + 6, 1500) < 1e-6, "le fader coupe aussitôt ({})", level(1100 + 6, 1500));
}

#[test]
fn a_new_graph_during_playback_is_applied_and_the_playback_tracks_come_back() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g1 = graph(6, 100);
    let mut g2 = g1.clone();
    g2.add_bus("ajouté en route");
    let (mut e, mut h) = engine(Some(cfg(50, 2)));
    h.load_graph(&g1).unwrap();
    let out = stream(&mut e, &mut h, true, 1200, |b, h| match b {
        0 => drop(h.send(Command::Play)),
        400 => h.load_graph(&g2).unwrap(),
        800 => h.load_graph(&g1).unwrap(),
        _ => {}
    });
    let rms = |from: usize, to: usize| (out[from * 64..to * 64].iter().map(|x| x * x).sum::<f32>() / ((to - from) * 64) as f32).sqrt();
    for (a, b) in [(200, 400), (450, 800), (850, 1200)] {
        assert!(rms(a, b) > 0.02, "la lecture sonne entre les blocs {a} et {b} ({})", rms(a, b));
    }
    assert!(out.iter().all(|x| x.is_finite()));
}

#[test]
fn arming_a_playback_track_makes_it_live_and_the_input_is_monitored() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // Une piste sans clip dont l'effet laisse tout passer : l'écoute de l'entrée se voit tel quel en sortie.
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_insert(t, "passe", pole(1.0, 0, 0));
    let (mut e, mut h) = engine(Some(cfg(50, 1)));
    h.load_graph(&g).unwrap();
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let input = vec![0.25f32; 64];
    let mut first_heard = None;
    let mut last = 0.0f32;
    for b in 0..1500usize {
        match b {
            0 => drop(h.send(Command::Play)),
            200 => drop(h.send(Command::SetInput { node: 1, armed: true, monitor: true, source: RecordSource::Left })),
            _ => {}
        }
        if b < 200 {
            assert!(e.ahead_wait_ready(64, WAIT) || b == 0);
        }
        e.process_inputs(&[&input, &input], &mut l, &mut r);
        last = l[63];
        if b >= 200 && first_heard.is_none() && l[63].abs() > 0.2 {
            first_heard = Some(b);
        }
        std::thread::sleep(Duration::from_micros(300)); // laisse aux fils de rendu le temps de rendre la piste
    }
    let at = first_heard.expect("l'entrée finit par être écoutée");
    assert!(at < 1500 - 100, "écoutée tard : bloc {at}");
    assert!((last - 0.25).abs() < 1e-6, "l'entrée passe telle quelle (gain 1) : {last}");
}

#[test]
fn the_audio_thread_never_allocates_with_the_lookahead_running_through_every_kind_of_event() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g1 = graph(8, 100);
    let g2 = {
        let mut g = g1.clone();
        g.add_bus("autre");
        g
    };
    let (mut e, mut h) = engine(Some(cfg(50, 2)));
    h.load_graph(&g1).unwrap();
    h.send(Command::SetLoop(Some((0, 30_000))));
    h.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..40 {
        e.ahead_wait_ready(64, WAIT);
        e.process(&mut l, &mut r);
    }
    reset_violations();
    for i in 0..1500u32 {
        // Les envois de commandes et le chargement d'un graphe se font côté contrôle (allocation permise) ; seul `process` est surveillé.
        match i {
            200 => drop(h.send(Command::Seek(777))),
            400 => drop(h.send(Command::SetInsertParam { insert: 3, param: 0, value: 0.4 })),
            600 => drop(h.send(Command::Panic)),
            800 => drop(h.send(Command::SetGain { node: 2, gain: 0.5 })),
            900 => drop(h.send(Command::Stop)),
            1000 => drop(h.send(Command::Play)),
            1200 => drop(h.send(Command::SetLoop(Some((100, 20_000))))),
            _ => {}
        }
        if i == 1300 {
            h.load_graph(&g2).unwrap();
        }
        e.ahead_wait_ready(64, Duration::from_millis(50));
        let _rt = RtScope::enter();
        e.process(&mut l, &mut r);
    }
    assert_eq!(violations(), 0, "allocation dans process() avec le calcul en avance");
}

#[test]
fn when_the_render_threads_cannot_keep_up_the_gaps_are_counted_and_nothing_hangs_or_blows_up() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // Beaucoup trop lourd pour un seul fil de rendu en temps réel ; le fil audio, lui, ne fait qu'avancer (sans attendre).
    let (mut e, mut h) = engine(Some(cfg(20, 1)));
    h.load_graph(&graph(8, 40_000)).unwrap();
    h.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    // Le premier bloc doit être prêt (sinon on est encore « au démarrage », où le silence n'est pas une coupure) ; ensuite on file sans attendre.
    e.process(&mut l, &mut r);
    assert!(e.ahead_wait_ready(64, WAIT));
    let t0 = std::time::Instant::now();
    for i in 0..4000 {
        e.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|x| x.is_finite()));
        if i % 100 == 0 {
            std::thread::sleep(Duration::from_micros(200));
        }
    }
    assert!(t0.elapsed() < Duration::from_secs(30), "le fil audio n'attend jamais les fils de rendu");
    let st = e.ahead_stats().unwrap();
    assert!(st.underruns.load(Ordering::Relaxed) > 0, "des silences faute de données sont comptés");
}

#[test]
fn dropping_the_engine_while_the_lookahead_runs_terminates() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let d2 = Arc::clone(&done);
    let t = std::thread::spawn(move || {
        let (mut e, mut h) = engine(Some(cfg(50, 3)));
        h.load_graph(&graph(8, 200)).unwrap();
        h.send(Command::Play);
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        for _ in 0..200 {
            e.ahead_wait_ready(64, WAIT);
            e.process(&mut l, &mut r);
        }
        drop(e);
        drop(h);
        d2.store(true, Ordering::SeqCst);
    });
    let t0 = std::time::Instant::now();
    while !done.load(Ordering::SeqCst) && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(done.load(Ordering::SeqCst), "le moteur s'arrête même quand l'avance est en marche");
    t.join().unwrap();
}

#[test]
fn a_long_random_sequence_of_transport_edits_and_arming_keeps_producing_sound() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g1 = graph(6, 100);
    let mut g2 = g1.clone();
    g2.add_bus("x");
    let (mut e, mut h) = engine(Some(cfg(50, 2)));
    h.load_graph(&g1).unwrap();
    let mut s = 0x1234_5678u64;
    let mut rnd = move |n: u64| {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s % n
    };
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let input = vec![0.1f32; 64];
    let mut swapped = false;
    h.send(Command::Play);
    for _ in 0..3000 {
        if rnd(25) == 0 {
            match rnd(9) {
                0 => drop(h.send(Command::Seek(rnd(100_000)))),
                1 => drop(h.send(Command::Stop)),
                2 => drop(h.send(Command::Play)),
                3 => drop(h.send(Command::SetLoop(if rnd(2) == 0 { None } else { Some((rnd(5_000), 20_000 + rnd(30_000))) }))),
                4 => {
                    swapped = !swapped;
                    h.load_graph(if swapped { &g2 } else { &g1 }).unwrap();
                }
                5 => drop(h.send(Command::SetInput { node: 2, armed: rnd(2) == 0, monitor: true, source: RecordSource::Left })),
                6 => drop(h.send(Command::SetInsertParam { insert: rnd(7) as u32, param: 0, value: 0.1 + rnd(9) as f64 / 10.0 })),
                7 => drop(h.send(Command::Panic)),
                _ => drop(h.send(Command::SetGain { node: 1 + rnd(5) as u32, gain: rnd(3) as f32 * 0.5 })),
            }
        }
        e.ahead_wait_ready(64, Duration::from_millis(20));
        e.process_inputs(&[&input, &input], &mut l, &mut r);
        assert!(l.iter().chain(&r).all(|x| x.is_finite() && x.abs() < 8.0), "sortie saine");
        h.collect_garbage();
    }
    // Rien n'est resté coincé : on remet la lecture en route proprement et on entend du son.
    h.send(Command::SetInput { node: 2, armed: false, monitor: false, source: RecordSource::Left });
    h.send(Command::SetLoop(None));
    h.send(Command::Seek(0));
    h.send(Command::Stop);
    h.send(Command::Play);
    let mut peak = 0.0f32;
    for _ in 0..600 {
        e.ahead_wait_ready(64, Duration::from_millis(50));
        e.process(&mut l, &mut r);
        peak = peak.max(l.iter().fold(0.0f32, |m, x| m.max(x.abs())));
    }
    assert!(peak > 0.05, "du son après la séquence : {peak}");
}
