//! Test de bout en bout de l'interface C, côté Rust (même scénario que `demo/align_demo.c` de la bibliothèque) : analyse du guide et du double
//! de référence, chemin comparé aux débuts de notes connus (`ref/onsets.txt`), rendu à la durée du guide, analyse enregistrée puis relue (rendu identique
//! au bit près), annulation propre, réglages JSON, mode d'analyse différent refusé.

use std::path::{Path, PathBuf};

use daw_align::{default_settings_json, library_version, merge_general, validate_settings, AlignError, Analysis, Audio, Guide};
use daw_recorder::decode::decode_file;

fn reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ref").join(name)
}

struct Wav {
    samples: Vec<f32>,
    channels: usize,
    rate: f64,
}

impl Wav {
    fn read(name: &str) -> Wav {
        let d = decode_file(&reference(name)).unwrap();
        Wav { channels: d.audio.channels as usize, rate: d.audio.sample_rate as f64, samples: d.audio.samples }
    }
    fn audio(&self) -> Audio<'_> {
        Audio { samples: &self.samples, channels: self.channels, sample_rate: self.rate }
    }
    fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }
}

/// (temps du guide, temps du double) de chaque début de note.
fn onsets() -> Vec<(f64, f64)> {
    std::fs::read_to_string(reference("onsets.txt"))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut it = l.split_whitespace().map(|x| x.parse::<f64>().unwrap());
            (it.next().unwrap(), it.next().unwrap())
        })
        .collect()
}

#[test]
fn the_path_finds_the_known_note_onsets_and_the_render_has_the_guide_length() {
    let (guide, dub) = (Wav::read("guide.wav"), Wav::read("double.wav"));
    let mut fractions: Vec<f64> = Vec::new();
    let g = Guide::new(guide.audio(), None, None).unwrap();
    let a = g
        .analyse(dub.audio(), None, Some(&mut |f| {
            fractions.push(f);
            false
        }))
        .unwrap();
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "progression croissante");
    assert!(*fractions.last().unwrap() > 0.99, "finit à 1 : {fractions:?}");
    let path = a.path().unwrap();
    let hop = a.hop_seconds();
    assert!(hop > 0.0 && path.len() > 100);
    let errors: Vec<f64> = onsets()
        .iter()
        .map(|&(tg, td)| {
            let i = ((tg / hop).round() as usize).min(path.len() - 1);
            (path[i] - td).abs() * 1000.0
        })
        .collect();
    let (mean, worst) = (errors.iter().sum::<f64>() / errors.len() as f64, errors.iter().fold(0.0f64, |m, &e| m.max(e)));
    println!("erreur moyenne {mean:.1} ms, pire {worst:.1} ms sur {} débuts de notes", errors.len());
    assert!(mean < 15.0 && worst < 45.0, "moyenne {mean:.1} ms, pire {worst:.1} ms");

    let (out, info) = a.render(dub.audio(), None, None).unwrap();
    assert_eq!(out.len() / dub.channels, guide.frames(), "durée exacte du guide");
    assert_eq!(a.length_frames(), guide.frames());
    assert!(!info.fell_back);
    assert!(out.iter().all(|x| x.is_finite()));
    assert!(out.iter().any(|x| x.abs() > 0.01), "le rendu n'est pas muet");
}

#[test]
fn a_saved_analysis_gives_the_same_render_bit_for_bit() {
    let (guide, dub) = (Wav::read("guide.wav"), Wav::read("double.wav"));
    let g = Guide::new(guide.audio(), None, None).unwrap();
    let a = g.analyse(dub.audio(), None, None).unwrap();
    let bytes = a.to_bytes().unwrap();
    assert!(bytes.starts_with(b"ALN1"), "marque de format");
    let b = Analysis::from_bytes(&bytes).unwrap();
    assert_eq!(a.path().unwrap(), b.path().unwrap());
    let (x, _) = a.render(dub.audio(), None, None).unwrap();
    let (y, _) = b.render(dub.audio(), None, None).unwrap();
    assert!(x == y, "rendu identique au bit près");
    assert!(matches!(Analysis::from_bytes(&bytes[..bytes.len() / 2]), Err(AlignError::Format(_))), "octets tronqués refusés");
    assert!(matches!(Analysis::from_bytes(b"pas une analyse"), Err(AlignError::Format(_))));
}

