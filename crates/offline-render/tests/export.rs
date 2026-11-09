//! Export : encodage WAV / FLAC, dither, normalisation, zone, queue, stems, annulation.

mod common;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use common::*;
use daw_core_types::NodeId;
use daw_engine::{GraphDesc, Insert, InsertFactory, TrackFormat};
use daw_offline_render::export::*;
use daw_offline_render::{render, Progress, RenderError, RenderSpec};
use daw_recorder::decode::{decode_bytes, Format as Fmt};

fn fmt(container: Container, depth: BitDepth, dither: bool) -> Format {
    Format { container, depth, dither }
}

fn sine(n: usize, amp: f32, period: f32) -> Vec<f32> {
    (0..n).map(|i| amp * (i as f32 * std::f32::consts::TAU / period).sin()).collect()
}

#[test]
fn float32_wav_is_lossless() {
    let l = noise(1, 5_000);
    let r = noise(2, 5_000);
    let e = encode(&l, &r, SR, fmt(Container::Wav, BitDepth::Float32, false), None).unwrap();
    let d = decode_bytes(&e.bytes).unwrap();
    assert_eq!((d.audio.sample_rate, d.audio.channels, d.audio.frames()), (SR, 2, 5_000));
    assert_eq!(d.audio.channel(0), l);
    assert_eq!(d.audio.channel(1), r);
    assert_eq!(e.report.frames, 5_000);
}

#[test]
fn integer_wav_and_flac_round_trip_exactly_on_representable_values() {
    for (depth, bits) in [(BitDepth::Pcm16, 16u32), (BitDepth::Pcm24, 24)] {
        let scale = (1u64 << (bits - 1)) as f32;
        // Valeurs exactement représentables : k / 2^(bits-1).
        let l: Vec<f32> = (0..3_000).map(|i| ((i * 37 % 2001) as f32 - 1000.0) / 1000.0 * 0.5).map(|v| (v * scale).round() / scale).collect();
        let r: Vec<f32> = l.iter().rev().copied().collect();
        for container in [Container::Wav, Container::Flac] {
            let e = encode(&l, &r, 44_100, fmt(container, depth, false), None).unwrap();
            let d = decode_bytes(&e.bytes).unwrap();
            assert_eq!(d.format, if container == Container::Wav { Fmt::Wav } else { Fmt::Flac });
            assert_eq!((d.audio.sample_rate, d.source_bits as u32), (44_100, bits));
            let got = d.audio.channel(0);
            assert_eq!(got.len(), l.len(), "{container:?} {bits} : longueur");
            if let Some(i) = got.iter().zip(&l).position(|(a, b)| a != b) {
                panic!("{container:?} {bits} : premier écart à {i} : {} au lieu de {} (sur {})", got[i], l[i], l.len());
            }
            assert_eq!(d.audio.channel(1), r, "{container:?} {bits}");
        }
    }
}

#[test]
fn flac_is_smaller_than_wav_for_tonal_audio_and_float_flac_is_refused() {
    let x = sine(48_000, 0.5, 100.0);
    let wav = encode(&x, &x, SR, fmt(Container::Wav, BitDepth::Pcm24, false), None).unwrap();
    let flac = encode(&x, &x, SR, fmt(Container::Flac, BitDepth::Pcm24, false), None).unwrap();
    assert!(flac.bytes.len() < wav.bytes.len() / 2, "{} contre {}", flac.bytes.len(), wav.bytes.len());
    assert!(matches!(encode(&x, &x, SR, fmt(Container::Flac, BitDepth::Float32, false), None), Err(ExportError::Invalid(_))));
    assert!(matches!(encode(&x, &x[..10], SR, Format::wav24(), None), Err(ExportError::Invalid(_))));
}

#[test]
fn tpdf_dither_is_deterministic_and_has_the_expected_statistics() {
    let zeros = vec![0.0f32; 200_000];
    let f = fmt(Container::Wav, BitDepth::Pcm16, true);
    let a = encode(&zeros, &zeros, SR, f, None).unwrap();
    assert_eq!(a, encode(&zeros, &zeros, SR, f, None).unwrap()); // mêmes octets
    let d = decode_bytes(&a.bytes).unwrap();
    let ints: Vec<i32> = d.audio.samples.iter().map(|&v| (v * 32_768.0).round() as i32).collect();
    assert!(ints.iter().all(|&v| (-1..=1).contains(&v)));
    let nonzero = ints.iter().filter(|&&v| v != 0).count() as f64 / ints.len() as f64;
    assert!((nonzero - 0.25).abs() < 0.01, "part non nulle {nonzero}"); // TPDF : 2 × 1/8
    let mean = ints.iter().map(|&v| v as f64).sum::<f64>() / ints.len() as f64;
    assert!(mean.abs() < 0.01, "moyenne {mean}");
    // Sans dither : silence exact.
    let plain = encode(&zeros, &zeros, SR, fmt(Container::Wav, BitDepth::Pcm16, false), None).unwrap();
    assert!(decode_bytes(&plain.bytes).unwrap().audio.samples.iter().all(|&v| v == 0.0));
    // Les deux canaux ne reçoivent pas le même bruit.
    assert_ne!(d.audio.channel(0), d.audio.channel(1));
}

