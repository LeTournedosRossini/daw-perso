//! Crêtes de forme d'onde : minimum et maximum par tranche d'échantillons, en plusieurs niveaux de détail.
//!
//! Le calcul est lent (il lit tout le fichier) : il se fait **hors du fil audio**, sur un fil de travail, une fois
//! par fichier. L'interface interroge ensuite [`Peaks::query`] pour une fenêtre donnée : le coût ne dépend que du
//! nombre de colonnes demandées, pas de la durée du fichier.

use daw_engine::AudioData;

/// Échantillons par crête du niveau le plus fin ; chaque niveau suivant en regroupe `FACTOR` fois plus.
pub const BASE: usize = 64;
pub const FACTOR: usize = 8;

#[derive(Debug, Clone, PartialEq)]
struct Level {
    /// Échantillons d'origine par crête.
    per_peak: usize,
    /// `min, max` entrelacés, un couple par crête et par canal : `[canal][2 * i]`, `[canal][2 * i + 1]`.
    data: Vec<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    frames: u64,
    channels: usize,
    levels: Vec<Level>,
}

impl Peaks {
    pub fn build(data: &AudioData) -> Peaks {
        let channels = data.channel_count();
        let frames = data.len();
        let mut base = Level { per_peak: BASE, data: Vec::with_capacity(channels) };
        for c in 0..channels {
            let mut v = Vec::with_capacity(2 * (frames as usize / BASE + 1));
            for chunk in data.channel(c).chunks(BASE) {
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for &x in chunk {
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
                v.push(lo);
                v.push(hi);
            }
            base.data.push(v);
        }
        let mut levels = vec![base];
        // Niveaux grossiers : tant que le précédent a plus d'une crête par `FACTOR`.
        while levels.last().map_or(0, |l| l.data[0].len() / 2) > FACTOR {
            let prev = levels.last().unwrap();
            let mut next = Level { per_peak: prev.per_peak * FACTOR, data: Vec::with_capacity(channels) };
            for c in 0..channels {
                let mut v = Vec::with_capacity(prev.data[c].len() / FACTOR + 2);
                for group in prev.data[c].chunks(2 * FACTOR) {
                    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                    for pair in group.chunks(2) {
                        lo = lo.min(pair[0]);
                        hi = hi.max(pair[1]);
                    }
                    v.push(lo);
                    v.push(hi);
                }
                next.data.push(v);
            }
            levels.push(next);
        }
        Peaks { frames, channels, levels }
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Niveaux de détail disponibles (échantillons par crête, du plus fin au plus grossier).
    pub fn resolutions(&self) -> Vec<usize> {
        self.levels.iter().map(|l| l.per_peak).collect()
    }

    /// `columns` couples `(min, max)` du canal `channel` pour les échantillons `[start, end[` : colonne `k` = la
    /// tranche `[start + k·(end-start)/columns, start + (k+1)·(end-start)/columns[`. Les crêtes déjà calculées sont
    /// utilisées au niveau le plus grossier qui reste plus fin qu'une colonne ; la valeur est donc **enveloppante**
    /// (jamais plus étroite que la vraie, au plus une crête de dépassement de chaque côté).
    pub fn query(&self, channel: usize, start: u64, end: u64, columns: usize) -> Vec<(f32, f32)> {
        if channel >= self.channels || columns == 0 || start >= end {
            return Vec::new();
        }
        let end = end.min(self.frames);
        if start >= end {
            return Vec::new();
        }
        let span = end - start;
        let per_col = (span / columns as u64).max(1) as usize;
        let level = self.levels.iter().rev().find(|l| l.per_peak <= per_col).unwrap_or(&self.levels[0]);
        let v = &level.data[channel];
        let n = v.len() / 2;
        (0..columns)
            .map(|k| {
                let a = start + span * k as u64 / columns as u64;
                let b = (start + span * (k as u64 + 1) / columns as u64).max(a + 1);
                let i0 = (a / level.per_peak as u64) as usize;
                let i1 = (((b - 1) / level.per_peak as u64) as usize).min(n.saturating_sub(1));
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for i in i0.min(i1)..=i1 {
                    lo = lo.min(v[2 * i]);
                    hi = hi.max(v[2 * i + 1]);
                }
                (lo, hi)
            })
            .collect()
    }
}
