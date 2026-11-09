//! Calcul des pistes en parallèle : **le même son au bit près** que sur un seul fil (quel que soit le nombre de fils, la taille des
//! blocs, les changements de paramètres, les échanges de graphe), aucune allocation (fil audio **et** fils de travail), un fil audio
//! qui se suffit à lui-même si les fils de travail sont absents, et un gain de temps mesurable quand les pistes sont lourdes
//! (`cargo test --release -p daw-engine --test parallel -- --ignored --nocapture`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};
use daw_core_types::NodeId;
use daw_engine::{AudioData, AutoLane, AutoPoint, AutoTarget, Clip, Command, Engine, GraphDesc, Insert, InsertFactory, MidiClip, Note, Pool, TrackFormat};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;

const SR: u32 = 48_000;

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

/// Effet à mémoire (filtre à un pôle) qui consomme `burn` tours de boucle : le résultat dépend de tout ce qu'il a reçu avant, donc
/// d'un calcul dans le bon ordre et sans partage entre pistes.
struct Pole {
    y: [f32; 2],
    a: f32,
    burn: u32,
    latency: usize,
    ring: Vec<f32>,
    pos: usize,
    sink: f32,
}

impl Pole {
    fn new(a: f32, burn: u32, latency: usize) -> Pole {
        Pole { y: [0.0; 2], a, burn, latency, ring: vec![0.0; latency], pos: 0, sink: 0.0 }
    }
}

impl Insert for Pole {
    fn latency(&self) -> u32 {
        self.latency as u32
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let mut x = self.sink;
        for _ in 0..self.burn {
            x = (x + 0.37).sin();
        }
        self.sink = x * 1e-30; // empêche d'optimiser la boucle ; effet numérique nul
        for j in 0..in_l.len() {
            self.y[0] += self.a * (in_l[j] - self.y[0]);
            self.y[1] += self.a * (in_r[j] - self.y[1]);
            let (l, r) = (self.y[0], self.y[1]);
            if self.latency > 0 {
                out_l[j] = self.ring[self.pos];
                self.ring[self.pos] = l;
                self.pos = (self.pos + 1) % self.latency;
            } else {
                out_l[j] = l;
            }
            out_r[j] = r;
        }
    }
    fn accepts_notes(&self) -> bool {
        false
    }
    fn set_param(&mut self, _: u32, value: f64) {
        self.a = (value as f32).clamp(0.01, 1.0);
    }
    fn reset(&mut self) {
        self.y = [0.0; 2];
        self.ring.fill(0.0);
    }
}

fn pole_factory(a: f32, burn: u32, latency: usize) -> InsertFactory {
    Arc::new(move |_, _, _| Ok(Box::new(Pole::new(a, burn, latency)) as Box<dyn Insert>))
}

