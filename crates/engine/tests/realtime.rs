//! Contraintes temps réel du moteur : aucune allocation dans `process`, échange de graphe sans perte,
//! et une mesure de coût (ignorée par défaut : `cargo test --release -p daw-engine -- --ignored --nocapture`).

use std::sync::Arc;
use std::time::Instant;

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};
use daw_engine::{AudioData, AutoLane, AutoPoint, AutoTarget, Clip, Command, Engine, GraphDesc, Insert, MidiClip, MidiEvent, MidiMsg, Note, TrackFormat};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;

const SR: u32 = 48_000;

/// Le compteur du garde-fou est global au processus : les tests qui le lisent passent l'un après l'autre.
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

fn big_graph(tracks: usize, buses: usize, seconds: usize, seed: u32) -> GraphDesc {
    let data: Arc<AudioData> = AudioData::new(SR, vec![noise(seed, SR as usize * seconds), noise(seed + 1, SR as usize * seconds)]).unwrap();
    let mut g = GraphDesc::new();
    let bus_ids: Vec<_> = (0..buses).map(|i| g.add_bus(&format!("bus{i}"))).collect();
    for i in 0..tracks {
        let fmt = if i % 2 == 0 { TrackFormat::Mono } else { TrackFormat::Stereo };
        let t = g.add_track(&format!("t{i}"), fmt);
        g.add_clip(t, Clip::whole(data.clone(), (i * 100) as u64));
        if !bus_ids.is_empty() {
            g.set_output(t, bus_ids[i % buses]);
            g.add_send(t, bus_ids[(i + 1) % buses], 0.2);
        }
    }
    g
}

