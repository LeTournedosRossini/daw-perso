//! Rééchantillonnage de qualité : sinc fenêtré de Kaiser, polyphasé, sans dépendance.
//!
//! Conçu pour l'import de fichiers et la conversion d'un projet (hors du fil audio, sur un fil de travail).
//!
//! * **Qualité** : bande passante jusqu'à 90 % de la fréquence de Nyquist la plus basse des deux fréquences,
//!   atténuation ≥ 100 dB au-delà de 100 % (fenêtre de Kaiser β = 12). Le noyau est symétrique (phase nulle) :
//!   aucun retard, l'échantillon de sortie `n` correspond exactement à l'instant `n × entrée / sortie`.
//! * **Déterminisme** : uniquement `+ − × / sqrt floor` en `f64` (exacts selon IEEE 754) ; `sin` est remplacé par
//!   [`sin_det`], car la fonction du système peut différer d'une plateforme à l'autre. Le résultat est donc le
//!   même sur toutes les plateformes (x86-64 et ARM, Windows, Linux, macOS, WebAssembly).
//! * Hors des données, le signal est considéré comme silencieux.

use std::f64::consts::PI;

const BETA: f64 = 12.0;
/// Atténuation visée, en dB, pour dimensionner le noyau.
const ATTENUATION_DB: f64 = 117.0;
/// Largeur de la zone de transition, en fraction du Nyquist le plus bas.
const TRANSITION: f64 = 0.1;
/// Au-delà de ce nombre de phases on calcule les coefficients à la volée au lieu de les mettre en cache.
const MAX_CACHED_PHASES: usize = 4096;

/// Sinus déterministe (précision ≈ 1e-14), sans appel à la bibliothèque mathématique du système.
pub fn sin_det(x: f64) -> f64 {
    let two_pi = 2.0 * PI;
    // Réduction à [-π, π] puis à [-π/2, π/2].
    let mut r = x - (x / two_pi + 0.5).floor() * two_pi;
    if r > PI / 2.0 {
        r = PI - r;
    } else if r < -PI / 2.0 {
        r = -PI - r;
    }
    let r2 = r * r;
    // Série de Taylor jusqu'à r^23 (schéma de Horner).
    let mut term = 1.0;
    let mut k = 23.0;
    while k >= 3.0 {
        term = 1.0 - r2 / (k * (k - 1.0)) * term;
        k -= 2.0;
    }
    r * term
}

