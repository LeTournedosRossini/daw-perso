//! Régularité des cycles audio : intervalle entre deux callbacks et temps de traitement.
//!
//! [`CycleTimer`] est appelée au début et à la fin de chaque callback. Elle n'alloue pas, ne bloque
//! pas et ne panique pas ; tout le calcul de synthèse ([`TimingSummary`]) se fait après coup.
//! Les pilotes USB livrent parfois les tampons par à-coups (un cycle long suivi d'un cycle quasi
//! nul) : on le détecte en classant chaque intervalle par rapport à la période nominale.

use std::time::Instant;

/// Horloge monotone en nanosecondes (sans allocation).
pub struct Clock(Instant);

impl Clock {
    pub fn new() -> Self {
        Clock(Instant::now())
    }
    #[inline]
    pub fn now_ns(&self) -> u64 {
        self.0.elapsed().as_nanos() as u64
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

/// Cycles ignorés au démarrage (établissement du flux, connexions).
const WARMUP_CYCLES: u64 = 20;

/// Bornes des classes d'intervalle, en fraction de la période nominale.
pub const BUCKET_LABELS: [&str; 6] = ["< 0,5", "0,5-0,9", "0,9-1,1", "1,1-1,5", "1,5-2,5", ">= 2,5"];

/// Classes du temps de traitement de notre callback, en pourcentage de la période du tampon.
pub const BUSY_LABELS: [&str; 7] = ["< 5 %", "5-10 %", "10-25 %", "25-50 %", "50-75 %", "75-100 %", ">= 100 %"];

pub fn busy_bucket(percent: f64) -> usize {
    match percent {
        p if p < 5.0 => 0,
        p if p < 10.0 => 1,
        p if p < 25.0 => 2,
        p if p < 50.0 => 3,
        p if p < 75.0 => 4,
        p if p < 100.0 => 5,
        _ => 6,
    }
}

fn bucket(ratio: f64) -> usize {
    match ratio {
        r if r < 0.5 => 0,
        r if r < 0.9 => 1,
        r if r <= 1.1 => 2,
        r if r <= 1.5 => 3,
        r if r < 2.5 => 4,
        _ => 5,
    }
}

/// Périodes (ns) dont on garde la répartition des instants de **tous** les callbacks, pour savoir si la phase des pires callbacks est
/// vraiment particulière : le pilote livre par salves d'environ 1 ms, donc tous les callbacks commencent à des phases voisines.
pub const PHASE_PERIODS_NS: [u64; 2] = [1_000_000, 15_625_000];
const PHASE_BINS: usize = 20;

/// Nombre de pires callbacks gardés (instant et durée) pour le diagnostic.
pub const WORST_KEPT: usize = 24;

/// Un callback lent : instant de son début (ms depuis le début de la mesure), durée de notre traitement et intervalle depuis le callback
/// précédent (pour distinguer « le pilote nous a livré tard » de « nous avons été lents »).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlowCycle {
    pub at_ms: f64,
    pub busy_ms: f64,
    pub since_previous_ms: f64,
}

pub struct CycleTimer {
    sample_rate: u32,
    cycles: u64,
    last_start: Option<u64>,
    last_nominal_ns: u64,
    start: u64,
    // Intervalles
    n: u64,
    buckets: [u64; 6],
    min_ns: u64,
    max_ns: u64,
    sum_ns: u64,
    /// Somme des carrés des intervalles en microsecondes (f64 : pas de dépassement).
    sumsq_us: f64,
    // Temps de traitement
    busy_n: u64,
    busy_sum_ns: u64,
    busy_max_ns: u64,
    busy_buckets: [u64; 7],
    /// Les `WORST_KEPT` traitements les plus longs de plus de la moitié de la période : (début en ns, durée en ns, intervalle précédent en ns).
    worst: [(u64, u64, u64); WORST_KEPT],
    first_ns: u64,
    prev_interval_ns: u64,
    /// Répartition (20 cases) des instants de début de tous les callbacks selon la phase dans chacune des `PHASE_PERIODS_NS`.
    phase_all: [[u32; PHASE_BINS]; 2],
}

impl CycleTimer {
    pub fn new(sample_rate: u32) -> Self {
        CycleTimer {
            sample_rate: sample_rate.max(1),
            cycles: 0,
            last_start: None,
            last_nominal_ns: 1,
            start: 0,
            n: 0,
            buckets: [0; 6],
            min_ns: u64::MAX,
            max_ns: 0,
            sum_ns: 0,
            sumsq_us: 0.0,
            busy_n: 0,
            busy_sum_ns: 0,
            busy_max_ns: 0,
            busy_buckets: [0; 7],
            worst: [(0, 0, 0); WORST_KEPT],
            first_ns: 0,
            prev_interval_ns: 0,
            phase_all: [[0; PHASE_BINS]; 2],
        }
    }

