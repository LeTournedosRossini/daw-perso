//! Temps musical vu par le moteur : ce qu'un plugin peut apprendre du transport (position en temps, tempo, chiffrage, boucle).
//!
//! Le moteur ne travaille qu'en échantillons ; `TimeMap` est la carte des tempos et des chiffrages **déjà convertie** en
//! échantillons par la couche qui la connaît (la session). Interrogation sans allocation ni verrou. Calculs en `f64` (`+ − × /`) :
//! cette information va aux plugins, elle ne change pas ce que le moteur produit lui-même.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Un palier de tempo : à partir de `start_sample` (qui correspond à `start_beat`, en noires), le tempo vaut `bpm` noires par minute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoSeg {
    pub start_sample: u64,
    pub start_beat: f64,
    pub bpm: f64,
}

/// Un chiffrage : à partir de `start_beat` (noires), qui est le début de la mesure numéro `start_bar` (à partir de 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterSeg {
    pub start_beat: f64,
    pub start_bar: u32,
    pub num: u16,
    pub den: u16,
}

/// Carte des tempos et des chiffrages en échantillons. Jamais vide : le premier palier commence à 0.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeMap {
    pub tempo: Vec<TempoSeg>,
    pub meters: Vec<MeterSeg>,
}

impl Default for TimeMap {
    /// 120 noires par minute, 4/4 (comme une session neuve).
    fn default() -> Self {
        TimeMap { tempo: vec![TempoSeg { start_sample: 0, start_beat: 0.0, bpm: 120.0 }], meters: vec![MeterSeg { start_beat: 0.0, start_bar: 1, num: 4, den: 4 }] }
    }
}

/// Ce que le transport dit à un plugin pour un bloc (ou un morceau de bloc). Positions en noires et en secondes de la ligne de temps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportInfo {
    pub playing: bool,
    pub beats: f64,
    pub seconds: f64,
    pub tempo: f64,
    pub tsig_num: u16,
    pub tsig_den: u16,
    /// Début de la mesure en cours (noires) et son numéro (à partir de 1).
    pub bar_start_beats: f64,
    pub bar_number: i32,
    pub loop_active: bool,
    pub loop_start_beats: f64,
    pub loop_end_beats: f64,
    pub loop_start_seconds: f64,
    pub loop_end_seconds: f64,
}

impl TransportInfo {
    /// La même information `samples` échantillons plus loin, à tempo constant (suffisant à l'intérieur d'un bloc).
    pub fn advanced(&self, samples: u32, sample_rate: u32) -> TransportInfo {
        if samples == 0 || !self.playing {
            return *self;
        }
        let secs = f64::from(samples) / f64::from(sample_rate.max(1));
        TransportInfo { beats: self.beats + secs * self.tempo / 60.0, seconds: self.seconds + secs, ..*self }
    }
}

impl TimeMap {
    fn seg_at(&self, sample: u64) -> &TempoSeg {
        let i = self.tempo.partition_point(|s| s.start_sample <= sample).saturating_sub(1);
        &self.tempo[i.min(self.tempo.len() - 1)]
    }

    /// Position en noires de l'échantillon `sample`.
    pub fn beats_at(&self, sample: u64, sample_rate: u32) -> f64 {
        let s = self.seg_at(sample);
        s.start_beat + (sample - s.start_sample) as f64 / f64::from(sample_rate.max(1)) * s.bpm / 60.0
    }

    /// Le transport à l'échantillon `sample`.
    pub fn info_at(&self, sample: u64, sample_rate: u32, playing: bool, loop_range: Option<(u64, u64)>) -> TransportInfo {
        let sr = f64::from(sample_rate.max(1));
        let seg = self.seg_at(sample);
        let beats = self.beats_at(sample, sample_rate);
        let mi = self.meters.partition_point(|m| m.start_beat <= beats).saturating_sub(1);
        let m = self.meters[mi.min(self.meters.len() - 1)];
        // Une mesure fait `num × 4 / den` noires.
        let bar_len = f64::from(m.num) * 4.0 / f64::from(m.den.max(1));
        let bars_in = ((beats - m.start_beat) / bar_len).floor().max(0.0);
        let (ls, le) = loop_range.unwrap_or((0, 0));
        TransportInfo {
            playing,
            beats,
            seconds: sample as f64 / sr,
            tempo: seg.bpm,
            tsig_num: m.num,
            tsig_den: m.den,
            bar_start_beats: m.start_beat + bars_in * bar_len,
            bar_number: (m.start_bar as f64 + bars_in) as i32,
            loop_active: loop_range.is_some(),
            loop_start_beats: if loop_range.is_some() { self.beats_at(ls, sample_rate) } else { 0.0 },
            loop_end_beats: if loop_range.is_some() { self.beats_at(le, sample_rate) } else { 0.0 },
            loop_start_seconds: ls as f64 / sr,
            loop_end_seconds: le as f64 / sr,
        }
    }
}