#[test]
fn dither_preserves_a_signal_smaller_than_one_bit() {
    // 0,3 LSB d'amplitude : sans dither tout s'arrondit à 0 ; avec dither la sinusoïde reste présente en moyenne.
    let lsb = 1.0 / 32_768.0;
    let x: Vec<f32> = sine(400_000, 0.3 * lsb, 100.0);
    let corr = |bytes: &[u8]| {
        let d = decode_bytes(bytes).unwrap();
        d.audio.channel(0).iter().zip(&x).map(|(&o, &i)| o as f64 * i as f64).sum::<f64>() / x.iter().map(|&i| (i as f64).powi(2)).sum::<f64>()
    };
    let off = encode(&x, &x, SR, fmt(Container::Wav, BitDepth::Pcm16, false), None).unwrap();
    let on = encode(&x, &x, SR, fmt(Container::Wav, BitDepth::Pcm16, true), None).unwrap();
    assert_eq!(corr(&off.bytes), 0.0);
    assert!((corr(&on.bytes) - 1.0).abs() < 0.1, "gain moyen avec dither : {}", corr(&on.bytes));
}

#[test]
fn normalization_reaches_the_target_peak_and_reports_the_gain() {
    let x = sine(10_000, 0.25, 77.0);
    let e = encode(&x, &x, SR, fmt(Container::Wav, BitDepth::Float32, false), Some(-1.0)).unwrap();
    let d = decode_bytes(&e.bytes).unwrap();
    let peak = d.audio.samples.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    assert!((peak - 10f32.powf(-1.0 / 20.0)).abs() < 1e-6, "pic {peak}");
    assert!((e.report.applied_gain_db - 20.0 * (10f32.powf(-0.05) / e.report.peak).log10()).abs() < 1e-3);
    assert_eq!(e.report.clipped_samples, 0);
    // Silence : pas de division par zéro.
    let z = vec![0.0f32; 100];
    let e = encode(&z, &z, SR, Format::wav24(), Some(-1.0)).unwrap();
    assert_eq!((e.report.applied_gain_db, e.report.peak), (0.0, 0.0));
}

#[test]
fn clipping_is_counted_and_clamped_in_integer_files_but_kept_in_float() {
    let x = vec![0.5f32, 1.5, -2.0, 1.0, f32::NAN, -1.0];
    let int = encode(&x, &x, SR, Format { container: Container::Wav, depth: BitDepth::Pcm16, dither: false }, None).unwrap();
    assert_eq!(int.report.clipped_samples, 4); // 1,5 et −2,0, dans chaque canal
    let d = decode_bytes(&int.bytes).unwrap();
    assert_eq!(d.audio.channel(0), vec![0.5, 32_767.0 / 32_768.0, -1.0, 32_767.0 / 32_768.0, 0.0, -1.0]);
    let fl = encode(&x, &x, SR, fmt(Container::Wav, BitDepth::Float32, false), None).unwrap();
    assert_eq!(decode_bytes(&fl.bytes).unwrap().audio.channel(0)[1], 1.5);
}

fn two_track_project() -> GraphDesc {
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    g.add_clip(a, clip(&stereo(noise(10, 30_000), noise(11, 30_000)), 0));
    let c = g.add_track("C", TrackFormat::Stereo);
    g.add_clip(c, clip(&stereo(noise(12, 20_000), noise(13, 20_000)), 5_000));
    g.node_mut(c).unwrap().params.gain = 0.7;
    g.node_mut(c).unwrap().params.pan = -0.4;
    g
}