/// Effet de test sans allocation dans `process` : retard fixe.
struct Delay {
    buf: Vec<f32>,
    pos: usize,
}
impl Insert for Delay {
    fn latency(&self) -> u32 {
        self.buf.len() as u32
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        for j in 0..in_l.len() {
            out_l[j] = self.buf[self.pos];
            self.buf[self.pos] = in_l[j];
            self.pos = (self.pos + 1) % self.buf.len();
            out_r[j] = in_r[j];
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {
        self.buf.fill(0.0);
    }
}

fn graph_with_inserts(delay: usize) -> GraphDesc {
    let mut g = big_graph(8, 2, 2, 3);
    for node in 3..8 {
        g.add_insert(daw_core_types::NodeId(node), "retard", std::sync::Arc::new(move |_, _, _| Ok(Box::new(Delay { buf: vec![0.0; delay], pos: 0 }) as Box<dyn Insert>))).unwrap();
    }
    g
}

#[test]
fn engine_with_inserts_and_pdc_never_allocates_even_when_adopting_effects() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut engine, mut handle) = Engine::new(SR, 64);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let g1 = graph_with_inserts(100);
    handle.load_graph(&g1).unwrap();
    handle.send(Command::Play);
    handle.send(Command::SetLoop(Some((0, 40_000))));
    let mut g2 = g1.clone(); // mêmes effets : adoptés à l'échange
    g2.add_bus("autre");
    handle.load_graph(&g2).unwrap();
    assert_eq!(handle.latency_samples(), 100);

    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..400u32 {
            handle.send(Command::SetInsertParam { insert: i % 5, param: 0, value: 1.0 });
            if i == 150 {
                handle.send(Command::Seek(5));
            }
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation dans process() avec effets et compensation");
    assert!(l.iter().chain(&r).any(|&s| s != 0.0));
}

/// Instrument de test : compte les notes reçues (atomique, sans allocation).
struct NoteCounter(std::sync::Arc<std::sync::atomic::AtomicU32>);
impl Insert for NoteCounter {
    fn latency(&self) -> u32 {
        0
    }
    fn accepts_notes(&self) -> bool {
        true
    }
    fn process(&mut self, i: &[f32], j: &[f32], o: &mut [f32], p: &mut [f32]) {
        o.copy_from_slice(i);
        p.copy_from_slice(j);
    }
    fn process_with_notes(&mut self, i: &[f32], j: &[f32], o: &mut [f32], p: &mut [f32], notes: &[MidiEvent]) {
        self.0.fetch_add(notes.len() as u32, std::sync::atomic::Ordering::Relaxed);
        self.process(i, j, o, p);
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

#[test]
fn notes_never_allocate_in_the_audio_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let c = count.clone();
    let mut g = GraphDesc::new();
    let t = g.add_track("synthé", TrackFormat::Stereo);
    g.add_insert(t, "compteur", std::sync::Arc::new(move |_, _, _| Ok(Box::new(NoteCounter(c.clone())) as Box<dyn Insert>))).unwrap();
    let notes: Vec<Note> = (0..200u64).map(|k| Note { start: k * 700, len: 500, key: (k % 100) as u8, velocity: 0.7, channel: (k % 16) as u8 }).collect();
    g.add_midi_clip(t, MidiClip::new(0, 200 * 700, std::sync::Arc::new(notes)));
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::SetLoop(Some((0, 60_000))));
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);

    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..2_000u32 {
            if i % 10 == 0 {
                handle.send(Command::Midi { node: t.0, msg: MidiMsg::NoteOn { channel: 0, key: (i % 128) as u8, velocity: 0.5 } });
            }
            if i == 700 {
                handle.send(Command::Seek(1_234));
            }
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation avec des notes (clips, direct, boucle, déplacement)");
    assert!(count.load(std::sync::atomic::Ordering::Relaxed) > 300, "les notes sont bien arrivées");
}

#[test]
fn automation_never_allocates_in_the_audio_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let c = count.clone();
    let mut g = GraphDesc::new();
    let t = g.add_track("synthé", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![noise(1, 60_000), noise(2, 60_000)]).unwrap(), 0));
    let ins = g.add_insert(t, "compteur", std::sync::Arc::new(move |_, _, _| Ok(Box::new(NoteCounter(c.clone())) as Box<dyn Insert>))).unwrap();
    let notes: Vec<Note> = (0..100u64).map(|k| Note { start: k * 500, len: 300, key: (k % 100) as u8, velocity: 0.7, channel: 0 }).collect();
    g.add_midi_clip(t, MidiClip::new(0, 50_000, std::sync::Arc::new(notes)));
    let pts = |v: f64| (0..200u64).map(|k| AutoPoint::new(k * 300, v * (k % 7) as f64 / 7.0)).collect::<Vec<_>>();
    g.set_automation(t, AutoLane::new(AutoTarget::Gain, pts(1.0)));
    g.set_automation(t, AutoLane::new(AutoTarget::Pan, pts(0.9)));
    g.set_automation(t, AutoLane::new(AutoTarget::InsertParam { insert: ins, param: 3 }, pts(5.0)));
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::SetLoop(Some((0, 55_000))));
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..2_000u32 {
            if i == 900 {
                handle.send(Command::Seek(1_234));
            }
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation avec de l'automation (gain, pan, paramètre d'effet, notes, boucle)");
    assert!(count.load(std::sync::atomic::Ordering::Relaxed) > 100);
}

#[test]
fn midi_input_and_recording_never_allocate_in_the_audio_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let c = count.clone();
    let mut g = GraphDesc::new();
    let t = g.add_track("clavier", TrackFormat::Stereo);
    g.add_insert(t, "compteur", std::sync::Arc::new(move |_, _, _| Ok(Box::new(NoteCounter(c.clone())) as Box<dyn Insert>))).unwrap();
    let (mut engine, mut handle) = Engine::new(SR, 64);
    let mut midi = handle.take_midi_input().expect("entrée MIDI");
    handle.load_graph(&g).unwrap();
    let mut rx = handle.start_midi_record(t.0, 100_000).unwrap();
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 200], vec![0.0f32; 200]); // 200 > bloc max 64 : découpage interne
    engine.process(&mut l, &mut r); // installe le graphe et la prise hors de la zone surveillée
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..3_000u32 {
            let key = (i % 100) as u8;
            midi.push(t.0, MidiMsg::NoteOn { channel: 0, key, velocity: 0.5 }, (i * 7) % 200);
            midi.push(t.0, MidiMsg::NoteOff { channel: 0, key, velocity: 0.0 }, (i * 7 + 90) % 200);
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation avec entrée MIDI et prise MIDI");
    assert!(count.load(std::sync::atomic::Ordering::Relaxed) > 5_000);
    handle.stop_midi_record(t.0);
    engine.process(&mut l, &mut r);
    assert!(rx.is_closed());
    assert!(rx.take_events().len() > 5_000);
}

