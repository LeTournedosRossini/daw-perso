//! Édition de clips : découpe, rognage, déplacement, fondus et fondus enchaînés, rendus par le vrai moteur.

mod common;

use common::*;
use daw_engine::{crossfade, Clip, EditError, Fade, FadeShape, GraphDesc, TrackFormat};
use daw_offline_render::{render, RenderSpec};

fn dc(len: usize, v: f32) -> std::sync::Arc<daw_engine::AudioData> {
    stereo(vec![v; len], vec![v; len])
}

fn out_l(clips: Vec<Clip>, frames: u64, block: usize) -> Vec<f32> {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    for c in clips {
        g.add_clip(t, c);
    }
    let mut spec = RenderSpec::new(SR, frames, g);
    spec.block = block;
    render(&spec).unwrap().left
}

#[test]
fn split_halves_replay_the_original_exactly() {
    let data = stereo(noise(1, 1_000), noise(2, 1_000));
    let whole = Clip::whole(data, 100);
    let reference = out_l(vec![whole.clone()], 1_200, 64);
    let (a, b) = whole.split(437).unwrap();
    assert_eq!((a.start, a.len, b.start, b.len, b.offset), (100, 337, 437, 663, 337));
    assert_eq!(out_l(vec![a, b], 1_200, 64), reference);
}

#[test]
fn split_outside_the_clip_is_refused() {
    let c = Clip::whole(dc(100, 1.0), 50);
    for at in [0, 50, 150, 400] {
        assert_eq!(c.split(at).err(), Some(EditError::OutOfRange), "{at}");
    }
}

#[test]
fn trim_start_and_end_keep_the_content_in_place() {
    let data = stereo(noise(3, 1_000), noise(4, 1_000));
    let whole = Clip::whole(data, 100);
    let reference = out_l(vec![whole.clone()], 1_200, 64);
    let cut = whole.trim_start(300).unwrap().trim_end(900).unwrap();
    let got = out_l(vec![cut], 1_200, 64);
    assert!(got[..300].iter().all(|&x| x == 0.0) && got[900..].iter().all(|&x| x == 0.0));
    assert_eq!(&got[300..900], &reference[300..900]);
    // On retrouve ce qu'on avait rogné, tant que les données existent.
    let back = cut_back(&whole);
    assert_eq!(out_l(vec![back], 1_200, 64), reference);
}

fn cut_back(whole: &Clip) -> Clip {
    let cut = whole.trim_start(300).unwrap().trim_end(900).unwrap();
    cut.trim_start(100).unwrap().trim_end(1_100).unwrap()
}

#[test]
fn trim_beyond_the_data_is_refused() {
    let c = Clip::whole(dc(100, 1.0), 50);
    assert_eq!(c.trim_start(10).err(), Some(EditError::OutOfRange));
    assert_eq!(c.trim_end(200).err(), Some(EditError::OutOfRange));
    assert_eq!(c.trim_end(50).err(), Some(EditError::OutOfRange));
    assert_eq!(c.trim_start(150).err(), Some(EditError::OutOfRange));
}

#[test]
fn move_changes_only_the_position() {
    let data = stereo(noise(5, 500), noise(6, 500));
    let c = Clip::whole(data, 0);
    let a = out_l(vec![c.clone()], 1_000, 64);
    let b = out_l(vec![c.moved(321)], 1_000, 64);
    assert_eq!(&b[321..821], &a[..500]);
}

#[test]
fn fades_follow_the_expected_curve_and_are_exact_at_the_edges() {
    let mut c = Clip::whole(dc(1_000, 1.0), 100);
    c.fade_in = Fade::new(100, FadeShape::Linear);
    c.fade_out = Fade::new(200, FadeShape::EqualPower);
    let o = out_l(vec![c], 1_200, 64);
    assert_eq!(o[99], 0.0);
    assert_eq!(o[100], 0.5 / 100.0);
    assert_eq!(o[150], 50.5 / 100.0);
    assert_eq!(o[200], 1.0);
    assert_eq!(o[899], 1.0);
    // Sortie en racine : le dernier échantillon est le plus bas, ensuite silence.
    assert_eq!(o[1_099], (0.5f32 / 200.0).sqrt());
    assert_eq!(o[1_100], 0.0);
    assert!(o[900..1_100].windows(2).all(|w| w[1] < w[0]));
}

