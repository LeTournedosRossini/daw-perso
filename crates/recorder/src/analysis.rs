//! Analyses d'une prise : alignement d'impulsions (calibration de la latence) et intégrité d'une tonalité (coupures,
//! clics). Tout est hors temps réel et en `f64` quand la précision compte.

/// Positions des impulsions : premier échantillon dont la valeur absolue dépasse `threshold`, puis on saute
/// `min_gap` échantillons (pour ne pas compter deux fois le même événement).
pub fn find_impulses(x: &[f32], threshold: f32, min_gap: usize) -> Vec<usize> {
    let (mut out, mut i) = (Vec::new(), 0);
    while i < x.len() {
        if x[i].abs() > threshold {
            out.push(i);
            i += min_gap.max(1);
        } else {
            i += 1;
        }
    }
    out
}

/// Écarts entre impulsions attendues et trouvées.
#[derive(Debug, Clone, PartialEq)]
pub struct Alignment {
    /// Impulsions attendues pour lesquelles une impulsion a été trouvée à moins de `window` échantillons.
    pub matched: usize,
    /// Attendues mais absentes (coupure, volume trop bas).
    pub missing: usize,
    /// Trouvées sans correspondance (parasites, clics).
    pub extra: usize,
    /// Décalage moyen `trouvée - attendue`, en échantillons (positif : l'enregistrement arrive en retard).
    pub mean: f64,
    pub min: i64,
    pub max: i64,
}

/// Apparie chaque impulsion attendue avec la plus proche trouvée à moins de `window` échantillons.
pub fn alignment(expected: &[usize], found: &[usize], window: usize) -> Alignment {
    let mut used = vec![false; found.len()];
    let mut offsets: Vec<i64> = Vec::new();
    for &e in expected {
        let best = found.iter().enumerate().filter(|(i, &f)| !used[*i] && f.abs_diff(e) <= window).min_by_key(|(_, &f)| f.abs_diff(e));
        if let Some((i, &f)) = best {
            used[i] = true;
            offsets.push(f as i64 - e as i64);
        }
    }
    let matched = offsets.len();
    Alignment {
        matched,
        missing: expected.len() - matched,
        extra: found.len() - matched,
        mean: if matched == 0 { 0.0 } else { offsets.iter().sum::<i64>() as f64 / matched as f64 },
        min: offsets.iter().copied().min().unwrap_or(0),
        max: offsets.iter().copied().max().unwrap_or(0),
    }
}

/// Intégrité d'une tonalité pure enregistrée.
#[derive(Debug, Clone, PartialEq)]
pub struct ToneReport {
    /// Amplitude de la tonalité retrouvée.
    pub amplitude: f64,
    /// Écart quadratique moyen avec la meilleure sinusoïde, relatif à l'amplitude (en dB, ex. -90).
    pub residual_db: f64,
    /// Début (indice d'échantillon) de chaque accident : suite d'échantillons où l'écart dépasse le seuil.
    pub glitches: Vec<usize>,
}