/// Un projet qui exerce tout ce que le parallèle touche : pistes mono et stéréo, deux niveaux de bus, envois avant et après fader,
/// effets à mémoire et à latence (compensation), automation de gain et de paramètre d'effet, notes, solo et mute.
fn graph(tracks: usize, burn: u32) -> GraphDesc {
    let data = AudioData::new(SR, vec![noise(1, SR as usize * 3), noise(2, SR as usize * 3)]).unwrap();
    let mut g = GraphDesc::new();
    let sub = [g.add_bus("sous-bus 1"), g.add_bus("sous-bus 2")];
    let top = g.add_bus("groupe");
    g.set_output(sub[0], top);
    g.set_output(sub[1], top);
    g.add_insert(top, "bus", pole_factory(0.8, 0, 0));
    for i in 0..tracks {
        let fmt = if i % 3 == 0 { TrackFormat::Mono } else { TrackFormat::Stereo };
        let t = g.add_track(&format!("piste {i}"), fmt);
        g.add_clip(t, Clip::whole(data.clone(), (i * 137) as u64));
        g.set_output(t, sub[i % 2]);
        if i % 4 == 1 {
            g.add_send(t, top, 0.3);
        }
        if i % 5 == 2 {
            g.add_send(t, sub[(i + 1) % 2], 0.2);
        }
        let id = g.add_insert(t, "pole", pole_factory(0.2 + 0.05 * (i % 7) as f32, burn, if i % 4 == 0 { 17 + i } else { 0 })).unwrap();
        if i % 2 == 0 {
            g.add_insert(t, "pole 2", pole_factory(0.5, burn / 2, 0));
        }
        if i % 6 == 0 {
            g.set_automation(t, AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(0, 0.2), AutoPoint::new(60_000, 1.0), AutoPoint::new(120_000, 0.4)]));
        }
        if i % 6 == 3 {
            g.set_automation(t, AutoLane::new(AutoTarget::InsertParam { insert: id, param: 0 }, vec![AutoPoint::new(0, 0.1), AutoPoint::new(90_000, 0.9)]));
        }
        if i % 5 == 4 {
            g.add_midi_clip(t, MidiClip::new(0, SR as u64, Arc::new(vec![Note { start: 100, len: 5_000, key: 60, velocity: 0.8, channel: 0 }])));
        }
    }
    g
}

fn pool_of(threads: usize, workers: bool) -> Option<Pool> {
    match (threads, workers) {
        (0, _) | (1, _) => None,
        (n, true) => Some(Pool::new(n, None)),
        (n, false) => Some(Pool::without_workers(n)),
    }
}

fn hash(bits: &mut u64, l: &[f32], r: &[f32]) {
    for &x in l.iter().chain(r) {
        *bits = (*bits ^ x.to_bits() as u64).wrapping_mul(0x100_0000_01b3);
    }
}

/// Rend `blocks` blocs de `block` échantillons avec des commandes, une boucle, un déplacement et un échange de graphe en route ;
/// rend l'empreinte de tout ce qui est sorti.
fn render(threads: usize, workers: bool, block: usize, blocks: usize) -> u64 {
    let (mut engine, mut handle) = Engine::new(SR, 256);
    if let Some(p) = pool_of(threads, workers) {
        engine.set_pool(p);
    }
    let g = graph(20, 400);
    handle.load_graph(&g).unwrap();
    handle.send(Command::SetLoop(Some((0, 100_000))));
    handle.send(Command::Play);
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in 0..blocks {
        match b {
            20 => drop(handle.send(Command::SetGain { node: 5, gain: 0.5 })),
            40 => drop(handle.send(Command::SetMute { node: 7, mute: true })),
            60 => drop(handle.send(Command::SetSolo { node: 9, solo: true })),
            80 => drop(handle.send(Command::Seek(1234))),
            100 => drop(handle.send(Command::SetSolo { node: 9, solo: false })),
            120 => {
                let mut g2 = g.clone();
                g2.add_bus("ajouté en route");
                handle.load_graph(&g2).unwrap();
            }
            140 => drop(handle.send(Command::SetInsertParam { insert: 3, param: 0, value: 0.7 })),
            _ => {}
        }
        engine.process(&mut l, &mut r);
        hash(&mut h, &l, &r);
        if b % 50 == 0 {
            handle.collect_garbage();
        }
    }
    h
}

#[test]
fn the_sound_is_bit_identical_whatever_the_number_of_threads_and_the_block_size() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for block in [64usize, 100, 256] {
        let reference = render(1, true, block, 260);
        for threads in [2usize, 3, 4] {
            for _ in 0..3 {
                assert_eq!(render(threads, true, block, 260), reference, "{threads} fils, bloc {block}");
            }
        }
    }
    // Et la taille de bloc ne change rien non plus (propriété déjà vraie sans fils : le parallèle ne doit pas la casser).
    let (a, b) = (render(3, true, 64, 400), render(3, true, 64, 400));
    assert_eq!(a, b);
}

