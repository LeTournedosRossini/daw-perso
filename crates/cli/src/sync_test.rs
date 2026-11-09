//! Commande `sync-test` : une prise rebouclée revient-elle **exactement** à sa place ?
//!
//! Un bruit large bande (déterministe) est joué par une piste du vrai moteur sur la sortie 1 de la carte ; un câble le ramène sur
//! l'entrée 1 ; une piste armée l'enregistre comme le fait l'application (toutes les entrées du pilote ouvertes, une piste = un
//! canal). On range alors la prise sur la ligne de temps **avec la même compensation que l'application** (latences annoncées par le
//! pilote + délai des convertisseurs, `playback::record_compensation`) et on mesure, par corrélation croisée avec l'original, de
//! combien d'échantillons la prise est en avance ou en retard, et ce qui reste quand on soustrait l'original (profondeur d'annulation).
//!
//! Avec `--fx NOM` la piste jouée traverse un plugin (de Jean) qui annonce une latence : on voit alors si la compensation des
//! plugins du moteur (PDC) ranger bien les prises. Avec `--ahead` / `--threads`, le moteur tourne comme l'application réglée ainsi.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use daw_audio_io::playback::{record_compensation, ControlFn, StreamInfo};
use daw_clap_host::clap_insert_factory;
use daw_engine::{AheadConfig, AudioData, Clip, Command, Engine, GraphDesc, RecordSource, TrackFormat};
use daw_recorder::wav::read_wav;
use daw_recorder::Recording;

use crate::demo::EngineProc;
use crate::load;

#[derive(Debug, Clone)]
pub struct SyncOptions {
    pub seconds: u32,
    pub file: PathBuf,
    /// Entrée à enregistrer (1 = la première).
    pub in_channel: u32,
    /// Plugin de Jean placé sur la piste jouée (sa latence annoncée met la compensation du moteur à l'épreuve).
    pub fx: Option<String>,
    /// Calcul en avance (millisecondes, fils de rendu).
    pub ahead: Option<(u32, usize)>,
    /// Fils de calcul (fil audio compris).
    pub threads: usize,
    /// Demi-largeur de la plage de retards essayée autour de la compensation (échantillons).
    pub window: u32,
}

/// Tout ce que la fonction de contrôle apprend pendant la lecture et dont le rapport a besoin.
#[derive(Default)]
pub struct Shared {
    pub rec: Option<Recording>,
    /// Latences annoncées par le pilote (entrée, sortie), lues après le démarrage du flux.
    pub latency: Option<(u32, u32)>,
    /// Latence ajoutée par les plugins (compensation du graphe).
    pub graph_latency: u32,
    pub driver: String,
    pub buffer: u32,
    pub inputs: u32,
}

pub type Slot = Rc<RefCell<Shared>>;

/// Bande passante du bruit de référence (Hz) : les convertisseurs d'une carte coupent vers 20 kHz ; un bruit blanc jusqu'à Nyquist
/// perdrait ~9 % de son énergie dans la boucle et limiterait la profondeur d'annulation à −10 dB quel que soit l'alignement.
pub const BAND_HZ: f64 = 16_000.0;

/// Bruit déterministe (xorshift64*) limité à `BAND_HZ` par un filtre à phase linéaire (sinc fenêtrée de 255 coefficients, retard
/// de groupe compensé), 50 ms de fondu au début et à la fin, niveau efficace ≈ 0,1 (crête < 0,5).
pub fn reference_signal(rate: u32, seconds: u32) -> Vec<f32> {
    let n = rate as usize * seconds as usize;
    let fade = (rate as usize / 20).max(1);
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let white: Vec<f64> = (0..n)
        .map(|_| {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f64 / (1u64 << 24) as f64 * 2.0 - 1.0
        })
        .collect();
    let taps = 255usize;
    let half = (taps / 2) as i64;
    let fc = (BAND_HZ / rate as f64).min(0.45);
    let h: Vec<f64> = (0..taps as i64)
        .map(|k| {
            let t = (k - half) as f64;
            let sinc = if t == 0.0 { 2.0 * fc } else { (std::f64::consts::TAU * fc * t).sin() / (std::f64::consts::PI * t) };
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / (taps - 1) as f64).cos();
            sinc * w
        })
        .collect();
    let filtered: Vec<f64> = (0..n as i64)
        .map(|i| h.iter().enumerate().map(|(k, c)| { let j = i + k as i64 - half; if j >= 0 && (j as usize) < n { c * white[j as usize] } else { 0.0 } }).sum())
        .collect();
    let rms = (filtered.iter().map(|v| v * v).sum::<f64>() / n.max(1) as f64).sqrt().max(1e-12);
    let gain = 0.1 / rms;
    filtered.iter().enumerate().map(|(i, v)| (gain * v) as f32 * (i as f32 / fade as f32).min(1.0).min((n - i) as f32 / fade as f32)).collect()
}

