//! Étirement par marqueurs (`warp_render`, interface C version 2) : durée exacte, hauteur conservée, instants des attaques, déterminisme,
//! refus, annulation, stéréo. Mesures sur des signaux fabriqués (aucun fichier).

use daw_align::{warp_render, AlignError, Audio, WarpKey, WARP_RATIO_MAX};

const SR: f64 = 48_000.0;
const ENGINES: [(&str, &str); 3] = [
    ("signalsmith", r#"{"general":{"engine":"signalsmith"}}"#),
    ("rubberband_r2", r#"{"general":{"engine":"rubberband_r2"}}"#),
    ("rubberband_r3", r#"{"general":{"engine":"rubberband_r3"}}"#),
];

fn key(source: f64, output: f64) -> WarpKey {
    WarpKey { source, output }
}

fn sine(seconds: f64, hz: f64, amp: f32) -> Vec<f32> {
    (0..(seconds * SR) as usize).map(|i| amp * (2.0 * std::f64::consts::PI * hz * i as f64 / SR).sin() as f32).collect()
}

/// Bruit déterministe (générateur congruentiel), pour des salves reproductibles.
fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((x >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        })
        .collect()
}

fn audio(samples: &[f32], channels: usize) -> Audio<'_> {
    Audio { samples, channels, sample_rate: SR }
}

/// Fréquence estimée par les passages à zéro montants entre deux instants (secondes).
fn zero_cross_hz(x: &[f32], from: f64, to: f64) -> f64 {
    let (a, b) = ((from * SR) as usize, (to * SR) as usize);
    let mut count = 0usize;
    let (mut first, mut last) = (0usize, 0usize);
    for i in a + 1..b {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            if count == 0 {
                first = i;
            }
            last = i;
            count += 1;
        }
    }
    (count - 1) as f64 * SR / (last - first) as f64
}

/// Centre d'énergie d'une salve entre deux instants (secondes), en secondes.
fn centroid(x: &[f32], from: f64, to: f64) -> f64 {
    let (a, b) = ((from * SR) as usize, ((to * SR) as usize).min(x.len()));
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (i, v) in x.iter().enumerate().take(b).skip(a) {
        let e = f64::from(*v) * f64::from(*v);
        num += e * i as f64;
        den += e;
    }
    num / den / SR
}

#[test]
fn the_output_has_the_exact_requested_length_and_the_pitch_does_not_move() {
    let src = sine(2.0, 440.0, 0.5);
    // 0 → 1 s étiré en 1,5 s (×1,5), 1 → 2 s inchangé : 2,5 s en tout.
    let keys = [key(0.0, 0.0), key(1.0, 1.5), key(2.0, 2.5)];
    for (name, settings) in ENGINES {
        let (out, info) = warp_render(audio(&src, 1), &keys, Some(settings), None).unwrap_or_else(|e| panic!("{name} : {e}"));
        assert_eq!(out.len(), 120_000, "{name} : longueur exacte");
        assert!(!info.fell_back, "{name} : pas de repli");
        let hz = zero_cross_hz(&out, 0.3, 1.2);
        assert!((hz - 440.0).abs() < 4.4, "{name} : 440 Hz conservés dans le tronçon étiré, mesuré {hz:.1}");
        let hz2 = zero_cross_hz(&out, 1.8, 2.3);
        assert!((hz2 - 440.0).abs() < 4.4, "{name} : 440 Hz conservés dans le tronçon intact, mesuré {hz2:.1}");
    }
}

#[test]
fn bursts_of_noise_land_where_the_markers_say() {
    // Trois salves de 8 ms à 0,5 s, 1,2 s et 1,7 s de la source (2 s). Cartes : [0, 1 s] → ×1,25 ; [1, 2 s] → ×0,8.
    let mut src = vec![0.0f32; 96_000];
    let burst = noise(384, 7);
    for t in [0.5, 1.2, 1.7] {
        let a = (t * SR) as usize;
        for (k, v) in burst.iter().enumerate() {
            src[a + k] = 0.5 * v;
        }
    }
    let keys = [key(0.0, 0.0), key(1.0, 1.25), key(2.0, 2.05)];
    let predicted = |t: f64| if t <= 1.0 { t * 1.25 } else { 1.25 + (t - 1.0) * 0.8 };
    // Tolérances annoncées par la bibliothèque : ± 4 ms pour Signalsmith, ± 20 ms pour Rubber Band (R3 étale les attaques).
    for (name, settings, tol) in [("signalsmith", ENGINES[0].1, 0.004), ("rubberband_r2", ENGINES[1].1, 0.020), ("rubberband_r3", ENGINES[2].1, 0.020)] {
        let (out, _) = warp_render(audio(&src, 1), &keys, Some(settings), None).unwrap();
        for t in [0.5, 1.2, 1.7] {
            let want = predicted(t + 0.004); // centre de la salve de 8 ms
            let got = centroid(&out, want - 0.06, want + 0.06);
            assert!((got - want).abs() < tol, "{name} : salve de {t} s attendue à {want:.4} s, trouvée à {got:.4} s");
        }
    }
}

