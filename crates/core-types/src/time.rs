//! Temps musical exact et carte des tempos.
//!
//! Décision (CLAUDE.md §12, reprise de Scorsaire) : une position musicale est une **fraction exacte de ronde**
//! ([`Frac`], paire réduite `n/d`) : noire `1/4`, croche de triolet `1/12`. L'égalité est exacte, sans tolérance. Les
//! échantillons n'apparaissent qu'à la sortie ([`TempoMap::to_samples`], arrondi au plus proche) et les ticks MIDI
//! aussi ([`TempoMap::to_ticks`]).
//!
//! Le tempo est un **rationnel** de noires par minute ([`Tempo`]) : 140 est exact, 133,33… s'écrit 400/3. Tout le calcul
//! de conversion se fait en entiers (rationnels 128 bits), sans flottants : le résultat est identique sur toutes les
//! plateformes. Un dépassement d'entier est signalé par `None`, jamais ignoré.
//!
//! Limites actuelles : tempos par paliers (pas de rampes), chiffrages qui changent seulement en début de mesure,
//! positions non négatives.

use std::cmp::Ordering;

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Rationnel positif interne (calculs intermédiaires). Toujours réduit, dénominateur non nul.
#[derive(Clone, Copy, Debug)]
struct Rat {
    n: u128,
    d: u128,
}

impl Rat {
    fn new(n: u128, d: u128) -> Option<Rat> {
        if d == 0 {
            return None;
        }
        let g = gcd(n, d).max(1);
        Some(Rat { n: n / g, d: d / g })
    }
    fn int(n: u128) -> Rat {
        Rat { n, d: 1 }
    }
    fn add(self, o: Rat) -> Option<Rat> {
        Rat::new(self.n.checked_mul(o.d)?.checked_add(o.n.checked_mul(self.d)?)?, self.d.checked_mul(o.d)?)
    }
    fn sub(self, o: Rat) -> Option<Rat> {
        Rat::new(self.n.checked_mul(o.d)?.checked_sub(o.n.checked_mul(self.d)?)?, self.d.checked_mul(o.d)?)
    }
    fn mul(self, o: Rat) -> Option<Rat> {
        // Réduction croisée d'abord : limite les dépassements.
        let (g1, g2) = (gcd(self.n, o.d).max(1), gcd(o.n, self.d).max(1));
        Rat::new((self.n / g1).checked_mul(o.n / g2)?, (self.d / g2).checked_mul(o.d / g1)?)
    }
    fn div(self, o: Rat) -> Option<Rat> {
        if o.n == 0 {
            return None;
        }
        self.mul(Rat { n: o.d, d: o.n })
    }
    fn cmp(self, o: Rat) -> Option<Ordering> {
        Some(self.n.checked_mul(o.d)?.cmp(&o.n.checked_mul(self.d)?))
    }
    /// Entier le plus proche (moitié vers le haut).
    fn round(self) -> u128 {
        let (q, r) = (self.n / self.d, self.n % self.d);
        if r >= self.d - r {
            q + 1
        } else {
            q
        }
    }
    fn frac(self) -> Option<Frac> {
        Some(Frac { n: u64::try_from(self.n).ok()?, d: u64::try_from(self.d).ok()? })
    }
}

/// Fraction exacte de ronde, non négative, toujours réduite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Frac {
    n: u64,
    d: u64,
}

impl Frac {
    pub const ZERO: Frac = Frac { n: 0, d: 1 };
    /// Une ronde.
    pub const WHOLE: Frac = Frac { n: 1, d: 1 };

    /// `n/d` réduite ; `None` si `d` est nul.
    pub fn new(n: u64, d: u64) -> Option<Frac> {
        Rat::new(n as u128, d as u128)?.frac()
    }
    pub fn num(&self) -> u64 {
        self.n
    }
    pub fn den(&self) -> u64 {
        self.d
    }
    fn rat(self) -> Rat {
        Rat { n: self.n as u128, d: self.d as u128 }
    }
    pub fn checked_add(self, o: Frac) -> Option<Frac> {
        self.rat().add(o.rat())?.frac()
    }
    /// `None` si le résultat serait négatif.
    pub fn checked_sub(self, o: Frac) -> Option<Frac> {
        self.rat().sub(o.rat())?.frac()
    }
    /// Multiple de `grid` le plus proche (moitié vers le haut) ; `None` si `grid` est nul ou en cas de dépassement.
    pub fn snap_to(self, grid: Frac) -> Option<Frac> {
        if grid.n == 0 {
            return None;
        }
        let k = self.rat().div(grid.rat())?.round();
        grid.rat().mul(Rat::int(k))?.frac()
    }
    /// Valeur approchée (affichage, interpolation ; jamais pour comparer).
    pub fn as_f64(&self) -> f64 {
        self.n as f64 / self.d as f64
    }
    pub fn checked_mul_int(self, k: u64) -> Option<Frac> {
        self.rat().mul(Rat::int(k as u128))?.frac()
    }
}