#[test]
fn project_export_matches_the_engine_render_bit_for_bit() {
    let g = two_track_project();
    assert_eq!(project_end(&g), 30_000);
    let range = ExportRange::whole_project(&g);
    let mut spec = ExportSpec::new(SR, g.clone(), range, fmt(Container::Wav, BitDepth::Float32, false));
    spec.tail_secs = 0.5;
    let e = export_mix(&spec).unwrap();
    let direct = render(&RenderSpec::new(SR, 30_000, g)).unwrap();
    let d = decode_bytes(&e.bytes).unwrap();
    assert_eq!(d.audio.frames(), 30_000); // la queue silencieuse est coupée
    assert_eq!(d.audio.channel(0), direct.left);
    assert_eq!(d.audio.channel(1), direct.right);
}

#[test]
fn a_selected_range_equals_the_same_slice_of_the_full_render() {
    let g = two_track_project();
    let full = render(&RenderSpec::new(SR, 30_000, g.clone())).unwrap();
    let mut spec = ExportSpec::new(SR, g, ExportRange { start: 7_321, end: 21_000 }, fmt(Container::Wav, BitDepth::Float32, false));
    spec.tail_secs = 0.0;
    let (l, r) = render_range(&spec).unwrap();
    assert_eq!(l.len(), 21_000 - 7_321);
    assert_eq!(l, full.left[7_321..21_000]);
    assert_eq!(r, full.right[7_321..21_000]);
    spec.range = ExportRange { start: 5, end: 5 };
    assert!(matches!(render_range(&spec), Err(ExportError::Invalid(_))));
}