// Un seul test : le compteur du garde-fou est global au processus.
#[test]
fn engine_process_never_allocates_even_when_swapping_graphs() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut engine, mut handle) = Engine::new(SR, 64);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);

    handle.load_graph(&big_graph(16, 4, 2, 1)).unwrap();
    assert!(handle.send(Command::SetLoop(Some((1000, 50_000)))));
    assert!(handle.send(Command::Play));

    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..300u32 {
            handle.send(Command::SetGain { node: 1 + i % 16, gain: (i % 7) as f32 * 0.2 });
            handle.send(Command::SetPan { node: 1 + i % 16, pan: ((i % 5) as f32 - 2.0) * 0.5 });
            handle.send(Command::SetSolo { node: 1 + i % 16, solo: i % 3 == 0 });
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation dans process() (premier graphe)");

    // Échange de graphe en cours de lecture.
    handle.load_graph(&big_graph(8, 2, 2, 9)).unwrap();
    reset_violations();
    {
        let _rt = RtScope::enter();
        for _ in 0..300 {
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation pendant l'échange de graphe");
    assert!(l.iter().chain(&r).any(|&s| s != 0.0), "le moteur produit du signal");
    assert_eq!(handle.collect_garbage(), 2, "les deux graphes remplacés sont rendus au contrôle");
    assert_eq!(handle.leaked_graphs(), 0);
    assert!(handle.position() > 0);

    // Contre-épreuve : le garde-fou voit bien une allocation.
    reset_violations();
    {
        let _rt = RtScope::enter();
        std::hint::black_box(Vec::<u8>::with_capacity(1024));
    }
    assert!(violations() >= 1);
}

#[test]
fn trash_overflow_leaks_instead_of_freeing_on_the_audio_thread() {
    let (mut engine, mut handle) = Engine::new(SR, 64);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    // Plus de remplacements que la file de rebut n'en contient (8), sans jamais la vider côté contrôle.
    for _ in 0..12 {
        handle.load_graph(&GraphDesc::new()).unwrap();
    }
    engine.process(&mut l, &mut r);
    assert!(handle.leaked_graphs() >= 4, "{} fuites", handle.leaked_graphs());
    assert!(handle.collect_garbage() >= 8);
}

#[test]
#[ignore = "mesure de coût : à lancer en release avec --nocapture"]
fn cost_of_a_large_session() {
    for (tracks, buses) in [(8, 2), (32, 4), (64, 8), (128, 16)] {
        let seconds = 10;
        let (mut engine, mut handle) = Engine::new(SR, 64);
        handle.load_graph(&big_graph(tracks, buses, seconds, 5)).unwrap();
        handle.send(Command::Play);
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        engine.process(&mut l, &mut r); // prise en compte du graphe
        let blocks = SR as usize * seconds / 64 - 1;
        let (mut worst, mut total) = (0u128, 0u128);
        for _ in 0..blocks {
            let t = Instant::now();
            engine.process(&mut l, &mut r);
            let ns = t.elapsed().as_nanos();
            worst = worst.max(ns);
            total += ns;
        }
        let period_ns = 64.0 * 1e9 / SR as f64;
        let mean = total as f64 / blocks as f64;
        println!(
            "{tracks:>4} pistes, {buses:>2} bus, bloc 64 : moyen {:>7.1} µs ({:>5.1} % de la période), pire {:>7.1} µs ({:>5.1} %)",
            mean / 1e3,
            100.0 * mean / period_ns,
            worst as f64 / 1e3,
            100.0 * worst as f64 / period_ns
        );
    }
}

/// Instrument de test qui garde tous les messages reçus (hors zone surveillée : il alloue).
struct NoteLog(std::sync::Arc<std::sync::Mutex<Vec<MidiMsg>>>);
impl Insert for NoteLog {
    fn latency(&self) -> u32 {
        0
    }
    fn accepts_notes(&self) -> bool {
        true
    }
    fn process(&mut self, i: &[f32], j: &[f32], o: &mut [f32], p: &mut [f32]) {
        o.copy_from_slice(i);
        p.copy_from_slice(j);
    }
    fn process_with_notes(&mut self, i: &[f32], j: &[f32], o: &mut [f32], p: &mut [f32], notes: &[MidiEvent]) {
        self.0.lock().unwrap().extend(notes.iter().map(|e| e.msg));
        self.process(i, j, o, p);
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn logged_track() -> (Engine, daw_engine::EngineHandle, daw_core_types::NodeId, std::sync::Arc<std::sync::Mutex<Vec<MidiMsg>>>) {
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let l2 = log.clone();
    let mut g = GraphDesc::new();
    let t = g.add_track("synthé", TrackFormat::Stereo);
    g.add_insert(t, "journal", std::sync::Arc::new(move |_, _, _| Ok(Box::new(NoteLog(l2.clone())) as Box<dyn Insert>))).unwrap();
    let (engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    (engine, handle, t, log)
}

#[test]
fn a_seek_releases_the_notes_that_sound_because_a_plugin_may_keep_its_voices_on_reset() {
    let (mut engine, mut handle, t, log) = logged_track();
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    handle.send(Command::Midi { node: t.0, msg: MidiMsg::NoteOn { channel: 2, key: 60, velocity: 0.8 } });
    handle.send(Command::Midi { node: t.0, msg: MidiMsg::ControlChange { channel: 2, controller: 64, value: 127 } });
    engine.process(&mut l, &mut r);
    handle.send(Command::Seek(10_000));
    engine.process(&mut l, &mut r);
    engine.process(&mut l, &mut r);
    let log = log.lock().unwrap();
    assert!(log.contains(&MidiMsg::NoteOff { channel: 2, key: 60, velocity: 0.0 }), "fin de note livrée après le déplacement : {log:?}");
    assert!(log.contains(&MidiMsg::ControlChange { channel: 2, controller: 64, value: 0 }), "pédale relâchée");
}

#[test]
fn panic_cuts_tracked_notes_and_sweeps_every_key_of_every_channel() {
    let (mut engine, mut handle, t, log) = logged_track();
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    handle.send(Command::Midi { node: t.0, msg: MidiMsg::NoteOn { channel: 3, key: 70, velocity: 0.8 } });
    engine.process(&mut l, &mut r);
    log.lock().unwrap().clear();
    handle.send(Command::Panic);
    for _ in 0..20 {
        engine.process(&mut l, &mut r);
    }
    let log = log.lock().unwrap();
    for channel in 0..16u8 {
        for controller in [123u8, 120] {
            assert!(log.contains(&MidiMsg::ControlChange { channel, controller, value: 0 }), "contrôleur {controller} canal {channel}");
        }
        for key in 0..128u8 {
            assert!(log.contains(&MidiMsg::NoteOff { channel, key, velocity: 0.0 }), "fin de note canal {channel} touche {key}");
        }
    }
}

#[test]
fn panic_does_not_move_the_transport_and_never_allocates() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let c = count.clone();
    let mut g = GraphDesc::new();
    let t = g.add_track("synthé", TrackFormat::Stereo);
    g.add_insert(t, "compteur", std::sync::Arc::new(move |_, _, _| Ok(Box::new(NoteCounter(c.clone())) as Box<dyn Insert>))).unwrap();
    let notes: Vec<Note> = (0..100u64).map(|k| Note { start: k * 700, len: 500, key: (k % 100) as u8, velocity: 0.7, channel: (k % 16) as u8 }).collect();
    g.add_midi_clip(t, MidiClip::new(0, 100 * 700, std::sync::Arc::new(notes)));
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    engine.process(&mut l, &mut r);
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..400u32 {
            if i % 50 == 20 {
                handle.send(Command::Panic);
            }
            if i == 300 {
                handle.send(Command::Seek(1_000));
            }
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation avec la panique et le déplacement");
    assert!(handle.position() > 0, "la lecture continue : la panique ne déplace rien");
    assert!(count.load(std::sync::atomic::Ordering::Relaxed) > 2_048);
}

#[test]
fn forgetting_inserts_makes_the_next_graph_create_every_instance_afresh() {
    let created = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let build = |tag: &'static str| {
        let c = created.clone();
        let mut g = GraphDesc::new();
        let t = g.add_track("t", TrackFormat::Stereo);
        g.add_insert(t, tag, std::sync::Arc::new(move |_, _, _| {
            c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(Box::new(NoteCounter(std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)))) as Box<dyn Insert>)
        }))
        .unwrap();
        g
    };
    let (mut engine, mut handle) = Engine::new(SR, 64);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    handle.load_graph(&build("a")).unwrap();
    engine.process(&mut l, &mut r);
    assert_eq!(created.load(std::sync::atomic::Ordering::Relaxed), 1);
    // Même identifiant d'effet (0) : l'instance existante est reprise, l'usine n'est pas rappelée.
    handle.load_graph(&build("b")).unwrap();
    engine.process(&mut l, &mut r);
    assert_eq!(created.load(std::sync::atomic::Ordering::Relaxed), 1, "instance adoptée");
    // Après `forget_inserts` : usine appelée, l'ancienne instance disparaît avec l'ancien graphe.
    handle.forget_inserts();
    handle.load_graph(&build("c")).unwrap();
    engine.process(&mut l, &mut r);
    assert_eq!(created.load(std::sync::atomic::Ordering::Relaxed), 2, "instance neuve");
}

/// Effet de test qui rend le silence avec une latence annoncée : sa sortie est donc reconnaissable du signal sec.
struct Silencer(u32);
impl Insert for Silencer {
    fn latency(&self) -> u32 {
        self.0
    }
    fn process(&mut self, _: &[f32], _: &[f32], o: &mut [f32], p: &mut [f32]) {
        o.fill(0.0);
        p.fill(0.0);
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

fn bypass_graph(bypassed: bool) -> (GraphDesc, u32, Vec<f32>) {
    let signal = noise(7, 2_000);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![signal.clone(), signal.clone()]).unwrap(), 0));
    let id = g.add_insert(t, "silencieux", std::sync::Arc::new(|_, _, _| Ok(Box::new(Silencer(40)) as Box<dyn Insert>))).unwrap();
    g.nodes[t.0 as usize].inserts[0].bypass = bypassed;
    (g, id, signal)
}

fn render_blocks(engine: &mut Engine, blocks: usize, size: usize) -> Vec<f32> {
    let (mut l, mut r) = (vec![0.0f32; size], vec![0.0f32; size]);
    let mut out = Vec::new();
    for _ in 0..blocks {
        engine.process(&mut l, &mut r);
        out.extend_from_slice(&l);
    }
    out
}

#[test]
fn a_bypassed_insert_passes_the_signal_delayed_by_its_latency() {
    let (g, _, signal) = bypass_graph(true);
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let out = render_blocks(&mut engine, 20, 64);
    // Le gain de la piste est lissé à l'arrivée du graphe : on compare sur la fin, quand il est établi.
    for j in 600..1_200 {
        assert_eq!(out[j], signal[j - 40], "échantillon {j} : le signal sec, retardé de 40 comme la sortie de l'effet l'aurait été");
    }
    // Sans contournement : la sortie de l'effet (silence).
    let (g, _, _) = bypass_graph(false);
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    assert!(render_blocks(&mut engine, 20, 64).iter().all(|&v| v == 0.0));
}

#[test]
fn bypass_changes_fade_in_over_a_short_ramp_without_a_click_and_come_back() {
    let (g, id, signal) = bypass_graph(false);
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    render_blocks(&mut engine, 2, 64);
    handle.send(Command::SetInsertBypass { insert: id, bypass: true });
    let out = render_blocks(&mut engine, 12, 64);
    // Pendant la rampe (256 échantillons) le niveau monte de façon monotone depuis le silence, sans saut.
    let start = 0;
    assert!(out[start..start + 4].iter().all(|v| v.abs() < 0.1), "départ en douceur");
    for j in 300..700 {
        assert_eq!(out[j], signal[j + 128 - 40], "rampe terminée : signal sec exact");
    }
    // Retour : l'effet (silence) reprend en rampe, puis plus rien.
    handle.send(Command::SetInsertBypass { insert: id, bypass: false });
    let back = render_blocks(&mut engine, 8, 64);
    assert!(back[300..].iter().all(|&v| v == 0.0), "de nouveau la sortie de l'effet");
}

#[test]
fn bypass_never_allocates_in_the_audio_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (g, id, _) = bypass_graph(false);
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    handle.send(Command::SetLoop(Some((0, 1_500))));
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    engine.process(&mut l, &mut r);
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..600u32 {
            if i % 25 == 0 {
                handle.send(Command::SetInsertBypass { insert: id, bypass: (i / 25) % 2 == 0 });
            }
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation avec un contournement qui bascule");
}

#[test]
fn multichannel_inputs_and_recording_never_allocate_in_the_audio_thread() {
    use daw_engine::RecordSource;
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut g = GraphDesc::new();
    let tracks: Vec<u32> = (0..3).map(|i| g.add_track(&format!("t{i}"), TrackFormat::Stereo).0).collect();
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let sources = [RecordSource::Channel(2), RecordSource::Channel(7), RecordSource::Pair(4, 5)];
    let mut receivers = Vec::new();
    for (t, s) in tracks.iter().zip(sources) {
        handle.send(Command::SetInput { node: *t, armed: true, monitor: *t != 2, source: s });
        receivers.push(handle.start_record(*t, s, 4.0).unwrap());
    }
    let data: Vec<Vec<f32>> = (0..10).map(|c| noise(c as u32 + 1, 200)).collect();
    let refs: Vec<&[f32]> = data.iter().map(Vec::as_slice).collect();
    let (mut l, mut r) = (vec![0.0f32; 200], vec![0.0f32; 200]); // 200 > bloc max 64 : découpage interne
    engine.process_inputs(&refs, &mut l, &mut r); // installe le graphe et les prises hors de la zone surveillée
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..500u32 {
            if i == 250 {
                handle.send(Command::Seek(1_000));
            }
            engine.process_inputs(&refs, &mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation avec 10 canaux d'entrée, écoute et trois prises");
    drop(receivers);
}

#[test]
fn toggling_the_polarity_never_allocates_in_the_audio_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let g = big_graph(4, 1, 2, 9);
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    engine.process(&mut l, &mut r);
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..400u32 {
            handle.send(Command::SetInvert { node: 1 + i % 4, invert: (i / 4) % 2 == 0 });
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation avec la polarité qui bascule");
}

#[test]
fn input_dynamics_never_allocate_in_the_audio_thread() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut g = GraphDesc::new();
    let t = g.add_track("voix", TrackFormat::Stereo);
    g.node_mut(t).unwrap().input_dyn = daw_engine::InputDynamics { enabled: true, gate_db: -60.0, record: true, ..Default::default() };
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::SetInput { node: t.0, armed: true, monitor: true, source: daw_engine::RecordSource::Channel(0) });
    let rx = handle.start_record(t.0, daw_engine::RecordSource::Channel(0), 4.0).unwrap();
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; 200], vec![0.0f32; 200]);
    let input = noise(9, 200);
    engine.process_inputs(&[&input], &mut l, &mut r); // installe le graphe et la prise hors de la zone surveillée
    reset_violations();
    {
        let _rt = RtScope::enter();
        for _ in 0..500 {
            engine.process_inputs(&[&input], &mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation avec la dynamique d'entrée (écoute et prise traitée)");
    handle.stop_record(t.0);
    engine.process(&mut l, &mut r);
    assert!(rx.consumer.slots() > 0);
}