#[test]
fn rendering_with_fades_does_not_depend_on_the_block_size() {
    let mut c = Clip::whole(stereo(noise(7, 3_000), noise(8, 3_000)), 77);
    c.fade_in = Fade::new(500, FadeShape::EqualPower);
    c.fade_out = Fade::new(700, FadeShape::Linear);
    let reference = out_l(vec![c.clone()], 3_300, 4_096);
    for block in [1, 7, 64, 333, 1_024] {
        assert_eq!(out_l(vec![c.clone()], 3_300, block), reference, "bloc {block}");
    }
}

#[test]
fn linear_crossfade_of_equal_signals_is_flat() {
    let a = Clip::whole(dc(1_000, 0.8), 0);
    let b = Clip::whole(dc(1_000, 0.8), 600);
    let (a, b) = crossfade(&a, &b, FadeShape::Linear).unwrap();
    assert_eq!((a.fade_out.len, b.fade_in.len), (400, 400));
    let o = out_l(vec![a, b], 1_600, 64);
    for (i, &x) in o.iter().enumerate() {
        assert!((x - 0.8).abs() < 1e-6, "échantillon {i} : {x}");
    }
}

#[test]
fn equal_power_crossfade_keeps_power_constant_for_unrelated_signals() {
    // Deux continus de signe opposé en quadrature ne se simulent pas ; on vérifie la somme des carrés des gains
    // avec un signal à 1 sur un clip et 1 sur l'autre via deux pistes : puissance = gain_a² + gain_b² = 1.
    let (a, b) = crossfade(&Clip::whole(dc(1_000, 1.0), 0), &Clip::whole(dc(1_000, 1.0), 600), FadeShape::EqualPower).unwrap();
    let oa = out_l(vec![a], 1_600, 64);
    let ob = out_l(vec![b], 1_600, 64);
    for i in 600..1_000 {
        let p = oa[i] * oa[i] + ob[i] * ob[i];
        assert!((p - 1.0).abs() < 1e-5, "échantillon {i} : {p}");
    }
}

#[test]
fn crossfade_needs_a_proper_overlap() {
    let a = Clip::whole(dc(100, 1.0), 0);
    let apart = Clip::whole(dc(100, 1.0), 200);
    let inside = Clip::whole(dc(20, 1.0), 10);
    assert_eq!(crossfade(&a, &apart, FadeShape::Linear).err(), Some(EditError::NoOverlap));
    assert_eq!(crossfade(&a, &inside, FadeShape::Linear).err(), Some(EditError::NoOverlap));
    assert_eq!(crossfade(&apart, &a, FadeShape::Linear).err(), Some(EditError::NoOverlap));
}

#[test]
fn split_keeps_each_fade_on_its_side_and_trim_clamps_them() {
    let mut c = Clip::whole(dc(1_000, 1.0), 0);
    c.fade_in = Fade::new(300, FadeShape::Linear);
    c.fade_out = Fade::new(300, FadeShape::Linear);
    let (a, b) = c.split(100).unwrap();
    assert_eq!((a.fade_in.len, a.fade_out.len), (100, 0));
    assert_eq!((b.fade_in.len, b.fade_out.len), (0, 300));
    let t = c.trim_end(200).unwrap();
    assert_eq!((t.fade_in.len, t.fade_out.len), (200, 200));
}

#[test]
fn fade_longer_than_the_clip_is_rejected_at_compile_time() {
    let mut c = Clip::whole(dc(100, 1.0), 0);
    c.fade_in = Fade::new(101, FadeShape::Linear);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, c);
    assert!(g.compile(SR, 64).is_err());
}