/// Fabrique le moteur (piste « prise » armée sur l'entrée choisie, piste de référence) et la fonction de contrôle.
pub fn make(opts: &SyncOptions, info: &StreamInfo, slot: &Slot) -> Result<(EngineProc, ControlFn), String> {
    let rate = info.sample_rate;
    let mut g = GraphDesc::new();
    let take = g.add_track("prise", TrackFormat::Stereo).0;
    let reference = g.add_track("référence", TrackFormat::Stereo);
    let x = reference_signal(rate, opts.seconds);
    g.add_clip(reference, Clip::whole(AudioData::new(rate, vec![x.clone(), x]).expect("deux canaux de même longueur"), 0));
    if let Some(name) = &opts.fx {
        let cat = load::catalog();
        let f = load::find(&cat, name).ok_or_else(|| format!("plugin « {name} » introuvable parmi ceux de Jean ({})", cat.iter().map(|f| f.info.name.as_str()).collect::<Vec<_>>().join(", ")))?;
        g.add_insert(reference, &f.info.name, clap_insert_factory(Arc::clone(&f.bundle), f.info.id.clone())).expect("piste créée");
    }
    let (mut engine, mut handle) = Engine::new(rate, 256);
    if let Some(p) = load::make_pool(opts.threads) {
        engine.set_pool(p);
    }
    if let Some((ms, n)) = opts.ahead {
        engine.enable_ahead(&mut handle, &AheadConfig { lookahead_ms: ms, block: 256, threads: n.max(1), on_start: Some(Arc::new(|_| { let _ = daw_audio_io::rt_priority::promote_current_thread(); })) });
    }
    handle.load_graph(&g).map_err(|e| format!("graphe refusé : {e:?}"))?;
    let source = RecordSource::Channel(opts.in_channel.saturating_sub(1).min(255) as u8);
    handle.send(Command::SetInput { node: take, armed: true, monitor: false, source });
    let receiver = handle.start_record(take, source, 4.0).map_err(|_| "file de commandes pleine au départ".to_string())?;
    {
        let mut s = slot.borrow_mut();
        s.rec = Some(Recording::start(receiver, &opts.file).map_err(|e| format!("création du fichier de prise : {e}"))?);
        s.graph_latency = handle.latency_samples();
        s.driver = info.driver.clone();
        s.buffer = info.buffer_size;
        s.inputs = info.inputs;
    }
    handle.send(Command::Play);
    let (slot2, latency, stop_at) = (slot.clone(), Arc::clone(&info.latency), opts.seconds as f64 + 0.4);
    let mut stopped = false;
    let control: ControlFn = Box::new(move |elapsed| {
        handle.collect_garbage();
        if slot2.borrow().latency.is_none() {
            slot2.borrow_mut().latency = latency.get();
        }
        if elapsed.as_secs_f64() >= stop_at && !stopped {
            handle.stop_record(take);
            stopped = true;
        }
        true
    });
    Ok((EngineProc(engine), control))
}

/// Résultat de la corrélation d'une prise avec l'original.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncReport {
    /// Retard de bout en bout mesuré (échantillons, interpolé) : l'échantillon `j` de la prise contient `original[départ + j − retard]`.
    pub lag: f64,
    pub lag_int: i64,
    /// Coefficient de corrélation au retard entier retenu (1 : même signal).
    pub corr: f64,
    /// Gain de la boucle (prise / original) en dB.
    pub gain_db: f64,
    /// Énergie restante après soustraction de l'original (au gain et au retard retenus), par rapport à celle de la prise, en dB.
    pub null_db: f64,
    /// Retard mesuré sur trois tiers de la prise : s'il dérive, les deux horloges ne marchent pas ensemble.
    pub segments: [f64; 3],
    /// Corrélation sur chacun des trois tiers (au retard retenu pour ce tiers) : un tiers abîmé (coupure, glissement d'un échantillon)
    /// ressort ici alors que le retard global paraît presque normal.
    pub segment_corr: [f64; 3],
    /// Le pic est au bord de la plage essayée : le vrai retard est peut-être au-delà.
    pub at_edge: bool,
}