/// Ce que partagent tous les nœuds d'un graphe compilé : la carte (immuable) et la boucle du transport (écrite par le fil audio avant
/// chaque bloc, lue par les nœuds, y compris sur les fils de calcul).
#[derive(Debug)]
pub struct TimeShared {
    pub map: Arc<TimeMap>,
    loop_on: AtomicBool,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
}

impl TimeShared {
    pub fn new(map: Arc<TimeMap>) -> TimeShared {
        TimeShared { map, loop_on: AtomicBool::new(false), loop_start: AtomicU64::new(0), loop_end: AtomicU64::new(0) }
    }

    pub fn set_loop(&self, range: Option<(u64, u64)>) {
        let (s, e) = range.unwrap_or((0, 0));
        self.loop_start.store(s, Ordering::Relaxed);
        self.loop_end.store(e, Ordering::Relaxed);
        self.loop_on.store(range.is_some(), Ordering::Release);
    }

    pub fn loop_range(&self) -> Option<(u64, u64)> {
        self.loop_on.load(Ordering::Acquire).then(|| (self.loop_start.load(Ordering::Relaxed), self.loop_end.load(Ordering::Relaxed)))
    }

    pub fn info_at(&self, sample: u64, sample_rate: u32, playing: bool) -> TransportInfo {
        self.map.info_at(sample, sample_rate, playing, self.loop_range())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> TimeMap {
        // 120 bpm pendant 2 s (4 noires), puis 60 bpm ; 4/4 puis 3/4 à partir de la mesure 3 (noire 8).
        TimeMap {
            tempo: vec![TempoSeg { start_sample: 0, start_beat: 0.0, bpm: 120.0 }, TempoSeg { start_sample: 96_000, start_beat: 4.0, bpm: 60.0 }],
            meters: vec![MeterSeg { start_beat: 0.0, start_bar: 1, num: 4, den: 4 }, MeterSeg { start_beat: 8.0, start_bar: 3, num: 3, den: 4 }],
        }
    }

    #[test]
    fn beats_follow_the_tempo_steps() {
        let m = map();
        assert_eq!(m.beats_at(0, 48_000), 0.0);
        assert_eq!(m.beats_at(48_000, 48_000), 2.0);
        assert_eq!(m.beats_at(96_000, 48_000), 4.0);
        assert_eq!(m.beats_at(96_000 + 48_000, 48_000), 5.0, "60 bpm : une noire par seconde");
    }

    #[test]
    fn bar_and_signature_come_with_the_position() {
        let m = map();
        let a = m.info_at(48_000, 48_000, true, None);
        assert_eq!((a.tsig_num, a.tsig_den, a.bar_number, a.bar_start_beats), (4, 4, 1, 0.0));
        // Noire 9,5 : en 3/4 depuis la noire 8 → mesure 3 commencée à 8, puis la 4ᵉ (mesure 4) commence à 11.
        let sample = 96_000 + 5 * 48_000 + 24_000; // noire 4 + 5,5 = 9,5
        let b = m.info_at(sample, 48_000, true, None);
        assert_eq!((b.tsig_num, b.tsig_den, b.bar_number, b.bar_start_beats), (3, 4, 3, 8.0));
        assert!(!b.loop_active);
    }

    #[test]
    fn the_loop_is_reported_in_beats_and_seconds() {
        let m = map();
        let i = m.info_at(0, 48_000, true, Some((48_000, 96_000)));
        assert!(i.loop_active);
        assert_eq!((i.loop_start_beats, i.loop_end_beats, i.loop_start_seconds, i.loop_end_seconds), (2.0, 4.0, 1.0, 2.0));
    }

    #[test]
    fn advancing_inside_a_block_follows_the_tempo() {
        let i = map().info_at(0, 48_000, true, None).advanced(24_000, 48_000);
        assert_eq!((i.beats, i.seconds), (1.0, 0.5));
        assert_eq!(map().info_at(0, 48_000, false, None).advanced(24_000, 48_000).beats, 0.0, "à l'arrêt, rien n'avance");
    }

    #[test]
    fn shared_loop_round_trips() {
        let t = TimeShared::new(Arc::new(TimeMap::default()));
        assert_eq!(t.loop_range(), None);
        t.set_loop(Some((10, 20)));
        assert_eq!(t.loop_range(), Some((10, 20)));
        t.set_loop(None);
        assert_eq!(t.loop_range(), None);
    }
}
