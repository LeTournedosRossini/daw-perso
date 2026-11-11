//! Détecteur de débuts de son (attaques) pour poser automatiquement des marqueurs d'étirement sur de la matière percussive ou rythmique.
//!
//! Méthode volontairement simple et déterministe (aucun flottant de bibliothèque non exact n'est requis : `ln` seulement) :
//! 1. mélange mono, puis **différence première** (souligne les transitoires, atténue les graves tenus) ;
//! 2. énergie par trames de 1 024 échantillons (saut de 256), en décibels ;
//! 3. **nouveauté** = montée de l'énergie par rapport à la moyenne des 4 trames précédentes ;
//! 4. pics de nouveauté au-dessus d'un seuil (6 à 18 dB selon la sensibilité), maximum local sur ±3 trames, au moins 60 ms entre deux attaques ;
//! 5. position **affinée** à l'échantillon : on remonte du pic de l'enveloppe jusqu'au moment où elle dépasse encore à peine 20 % du pic.

/// Trame d'analyse, saut et écart minimal entre deux attaques (secondes).
const FRAME: usize = 1024;
const HOP: usize = 256;
const MIN_GAP: f64 = 0.060;

/// `sensitivity` de 0 (peu d'attaques, seulement les franches) à 1 (beaucoup) ; rend les positions en échantillons, croissantes.
pub fn detect(channels: &[&[f32]], rate: f64, sensitivity: f64) -> Vec<usize> {
    let Some(first) = channels.first() else { return Vec::new() };
    let n = channels.iter().map(|c| c.len()).min().unwrap_or(0).min(first.len());
    if n < FRAME * 2 {
        return Vec::new();
    }
    let mono: Vec<f32> = (0..n).map(|i| channels.iter().map(|c| c[i]).sum::<f32>() / channels.len() as f32).collect();
    let diff: Vec<f32> = std::iter::once(0.0).chain(mono.windows(2).map(|w| w[1] - w[0])).collect();
    let frames = (n - FRAME) / HOP + 1;
    let db: Vec<f64> = (0..frames)
        .map(|k| {
            let e: f64 = diff[k * HOP..k * HOP + FRAME].iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / FRAME as f64;
            10.0 * (e + 1e-12).log10()
        })
        .collect();
    // Plancher d'énergie : en dessous, rien n'est une attaque (silence, bruit de fond).
    let top = db.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let floor = top - 50.0;
    let nov: Vec<f64> = (0..frames)
        .map(|k| {
            if k < 4 || db[k] < floor {
                return 0.0;
            }
            let prev = db[k - 4..k].iter().sum::<f64>() / 4.0;
            (db[k] - prev).max(0.0)
        })
        .collect();
    let threshold = 18.0 - 12.0 * sensitivity.clamp(0.0, 1.0);
    let min_gap = (MIN_GAP * rate) as usize;
    let mut out: Vec<usize> = Vec::new();
    for k in 0..frames {
        if nov[k] < threshold {
            continue;
        }
        let (a, b) = (k.saturating_sub(3), (k + 3).min(frames - 1));
        if nov[a..=b].iter().any(|&v| v > nov[k]) || (k > 0 && nov[a..k].iter().any(|&v| v == nov[k])) {
            continue; // pas le maximum local (à égalité, le premier gagne)
        }
        let pos = refine(&mono, k * HOP, rate);
        if out.last().is_none_or(|&p| pos >= p + min_gap) {
            out.push(pos);
        }
    }
    out
}

/// Cherche autour de la trame `at` le pic de l'enveloppe (valeur absolue lissée sur 32 échantillons), puis recule jusqu'à 20 % de ce pic.
fn refine(x: &[f32], at: usize, rate: f64) -> usize {
    let span = (0.012 * rate) as usize;
    let (lo, hi) = (at.saturating_sub(FRAME / 2), (at + FRAME + span).min(x.len().saturating_sub(1)));
    let smooth = 32usize;
    let env = |i: usize| -> f32 {
        let a = i.saturating_sub(smooth / 2);
        let b = (i + smooth / 2).min(x.len() - 1);
        x[a..=b].iter().map(|v| v.abs()).sum::<f32>() / (b - a + 1) as f32
    };
    let mut peak = (lo, 0.0f32);
    for i in (lo..=hi).step_by(8) {
        let e = env(i);
        if e > peak.1 {
            peak = (i, e);
        }
    }
    let limit = peak.1 * 0.2;
    let mut i = peak.0;
    while i > lo && env(i) > limit {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    /// Bruit déterministe.
    fn noise(seed: &mut u64) -> f32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*seed >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    }

    /// 4 s de très faible bruit de fond avec des « coups » (bruit décroissant de 80 ms) aux instants donnés.
    fn hits(at: &[f64]) -> Vec<f32> {
        let mut seed = 7u64;
        let mut x: Vec<f32> = (0..(4.0 * RATE) as usize).map(|_| noise(&mut seed) * 0.0005).collect();
        for &t in at {
            let s = (t * RATE) as usize;
            for k in 0..(0.08 * RATE) as usize {
                if s + k < x.len() {
                    x[s + k] += noise(&mut seed) * 0.8 * (-(k as f32) / 800.0).exp();
                }
            }
        }
        x
    }

    #[test]
    fn finds_hits_within_a_few_milliseconds() {
        let times = [0.5, 1.0, 1.5, 2.2, 3.1];
        let x = hits(&times);
        let found = detect(&[&x], RATE, 0.5);
        assert_eq!(found.len(), times.len(), "{found:?}");
        for (f, t) in found.iter().zip(times) {
            let err = (*f as f64 / RATE - t).abs();
            assert!(err < 0.008, "attaque à {t} s trouvée à {} s", *f as f64 / RATE);
        }
    }

    #[test]
    fn silence_and_a_steady_tone_have_no_attack() {
        assert!(detect(&[&vec![0.0f32; 96_000]], RATE, 1.0).is_empty());
        let tone: Vec<f32> = (0..96_000).map(|i| 0.5 * (i as f32 * std::f32::consts::TAU * 220.0 / RATE as f32).sin()).collect();
        assert!(detect(&[&tone], RATE, 1.0).is_empty());
    }

    #[test]
    fn two_hits_closer_than_60_ms_count_once_and_short_input_is_safe() {
        let x = hits(&[1.0, 1.03]);
        assert_eq!(detect(&[&x], RATE, 0.5).len(), 1);
        assert!(detect(&[&[0.1f32; 100]], RATE, 0.5).is_empty());
        assert!(detect(&[], RATE, 0.5).is_empty());
    }

    #[test]
    fn higher_sensitivity_never_finds_fewer_attacks() {
        let mut x = hits(&[0.5, 1.5, 2.5]);
        // Un coup beaucoup plus faible.
        let mut seed = 99u64;
        for k in 0..2000 {
            x[(3.2 * RATE) as usize + k] += noise(&mut seed) * 0.02 * (-(k as f32) / 500.0).exp();
        }
        let (low, high) = (detect(&[&x], RATE, 0.0).len(), detect(&[&x], RATE, 1.0).len());
        assert!(high >= low && low >= 3, "{low} {high}");
    }
}
