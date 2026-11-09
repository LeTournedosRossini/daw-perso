//! Boucle simulée : la sortie du moteur, retardée de `L` échantillons comme le ferait la carte son, revient sur l'entrée d'une piste
//! armée. Où tombe la prise par rapport à l'original, avec et sans plugin qui annonce une latence, avec et sans calcul en avance ?
//!
//! Constat qu'on épingle : la compensation du moteur (PDC) retarde **toute la lecture** de la latence des plugins ; ce que le musicien
//! entend (donc ce qu'il rejoue, et ce que la boucle ramène) est en retard de cette latence sur la ligne de temps. Une prise rangée
//! avec la seule latence de la carte tombe donc **en retard de la latence des plugins** sur l'original.

use std::sync::Arc;
use std::time::Duration;

use daw_engine::{AheadConfig, AudioData, Clip, Command, Engine, GraphDesc, Insert, InsertFactory, RecordSource, TrackFormat};

const SR: u32 = 48_000;

/// Retard pur de `n` échantillons annoncé comme latence : ce que fait un limiteur à anticipation.
struct Delay {
    ring: Vec<f32>,
    ring_r: Vec<f32>,
    pos: usize,
}

impl Insert for Delay {
    fn latency(&self) -> u32 {
        self.ring.len() as u32
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        for j in 0..in_l.len() {
            out_l[j] = self.ring[self.pos];
            out_r[j] = self.ring_r[self.pos];
            self.ring[self.pos] = in_l[j];
            self.ring_r[self.pos] = in_r[j];
            self.pos = (self.pos + 1) % self.ring.len();
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.ring_r.fill(0.0);
    }
}

fn delay(n: usize) -> InsertFactory {
    Arc::new(move |_, _, _| Ok(Box::new(Delay { ring: vec![0.0; n], ring_r: vec![0.0; n], pos: 0 }) as Box<dyn Insert>))
}

fn noise(len: usize) -> Vec<f32> {
    let mut s = 0x1234_5678u32;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 8) as f32 / 16_777_216.0 - 0.5) * 0.5
        })
        .collect()
}

/// Joue la référence, boucle la sortie vers l'entrée avec `loop_delay` échantillons de retard, enregistre. Retourne (position de départ
/// de la prise, prise, référence, latence annoncée par les plugins).
fn run(plugin_latency: usize, loop_delay: usize, ahead: bool, block: usize) -> (u64, Vec<f32>, Vec<f32>, u32) {
    let reference = noise(20_000);
    let mut g = GraphDesc::new();
    let take = g.add_track("prise", TrackFormat::Stereo).0;
    let play = g.add_track("référence", TrackFormat::Stereo);
    g.add_clip(play, Clip::whole(AudioData::new(SR, vec![reference.clone(), reference.clone()]).unwrap(), 0));
    if plugin_latency > 0 {
        g.add_insert(play, "retard", delay(plugin_latency)).unwrap();
    }
    let (mut engine, mut handle) = Engine::new(SR, 256);
    if ahead {
        engine.enable_ahead(&mut handle, &AheadConfig { lookahead_ms: 50, block: 256, threads: 1, on_start: None });
    }
    handle.load_graph(&g).unwrap();
    let latency = handle.latency_samples();
    handle.send(Command::SetInput { node: take, armed: true, monitor: false, source: RecordSource::Channel(0) });
    let mut rx = handle.start_record(take, RecordSource::Channel(0), 2.0).unwrap();
    handle.send(Command::Play);
    let total = 24_000usize;
    let mut returning = std::collections::VecDeque::from(vec![0.0f32; loop_delay]);
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let mut done = 0;
    while done < total {
        let n = block.min(total - done);
        let input: Vec<f32> = returning.drain(..n).collect();
        if ahead && !engine.ahead_wait_ready(n, Duration::from_secs(5)) {
            panic!("fils de rendu en retard");
        }
        engine.process_inputs(&[&input], &mut l[..n], &mut r[..n]);
        returning.extend(l[..n].iter().copied());
        done += n;
        handle.collect_garbage();
    }
    let n = rx.consumer.slots();
    let chunk = rx.consumer.read_chunk(n).unwrap();
    let (a, b) = chunk.as_slices();
    let take_samples = [a, b].concat();
    chunk.commit_all();
    // Le moteur de test n'expose pas `start_pos` sans arrêt de prise : la prise commence à la position 0 du transport.
    (0, take_samples, reference, latency)
}

/// Retard de la prise sur l'original : le plus grand `L` tel que `prise[j] = original[j − L] × gain` (recherche exacte).
fn measured_lag(take: &[f32], reference: &[f32], search: std::ops::RangeInclusive<usize>) -> usize {
    let score = |lag: usize| -> f64 { (lag + 2_000..lag + 12_000).map(|j| take[j] as f64 * reference[j - lag] as f64).sum() };
    search.max_by(|&a, &b| score(a).total_cmp(&score(b))).unwrap()
}

#[test]
fn without_plugins_the_take_lands_exactly_where_the_loop_delay_says() {
    for (ahead, block) in [(false, 64), (false, 100), (true, 64)] {
        let (_, take, reference, latency) = run(0, 300, ahead, block);
        assert_eq!(latency, 0);
        assert_eq!(measured_lag(&take, &reference, 250..=350), 300, "ahead={ahead} block={block}");
        // Et la prise est exactement l'original retardé : aucun échantillon différent.
        assert!((2_000..12_000).all(|j| take[j] == reference[j - 300]), "ahead={ahead} block={block}");
    }
}

#[test]
fn a_plugin_latency_delays_the_whole_playback_and_so_the_take_by_that_latency() {
    for ahead in [false, true] {
        let (_, take, reference, latency) = run(480, 300, ahead, 64);
        assert_eq!(latency, 480, "latence annoncée par les plugins");
        let lag = measured_lag(&take, &reference, 700..=850);
        assert_eq!(lag, 300 + 480, "ahead={ahead} : la prise est en retard de la boucle (300) plus la latence des plugins (480)");
    }
}