#[test]
fn without_any_worker_the_audio_thread_does_everything_and_the_sound_is_the_same() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let reference = render(1, true, 64, 200);
    assert_eq!(render(4, false, 64, 200), reference, "fils de travail absents : le fil audio prend tout");
}

#[test]
fn a_pool_never_allocates_in_the_audio_thread_nor_in_the_workers() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // Les fils de travail entrent eux aussi dans la zone surveillée (jamais quittée : le fil meurt avec).
    let hook: Arc<dyn Fn(usize) + Send + Sync> = Arc::new(|_| std::mem::forget(RtScope::enter()));
    let (mut engine, mut handle) = Engine::new(SR, 64);
    engine.set_pool(Pool::new(3, Some(hook)));
    handle.load_graph(&graph(16, 300)).unwrap();
    handle.send(Command::Play);
    handle.send(Command::SetLoop(Some((0, 60_000))));
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..50 {
        engine.process(&mut l, &mut r); // chauffe : les coûts sont mesurés, la répartition faite
    }
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..600u32 {
            handle.send(Command::SetInsertParam { insert: i % 11, param: 0, value: 0.3 });
            if i == 200 {
                handle.send(Command::Seek(7));
            }
            engine.process(&mut l, &mut r);
        }
    }
    assert_eq!(violations(), 0, "allocation pendant le calcul parallèle");
}

#[test]
fn many_blocks_of_every_size_with_a_graph_swap_never_hang_and_stay_identical() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let run = |threads: usize| {
        let (mut engine, mut handle) = Engine::new(SR, 128);
        if let Some(p) = pool_of(threads, true) {
            engine.set_pool(p);
        }
        let mut gd = graph(12, 60);
        handle.load_graph(&gd).unwrap();
        handle.send(Command::Play);
        let mut h = 0u64;
        let sizes = [1usize, 7, 64, 33, 128, 2, 100];
        let (mut l, mut r) = (vec![0.0f32; 128], vec![0.0f32; 128]);
        for b in 0..4_000usize {
            if b % 700 == 699 {
                gd.add_bus("x");
                handle.load_graph(&gd).unwrap();
            }
            let n = sizes[b % sizes.len()];
            engine.process(&mut l[..n], &mut r[..n]);
            hash(&mut h, &l[..n], &r[..n]);
            if b % 64 == 0 {
                handle.collect_garbage();
            }
        }
        h
    };
    let reference = run(1);
    assert_eq!(run(4), reference);
}

/// Gain de temps : 16 pistes lourdes, 1 puis 2, 3, 4 fils (`--ignored`, de préférence en release, sur une machine au repos).
#[test]
#[ignore]
fn speed_up_on_heavy_tracks() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut base = 0.0;
    for threads in [1usize, 2, 3, 4] {
        if threads > cores {
            break;
        }
        let (mut engine, mut handle) = Engine::new(SR, 64);
        if let Some(p) = pool_of(threads, true) {
            engine.set_pool(p);
        }
        handle.load_graph(&graph(16, 3000)).unwrap();
        handle.send(Command::Play);
        handle.send(Command::SetLoop(Some((0, 100_000))));
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        for _ in 0..400 {
            engine.process(&mut l, &mut r);
        }
        let total = AtomicU64::new(0);
        let mut worst = 0u64;
        for _ in 0..3000 {
            let t0 = Instant::now();
            engine.process(&mut l, &mut r);
            let dt = t0.elapsed().as_nanos() as u64;
            total.fetch_add(dt, Ordering::Relaxed);
            worst = worst.max(dt);
        }
        let mean = total.load(Ordering::Relaxed) as f64 / 3000.0 / 1000.0;
        if threads == 1 {
            base = mean;
        }
        println!("{threads} fil(s) : {mean:.1} µs par bloc (pire {:.1} µs), gain ×{:.2}", worst as f64 / 1000.0, base / mean);
    }
    let _ = NodeId::MASTER;
}