#[test]
fn fade_curves_have_the_documented_values_and_complement_each_other() {
    // x = (k + 0,5) / L. Avec L = 2 : k = 0 → x = 0,25 et k = 1 → x = 0,75.
    let at = |f: Fade| [f.gain(0), f.gain(1)];
    let near = |got: [f32; 2], want: [f32; 2], what: &str| assert!((got[0] - want[0]).abs() < 1e-6 && (got[1] - want[1]).abs() < 1e-6, "{what} : {got:?} au lieu de {want:?}");
    near(at(Fade::new(2, FadeShape::Linear)), [0.25, 0.75], "linéaire");
    near(at(Fade::new(2, FadeShape::EqualPower)), [0.5, 0.866_025_4], "puissance constante");
    // Adoucissement en S (1000) = 3x² − 2x³ ; à moitié : moyenne de la droite et de la courbe en S.
    near(at(Fade::curved(2, FadeShape::Linear, 0, 1000)), [0.156_25, 0.843_75], "S complet");
    near(at(Fade::curved(2, FadeShape::Linear, 0, 500)), [0.5 * (0.25 + 0.156_25), 0.5 * (0.75 + 0.843_75)], "S à moitié");
    // Courbure : +1000 = x² (convexe, monte tard), −1000 = 2x − x² (concave, monte tôt), 0 = droite, 500 = demi-courbure.
    near(at(Fade::curved(2, FadeShape::Linear, 1000, 0)), [0.062_5, 0.562_5], "convexe");
    near(at(Fade::curved(2, FadeShape::Linear, -1000, 0)), [0.437_5, 0.937_5], "concave");
    near(at(Fade::curved(2, FadeShape::Linear, 500, 0)), [0.25 - 0.5 * 0.25 * 0.75, 0.75 - 0.5 * 0.75 * 0.25], "demi-convexe");
    // Composition : S puis courbure puis racine (puissance constante), à x = 0,25.
    let u: f32 = 0.156_25; // S complet à x = 0,25
    let want: f32 = (u - u * (1.0 - u)).sqrt();
    let got = Fade::curved(2, FadeShape::EqualPower, 1000, 1000).gain(0);
    assert!((got - want).abs() < 1e-6, "S + convexe + puissance : {got} au lieu de {want}");
    // Les bornes sont imposées.
    let f = Fade::curved(10, FadeShape::Linear, 5_000, 9_000);
    assert_eq!((f.bend, f.smooth), (1000, 1000));
    assert_eq!(Fade::curved(10, FadeShape::Linear, -5_000, 0).bend, -1000);
    // Toute courbe est croissante entre 0 et 1, sans dépassement (« croissante » et non « strictement » : à l'extrémité d'une
    // courbe très aplatie, deux échantillons voisins peuvent être égaux à la précision d'un flottant).
    for shape in FadeShape::ALL {
        for bend in [-1000i16, -600, -150, 0, 150, 600, 1000] {
            for smooth in [0u16, 250, 700, 1000] {
                let fade = Fade::curved(300, shape, bend, smooth);
                let g: Vec<f32> = (0..300).map(|k| fade.gain(k)).collect();
                assert!(g.windows(2).all(|w| w[1] >= w[0]) && g[0] >= 0.0 && g[299] <= 1.0 && g[299] > g[0] + 0.5, "{shape:?} {bend} {smooth}");
            }
        }
    }
    // Fondu enchaîné : courbures opposées de même adoucissement = somme des gains constante, quelle que soit la courbure.
    let l = 400u64;
    for bend in [-1000i16, -400, 0, 400, 1000] {
        for smooth in [0u16, 500, 1000] {
            let sort = Fade::curved(l, FadeShape::Linear, bend, smooth);
            let entre = Fade::curved(l, FadeShape::Linear, -bend, smooth);
            for k in 0..l {
                let sum = entre.gain(k) + sort.gain(l - 1 - k);
                assert!((sum - 1.0).abs() < 2e-5, "courbure {bend}, S {smooth} : somme {sum} à {k}");
            }
        }
    }
    // Puissance constante : somme des carrés = 1 (sans courbure ni adoucissement).
    let ep = Fade::new(l, FadeShape::EqualPower);
    for k in 0..l {
        assert!((ep.gain(k).powi(2) + ep.gain(l - 1 - k).powi(2) - 1.0).abs() < 1e-5);
    }
}

#[test]
fn rendering_with_curved_fades_is_exact_and_independent_of_the_block_size() {
    for (shape, bend, smooth) in [(FadeShape::Linear, 0, 0), (FadeShape::EqualPower, 0, 0), (FadeShape::Linear, 700, 0), (FadeShape::Linear, -450, 600), (FadeShape::EqualPower, 300, 1000)] {
        let mut c = Clip::whole(stereo(noise(11, 3_000), noise(12, 3_000)), 77);
        c.fade_in = Fade::curved(500, shape, bend, smooth);
        c.fade_out = Fade::curved(700, shape, -bend, smooth);
        let reference = out_l(vec![c.clone()], 3_300, 4_096);
        for block in [1, 7, 64, 333, 1_024] {
            assert_eq!(out_l(vec![c.clone()], 3_300, block), reference, "{shape:?} {bend} {smooth}, bloc {block}");
        }
    }
}