fn dot(rec: &[f32], reference: &[f32], j0: usize, j1: usize, start: i64, lag: i64) -> f64 {
    let r0 = (start + j0 as i64 - lag) as usize;
    rec[j0..j1].iter().zip(&reference[r0..r0 + (j1 - j0)]).map(|(a, b)| *a as f64 * *b as f64).sum()
}

fn parabola(cm: f64, c0: f64, cp: f64) -> f64 {
    let d = cm - 2.0 * c0 + cp;
    if d.abs() < 1e-30 {
        0.0
    } else {
        (0.5 * (cm - cp) / d).clamp(-0.5, 0.5)
    }
}

/// Retard de la prise sur l'original : cherche, parmi les retards entiers de `center ± window`, le maximum de la corrélation croisée
/// (zone de la prise où tous les retards essayés tombent dans l'original, sans les `skip` premiers et derniers échantillons), puis
/// affine par une parabole. `None` si la zone est trop courte ou le signal absent.
pub fn analyze_sync(rec: &[f32], reference: &[f32], start_pos: u64, center: i64, window: i64, skip: usize) -> Option<SyncReport> {
    let start = start_pos as i64;
    let j0 = skip.max((center + window - start).max(0) as usize);
    let j1_signed = (rec.len().saturating_sub(skip) as i64).min(reference.len() as i64 + center - window - start);
    if j1_signed <= j0 as i64 + 3 * 256 {
        return None;
    }
    let j1 = j1_signed as usize;
    let lags: Vec<i64> = (center - window..=center + window).collect();
    let c: Vec<f64> = lags.iter().map(|&l| dot(rec, reference, j0, j1, start, l)).collect();
    let (best, &cmax) = c.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    if cmax <= 0.0 {
        return None;
    }
    let at_edge = best == 0 || best == c.len() - 1;
    let l0 = lags[best];
    let frac = if at_edge { 0.0 } else { parabola(c[best - 1], c[best], c[best + 1]) };
    let srr: f64 = rec[j0..j1].iter().map(|v| (*v as f64).powi(2)).sum();
    let r0 = (start + j0 as i64 - l0) as usize;
    let sff: f64 = reference[r0..r0 + (j1 - j0)].iter().map(|v| (*v as f64).powi(2)).sum();
    let g = cmax / sff;
    let residual = (srr - cmax * cmax / sff).max(0.0);
    let third = (j1 - j0) / 3;
    let mut segments = [0.0; 3];
    let mut segment_corr = [0.0; 3];
    for (k, seg) in segments.iter_mut().enumerate() {
        let (a, b) = (j0 + k * third, j0 + (k + 1) * third);
        let cs: Vec<f64> = (l0 - 3..=l0 + 3).map(|l| dot(rec, reference, a, b, start, l)).collect();
        let m = cs.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)).map(|(i, _)| i).unwrap_or(3);
        *seg = (l0 - 3 + m as i64) as f64 + if m == 0 || m == 6 { 0.0 } else { parabola(cs[m - 1], cs[m], cs[m + 1]) };
        let lag = l0 - 3 + m as i64;
        let e_rec: f64 = rec[a..b].iter().map(|v| (*v as f64).powi(2)).sum();
        let r_a = (start + a as i64 - lag) as usize;
        let e_ref: f64 = reference[r_a..r_a + (b - a)].iter().map(|v| (*v as f64).powi(2)).sum();
        segment_corr[k] = cs[m] / (e_rec * e_ref).sqrt().max(1e-30);
    }
    Some(SyncReport { lag: l0 as f64 + frac, lag_int: l0, corr: cmax / (srr * sff).sqrt(), gain_db: 20.0 * g.abs().max(1e-12).log10(), null_db: 10.0 * (residual / srr).max(1e-30).log10(), segments, segment_corr, at_edge })
}

/// Tolérance sur l'alignement (échantillons) : au-delà, la prise est audiblement décalée dans les aigus.
pub const TOLERANCE: f64 = 3.0;