impl PartialOrd for Frac {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Frac {
    fn cmp(&self, other: &Self) -> Ordering {
        // u64 × u64 tient toujours dans un u128.
        (self.n as u128 * other.d as u128).cmp(&(other.n as u128 * self.d as u128))
    }
}

/// Tempo en noires par minute, rationnel `num/den`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tempo {
    num: u32,
    den: u32,
}

impl Tempo {
    pub fn new(num: u32, den: u32) -> Option<Tempo> {
        (num != 0 && den != 0).then(|| {
            let g = gcd(num as u128, den as u128) as u32;
            Tempo { num: num / g, den: den / g }
        })
    }
    /// Tempo entier en noires par minute.
    pub fn bpm(bpm: u32) -> Option<Tempo> {
        Tempo::new(bpm, 1)
    }
    pub fn num(&self) -> u32 {
        self.num
    }
    pub fn den(&self) -> u32 {
        self.den
    }
    pub fn as_f64(&self) -> f64 {
        self.num as f64 / self.den as f64
    }
    /// Échantillons par ronde : `4 noires × 60 s ÷ tempo × fréquence`.
    fn samples_per_whole(&self, sample_rate: u32) -> Option<Rat> {
        Rat::new(sample_rate as u128 * 240 * self.den as u128, self.num as u128)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TempoError {
    /// Le tempo initial (position 0) ne peut pas être supprimé.
    CannotRemoveFirst,
    NotFound,
}

/// Carte des tempos : un tempo par palier, le premier à la position 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempoMap {
    /// Triés par position strictement croissante ; `[0].0 == Frac::ZERO`.
    changes: Vec<(Frac, Tempo)>,
}

impl TempoMap {
    pub fn new(initial: Tempo) -> TempoMap {
        TempoMap { changes: vec![(Frac::ZERO, initial)] }
    }

    pub fn changes(&self) -> &[(Frac, Tempo)] {
        &self.changes
    }

    /// Pose un tempo à `at` (remplace celui qui s'y trouve déjà).
    pub fn set_tempo(&mut self, at: Frac, tempo: Tempo) {
        match self.changes.binary_search_by(|(p, _)| p.cmp(&at)) {
            Ok(i) => self.changes[i].1 = tempo,
            Err(i) => self.changes.insert(i, (at, tempo)),
        }
    }

    pub fn remove_tempo(&mut self, at: Frac) -> Result<(), TempoError> {
        if at == Frac::ZERO {
            return Err(TempoError::CannotRemoveFirst);
        }
        let i = self.changes.binary_search_by(|(p, _)| p.cmp(&at)).map_err(|_| TempoError::NotFound)?;
        self.changes.remove(i);
        Ok(())
    }

    pub fn tempo_at(&self, pos: Frac) -> Tempo {
        let i = self.changes.partition_point(|(p, _)| *p <= pos);
        self.changes[i - 1].1
    }

    /// Position musicale → échantillon (arrondi au plus proche). `None` si dépassement d'entier.
    pub fn to_samples(&self, pos: Frac, sample_rate: u32) -> Option<u64> {
        let mut acc = Rat::int(0);
        for (i, (start, tempo)) in self.changes.iter().enumerate() {
            let spw = tempo.samples_per_whole(sample_rate)?;
            let end = self.changes.get(i + 1).map(|c| c.0);
            match end {
                Some(e) if pos >= e => acc = acc.add(e.checked_sub(*start)?.rat().mul(spw)?)?,
                _ => return u64::try_from(acc.add(pos.checked_sub(*start)?.rat().mul(spw)?)?.round()).ok(),
            }
        }
        None
    }