#[test]
fn the_same_input_gives_the_same_samples_for_every_engine() {
    let src = noise(48_000, 42);
    let keys = [key(0.0, 0.0), key(0.4, 0.6), key(1.0, 0.9)];
    for (name, settings) in ENGINES {
        let (a, _) = warp_render(audio(&src, 1), &keys, Some(settings), None).unwrap();
        let (b, _) = warp_render(audio(&src, 1), &keys, Some(settings), None).unwrap();
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()), "{name} : deux rendus identiques au bit près");
    }
}

#[test]
fn without_settings_the_default_engine_is_used_and_stereo_keeps_both_channels() {
    let l = sine(1.0, 330.0, 0.4);
    let r = sine(1.0, 550.0, 0.4);
    let inter: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
    let (out, _) = warp_render(audio(&inter, 2), &[key(0.0, 0.0), key(1.0, 1.2)], None, None).unwrap();
    assert_eq!(out.len(), 2 * 57_600, "1,2 s en stéréo");
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
    assert!((zero_cross_hz(&left, 0.2, 1.0) - 330.0).abs() < 4.0, "voie gauche à 330 Hz");
    assert!((zero_cross_hz(&right, 0.2, 1.0) - 550.0).abs() < 6.0, "voie droite à 550 Hz");
}

#[test]
fn bad_markers_are_refused_with_a_clear_message_and_nothing_is_returned() {
    let src = sine(2.0, 440.0, 0.5);
    let cases: Vec<(&str, Vec<WarpKey>)> = vec![
        ("un seul marqueur", vec![key(0.0, 0.0)]),
        ("premier marqueur hors de (0, 0)", vec![key(0.1, 0.1), key(2.0, 2.0)]),
        ("dernier instant de source différent de la durée", vec![key(0.0, 0.0), key(1.5, 1.5)]),
        ("temps de sortie qui ne croissent pas", vec![key(0.0, 0.0), key(1.0, 1.0), key(2.0, 0.9)]),
        ("temps de source qui ne croissent pas", vec![key(0.0, 0.0), key(1.0, 1.0), key(1.0, 1.5), key(2.0, 2.0)]),
        ("rapport trop grand", vec![key(0.0, 0.0), key(1.0, 1.0 * (WARP_RATIO_MAX + 1.0)), key(2.0, 6.0)]),
        ("rapport trop petit", vec![key(0.0, 0.0), key(1.0, 0.1), key(2.0, 1.1)]),
        ("tronçon de moins de 10 ms", vec![key(0.0, 0.0), key(1.0, 1.0), key(1.005, 1.005), key(2.0, 2.0)]),
        ("valeur non finie", vec![key(0.0, 0.0), key(f64::NAN, 1.0), key(2.0, 2.0)]),
    ];
    for (what, keys) in cases {
        match warp_render(audio(&src, 1), &keys, None, None) {
            Err(AlignError::Argument(m)) => assert!(!m.is_empty(), "{what} : message vide"),
            other => panic!("{what} : attendu un refus, obtenu {:?}", other.map(|(v, _)| v.len())),
        }
    }
    assert!(matches!(warp_render(audio(&src[..100], 1), &[key(0.0, 0.0), key(100.0 / SR, 0.01)], None, None), Err(AlignError::Argument(_))), "source de moins de 50 ms");
}

#[test]
fn cancelling_from_the_progress_callback_is_clean_and_the_next_call_works() {
    let src = noise(96_000, 5);
    let keys = [key(0.0, 0.0), key(2.0, 2.4)];
    for (name, settings) in ENGINES {
        let mut calls = 0;
        let mut cancel = |_f: f64| {
            calls += 1;
            calls >= 2
        };
        let r = warp_render(audio(&src, 1), &keys, Some(settings), Some(&mut cancel));
        assert!(matches!(r, Err(AlignError::Cancelled)), "{name} : annulé proprement");
        assert!(warp_render(audio(&src, 1), &keys, Some(settings), None).is_ok(), "{name} : l'appel suivant fonctionne");
    }
}

#[test]
fn progress_grows_and_ends_at_one() {
    let src = noise(48_000, 9);
    let mut seen = Vec::new();
    let mut p = |f: f64| {
        seen.push(f);
        false
    };
    warp_render(audio(&src, 1), &[key(0.0, 0.0), key(1.0, 1.3)], None, Some(&mut p)).unwrap();
    assert!(!seen.is_empty() && seen.windows(2).all(|w| w[1] >= w[0]) && *seen.last().unwrap() > 0.9, "progression croissante : {seen:?}");
}