    /// À appeler en tout début de callback ; `frames` = taille du tampon de ce cycle.
    #[inline]
    pub fn begin(&mut self, now_ns: u64, frames: usize) {
        self.cycles += 1;
        if self.cycles == WARMUP_CYCLES + 1 {
            self.first_ns = now_ns;
        }
        self.prev_interval_ns = self.last_start.map_or(0, |p| now_ns.saturating_sub(p));
        if self.cycles > WARMUP_CYCLES {
            for (k, period) in PHASE_PERIODS_NS.iter().enumerate() {
                let bin = ((now_ns.saturating_sub(self.first_ns) % period) * PHASE_BINS as u64 / period) as usize;
                self.phase_all[k][bin.min(PHASE_BINS - 1)] += 1;
            }
        }
        if let Some(prev) = self.last_start {
            if self.cycles > WARMUP_CYCLES {
                let interval = now_ns.saturating_sub(prev);
                let ratio = interval as f64 / self.last_nominal_ns as f64;
                self.buckets[bucket(ratio)] += 1;
                self.n += 1;
                self.min_ns = self.min_ns.min(interval);
                self.max_ns = self.max_ns.max(interval);
                self.sum_ns = self.sum_ns.saturating_add(interval);
                let us = interval as f64 / 1000.0;
                self.sumsq_us += us * us;
            }
        }
        self.last_start = Some(now_ns);
        self.last_nominal_ns = (frames as u64 * 1_000_000_000 / self.sample_rate as u64).max(1);
        self.start = now_ns;
    }

    /// À appeler à la toute fin du callback.
    #[inline]
    pub fn end(&mut self, now_ns: u64) {
        if self.cycles > WARMUP_CYCLES {
            let busy = now_ns.saturating_sub(self.start);
            self.busy_n += 1;
            self.busy_sum_ns = self.busy_sum_ns.saturating_add(busy);
            self.busy_max_ns = self.busy_max_ns.max(busy);
            self.busy_buckets[busy_bucket(100.0 * busy as f64 / self.last_nominal_ns as f64)] += 1;
            if busy * 2 > self.last_nominal_ns {
                // On remplace le moins long des pires gardés (parcours de 24 cases : borné, sans allocation).
                let slot = (0..WORST_KEPT).fold(0, |m, k| if self.worst[k].1 < self.worst[m].1 { k } else { m });
                if busy > self.worst[slot].1 {
                    self.worst[slot] = (self.start, busy, self.prev_interval_ns);
                }
            }
        }
    }