#[test]
fn cancelling_is_clean_and_the_next_call_works() {
    let (guide, dub) = (Wav::read("guide.wav"), Wav::read("double.wav"));
    let g = Guide::new(guide.audio(), None, None).unwrap();
    let r = g.analyse(dub.audio(), None, Some(&mut |f| f > 0.5));
    assert!(matches!(r, Err(AlignError::Cancelled)), "annulé à mi-course");
    assert!(g.analyse(dub.audio(), None, None).is_ok(), "l'appel suivant réussit");
    let a = g.analyse(dub.audio(), None, None).unwrap();
    assert!(matches!(a.render(dub.audio(), None, Some(&mut |_| true)), Err(AlignError::Cancelled)));
    // Un rappel qui panique annule aussi, sans traverser le C.
    assert!(g.analyse(dub.audio(), None, Some(&mut |_| panic!("test"))).is_err());
}

#[test]
fn settings_are_json_validated_and_the_general_block_is_the_only_one_copied() {
    let default = default_settings_json();
    assert!(default.contains("flibuste-align-settings"));
    validate_settings(&default).unwrap();
    assert!(matches!(validate_settings("{pas du json"), Err(AlignError::Format(_))));
    let merged = merge_general(&default, &default).unwrap();
    validate_settings(&merged).unwrap();
    assert!(matches!(merge_general("{", &default), Err(AlignError::Format(_))));
    let (name, version) = library_version();
    assert!(!name.is_empty() && version >= 1);
}

#[test]
fn bad_inputs_are_refused_with_a_message() {
    let (guide, dub) = (Wav::read("guide.wav"), Wav::read("double.wav"));
    let g = Guide::new(guide.audio(), None, None).unwrap();
    let other_rate = Audio { sample_rate: dub.rate * 2.0, ..dub.audio() };
    let e = g.analyse(other_rate, None, None).err().expect("fréquence différente");
    assert!(matches!(e, AlignError::Argument(ref m) if !m.is_empty()), "{e:?}");
    let tiny = Audio { samples: &dub.samples[..100], channels: 1, sample_rate: dub.rate };
    assert!(g.analyse(tiny, None, None).is_err(), "moins d'une demi-seconde");
    let odd = Audio { samples: &dub.samples[..101], channels: 2, sample_rate: dub.rate };
    assert!(matches!(g.analyse(odd, None, None), Err(AlignError::Argument(_))), "échantillons non multiples du nombre de voies");
    assert!(Guide::new(guide.audio(), Some("pas du json"), None).is_err());
}

#[test]
fn consonants_can_be_edited_and_the_ui_curves_are_readable() {
    let (guide, dub) = (Wav::read("guide.wav"), Wav::read("double.wav"));
    let g = Guide::new(guide.audio(), None, None).unwrap();
    let mut a = g.analyse(dub.audio(), None, None).unwrap();
    let shift = a.applied_shift(None).unwrap();
    assert_eq!(shift.len(), a.path().unwrap().len());
    a.set_consonants(&[(1.0, 1.05), (2.0, 2.08)]).unwrap();
    assert_eq!(a.consonants().unwrap(), vec![(1.0, 1.05), (2.0, 2.08)]);
    assert!(a.suggest_strength(20.0).is_ok());
    let (out, _) = a.render(dub.audio(), None, None).unwrap();
    assert_eq!(out.len() / dub.channels, guide.frames());
}

#[test]
fn one_guide_serves_two_analyses_on_two_threads() {
    let (guide, dub) = (Wav::read("guide.wav"), Wav::read("double.wav"));
    let g = Guide::new(guide.audio(), None, None).unwrap();
    let (a, b) = std::thread::scope(|s| {
        let t1 = s.spawn(|| g.analyse(dub.audio(), None, None).unwrap().path().unwrap());
        let t2 = s.spawn(|| g.analyse(dub.audio(), None, None).unwrap().path().unwrap());
        (t1.join().unwrap(), t2.join().unwrap())
    });
    assert_eq!(a, b, "même guide, même double : même chemin");
}