/// Réverbération rudimentaire : y = x + 0,9995 · y(précédent). La queue dure ≈ 20 000 échantillons jusqu'à −90 dB.
struct Ring(f32, f32);
impl Insert for Ring {
    fn latency(&self) -> u32 {
        0
    }
    fn process(&mut self, il: &[f32], ir: &[f32], ol: &mut [f32], or: &mut [f32]) {
        for j in 0..il.len() {
            self.0 = il[j] + 0.9995 * self.0;
            self.1 = ir[j] + 0.9995 * self.1;
            ol[j] = self.0;
            or[j] = self.1;
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {
        self.0 = 0.0;
        self.1 = 0.0;
    }
}

#[test]
fn the_tail_is_rendered_then_cut_when_silent() {
    let mut g = GraphDesc::new();
    let t = impulse_track(&mut g, "t", 100, 1_000);
    let f: InsertFactory = Arc::new(|_, _, _| Ok(Box::new(Ring(0.0, 0.0)) as Box<dyn Insert>));
    g.add_insert(t, "ring", f).unwrap();
    let range = ExportRange { start: 0, end: 1_000 };
    let mut spec = ExportSpec::new(SR, g, range, fmt(Container::Wav, BitDepth::Float32, false));
    spec.tail_secs = 0.0;
    assert_eq!(render_range(&spec).unwrap().0.len(), 1_000); // sans queue : la zone seulement
    spec.tail_secs = 5.0;
    let (l, _) = render_range(&spec).unwrap();
    // 0,9995^n < 10^(-90/20) pour n ≈ 20 700 (à 100 près de l'impulsion).
    assert!((20_000..22_500).contains(&l.len()), "{} trames", l.len());
    assert!(l[l.len() - 1].abs() > 10f32.powf(-90.0 / 20.0) * 0.9);
    spec.tail_secs = 0.1; // queue plafonnée à 4 800 échantillons
    assert_eq!(render_range(&spec).unwrap().0.len(), 1_000 + 4_800);
}

#[test]
fn stems_add_up_to_the_mix_exactly() {
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    g.add_clip(a, clip(&stereo(noise(20, 9_000), noise(21, 9_000)), 0));
    let c = g.add_track("C", TrackFormat::Stereo);
    g.add_clip(c, clip(&stereo(noise(22, 9_000), noise(23, 9_000)), 1_000));
    let b = g.add_bus("B");
    g.set_output(a, b);
    g.add_send(c, b, 0.5); // C envoie aussi dans B ; sa sortie principale va à la sortie principale
    g.node_mut(b).unwrap().params.gain = 0.6;
    g.node_mut(a).unwrap().params.gain = 0.8;
    let f = fmt(Container::Wav, BitDepth::Float32, false);
    let mut spec = ExportSpec::new(SR, g.clone(), ExportRange { start: 0, end: 10_000 }, f);
    spec.tail_secs = 0.0;
    let mix = decode_bytes(&export_mix(&spec).unwrap().bytes).unwrap().audio;
    let mut stems = std::collections::HashMap::new();
    export_stems(&spec, &[a, b, c], |n, e| {
        stems.insert(n, decode_bytes(&e.bytes).unwrap().audio);
        Ok(())
    })
    .unwrap();
    // Bus B (avec A et l'envoi de C) + C seule (sans son envoi) = mixage ; A seule = A à travers son fader seulement.
    for ch in 0..2 {
        let sum: Vec<f32> = stems[&b].channel(ch).iter().zip(stems[&c].channel(ch)).map(|(x, y)| x + y).collect();
        let m = mix.channel(ch);
        if let Some(i) = sum.iter().zip(&m).position(|(a, b)| a != b) {
            panic!("canal {ch} : premier écart à {i} : somme {} mixage {} (stem B {} stem C {})", sum[i], m[i], stems[&b].channel(ch)[i], stems[&c].channel(ch)[i]);
        }
    }
    let (sa, sb) = (&stems[&a], &stems[&b]);
    assert!(sa.channel(0).iter().any(|&v| v != 0.0));
    // Le stem de A est la sortie de A seule : 0,8 × données (A est un clip direct à gain 1,0 × fader 0,8).
    let expected: Vec<f32> = noise(20, 9_000).iter().map(|&v| v * 0.8).chain(std::iter::repeat(0.0)).take(10_000).collect();
    let got = sa.channel(0);
    assert!(got.iter().zip(&expected).all(|(a, b)| (a - b).abs() < 1e-6));
    // B contient plus de choses que A seule (A × 0,6 + envoi de C).
    assert!(sb.channel(0) != sa.channel(0));
    assert!(matches!(stem_graph(&g, NodeId::MASTER), Err(ExportError::Invalid(_))));
    assert!(matches!(stem_graph(&g, NodeId(99)), Err(ExportError::Invalid(_))));
}

#[test]
fn stems_ignore_solo_and_master_processing() {
    let mut g = two_track_project();
    g.node_mut(NodeId(1)).unwrap().params.solo = true;
    g.node_mut(NodeId::MASTER).unwrap().params.gain = 0.1;
    let mut spec = ExportSpec::new(SR, g, ExportRange { start: 0, end: 12_000 }, fmt(Container::Wav, BitDepth::Float32, false));
    spec.tail_secs = 0.0;
    let mut got = None;
    export_stems(&spec, &[NodeId(2)], |_, e| {
        got = Some(decode_bytes(&e.bytes).unwrap().audio);
        Ok(())
    })
    .unwrap();
    let c = got.unwrap();
    // C joue seule, à 0,7 de fader, malgré le solo de A et le gain 0,1 de la sortie principale ; avant 5 000 : silence.
    assert!(c.channel(0)[..5_000].iter().all(|&v| v == 0.0));
    assert!(c.channel(0)[5_000..].iter().any(|&v| v.abs() > 0.01));
}

#[test]
fn progress_is_reported_and_can_cancel() {
    let g = two_track_project();
    let calls = Arc::new(AtomicU32::new(0));
    let c2 = calls.clone();
    let mut spec = ExportSpec::new(SR, g, ExportRange { start: 0, end: 30_000 }, Format::wav24());
    spec.tail_secs = 0.0;
    spec.progress = Some(Progress(Arc::new(move |done, total| {
        assert!(done <= total);
        c2.fetch_add(1, Ordering::Relaxed);
        true
    })));
    export_mix(&spec).unwrap();
    assert!(calls.load(Ordering::Relaxed) >= 100); // un appel par bloc de 256
    let stop_after = Arc::new(AtomicU32::new(0));
    let s2 = stop_after.clone();
    spec.progress = Some(Progress(Arc::new(move |_, _| s2.fetch_add(1, Ordering::Relaxed) < 5)));
    assert_eq!(export_mix(&spec), Err(ExportError::Cancelled));
    assert_eq!(stop_after.load(Ordering::Relaxed), 6); // arrêt immédiat
    assert_eq!(render(&RenderSpec { progress: Some(Progress(Arc::new(|_, _| false))), ..RenderSpec::new(SR, 1000, GraphDesc::new()) }), Err(RenderError::Cancelled));
}

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &v| (h ^ v as u64).wrapping_mul(0x100_0000_01b3))
}

#[test]
fn reference_export_hash_is_pinned() {
    // Mixage 16 bits avec dither : mêmes octets sur toutes les plateformes (CLAUDE.md, tranche 31).
    let g = two_track_project();
    let mut spec = ExportSpec::new(SR, g, ExportRange { start: 0, end: 30_000 }, Format::wav16());
    spec.tail_secs = 0.0;
    let h = fnv(&export_mix(&spec).unwrap().bytes);
    println!("HASH={h:#018x}");
    assert_eq!(h, PINNED);
}

const PINNED: u64 = 0x4cb88e4b5f1cd238;
