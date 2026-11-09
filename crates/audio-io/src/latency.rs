//! Mesure de latence aller-retour par impulsion.
//!
//! [`LoopbackProbe`] est appelée à chaque cycle audio avec le tampon d'entrée et celui de sortie.
//! Elle émet périodiquement une impulsion en sortie et cherche son retour en entrée (câble de
//! bouclage, ou connexion logicielle). Latence = écart, en échantillons, entre le rang absolu de
//! l'impulsion émise et celui de la première entrée au-dessus du seuil.
//!
//! `process` n'alloue pas, ne bloque pas et ne panique pas : elle est faite pour le callback.

use std::sync::Arc;

use crate::spsc::Spsc;
use crate::timing::{Clock, CycleTimer, TimingSummary};

/// Résultat d'une impulsion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Latence aller-retour en échantillons.
    Latency(u32),
    /// Impulsion jamais revenue avant le délai maximal.
    Lost,
}

impl Event {
    /// Codage sur 32 bits pour la file SPSC (`u32::MAX` = perdue).
    pub fn encode(self) -> u32 {
        match self {
            Event::Latency(n) => n.min(u32::MAX - 1),
            Event::Lost => u32::MAX,
        }
    }
    pub fn decode(v: u32) -> Self {
        if v == u32::MAX { Event::Lost } else { Event::Latency(v) }
    }
}

#[derive(Clone, Copy)]
enum State {
    /// Prochaine impulsion à émettre au rang absolu donné.
    Idle { next_emit: u64 },
    /// Impulsion émise au rang `emit`, retour attendu.
    Waiting { emit: u64 },
}

pub struct LoopbackProbe {
    frame: u64,
    state: State,
    period: u64,
    timeout: u64,
    amplitude: f32,
    threshold: f32,
}

impl LoopbackProbe {
    /// Une impulsion toutes les 0,5 s, délai maximal de retour 0,4 s.
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as u64;
        LoopbackProbe {
            frame: 0,
            state: State::Idle { next_emit: sr / 2 },
            period: sr / 2,
            timeout: sr * 4 / 10,
            amplitude: 0.5,
            threshold: 0.1,
        }
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32], mut sink: impl FnMut(Event)) {
        output.fill(0.0);
        let n = input.len().min(output.len());
        for i in 0..n {
            let abs = self.frame + i as u64;
            if let State::Idle { next_emit } = self.state {
                if abs >= next_emit {
                    output[i] = self.amplitude;
                    self.state = State::Waiting { emit: abs };
                }
            }
            if let State::Waiting { emit } = self.state {
                if input[i].abs() > self.threshold {
                    // Pas de détection sur le rang même de l'émission si l'entrée vient d'avant :
                    // en pratique l'entrée à `abs` ne peut provenir de l'impulsion que si la latence est nulle.
                    let lat = abs - emit;
                    sink(Event::Latency(lat.min(u32::MAX as u64 - 1) as u32));
                    self.state = State::Idle { next_emit: emit + self.period };
                } else if abs - emit > self.timeout {
                    sink(Event::Lost);
                    self.state = State::Idle { next_emit: emit + self.period };
                }
            }
        }
        self.frame += n as u64;
    }
}

/// Tout ce qu'un backend branche dans son callback : sonde de latence, file de sortie des mesures
/// et chronométrage des cycles. `process` est appelée une fois par cycle, depuis le thread audio.
pub struct Rig {
    probe: LoopbackProbe,
    queue: Arc<Spsc>,
    timer: CycleTimer,
    clock: Clock,
}

impl Rig {
    pub fn new(sample_rate: u32, queue: Arc<Spsc>) -> Self {
        Rig { probe: LoopbackProbe::new(sample_rate), queue, timer: CycleTimer::new(sample_rate), clock: Clock::new() }
    }

    #[inline]
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        self.timer.begin(self.clock.now_ns(), input.len());
        let queue = &self.queue;
        self.probe.process(input, output, |e| {
            queue.push(e.encode());
        });
        self.timer.end(self.clock.now_ns());
    }

    /// Régularité des cycles, à lire une fois le flux arrêté.
    pub fn timing(&self) -> Option<TimingSummary> {
        self.timer.summary()
    }
}

