//! Commande `record` : enregistre l'entrée du pilote (avec écoute et signal de référence facultatifs), puis analyse la
//! prise. Sert d'outil de **calibration** (latence de bout en bout, à appliquer comme décalage d'enregistrement) et de
//! **contrôle d'intégrité** (coupures, clics) avec un câble de bouclage sortie → entrée.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use daw_audio_io::playback::ControlFn;
use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, RecordSource, TrackFormat};
use daw_recorder::analysis::{alignment, find_impulses, tone_integrity};
use daw_recorder::wav::read_wav;
use daw_recorder::Recording;

use crate::demo::EngineProc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference {
    /// Une impulsion toutes les 0,5 s : mesure la latence de bout en bout.
    Clicks,
    /// Sinusoïde de 1 kHz : cherche coupures et clics.
    Tone,
}

#[derive(Debug, Clone)]
pub struct RecordOptions {
    pub seconds: u32,
    pub file: PathBuf,
    pub source: RecordSource,
    pub monitor: bool,
    pub reference: Option<Reference>,
    /// Décalage d'enregistrement déjà connu (échantillons) : sert seulement à calculer l'erreur résiduelle affichée.
    pub offset: i64,
}

pub type Slot = Rc<RefCell<Option<Recording>>>;

const TONE_HZ: f64 = 1_000.0;
const TONE_LEVEL: f32 = 0.3;
const CLICK_LEVEL: f32 = 0.5;

/// Position (échantillons de la ligne de temps) de la k-ième impulsion de référence.
fn click_position(rate: u32, k: u64) -> u64 {
    (k + 1) * rate as u64 / 2
}

fn reference_clip(reference: Reference, rate: u32, seconds: u32) -> Clip {
    let n = rate as usize * seconds as usize;
    let mut x = vec![0.0f32; n];
    match reference {
        Reference::Clicks => {
            let mut k = 0;
            while (click_position(rate, k) as usize) < n.saturating_sub(rate as usize / 10) {
                x[click_position(rate, k) as usize] = CLICK_LEVEL;
                k += 1;
            }
        }
        Reference::Tone => {
            let fade = (rate as usize / 20).max(1); // 50 ms : pas de clic de début ni de fin
            for (i, v) in x.iter_mut().enumerate() {
                let env = (i as f32 / fade as f32).min(1.0).min((n - i) as f32 / fade as f32);
                *v = TONE_LEVEL * env * (std::f64::consts::TAU * TONE_HZ * i as f64 / rate as f64).sin() as f32;
            }
        }
    }
    Clip::whole(AudioData::new(rate, vec![x.clone(), x]).expect("deux canaux de même longueur"), 0)
}

/// Fabrique le moteur (piste « prise » armée, piste de référence éventuelle) et la fonction de contrôle qui arrête la
/// prise à l'heure. La prise est confiée à `slot` pour être terminée une fois le flux arrêté.
pub fn make(opts: &RecordOptions, slot: &Slot, rate: u32) -> (EngineProc, ControlFn) {
    let mut g = GraphDesc::new();
    let take_track = g.add_track("prise", TrackFormat::Stereo).0;
    if let Some(r) = opts.reference {
        let t = g.add_track("référence", TrackFormat::Stereo);
        g.add_clip(t, reference_clip(r, rate, opts.seconds));
    }
    let (engine, mut handle) = Engine::new(rate, 256);
    handle.load_graph(&g).expect("graphe valide");
    handle.send(Command::SetInput { node: take_track, armed: true, monitor: opts.monitor, source: opts.source });
    let receiver = handle.start_record(take_track, opts.source, 4.0).expect("file de commandes vide au départ");
    *slot.borrow_mut() = Some(Recording::start(receiver, &opts.file).expect("création du fichier de prise"));
    handle.send(Command::Play);
    let seconds = opts.seconds as f64;
    let mut stopped = false;
    let control: ControlFn = Box::new(move |elapsed| {
        handle.collect_garbage();
        if elapsed.as_secs_f64() >= seconds && !stopped {
            handle.stop_record(take_track);
            stopped = true;
        }
        true
    });
    (EngineProc(engine), control)
}

/// Termine la prise, la relit et affiche l'analyse. Retourne le code de sortie (0 : tout va bien).
pub fn report(opts: &RecordOptions, slot: &Slot, rate: u32) -> i32 {
    let Some(rec) = slot.borrow_mut().take() else {
        eprintln!("erreur : la prise n'a pas démarré");
        return 1;
    };
    let info = match rec.finish() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("erreur d'écriture de la prise : {e}");
            return 1;
        }
    };
    println!("\nPrise : {} ({} trames, {} canal(aux), {} Hz)", info.path.display(), info.frames, info.channels, info.sample_rate);
    println!("  départ à la position {} de la ligne de temps ; blocs perdus par le disque : {}", info.start_pos, info.overruns);
    let wav = match read_wav(&info.path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("erreur de relecture : {e}");
            return 1;
        }
    };
    let ch = wav.channel(0);
    let peak = ch.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    println!("  crête de l'entrée : {:.3} ({:.1} dBFS)", peak, 20.0 * peak.max(1e-9).log10());
    let ms = |frames: f64| frames * 1000.0 / rate as f64;
    let mut ok = info.overruns == 0;
    match opts.reference {
        None => {}
        Some(Reference::Clicks) => {
            if peak < 0.02 {
                println!("  aucun signal sur l'entrée : câble de bouclage branché ? gain d'entrée trop bas ?");
                return 1;
            }
            let found = find_impulses(&ch, 0.3 * peak, rate as usize / 10);
            let expected: Vec<usize> = (0..).map(|k| click_position(rate, k)).take_while(|&p| p + rate as u64 / 5 < info.frames + info.start_pos).map(|p| (p - info.start_pos) as usize).collect();
            let a = alignment(&expected, &found, rate as usize / 20);
            println!("\nImpulsions de référence : {} attendues, {} retrouvées, {} manquantes, {} parasites", expected.len(), a.matched, a.missing, a.extra);
            if a.matched > 0 {
                println!("  latence de bout en bout mesurée : {:.1} éch. ({:.3} ms)   min {} / max {} (dispersion {} éch.)", a.mean, ms(a.mean), a.min, a.max, a.max - a.min);
                println!("  => décalage d'enregistrement à appliquer : {:.0} échantillons", a.mean.round());
                println!("  erreur résiduelle avec le décalage {} donné : {:+.1} éch.", opts.offset, a.mean - opts.offset as f64);
            }
            ok &= a.matched > 0 && a.missing == 0 && a.extra == 0 && a.max - a.min <= 1;
        }
        Some(Reference::Tone) => {
            let skip = rate as usize / 2;
            if ch.len() <= 2 * skip || peak < 0.02 {
                println!("  signal trop court ou absent : câble de bouclage branché ?");
                return 1;
            }
            let body = &ch[skip..ch.len() - skip / 2];
            let r = tone_integrity(body, rate, TONE_HZ, 0.05);
            println!("\nTonalité de {TONE_HZ} Hz : amplitude {:.4}, écart résiduel {:.1} dB, accidents détectés : {}", r.amplitude, r.residual_db, r.glitches.len());
            for g in r.glitches.iter().take(10) {
                println!("  accident à {:.3} s de la prise", (g + skip) as f64 / rate as f64);
            }
            ok &= r.glitches.is_empty();
        }
    }
    println!("{}", if ok { "\nRésultat : OK" } else { "\nRésultat : PROBLÈME (voir ci-dessus)" });
    i32::from(!ok)
}