    /// Synthèse (hors temps réel). `None` s'il n'y a pas eu assez de cycles.
    pub fn summary(&self) -> Option<TimingSummary> {
        if self.n == 0 {
            return None;
        }
        let ms = |ns: f64| ns / 1e6;
        let mean_ns = self.sum_ns as f64 / self.n as f64;
        let mean_us = mean_ns / 1000.0;
        let var_us = (self.sumsq_us / self.n as f64 - mean_us * mean_us).max(0.0);
        Some(TimingSummary {
            measured: self.n,
            nominal_ms: ms(self.last_nominal_ns as f64),
            interval_min_ms: ms(self.min_ns as f64),
            interval_mean_ms: ms(mean_ns),
            interval_max_ms: ms(self.max_ns as f64),
            interval_jitter_ms: var_us.sqrt() / 1000.0,
            buckets: self.buckets,
            busy_mean_ms: if self.busy_n > 0 { ms(self.busy_sum_ns as f64 / self.busy_n as f64) } else { 0.0 },
            busy_max_ms: ms(self.busy_max_ns as f64),
            busy_buckets: self.busy_buckets,
            phase_all: self.phase_all,
            slow: {
                let mut v: Vec<SlowCycle> = self.worst.iter().filter(|w| w.1 > 0).map(|w| SlowCycle { at_ms: w.0.saturating_sub(self.first_ns) as f64 / 1e6, busy_ms: w.1 as f64 / 1e6, since_previous_ms: w.2 as f64 / 1e6 }).collect();
                v.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));
                v
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimingSummary {
    /// Intervalles pris en compte (après la période de chauffe).
    pub measured: u64,
    pub nominal_ms: f64,
    pub interval_min_ms: f64,
    pub interval_mean_ms: f64,
    pub interval_max_ms: f64,
    /// Écart type des intervalles.
    pub interval_jitter_ms: f64,
    /// Nombre d'intervalles par classe (voir [`BUCKET_LABELS`]).
    pub buckets: [u64; 6],
    /// Temps passé dans notre callback.
    pub busy_mean_ms: f64,
    pub busy_max_ms: f64,
    /// Nombre de callbacks par classe de temps de traitement (voir [`BUSY_LABELS`]).
    pub busy_buckets: [u64; 7],
    /// Les pires traitements (de plus de la moitié de la période), classés par instant : pour chercher une périodicité.
    pub slow: Vec<SlowCycle>,
    /// Répartition des instants de tous les callbacks selon la phase (voir `PHASE_PERIODS_NS`).
    pub phase_all: [[u32; 20]; 2],
}

impl TimingSummary {
    /// Comme `periodicity` pour les périodes de `PHASE_PERIODS_NS`, mais rend aussi **la part de tous les callbacks** qui tombent dans la
    /// même fenêtre de phase : seule une part des pires callbacks **très supérieure** à celle de l'ensemble désigne une vraie périodicité.
    /// Rend `(part des pires, part de tous, phase en ms)`.
    pub fn periodicity_vs_all(&self, period_ms: f64) -> Option<(f64, f64, f64)> {
        let k = PHASE_PERIODS_NS.iter().position(|&p| (p as f64 / 1e6 - period_ms).abs() < 1e-9)?;
        let (share, phase_ms) = self.periodicity(period_ms)?;
        let total: u32 = self.phase_all[k].iter().sum();
        if total == 0 {
            return None;
        }
        // Fenêtre de ±10 % de la période autour de la phase retenue, comptée sur les 20 cases.
        let center = phase_ms / period_ms * PHASE_BINS as f64;
        let in_window = |bin: usize| {
            let d = (bin as f64 + 0.5 - center).abs();
            d.min(PHASE_BINS as f64 - d) <= 0.1 * PHASE_BINS as f64
        };
        let all: u32 = (0..PHASE_BINS).filter(|&b| in_window(b)).map(|b| self.phase_all[k][b]).sum();
        Some((share, all as f64 / total as f64, phase_ms))
    }

    /// Écarts entre instants successifs des pires callbacks (ms). Des écarts réguliers désignent une cause périodique (tic de
    /// l'ordonnanceur, tâche de fond) ; des écarts quelconques, du bruit.
    pub fn slow_gaps_ms(&self) -> Vec<f64> {
        self.slow.windows(2).map(|w| w[1].at_ms - w[0].at_ms).collect()
    }
    /// Part (de 0 à 1) des pires callbacks dont l'instant tombe, modulo `period_ms`, dans la même fenêtre de ±10 % de la période que le
    /// plus fréquent d'entre eux : proche de 1 = périodique à cette période. Rend aussi la phase retenue (ms).
    pub fn periodicity(&self, period_ms: f64) -> Option<(f64, f64)> {
        if self.slow.len() < 6 || period_ms <= 0.0 {
            return None;
        }
        let phase = |t: f64| (t % period_ms) / period_ms;
        let near = |a: f64, b: f64| {
            let d = (a - b).abs();
            d.min(1.0 - d) <= 0.1
        };
        let (best, count) = self.slow.iter().map(|c| phase(c.at_ms)).map(|p| (p, self.slow.iter().filter(|c| near(phase(c.at_ms), p)).count())).max_by_key(|x| x.1)?;
        Some((count as f64 / self.slow.len() as f64, best * period_ms))
    }
    /// Callbacks dont le traitement a duré au moins une période entière : le pilote n'a pas eu son tampon à temps (coupure probable).
    pub fn overruns(&self) -> u64 {
        self.busy_buckets[6]
    }
    /// Callbacks qui ont consommé plus de la moitié de la période : la marge devient mince.
    pub fn heavy(&self) -> u64 {
        self.busy_buckets[4] + self.busy_buckets[5] + self.busy_buckets[6]
    }
    /// Cycles arrivés trop tard (intervalle > 1,1 × période).
    pub fn late(&self) -> u64 {
        self.buckets[3] + self.buckets[4] + self.buckets[5]
    }
    /// Cycles arrivés trop tôt, typiquement collés au précédent (intervalle < 0,9 × période).
    pub fn bunched(&self) -> u64 {
        self.buckets[0] + self.buckets[1]
    }
    pub fn irregular_percent(&self) -> f64 {
        100.0 * (self.late() + self.bunched()) as f64 / self.measured as f64
    }
    /// Part de la période consommée par notre callback, au pire cas.
    pub fn busy_max_percent(&self) -> f64 {
        100.0 * self.busy_max_ms / self.nominal_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;
    const FRAMES: usize = 64;
    const PERIOD: u64 = 64 * 1_000_000_000 / 48_000; // 1 333 333 ns

    fn run(intervals: impl Iterator<Item = u64>, busy: u64) -> CycleTimer {
        let mut t = CycleTimer::new(SR);
        let mut now = 1_000;
        t.begin(now, FRAMES);
        t.end(now + busy);
        for iv in intervals {
            now += iv;
            t.begin(now, FRAMES);
            t.end(now + busy);
        }
        t
    }

    #[test]
    fn perfectly_regular() {
        let t = run(std::iter::repeat(PERIOD).take(500), 100_000);
        let s = t.summary().unwrap();
        assert_eq!(s.buckets, [0, 0, s.measured, 0, 0, 0]);
        assert_eq!(s.late() + s.bunched(), 0);
        assert!(s.interval_jitter_ms < 1e-6);
        assert!((s.interval_mean_ms - s.nominal_ms).abs() < 1e-3);
        assert!((s.busy_max_ms - 0.1).abs() < 1e-9);
        assert!(s.busy_max_percent() < 10.0);
    }

    #[test]
    fn detects_late_then_bunched_delivery() {
        // Tous les 10 cycles : un cycle en retard (2 x période) suivi d'un cycle collé (~0 x).
        let ivs = (0..500).map(|i| match i % 10 {
            4 => 2 * PERIOD - 20_000,
            5 => 20_000,
            _ => PERIOD,
        });
        let s = run(ivs, 50_000).summary().unwrap();
        // 500 intervalles ; les 19 premiers (période de chauffe) sont ignorés, ce qui retire 2 motifs sur 50.
        assert_eq!(s.late(), 48);
        assert_eq!(s.bunched(), 48);
        assert!((s.irregular_percent() - 100.0 * 96.0 / s.measured as f64).abs() < 1e-9);
        assert!(s.interval_jitter_ms > 0.1);
        assert!((s.interval_mean_ms - s.nominal_ms).abs() < 0.02, "la moyenne reste correcte");
    }

    #[test]
    fn warmup_is_ignored() {
        // 20 premiers cycles absurdes, ensuite réguliers.
        let ivs = (0..120).map(|i| if i < 15 { 10 * PERIOD } else { PERIOD });
        let s = run(ivs, 0).summary().unwrap();
        assert_eq!(s.buckets[5], 0);
        assert_eq!(s.late() + s.bunched(), 0);
    }

    #[test]
    fn busy_histogram_counts_each_callback_by_share_of_the_period() {
        // Période 1 333 333 ns : 3 %, 8 %, 20 %, 40 %, 60 %, 90 %, 150 % de la période.
        let shares = [3.0, 8.0, 20.0, 40.0, 60.0, 90.0, 150.0];
        let mut t = CycleTimer::new(SR);
        let mut now = 1_000;
        for _ in 0..WARMUP_CYCLES {
            t.begin(now, FRAMES);
            t.end(now + 1_000);
            now += PERIOD;
        }
        for sh in shares {
            t.begin(now, FRAMES);
            t.end(now + (PERIOD as f64 * sh / 100.0) as u64);
            now += PERIOD;
        }
        t.begin(now, FRAMES);
        let s = t.summary().unwrap();
        // Les cycles de chauffe ne comptent pas ; chaque classe reçoit exactement un de nos callbacks.
        assert_eq!(s.busy_buckets, [1, 1, 1, 1, 1, 1, 1]);
        assert_eq!(s.overruns(), 1);
        assert_eq!(s.heavy(), 3);
    }

    #[test]
    fn busy_bucket_edges() {
        assert_eq!([busy_bucket(4.9), busy_bucket(5.0), busy_bucket(9.9), busy_bucket(10.0), busy_bucket(24.9), busy_bucket(25.0), busy_bucket(49.9), busy_bucket(50.0), busy_bucket(74.9), busy_bucket(75.0), busy_bucket(99.9), busy_bucket(100.0)], [0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6]);
    }

    #[test]
    fn the_worst_callbacks_are_kept_with_their_instant_and_the_delivery_gap() {
        // Un callback lent (80 % de la période) toutes les 50 périodes, le reste à 10 %.
        let mut t = CycleTimer::new(SR);
        let mut now = 1_000;
        for i in 0..2_000u64 {
            t.begin(now, FRAMES);
            t.end(now + if i % 50 == 25 { PERIOD * 8 / 10 } else { PERIOD / 10 });
            now += PERIOD;
        }
        let s = t.summary().unwrap();
        assert!(!s.slow.is_empty() && s.slow.len() <= WORST_KEPT);
        assert!(s.slow.iter().all(|c| (c.busy_ms - 0.8 * s.nominal_ms).abs() < 1e-6), "seuls les callbacks lents sont gardés");
        assert!(s.slow.windows(2).all(|w| w[0].at_ms < w[1].at_ms), "classés par instant");
        assert!(s.slow.iter().all(|c| (c.since_previous_ms - s.nominal_ms).abs() < 1e-3), "intervalle précédent régulier");
        // Écart entre deux pires callbacks consécutifs : 50 périodes.
        let gaps = s.slow_gaps_ms();
        assert!(gaps.iter().all(|g| (g - 50.0 * s.nominal_ms).abs() < 1e-3), "{gaps:?}");
        // Périodicité à 50 périodes : tous dans la même phase.
        let (share, _) = s.periodicity(50.0 * s.nominal_ms).unwrap();
        assert!(share > 0.99, "{share}");
        // Aucun callback lent : rien de gardé.
        assert!(run(std::iter::repeat(PERIOD).take(500), 100_000).summary().unwrap().slow.is_empty());
    }

    #[test]
    fn periodicity_is_low_for_irregular_instants_and_needs_enough_points() {
        let mk = |ts: &[f64]| TimingSummary {
            measured: 1, nominal_ms: 1.0, interval_min_ms: 1.0, interval_mean_ms: 1.0, interval_max_ms: 1.0, interval_jitter_ms: 0.0, buckets: [0; 6], busy_mean_ms: 0.0, busy_max_ms: 0.0, busy_buckets: [0; 7], phase_all: [[0; 20]; 2],
            slow: ts.iter().map(|&t| SlowCycle { at_ms: t, busy_ms: 1.0, since_previous_ms: 1.0 }).collect(),
        };
        assert!(mk(&[1.0, 2.0, 3.0]).periodicity(15.625).is_none(), "trop peu de points");
        let irregular: Vec<f64> = (0..24).map(|k| (k * k * 37 % 997) as f64 * 1.37).collect();
        let (share, _) = mk(&irregular).periodicity(15.625).unwrap();
        assert!(share < 0.5, "{share}");
        let tick: Vec<f64> = (0..12).map(|k| k as f64 * 15.625 * 7.0 + 3.0).collect();
        let (share, phase) = mk(&tick).periodicity(15.625).unwrap();
        assert!(share > 0.99 && (phase - 3.0).abs() < 1.6, "{share} {phase}");
    }

    #[test]
    fn the_phase_of_the_worst_callbacks_is_compared_with_the_phase_of_all_callbacks() {
        // Tous les callbacks commencent près d'un multiple de 1 ms (salves du pilote) : une phase partagée par les pires n'a rien d'étonnant.
        let mut t = CycleTimer::new(SR);
        let mut now = 5_000_000u64;
        for i in 0..3_000u64 {
            t.begin(now + (i % 3) * 20_000, 32); // salves : légère dispersion autour de la milliseconde
            t.end(now + (i % 3) * 20_000 + if i % 40 == 7 { 600_000 } else { 50_000 });
            now += 1_000_000;
        }
        let s = t.summary().unwrap();
        let (slow, all, _) = s.periodicity_vs_all(1.0).unwrap();
        assert!(slow > 0.9 && all > 0.9, "tous (et les pires) tombent dans la même fenêtre : {slow} {all}");
        // Période inconnue : pas de comparaison.
        assert!(s.periodicity_vs_all(7.0).is_none());
    }

    #[test]
    fn too_short_returns_none() {
        assert!(run(std::iter::repeat(PERIOD).take(5), 0).summary().is_none());
    }

    #[test]
    fn bucket_edges() {
        assert_eq!([bucket(0.49), bucket(0.5), bucket(0.89), bucket(0.9), bucket(1.1), bucket(1.11), bucket(1.5), bucket(1.51), bucket(2.49), bucket(2.5)], [0, 1, 1, 2, 2, 3, 3, 4, 4, 5]);
    }
}