/// Statistiques sur une série de mesures (hors temps réel).
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub count: usize,
    pub lost: usize,
    pub min: u32,
    pub max: u32,
    pub mean: f64,
    /// Écart type, en échantillons.
    pub jitter: f64,
}

impl Stats {
    pub fn from_samples(latencies: &[u32], lost: usize) -> Option<Self> {
        if latencies.is_empty() {
            return None;
        }
        let n = latencies.len() as f64;
        let mean = latencies.iter().map(|&v| v as f64).sum::<f64>() / n;
        let var = latencies.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / n;
        Some(Stats {
            count: latencies.len(),
            lost,
            min: *latencies.iter().min()?,
            max: *latencies.iter().max()?,
            mean,
            jitter: var.sqrt(),
        })
    }
}

/// Résultat d'un `latency-test`, commun à tous les backends.
#[derive(Debug)]
pub struct LatencyReport {
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub input: String,
    pub output: String,
    pub stats: Option<Stats>,
    /// Impulsions jamais revenues (renseigné même sans aucune mesure).
    pub lost: usize,
    /// Dépassements / pertes de synchro signalés par le serveur ou le pilote.
    pub xruns: u32,
    /// Allocations/libérations constatées dans notre callback (doit rester 0).
    pub callback_allocations: u64,
    /// Latences entrée/sortie annoncées par le pilote (ASIO), en échantillons.
    pub reported_latency: Option<(u32, u32)>,
    /// Régularité des cycles et temps de traitement du callback.
    pub timing: Option<TimingSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boucle simulée avec la causalité d'un vrai pilote : l'entrée d'un cycle est lue avant que
    /// la sortie du même cycle existe. Le retard ne peut donc pas être inférieur à `block`.
    fn simulate(delay: usize, block: usize, seconds: usize, gain: f32) -> (Vec<u32>, usize) {
        assert!(delay >= block);
        let sr = 48_000usize;
        let total = sr * seconds / block * block;
        let mut played = vec![0.0f32; total];
        let mut probe = LoopbackProbe::new(sr as u32);
        let (mut got, mut lost) = (Vec::new(), 0);
        let (mut inp, mut out) = (vec![0.0f32; block], vec![0.0f32; block]);
        for f in (0..total).step_by(block) {
            for (i, s) in inp.iter_mut().enumerate() {
                *s = if f + i >= delay { played[f + i - delay] * gain } else { 0.0 };
            }
            probe.process(&inp, &mut out, |e| match e {
                Event::Latency(n) => got.push(n),
                Event::Lost => lost += 1,
            });
            played[f..f + block].copy_from_slice(&out);
        }
        (got, lost)
    }

    #[test]
    fn measures_exact_delay_for_many_block_sizes() {
        for &(block, delay) in &[(32, 32), (32, 33), (32, 70), (64, 64), (64, 200), (128, 128), (128, 1000), (256, 600)] {
            let (got, lost) = simulate(delay, block, 5, 1.0);
            assert_eq!(lost, 0, "block {block} delay {delay}");
            assert!(got.len() >= 8, "block {block} delay {delay}: {} mesures", got.len());
            assert!(got.iter().all(|&l| l as usize == delay), "block {block} delay {delay}: {got:?}");
        }
    }

    #[test]
    fn weak_signal_below_threshold_is_lost() {
        let (got, lost) = simulate(100, 64, 3, 0.05);
        assert!(got.is_empty());
        assert!(lost >= 3);
    }

    #[test]
    fn attenuated_signal_above_threshold_is_found() {
        let (got, lost) = simulate(100, 64, 3, 0.5);
        assert_eq!(lost, 0);
        assert!(got.iter().all(|&l| l == 100));
    }

    #[test]
    fn stats() {
        let s = Stats::from_samples(&[100, 102, 98, 100], 1).unwrap();
        assert_eq!((s.count, s.lost, s.min, s.max), (4, 1, 98, 102));
        assert!((s.mean - 100.0).abs() < 1e-9);
        assert!((s.jitter - 2f64.sqrt()).abs() < 1e-9);
        assert!(Stats::from_samples(&[], 3).is_none());
    }

    #[test]
    fn event_codec() {
        assert_eq!(Event::decode(Event::Latency(123).encode()), Event::Latency(123));
        assert_eq!(Event::decode(Event::Lost.encode()), Event::Lost);
    }
}