    /// Échantillon → position musicale **exacte** (le rationnel n'est pas arrondi). `None` si dépassement.
    pub fn to_frac(&self, sample: u64, sample_rate: u32) -> Option<Frac> {
        let s = Rat::int(sample as u128);
        let mut acc = Rat::int(0);
        for (i, (start, tempo)) in self.changes.iter().enumerate() {
            let spw = tempo.samples_per_whole(sample_rate)?;
            if let Some(next) = self.changes.get(i + 1) {
                let end = acc.add(next.0.checked_sub(*start)?.rat().mul(spw)?)?;
                if s.cmp(end)? != Ordering::Less {
                    acc = end;
                    continue;
                }
            }
            return start.rat().add(s.sub(acc)?.div(spw)?)?.frac();
        }
        None
    }

    /// Ticks MIDI (`ppq` ticks par noire), arrondis au plus proche.
    pub fn to_ticks(pos: Frac, ppq: u32) -> Option<u64> {
        u64::try_from(pos.rat().mul(Rat::int(4 * ppq as u128))?.round()).ok()
    }

    pub fn from_ticks(ticks: u64, ppq: u32) -> Option<Frac> {
        Frac::new(ticks, 4 * ppq as u64)
    }

    /// Aimante un échantillon sur la grille `grid` (fraction de ronde) la plus proche, dans le temps musical (donc
    /// correct à travers les changements de tempo). Retourne l'échantillon aimanté.
    pub fn snap(&self, sample: u64, grid: Frac, sample_rate: u32) -> Option<u64> {
        let pos = self.to_frac(sample, sample_rate)?.rat();
        let k = pos.div(grid.rat())?.round();
        let on_grid = grid.rat().mul(Rat::int(k))?.frac()?;
        self.to_samples(on_grid, sample_rate)
    }
}

/// Chiffrage : `num` temps de `1/den` de ronde. `den` doit être une puissance de deux.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meter {
    num: u32,
    den: u32,
}

impl Meter {
    pub fn new(num: u32, den: u32) -> Option<Meter> {
        (num != 0 && den.is_power_of_two()).then_some(Meter { num, den })
    }
    pub fn num(&self) -> u32 {
        self.num
    }
    pub fn den(&self) -> u32 {
        self.den
    }
    /// Longueur d'une mesure en rondes.
    pub fn bar_len(&self) -> Frac {
        Frac::new(self.num as u64, self.den as u64).expect("den non nul")
    }
    pub fn beat_len(&self) -> Frac {
        Frac::new(1, self.den as u64).expect("den non nul")
    }
}

/// Carte des chiffrages : un chiffrage par palier, à des numéros de mesure (la première mesure est la 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeterMap {
    /// Triés par numéro de mesure strictement croissant ; `[0].0 == 0`.
    changes: Vec<(u32, Meter)>,
}

impl MeterMap {
    pub fn new(initial: Meter) -> MeterMap {
        MeterMap { changes: vec![(0, initial)] }
    }

    pub fn changes(&self) -> &[(u32, Meter)] {
        &self.changes
    }

    /// Pose un chiffrage à partir de la mesure `bar` (remplace celui qui s'y trouve).
    pub fn set_meter(&mut self, bar: u32, meter: Meter) {
        match self.changes.binary_search_by(|(b, _)| b.cmp(&bar)) {
            Ok(i) => self.changes[i].1 = meter,
            Err(i) => self.changes.insert(i, (bar, meter)),
        }
    }

    /// Retire le changement de chiffrage posé à la mesure `bar` (le premier ne se retire pas). `false` : rien à retirer.
    pub fn remove_meter(&mut self, bar: u32) -> bool {
        match self.changes.binary_search_by(|(b, _)| b.cmp(&bar)) {
            Ok(i) if i > 0 => {
                self.changes.remove(i);
                true
            }
            _ => false,
        }
    }

    pub fn meter_at_bar(&self, bar: u32) -> Meter {
        let i = self.changes.partition_point(|(b, _)| *b <= bar);
        self.changes[i - 1].1
    }

    /// Position du début de la mesure `bar`.
    pub fn bar_start(&self, bar: u32) -> Option<Frac> {
        let mut pos = Frac::ZERO;
        for (i, (b, m)) in self.changes.iter().enumerate() {
            let next = self.changes.get(i + 1).map_or(bar, |c| c.0.min(bar));
            if next > *b {
                pos = pos.checked_add(m.bar_len().checked_mul_int((next - *b) as u64)?)?;
            }
            if next == bar {
                break;
            }
        }
        Some(pos)
    }