/// Termine la prise, la range avec la compensation de l'application et affiche l'analyse. Code de sortie : 0 si tout va bien.
pub fn report(opts: &SyncOptions, slot: &Slot, rate: u32) -> i32 {
    let (rec, latency, graph_latency, driver, buffer, inputs) = {
        let mut s = slot.borrow_mut();
        (s.rec.take(), s.latency, s.graph_latency, s.driver.clone(), s.buffer, s.inputs)
    };
    let Some(rec) = rec else {
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
    let Some((lat_in, lat_out)) = latency else {
        eprintln!("erreur : le pilote n'a pas annoncé ses latences");
        return 1;
    };
    // Compensation de l'application : latence de la carte (annoncée + convertisseurs) puis latence des plugins (la lecture que le
    // musicien entend en est retardée), relevée au départ de la prise (`Host::finish_recording`).
    let card = record_compensation(lat_in, lat_out);
    let comp = card + graph_latency as u64;
    let ms = |s: f64| s * 1000.0 / rate as f64;
    println!("\nPilote : {driver}, {rate} Hz, tampon {buffer}, {inputs} entrée(s) ; prise sur l'entrée {}", opts.in_channel);
    println!("Latences annoncées : {lat_in} en entrée + {lat_out} en sortie + {} (convertisseurs) = {card} échantillons ({:.3} ms) pour la carte", daw_audio_io::playback::CONVERTER_DELAY, ms(card as f64));
    if graph_latency > 0 {
        println!("Latence ajoutée par les plugins (compensation du moteur) : {graph_latency} échantillons ({:.3} ms)", ms(graph_latency as f64));
    }
    println!("Compensation de l'application : {comp} échantillons ({:.3} ms)", ms(comp as f64));
    println!("Prise : {} trames, départ à la position {}, blocs perdus par le disque : {}", info.frames, info.start_pos, info.overruns);
    let wav = match read_wav(&info.path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("erreur de relecture : {e}");
            return 1;
        }
    };
    let take = wav.channel(0);
    let peak = take.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    println!("Crête de l'entrée : {peak:.3} ({:.1} dBFS)", 20.0 * peak.max(1e-9).log10());
    if peak < 0.02 {
        println!("Aucun signal sur l'entrée : câble de bouclage (sortie 1 → entrée {}) branché ? gain d'entrée trop bas ?", opts.in_channel);
        return 1;
    }
    let reference = reference_signal(rate, opts.seconds);
    let window = opts.window as i64;
    let Some(r) = analyze_sync(&take, &reference, info.start_pos, comp as i64, window, rate as usize / 2) else {
        println!("Analyse impossible : prise trop courte ou signal sans rapport avec la référence.");
        return 1;
    };
    // Une prise rangée avec la compensation `comp` est en retard de (retard mesuré − comp) échantillons sur l'original.
    let error = r.lag - comp as f64;
    let spread = r.segments.iter().cloned().fold(f64::MIN, f64::max) - r.segments.iter().cloned().fold(f64::MAX, f64::min);
    println!("\nRetard de bout en bout mesuré : {:.2} échantillons ({:.3} ms) ; corrélation {:.4} ; gain de la boucle {:+.2} dB", r.lag, ms(r.lag), r.corr, r.gain_db);
    println!("Retard sur les trois tiers de la prise : {:.2} / {:.2} / {:.2} (dérive {:.2} éch.) ; corrélation par tiers : {:.4} / {:.4} / {:.4}", r.segments[0], r.segments[1], r.segments[2], spread, r.segment_corr[0], r.segment_corr[1], r.segment_corr[2]);
    println!("Après soustraction de l'original (retard {} éch., gain ajusté) : il reste {:.1} dB de la prise", r.lag_int, r.null_db);
    println!("\n=> Rangée avec la compensation de l'application ({comp} éch.), la prise tombe {:+.2} échantillon(s) ({:+.3} ms) par rapport à l'original{}.", error, ms(error), if error > 0.0 { " (en retard)" } else if error < 0.0 { " (en avance)" } else { "" });
    if graph_latency > 0 {
        println!("   Avec la seule latence de la carte (comportement d'avant la correction du 7 novembre), la prise serait tombée {:+.2} échantillon(s) ({:+.3} ms) par rapport à l'original.", r.lag - card as f64, ms(r.lag - card as f64));
    }
    let ok = r.corr > 0.9 && !r.at_edge && error.abs() <= TOLERANCE && spread <= 1.0 && info.overruns == 0;
    if r.at_edge {
        println!("   Le pic est au bord de la plage essayée (±{} éch.) : élargir avec --window.", opts.window);
    }
    println!("{}", if ok { "\nRésultat : OK (alignement à moins de 3 échantillons, sans dérive)" } else { "\nRésultat : PROBLÈME ou écart au-delà de la tolérance de 3 échantillons (voir ci-dessus)" });
    i32::from(!ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signal continu à bande limitée (somme de sinusoïdes lentes) : on peut l'évaluer à un instant fractionnaire.
    fn smooth(t: f64) -> f32 {
        let w = [0.011, 0.037, 0.083, 0.151, 0.229, 0.307];
        (w.iter().enumerate().map(|(i, w)| (w * t + i as f64).sin() / (1.0 + i as f64 * 0.3)).sum::<f64>() * 0.2) as f32
    }

    fn pair(delay: f64, start: u64, gain: f32) -> (Vec<f32>, Vec<f32>) {
        let reference: Vec<f32> = (0..30_000).map(|i| smooth(i as f64)).collect();
        let rec: Vec<f32> = (0..20_000).map(|j| gain * smooth(start as f64 + j as f64 - delay)).collect();
        (reference, rec)
    }

    #[test]
    fn an_exact_delay_is_found_with_a_deep_null() {
        let (reference, rec) = pair(123.0, 64, 0.5);
        let r = analyze_sync(&rec, &reference, 64, 100, 60, 500).unwrap();
        assert_eq!(r.lag_int, 123);
        assert!((r.lag - 123.0).abs() < 0.01, "{}", r.lag);
        assert!(r.corr > 0.9999 && r.null_db < -60.0, "{r:?}");
        assert!((r.gain_db - (-6.02)).abs() < 0.05, "{}", r.gain_db);
        assert!(r.segments.iter().all(|s| (s - 123.0).abs() < 0.05), "{:?}", r.segments);
        assert!(r.segment_corr.iter().all(|c| *c > 0.9999), "{:?}", r.segment_corr);
        assert!(!r.at_edge);
    }

    #[test]
    fn a_fractional_delay_is_measured_to_a_tenth_of_a_sample() {
        for delay in [123.4, 98.75, 140.1] {
            let (reference, rec) = pair(delay, 0, 1.0);
            let r = analyze_sync(&rec, &reference, 0, 120, 60, 500).unwrap();
            assert!((r.lag - delay).abs() < 0.1, "délai {delay} mesuré {}", r.lag);
        }
    }

    #[test]
    fn noise_in_the_take_lowers_the_null_but_not_the_delay() {
        let (reference, mut rec) = pair(77.0, 0, 1.0);
        let mut s = 12345u32;
        for v in rec.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *v += ((s >> 8) as f32 / 16_777_216.0 - 0.5) * 0.02;
        }
        let r = analyze_sync(&rec, &reference, 0, 80, 40, 500).unwrap();
        assert!((r.lag - 77.0).abs() < 0.1);
        assert!(r.null_db < -15.0 && r.null_db > -60.0, "{}", r.null_db);
    }

    #[test]
    fn a_peak_at_the_window_edge_is_reported_and_silence_or_unrelated_signals_give_nothing() {
        let (reference, rec) = pair(150.0, 0, 1.0);
        let r = analyze_sync(&rec, &reference, 0, 100, 50, 500).unwrap();
        assert!(r.at_edge, "le vrai retard (150) est au bord de la plage 50..150");
        assert!(analyze_sync(&vec![0.0; 20_000], &reference, 0, 100, 50, 500).is_none());
        assert!(analyze_sync(&rec[..1_000], &reference, 0, 100, 50, 500).is_none());
    }

    #[test]
    fn the_reference_noise_is_deterministic_bounded_and_fades() {
        let a = reference_signal(48_000, 2);
        assert_eq!(a, reference_signal(48_000, 2));
        assert_eq!(a.len(), 96_000);
        assert!(a.iter().all(|v| v.abs() < 0.5), "crête {}", a.iter().fold(0.0f32, |m, v| m.max(v.abs())));
        assert!(a[0].abs() < 0.001 && a[a.len() - 1].abs() < 0.001);
        let rms = (a[5_000..90_000].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / 85_000.0).sqrt();
        assert!((rms - 0.1).abs() < 0.01, "{rms}");
        // Bande limitée : presque rien au-dessus de 18 kHz (énergie des différences d'ordre 1 comme mesure grossière des hautes fréquences).
        let hf = (5_000..90_000).map(|i| ((a[i] - 2.0 * a[i - 1] + a[i - 2]) as f64).powi(2)).sum::<f64>() / 85_000.0;
        let white_hf = 6.0 * 0.01; // un bruit blanc de même niveau donnerait ≈ 6 × variance
        assert!(hf < 0.5 * white_hf, "{hf}");
    }
}