/// Cherche les accidents dans un enregistrement d'une sinusoïde de fréquence `freq` : on ajuste amplitude et phase aux
/// moindres carrés (sur toute la prise), puis on signale les endroits où l'écart dépasse `tolerance × amplitude`.
/// Une coupure, un clic ou un saut de tampon laissent un écart bien plus grand que le bruit des convertisseurs.
pub fn tone_integrity(x: &[f32], sample_rate: u32, freq: f64, tolerance: f64) -> ToneReport {
    let w = std::f64::consts::TAU * freq / sample_rate as f64;
    // Normales du système x[n] ≈ a·sin(wn) + b·cos(wn).
    let (mut ss, mut cc, mut sc, mut xs, mut xc) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (n, &v) in x.iter().enumerate() {
        let (s, c) = (w * n as f64).sin_cos();
        ss += s * s;
        cc += c * c;
        sc += s * c;
        xs += v as f64 * s;
        xc += v as f64 * c;
    }
    let det = ss * cc - sc * sc;
    if det.abs() < 1e-9 {
        return ToneReport { amplitude: 0.0, residual_db: 0.0, glitches: Vec::new() };
    }
    let (a, b) = ((xs * cc - xc * sc) / det, (xc * ss - xs * sc) / det);
    let amplitude = a.hypot(b);
    let (mut sq, mut glitches, mut in_glitch) = (0.0f64, Vec::new(), false);
    for (n, &v) in x.iter().enumerate() {
        let (s, c) = (w * n as f64).sin_cos();
        let r = v as f64 - (a * s + b * c);
        sq += r * r;
        let bad = r.abs() > tolerance * amplitude;
        if bad && !in_glitch {
            glitches.push(n);
        }
        in_glitch = bad;
    }
    let rms = (sq / x.len().max(1) as f64).sqrt();
    ToneReport { amplitude, residual_db: if amplitude > 0.0 && rms > 0.0 { 20.0 * (rms / amplitude).log10() } else { -200.0 }, glitches }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(len: usize, rate: u32, freq: f64, amp: f64, phase: f64) -> Vec<f32> {
        (0..len).map(|n| (amp * (std::f64::consts::TAU * freq * n as f64 / rate as f64 + phase).sin()) as f32).collect()
    }

    #[test]
    fn finds_impulses_with_a_minimum_gap() {
        let mut x = vec![0.0f32; 1_000];
        for p in [10, 12, 300, 301, 302, 900] {
            x[p] = 0.9;
        }
        assert_eq!(find_impulses(&x, 0.5, 50), vec![10, 300, 900]);
        assert_eq!(find_impulses(&x, 0.95, 50), Vec::<usize>::new());
    }

    #[test]
    fn alignment_reports_offsets_missing_and_extra() {
        let expected = [100, 600, 1_100, 1_600];
        let found = [263, 763, 1_900, 1_263 + 600];
        // 100→263 (+163), 600→763 (+163), 1100→(rien proche), 1600→1863 (+263) ; 1900 : sans correspondance
        let a = alignment(&expected, &found, 300);
        assert_eq!((a.matched, a.missing, a.extra), (3, 1, 1));
        assert_eq!((a.min, a.max), (163, 263));
        assert!((a.mean - (163.0 + 163.0 + 263.0) / 3.0).abs() < 1e-9);
    }

    #[test]
    fn perfect_alignment_has_zero_spread() {
        let e: Vec<usize> = (0..10).map(|k| 500 + k * 2_000).collect();
        let f: Vec<usize> = e.iter().map(|x| x + 263).collect();
        let a = alignment(&e, &f, 1_000);
        assert_eq!((a.matched, a.missing, a.extra, a.min, a.max), (10, 0, 0, 263, 263));
        assert_eq!(a.mean, 263.0);
    }

    #[test]
    fn clean_tone_has_no_glitch_and_a_tiny_residual() {
        let x = tone(48_000, 48_000, 1_000.0, 0.5, 0.7);
        let r = tone_integrity(&x, 48_000, 1_000.0, 0.02);
        assert!(r.glitches.is_empty());
        assert!((r.amplitude - 0.5).abs() < 1e-4);
        assert!(r.residual_db < -100.0, "{}", r.residual_db);
    }

    #[test]
    fn dropout_click_and_sample_slip_are_each_detected() {
        let clean = tone(48_000, 48_000, 1_000.0, 0.5, 0.0);
        let mut x = clean.clone();
        x[10_000..10_005].fill(0.0); // coupure de 5 échantillons
        x[20_000] += 0.3; // clic isolé
        let slipped: Vec<f32> = clean.iter().take(30_000).chain(clean[30_003..].iter()).copied().collect(); // 3 échantillons manquants
        let r = tone_integrity(&x, 48_000, 1_000.0, 0.05);
        assert_eq!(r.glitches.len(), 2, "{:?}", r.glitches);
        assert!((10_000..10_005).contains(&r.glitches[0]));
        assert_eq!(r.glitches[1], 20_000);
        let r2 = tone_integrity(&slipped, 48_000, 1_000.0, 0.05);
        assert!(!r2.glitches.is_empty(), "un saut d'échantillons casse la phase : détecté");
    }

    #[test]
    fn silence_and_garbage_do_not_panic() {
        assert_eq!(tone_integrity(&[], 48_000, 1_000.0, 0.1).glitches.len(), 0);
        assert!(tone_integrity(&vec![0.0; 1_000], 48_000, 1_000.0, 0.1).amplitude < 1e-9);
    }
}