    /// (numéro de mesure, position dans la mesure) d'une position absolue.
    pub fn locate(&self, pos: Frac) -> Option<(u32, Frac)> {
        let mut start = Frac::ZERO;
        for (i, (b, m)) in self.changes.iter().enumerate() {
            let len = m.bar_len();
            if let Some(next) = self.changes.get(i + 1) {
                let end = start.checked_add(len.checked_mul_int((next.0 - *b) as u64)?)?;
                if pos >= end {
                    start = end;
                    continue;
                }
            }
            // La mesure est dans ce palier : combien de mesures entières depuis `start` ?
            let rel = pos.checked_sub(start)?;
            let q = rel.rat().div(len.rat())?;
            let k = u64::try_from(q.n / q.d).ok()?; // mesures entières écoulées
            let bar = *b + u32::try_from(k).ok()?;
            let within = rel.checked_sub(len.checked_mul_int(k)?)?;
            return Some((bar, within));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn f(n: u64, d: u64) -> Frac {
        Frac::new(n, d).unwrap()
    }
    fn bpm(b: u32) -> Tempo {
        Tempo::bpm(b).unwrap()
    }

    #[test]
    fn fractions_are_reduced_and_exact() {
        assert_eq!(f(2, 8), f(1, 4));
        assert_eq!((f(1, 4).num(), f(1, 4).den()), (1, 4));
        assert_eq!(f(0, 7), Frac::ZERO);
        assert_eq!(Frac::new(1, 0), None);
        // Un triolet de croches = une noire, sans tolérance.
        assert_eq!(f(1, 12).checked_mul_int(3), Some(f(1, 4)));
        assert_eq!(f(1, 3).checked_add(f(1, 6)), Some(f(1, 2)));
        assert_eq!(f(1, 4).checked_sub(f(1, 3)), None);
        assert!(f(1, 3) < f(1, 2) && f(2, 3) > f(3, 5) && f(2, 4) == f(1, 2));
        assert_eq!(f(u64::MAX, 1).checked_add(f(1, 1)), None);
        assert_eq!(f(5, 16).snap_to(f(1, 8)), Some(f(3, 8))); // 2,5 croches : moitié vers le haut
        assert_eq!(f(1, 5).snap_to(f(1, 16)), Some(f(3, 16)));
        assert_eq!(f(1, 5).snap_to(Frac::ZERO), None);
    }

    #[test]
    fn constant_tempo_conversions() {
        let m = TempoMap::new(bpm(120)); // une ronde = 2 s
        assert_eq!(m.to_samples(Frac::WHOLE, SR), Some(96_000));
        assert_eq!(m.to_samples(f(1, 4), SR), Some(24_000));
        assert_eq!(m.to_samples(Frac::ZERO, SR), Some(0));
        assert_eq!(m.to_frac(24_000, SR), Some(f(1, 4)));
        assert_eq!(m.to_frac(12_000, SR), Some(f(1, 8)));
        // 133,33… bpm (400/3) : exact en rationnel.
        let m = TempoMap::new(Tempo::new(400, 3).unwrap());
        assert_eq!(m.to_samples(f(1, 4), SR), Some(21_600));
    }

    #[test]
    fn tempo_changes_accumulate_exactly() {
        let mut m = TempoMap::new(bpm(120));
        m.set_tempo(Frac::WHOLE, bpm(60)); // dès la 2e ronde : 4 s par ronde
        assert_eq!(m.to_samples(Frac::WHOLE, SR), Some(96_000));
        assert_eq!(m.to_samples(f(3, 2), SR), Some(96_000 + 96_000));
        assert_eq!(m.to_samples(f(2, 1), SR), Some(96_000 + 192_000));
        assert_eq!(m.tempo_at(f(1, 2)), bpm(120));
        assert_eq!(m.tempo_at(Frac::WHOLE), bpm(60));
        assert_eq!(m.to_frac(96_000 + 48_000, SR), Some(f(5, 4)));
        // Remplacement au même endroit, suppression, premier palier protégé.
        m.set_tempo(Frac::WHOLE, bpm(240));
        assert_eq!(m.changes().len(), 2);
        assert_eq!(m.to_samples(f(2, 1), SR), Some(96_000 + 48_000));
        assert_eq!(m.remove_tempo(Frac::ZERO), Err(TempoError::CannotRemoveFirst));
        assert_eq!(m.remove_tempo(f(1, 3)), Err(TempoError::NotFound));
        m.remove_tempo(Frac::WHOLE).unwrap();
        assert_eq!(m.to_samples(f(2, 1), SR), Some(192_000));
    }

    #[test]
    fn samples_round_trip_through_musical_time() {
        let mut m = TempoMap::new(Tempo::new(1_000, 7).unwrap());
        m.set_tempo(f(3, 8), Tempo::new(97, 1).unwrap());
        m.set_tempo(f(5, 3), bpm(173));
        for rate in [44_100, 48_000, 96_000] {
            for s in (0..2_000_000u64).step_by(9_973).chain([0, 1, 2, 3, 1_999_999]) {
                let p = m.to_frac(s, rate).unwrap();
                assert_eq!(m.to_samples(p, rate), Some(s), "{s} à {rate} Hz");
            }
        }
    }

    #[test]
    fn musical_order_is_sample_order() {
        let mut m = TempoMap::new(bpm(90));
        m.set_tempo(f(1, 2), bpm(200));
        let mut last = 0;
        for k in 0..400u64 {
            let s = m.to_samples(f(k, 64), SR).unwrap();
            assert!(s >= last);
            last = s;
        }
    }

    #[test]
    fn midi_ticks() {
        assert_eq!(TempoMap::to_ticks(f(1, 4), 960), Some(960));
        assert_eq!(TempoMap::to_ticks(f(1, 12), 960), Some(320)); // croche de triolet
        assert_eq!(TempoMap::to_ticks(f(1, 64), 960), Some(60));
        assert_eq!(TempoMap::from_ticks(960, 960), Some(f(1, 4)));
        // Valeur qui ne tombe pas juste : arrondi au plus proche.
        assert_eq!(TempoMap::to_ticks(f(1, 7), 960), Some(549)); // 548,57
    }

    #[test]
    fn snapping_follows_the_tempo_map() {
        let mut m = TempoMap::new(bpm(120));
        m.set_tempo(Frac::WHOLE, bpm(60));
        let eighth = f(1, 8);
        // Croche à 120 : 12 000 échantillons.
        assert_eq!(m.snap(11_000, eighth, SR), Some(12_000));
        assert_eq!(m.snap(5_000, eighth, SR), Some(0));
        // Après le changement : croche = 24 000 échantillons.
        assert_eq!(m.snap(96_000 + 25_000, eighth, SR), Some(96_000 + 24_000));
        assert_eq!(m.snap(96_000 + 37_000, eighth, SR), Some(96_000 + 48_000));
    }

    #[test]
    fn overflow_is_reported_not_wrapped() {
        let m = TempoMap::new(bpm(120));
        assert_eq!(m.to_samples(Frac::new(u64::MAX, 1).unwrap(), u32::MAX), None);
        assert!(Tempo::new(0, 1).is_none() && Tempo::new(1, 0).is_none());
        assert_eq!(Tempo::new(240, 2), Tempo::bpm(120));
    }

    #[test]
    fn meters_and_bars() {
        let mut mm = MeterMap::new(Meter::new(4, 4).unwrap());
        assert_eq!(mm.bar_start(0), Some(Frac::ZERO));
        assert_eq!(mm.bar_start(3), Some(f(3, 1)));
        mm.set_meter(2, Meter::new(3, 4).unwrap()); // mesures 0,1 en 4/4 ; ensuite 3/4
        mm.set_meter(4, Meter::new(7, 8).unwrap());
        assert_eq!(mm.bar_start(2), Some(f(2, 1)));
        assert_eq!(mm.bar_start(3), Some(f(11, 4)));
        assert_eq!(mm.bar_start(4), Some(f(7, 2)));
        assert_eq!(mm.bar_start(5), Some(f(7, 2).checked_add(f(7, 8)).unwrap()));
        assert_eq!(mm.meter_at_bar(1), Meter::new(4, 4).unwrap());
        assert_eq!(mm.meter_at_bar(9), Meter::new(7, 8).unwrap());
        for bar in 0..12u32 {
            let s = mm.bar_start(bar).unwrap();
            assert_eq!(mm.locate(s), Some((bar, Frac::ZERO)), "début de mesure {bar}");
            let inside = s.checked_add(f(1, 8)).unwrap();
            assert_eq!(mm.locate(inside), Some((bar, f(1, 8))), "dans la mesure {bar}");
        }
        assert!(Meter::new(4, 3).is_none() && Meter::new(0, 4).is_none());
        assert_eq!(Meter::new(6, 8).unwrap().bar_len(), f(3, 4));
        assert_eq!(Meter::new(6, 8).unwrap().beat_len(), f(1, 8));
    }
}