/// Fonction de Bessel modifiée d'ordre 0 (série).
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut sum, mut term) = (1.0, 1.0);
    let mut k = 1.0;
    while term > sum * 1e-17 {
        term *= q / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Nombre de trames obtenues en convertissant `frames` trames de `in_rate` à `out_rate` (arrondi au plus proche).
pub fn output_len(frames: usize, in_rate: u32, out_rate: u32) -> usize {
    if in_rate == 0 {
        return 0;
    }
    let num = frames as u128 * out_rate as u128;
    ((num * 2 + in_rate as u128) / (2 * in_rate as u128)) as usize
}

struct Kernel {
    fc: f64,
    half: f64,
    h: i64,
    i0_beta: f64,
}

impl Kernel {
    fn new(in_rate: u32, out_rate: u32) -> Kernel {
        let m = (out_rate as f64 / in_rate as f64).min(1.0);
        let dw = PI * TRANSITION * m;
        let taps = ((ATTENUATION_DB - 7.95) / (2.285 * dw)).ceil();
        let half = taps / 2.0;
        Kernel { fc: (1.0 - TRANSITION / 2.0) * m, half, h: half.ceil() as i64, i0_beta: bessel_i0(BETA) }
    }

    fn value(&self, x: f64) -> f64 {
        let a = x.abs();
        if a >= self.half {
            return 0.0;
        }
        let r = a / self.half;
        let w = bessel_i0(BETA * (1.0 - r * r).sqrt()) / self.i0_beta;
        let px = PI * self.fc * x;
        let s = if x == 0.0 { 1.0 } else { sin_det(px) / px };
        self.fc * s * w
    }

    /// Coefficients pour une sortie située à `frac` (0 ≤ frac < 1) après l'échantillon source `base`.
    /// Ils portent sur les échantillons `base − h + 1 ..= base + h`. Somme normalisée à 1.
    fn taps(&self, frac: f64) -> Vec<f64> {
        let mut t: Vec<f64> = (0..2 * self.h).map(|j| self.value((j - self.h + 1) as f64 - frac)).collect();
        let sum: f64 = t.iter().sum();
        for v in &mut t {
            *v /= sum;
        }
        t
    }
}

/// Convertit un canal de `in_rate` à `out_rate`. Même fréquence : copie exacte.
pub fn resample(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate || input.is_empty() || in_rate == 0 || out_rate == 0 {
        return if in_rate == out_rate { input.to_vec() } else { Vec::new() };
    }
    let kernel = Kernel::new(in_rate, out_rate);
    let g = gcd(in_rate as u64, out_rate as u64);
    let phases = (out_rate as u64 / g) as usize;
    let mut cache: Vec<Option<Vec<f64>>> = if phases <= MAX_CACHED_PHASES { vec![None; phases] } else { Vec::new() };
    let n_out = output_len(input.len(), in_rate, out_rate);
    let mut out = Vec::with_capacity(n_out);
    let len = input.len() as i64;
    let width = 2 * kernel.h;
    let mut scratch;
    for n in 0..n_out as u64 {
        let num = n * in_rate as u64;
        let base = (num / out_rate as u64) as i64;
        let rem = num % out_rate as u64;
        let taps: &[f64] = if phases <= MAX_CACHED_PHASES {
            let p = (rem / g) as usize;
            cache[p].get_or_insert_with(|| kernel.taps(rem as f64 / out_rate as f64))
        } else {
            scratch = kernel.taps(rem as f64 / out_rate as f64);
            &scratch
        };
        let start = base - kernel.h + 1;
        let mut acc = 0.0f64;
        if start >= 0 && start + width <= len {
            for (j, &c) in taps.iter().enumerate() {
                acc += c * input[start as usize + j] as f64;
            }
        } else {
            for (j, &c) in taps.iter().enumerate() {
                let i = start + j as i64;
                if i >= 0 && i < len {
                    acc += c * input[i as usize] as f64;
                }
            }
        }
        out.push(acc as f32);
    }
    out
}

/// Convertit des échantillons entrelacés (`channels` canaux).
pub fn resample_interleaved(input: &[f32], channels: usize, in_rate: u32, out_rate: u32) -> Vec<f32> {
    if channels == 0 {
        return Vec::new();
    }
    if channels == 1 || in_rate == out_rate {
        return resample(input, in_rate, out_rate);
    }
    let per: Vec<Vec<f32>> = (0..channels)
        .map(|c| resample(&input.iter().skip(c).step_by(channels).copied().collect::<Vec<_>>(), in_rate, out_rate))
        .collect();
    let frames = per[0].len();
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        for ch in &per {
            out.push(ch[i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, rate: u32, frames: usize, amp: f64) -> Vec<f32> {
        (0..frames).map(|n| (amp * (2.0 * std::f64::consts::PI * freq * n as f64 / rate as f64).sin()) as f32).collect()
    }

    /// Écart RMS (en dB sous l'amplitude de référence) entre `got` et une sinusoïde idéale, bords exclus.
    fn error_db(got: &[f32], freq: f64, rate: u32, amp: f64, margin: usize) -> f64 {
        let mut sq = 0.0;
        let mut n = 0.0;
        for i in margin..got.len() - margin {
            let ideal = amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin();
            sq += (got[i] as f64 - ideal).powi(2);
            n += 1.0;
        }
        20.0 * ((sq / n).sqrt() / amp).log10()
    }

    fn rms(x: &[f32], margin: usize) -> f64 {
        let s = &x[margin..x.len() - margin];
        (s.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
    }

    #[test]
    fn sin_det_matches_std_over_a_wide_range() {
        let mut worst = 0.0f64;
        let mut x = -2000.0;
        while x < 2000.0 {
            worst = worst.max((sin_det(x) - x.sin()).abs());
            x += 0.37311;
        }
        assert!(worst < 1e-12, "écart max {worst}");
        assert_eq!(sin_det(0.0), 0.0);
    }

    #[test]
    fn bessel_i0_known_values() {
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-15);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_2).abs() < 1e-14);
        assert!((bessel_i0(12.0) - 18_948.925_349_296_31).abs() < 1e-7 * 18_948.0);
    }

    #[test]
    fn same_rate_is_an_exact_copy_and_empty_is_empty() {
        let x = tone(440.0, 48_000, 1000, 0.5);
        assert_eq!(resample(&x, 48_000, 48_000), x);
        assert!(resample(&[], 44_100, 48_000).is_empty());
    }

    #[test]
    fn output_lengths() {
        assert_eq!(output_len(44_100, 44_100, 48_000), 48_000);
        assert_eq!(output_len(48_000, 48_000, 44_100), 44_100);
        assert_eq!(output_len(1, 44_100, 96_000), 2);
        assert_eq!(resample(&vec![0.0; 44_100], 44_100, 48_000).len(), 48_000);
        assert_eq!(resample(&vec![0.0; 48_000], 48_000, 44_100).len(), 44_100);
    }

    #[test]
    fn passband_tones_are_reproduced_to_better_than_minus_100_db() {
        for &(from, to, f) in
            &[(48_000u32, 44_100u32, 1_000.0), (48_000, 44_100, 18_000.0), (44_100, 48_000, 1_000.0), (44_100, 96_000, 15_000.0), (96_000, 44_100, 10_000.0), (44_100, 48_000, 19_000.0)]
        {
            let x = tone(f, from, from as usize / 2, 0.8);
            let y = resample(&x, from, to);
            let e = error_db(&y, f, to, 0.8, 600);
            assert!(e < -100.0, "{from}→{to} à {f} Hz : écart {e:.1} dB");
        }
    }

    #[test]
    fn above_the_new_nyquist_is_rejected_instead_of_aliased() {
        // 23 kHz et 30 kHz n'existent pas à 44,1 kHz : ils ne doivent pas se replier dans la bande audible.
        for &(f, rate) in &[(23_000.0, 48_000u32), (30_000.0, 96_000)] {
            let x = tone(f, rate, rate as usize / 2, 0.8);
            let y = resample(&x, rate, 44_100);
            let r = 20.0 * (rms(&y, 800) / (0.8 / 2f64.sqrt())).log10();
            assert!(r < -100.0, "{f} Hz : reste {r:.1} dB");
        }
    }

    #[test]
    fn dc_is_preserved_and_edges_fade_to_silence() {
        let y = resample(&vec![0.5; 20_000], 44_100, 48_000);
        assert!(y[5_000..15_000].iter().all(|&v| (v - 0.5).abs() < 1e-6));
        assert!(y[0] > 0.2 && y[0] < 0.6);
    }

    #[test]
    fn round_trip_through_another_rate_returns_the_signal() {
        let x: Vec<f32> = (0..30_000)
            .map(|n| {
                let t = n as f64 / 44_100.0;
                (0.3 * (2.0 * PI * 440.0 * t).sin() + 0.2 * (2.0 * PI * 3_000.0 * t).sin() + 0.1 * (2.0 * PI * 12_000.0 * t).sin()) as f32
            })
            .collect();
        let y = resample(&resample(&x, 44_100, 48_000), 48_000, 44_100);
        let err: f64 = x[1_500..28_000].iter().zip(&y[1_500..28_000]).map(|(a, b)| ((a - b) as f64).powi(2)).sum::<f64>() / 26_500.0;
        assert!(10.0 * err.log10() < -105.0, "écart {:.1} dB", 10.0 * err.log10());
    }

    #[test]
    fn awkward_ratio_without_phase_table_is_still_correct() {
        // 44 100 → 47 999 : 47 999 phases, calcul des coefficients à la volée.
        let x = tone(1_000.0, 44_100, 3_000, 0.5);
        let y = resample(&x, 44_100, 47_999);
        assert_eq!(y.len(), output_len(3_000, 44_100, 47_999));
        assert!(error_db(&y, 1_000.0, 47_999, 0.5, 700) < -100.0);
    }

    #[test]
    fn interleaved_matches_per_channel() {
        let l = tone(500.0, 44_100, 4_000, 0.5);
        let r = tone(1_500.0, 44_100, 4_000, 0.25);
        let inter: Vec<f32> = l.iter().zip(&r).flat_map(|(&a, &b)| [a, b]).collect();
        let out = resample_interleaved(&inter, 2, 44_100, 48_000);
        let (yl, yr) = (resample(&l, 44_100, 48_000), resample(&r, 44_100, 48_000));
        assert_eq!(out.len(), yl.len() * 2);
        assert!(out.chunks(2).zip(yl.iter().zip(&yr)).all(|(p, (&a, &b))| p[0] == a && p[1] == b));
    }

    fn fnv(x: &[f32]) -> u64 {
        x.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, v| (h ^ v.to_bits() as u64).wrapping_mul(0x100_0000_01b3))
    }

    #[test]
    fn reference_hash_is_pinned() {
        // Empreinte de référence : doit être identique sur toutes les plateformes (voir CLAUDE.md, tranche 30).
        let x: Vec<f32> = (0..5_000).map(|n| ((n * 2_654_435_761u64 % 1000) as f32 / 500.0 - 1.0) * 0.5).collect();
        let h = fnv(&resample(&x, 44_100, 48_000)) ^ fnv(&resample(&x, 48_000, 44_100)).rotate_left(1);
        println!("HASH={h:#018x}");
        assert_eq!(h, PINNED);
    }

    const PINNED: u64 = 0x3bcefc22720a2347;
}
