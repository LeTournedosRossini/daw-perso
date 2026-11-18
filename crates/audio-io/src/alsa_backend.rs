//! Backend ALSA (Linux) : la carte son directement, sans serveur JACK.
//!
//! Sous PipeWire, le périphérique « default » (ou « pipewire ») passe par le greffon ALSA de PipeWire : l'application marche donc sans
//! `pw-jack`. Un périphérique « hw:… » ouvre la carte elle-même (il faut qu'aucun autre programme, PipeWire compris, la tienne) et donne la
//! latence la plus basse — mais **il n'y a pas d'objectif de basse latence sous Linux** (décision du 4 novembre 2026) : on choisit la
//! simplicité.
//!
//! Un fil « audio » fait tourner le cycle : lecture bloquante d'une période de capture, traitement par le [`StereoProcessor`], écriture
//! bloquante d'une période de lecture. Les tampons sont alloués avant le départ (l'allocateur surveillé le vérifie). Les dépassements
//! (`EPIPE`) sont comptés et rattrapés. Le fil essaie `SCHED_FIFO` (accordé seulement si les limites du système le permettent).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alsa::device_name::HintIter;
use alsa::pcm::{Access, Format, HwParams, IO, PCM};
use alsa::{Direction, ValueOr};

use crate::alloc_guard::{self, RtScope};
use crate::capture_ring::CaptureRing;
use crate::playback::{ControlFn, PlaybackReport, StereoProcessor, StreamInfo, StreamLatency, TimedProcessor};

/// Nombre maximal de canaux ouverts dans un sens.
pub const MAX_CHANNELS: usize = 32;

/// Un périphérique ALSA qu'on peut proposer à l'utilisateur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEntry {
    /// Nom à donner à ALSA (`default`, `pipewire`, `hw:CARD=PCH,DEV=0`…).
    pub name: String,
    /// Première ligne de la description donnée par ALSA.
    pub description: String,
    pub playback: bool,
    pub capture: bool,
}

/// Périphériques utiles : `default`, `pipewire`, `pulse`, et les cartes en accès direct (`hw:` / `plughw:`). Les variantes surround,
/// dmix, iec958, front… ne servent pas à une station audio : elles sont écartées.
pub fn list_devices() -> Vec<DeviceEntry> {
    let Ok(hints) = HintIter::new_str(None, "pcm") else { return Vec::new() };
    let mut out: Vec<DeviceEntry> = Vec::new();
    for h in hints {
        let Some(name) = h.name else { continue };
        let keep = matches!(name.as_str(), "default" | "pipewire" | "pulse") || name.starts_with("hw:") || name.starts_with("plughw:");
        if !keep || out.iter().any(|d| d.name == name) {
            continue;
        }
        let (playback, capture) = match h.direction {
            None => (true, true),
            Some(Direction::Playback) => (true, false),
            Some(Direction::Capture) => (false, true),
        };
        let description = h.desc.unwrap_or_default().lines().next().unwrap_or("").trim().to_owned();
        out.push(DeviceEntry { name, description, playback, capture });
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct PlayOptions {
    pub seconds: u32,
    /// Périphérique de lecture (sinon `default`).
    pub device: Option<String>,
    /// Périphérique de capture (sinon le même que la lecture).
    pub capture_device: Option<String>,
    /// Taille de la période, en échantillons (sinon 256).
    pub buffer: Option<u32>,
    /// Fréquence demandée (sinon 48 000 Hz) ; le périphérique peut en donner une autre.
    pub rate: Option<u32>,
    /// Nombre de canaux d'entrée ouverts (plafonné à ce que le périphérique a) ; 0 : pas de capture.
    pub input_count: u32,
    /// Nombre de canaux de sortie ouverts (au moins 2, plafonné à ce que le périphérique a).
    pub output_count: u32,
}

/// Format de transfert choisi pour un périphérique.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sample {
    F32,
    S32,
    S16,
}

struct Stream {
    pcm: PCM,
    sample: Sample,
    channels: usize,
    period: usize,
    rate: u32,
}

fn open_stream(name: &str, dir: Direction, rate: u32, channels_wanted: usize, period: u32, nonblock: bool) -> Result<Stream, String> {
    let what = if dir == Direction::Playback { "lecture" } else { "capture" };
    let pcm = PCM::new(name, dir, nonblock).map_err(|e| format!("ouverture de « {name} » ({what}) impossible : {e}"))?;
    let (sample, channels, period_got, rate_got);
    {
        let hwp = HwParams::any(&pcm).map_err(|e| format!("« {name} » : paramètres : {e}"))?;
        hwp.set_access(Access::RWInterleaved).map_err(|e| format!("« {name} » : accès entrelacé refusé : {e}"))?;
        sample = [(Format::FloatLE, Sample::F32), (Format::S32LE, Sample::S32), (Format::S16LE, Sample::S16)]
            .into_iter()
            .find(|(f, _)| hwp.set_format(*f).is_ok())
            .map(|(_, s)| s)
            .ok_or_else(|| format!("« {name} » : aucun format (flottant, 32 ou 16 bits) accepté"))?;
        let max = hwp.get_channels_max().unwrap_or(2).max(1) as usize;
        let min = hwp.get_channels_min().unwrap_or(1).max(1) as usize;
        channels = channels_wanted.clamp(min, max.min(MAX_CHANNELS).max(min));
        hwp.set_channels(channels as u32).map_err(|e| format!("« {name} » : {channels} canaux refusés : {e}"))?;
        rate_got = hwp.set_rate_near(rate, ValueOr::Nearest).map(|_| hwp.get_rate().unwrap_or(rate)).map_err(|e| format!("« {name} » : fréquence {rate} Hz refusée : {e}"))?;
        // Période demandée, tampon de quatre périodes pour absorber la gigue du système (pas de basse latence ici).
        hwp.set_period_size_near(period as alsa::pcm::Frames, ValueOr::Nearest).map_err(|e| format!("« {name} » : période : {e}"))?;
        hwp.set_buffer_size_near(period as alsa::pcm::Frames * 4).map_err(|e| format!("« {name} » : tampon : {e}"))?;
        pcm.hw_params(&hwp).map_err(|e| format!("« {name} » : {e}"))?;
        period_got = hwp.get_period_size().map_err(|e| e.to_string())? as usize;
    }
    {
        let hwp = pcm.hw_params_current().map_err(|e| e.to_string())?;
        let swp = pcm.sw_params_current().map_err(|e| e.to_string())?;
        // On démarre nous-mêmes, une fois la période de départ écrite.
        swp.set_start_threshold(hwp.get_buffer_size().map_err(|e| e.to_string())? * 2).map_err(|e| e.to_string())?;
        pcm.sw_params(&swp).map_err(|e| e.to_string())?;
    }
    Ok(Stream { pcm, sample, channels, period: period_got, rate: rate_got })
}

/// Lecture / écriture entrelacées convertissant depuis et vers des `f32` planaires.
enum Io<'a> {
    F32(IO<'a, f32>),
    S32(IO<'a, i32>),
    S16(IO<'a, i16>),
}

/// Tampons de transfert (alloués avant le départ).
struct Bufs {
    f32: Vec<f32>,
    i32: Vec<i32>,
    i16: Vec<i16>,
}

impl Bufs {
    fn new(sample: Sample, len: usize) -> Bufs {
        match sample {
            Sample::F32 => Bufs { f32: vec![0.0; len], i32: Vec::new(), i16: Vec::new() },
            Sample::S32 => Bufs { f32: vec![0.0; len], i32: vec![0; len], i16: Vec::new() },
            Sample::S16 => Bufs { f32: vec![0.0; len], i32: Vec::new(), i16: vec![0; len] },
        }
    }
}

/// Lit sans attendre ce que la capture a déjà (au plus `max_frames` trames) et l'ajoute à `ring`. Rend le nombre de trames lues.
fn read_available(io: &Io<'_>, bufs: &mut Bufs, channels: usize, max_frames: usize, ring: &mut CaptureRing) -> alsa::Result<usize> {
    let n = match io {
        Io::F32(io) => io.readi(&mut bufs.f32[..max_frames * channels])?,
        Io::S32(io) => {
            let n = io.readi(&mut bufs.i32[..max_frames * channels])?;
            for (d, s) in bufs.f32[..n * channels].iter_mut().zip(&bufs.i32[..n * channels]) {
                *d = *s as f32 * (1.0 / 2_147_483_648.0);
            }
            n
        }
        Io::S16(io) => {
            let n = io.readi(&mut bufs.i16[..max_frames * channels])?;
            for (d, s) in bufs.f32[..n * channels].iter_mut().zip(&bufs.i16[..n * channels]) {
                *d = *s as f32 * (1.0 / 32_768.0);
            }
            n
        }
    };
    ring.push(&bufs.f32, channels, n);
    Ok(n)
}

/// Entrelace les canaux planaires (écrêtés à ±1 pour les formats entiers) et écrit une période de lecture.
fn write_period(io: &Io<'_>, bufs: &mut Bufs, channels: usize, planar: &[Vec<f32>]) -> alsa::Result<()> {
    let period = planar.first().map_or(0, |p| p.len());
    match io {
        Io::F32(io) => {
            for (c, p) in planar.iter().enumerate() {
                for (i, v) in p.iter().enumerate() {
                    bufs.f32[i * channels + c] = if v.is_finite() { *v } else { 0.0 };
                }
            }
            let mut done = 0;
            while done < period {
                done += io.writei(&bufs.f32[done * channels..period * channels])?;
            }
        }
        Io::S32(io) => {
            for (c, p) in planar.iter().enumerate() {
                for (i, v) in p.iter().enumerate() {
                    bufs.i32[i * channels + c] = (if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 } * 2_147_483_647.0) as i32;
                }
            }
            let mut done = 0;
            while done < period {
                done += io.writei(&bufs.i32[done * channels..period * channels])?;
            }
        }
        Io::S16(io) => {
            for (c, p) in planar.iter().enumerate() {
                for (i, v) in p.iter().enumerate() {
                    bufs.i16[i * channels + c] = (if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 } * 32_767.0) as i16;
                }
            }
            let mut done = 0;
            while done < period {
                done += io.writei(&bufs.i16[done * channels..period * channels])?;
            }
        }
    }
    Ok(())
}

fn io_of(s: &Stream) -> Result<Io<'_>, String> {
    match s.sample {
        Sample::F32 => s.pcm.io_f32().map(Io::F32),
        Sample::S32 => s.pcm.io_i32().map(Io::S32),
        Sample::S16 => s.pcm.io_i16().map(Io::S16),
    }
    .map_err(|e| e.to_string())
}

/// `SCHED_FIFO` pour le fil courant, si le système l'accorde (limite `rtprio`, ou service rtkit non utilisé ici). Rend vrai si accepté.
fn try_realtime() -> bool {
    #[repr(C)]
    struct SchedParam {
        sched_priority: i32,
    }
    extern "C" {
        fn sched_setscheduler(pid: i32, policy: i32, param: *const SchedParam) -> i32;
    }
    const SCHED_FIFO: i32 = 1;
    let p = SchedParam { sched_priority: 50 };
    // SAFETY: appel système sans effet mémoire : `p` est une structure C valide qui vit pendant l'appel, `pid` 0 désigne le fil appelant.
    unsafe { sched_setscheduler(0, SCHED_FIFO, &p) == 0 }
}

/// Joue (et enregistre) par ALSA : même contrat que les autres backends. `make` reçoit ce qu'on sait du flux et rend le processeur et la
/// fonction de contrôle (appelée toutes les ~10 ms sur le fil appelant, hors temps réel).
pub fn run_playback_ex<P: StereoProcessor>(opts: &PlayOptions, make: impl FnOnce(&StreamInfo) -> (P, ControlFn)) -> Result<PlaybackReport, String> {
    let dev_out = opts.device.clone().unwrap_or_else(|| "default".into());
    let period_req = opts.buffer.unwrap_or(256).clamp(16, 8192);
    let rate_req = opts.rate.unwrap_or(48_000);
    let n_out_req = (if opts.output_count == 0 { 2 } else { opts.output_count.max(2) }) as usize;
    let play = open_stream(&dev_out, Direction::Playback, rate_req, n_out_req, period_req, false)?;
    if play.channels < 2 {
        return Err(format!("« {dev_out} » n'a qu'une sortie : au moins deux sont nécessaires"));
    }
    // Capture : sur la même fréquence et la même période que la lecture ; un échec n'empêche pas de jouer (comme CoreAudio).
    let dev_in = opts.capture_device.clone().unwrap_or_else(|| dev_out.clone());
    let capture = if opts.input_count > 0 {
        match open_stream(&dev_in, Direction::Capture, play.rate, opts.input_count as usize, play.period as u32, true) {
            Ok(c) if c.rate == play.rate && c.period == play.period => Some(c),
            Ok(c) => {
                eprintln!("capture « {dev_in} » ignorée : fréquence {} Hz / période {} différentes de la lecture ({} Hz / {})", c.rate, c.period, play.rate, play.period);
                None
            }
            Err(e) => {
                eprintln!("capture ignorée : {e}");
                None
            }
        }
    } else {
        None
    };
    let period = play.period;
    let rate = play.rate;
    let n_out = play.channels;
    let n_in = capture.as_ref().map_or(0, |c| c.channels);
    let latency = Arc::new(StreamLatency::default());
    let info = StreamInfo { driver: format!("ALSA « {dev_out} »"), sample_rate: rate, buffer_size: period as u32, inputs: n_in as u32, outputs: n_out as u32, latency: Arc::clone(&latency) };
    let (processor, mut control) = make(&info);

    let stop = Arc::new(AtomicBool::new(false));
    let xruns = Arc::new(AtomicU32::new(0));
    let cycles = Arc::new(std::sync::atomic::AtomicU64::new(0));
    alloc_guard::reset_violations();
    let cycles2 = Arc::clone(&cycles);
    let peak = Arc::new(AtomicU32::new(0));
    let peak2 = Arc::clone(&peak);
    let debug_on = std::env::var_os("DAW_DEBUG_AUDIO").is_some();
    let (stop2, xruns2, latency2) = (Arc::clone(&stop), Arc::clone(&xruns), Arc::clone(&latency));
    let worker = std::thread::Builder::new()
        .name("daw-alsa".into())
        .spawn(move || -> Option<crate::timing::TimingSummary> {
            let realtime = try_realtime();
            if !realtime {
                eprintln!("ALSA : priorité temps réel refusée par le système (limite rtprio) : le cycle audio tourne en priorité normale");
            }
            let play_io = io_of(&play).ok()?;
            let cap_io = capture.as_ref().and_then(|c| io_of(c).ok());
            let mut play_bufs = Bufs::new(play.sample, period * n_out);
            let read_max = period * 4;
            let mut cap_bufs = Bufs::new(capture.as_ref().map_or(Sample::F32, |c| c.sample), read_max * n_in.max(1));
            let mut ring = CaptureRing::new(n_in, period * 16);
            let mut out_planar: Vec<Vec<f32>> = (0..n_out).map(|_| vec![0.0; period]).collect();
            let mut in_planar: Vec<Vec<f32>> = (0..n_in.max(2)).map(|_| vec![0.0; period]).collect();
            let mut tp = TimedProcessor::new(processor, rate);
            // Départ : on remplit le tampon de lecture de silence puis on lance les deux flux.
            let _ = play.pcm.prepare();
            for _ in 0..2 {
                for p in out_planar.iter_mut() {
                    p.fill(0.0);
                }
                if write_period(&play_io, &mut play_bufs, n_out, &out_planar).is_err() {
                    return None;
                }
            }
            if let Some(c) = &capture {
                let _ = c.pcm.prepare();
                let _ = c.pcm.start();
            }
            let _ = play.pcm.start();
            let out_delay = play.pcm.delay().unwrap_or(0).max(0) as u32;
            latency2.set(if capture.is_some() { period as u32 * 2 } else { 0 }, out_delay);
            while !stop2.load(Ordering::Relaxed) {
                cycles2.fetch_add(1, Ordering::Relaxed);
                let _rt = RtScope::enter();
                // Capture : tout ce qui est déjà arrivé, sans attendre (EAGAIN : rien de prêt) ; puis une période de la file. Si la file
                // n'a pas encore de quoi faire une période, on joue du silence en entrée ; si elle déborde, on rattrape en jetant le plus ancien.
                if let (Some(io), Some(c)) = (&cap_io, &capture) {
                    match read_available(io, &mut cap_bufs, c.channels, read_max, &mut ring) {
                        Ok(_) => {}
                        Err(e) if e.errno() == 11 => {}
                        Err(e) => {
                            xruns2.fetch_add(1, Ordering::Relaxed);
                            let _ = c.pcm.try_recover(e, true);
                            let _ = c.pcm.start();
                        }
                    }
                    ring.trim(period * 3);
                    if !ring.pop(&mut in_planar[..n_in]) {
                        for p in in_planar.iter_mut() {
                            p.fill(0.0);
                        }
                    }
                } else {
                    for p in in_planar.iter_mut() {
                        p.fill(0.0);
                    }
                }
                {
                    let ins: [&[f32]; MAX_CHANNELS] = std::array::from_fn(|i| in_planar.get(i).map_or(&[][..], |v| &v[..]));
                    let n_ins = n_in.max(2);
                    let mut outs: [&mut [f32]; MAX_CHANNELS] = {
                        let mut it = out_planar.iter_mut();
                        std::array::from_fn(|_| it.next().map_or(&mut [][..], |v| &mut v[..]))
                    };
                    tp.process_outputs(&ins[..n_ins], &mut outs[..n_out]);
                }
                if debug_on {
                    let m = out_planar.iter().take(2).flat_map(|p| p.iter()).fold(0.0f32, |a, v| a.max(v.abs()));
                    peak2.fetch_max(m.to_bits(), Ordering::Relaxed);
                }
                if let Err(e) = write_period(&play_io, &mut play_bufs, n_out, &out_planar) {
                    xruns2.fetch_add(1, Ordering::Relaxed);
                    if play.pcm.try_recover(e, true).is_ok() {
                        // Après un dépassement, le tampon est vide : on le réamorce de silence avant de repartir.
                        for p in out_planar.iter_mut() {
                            p.fill(0.0);
                        }
                        let _ = write_period(&play_io, &mut play_bufs, n_out, &out_planar);
                        let _ = write_period(&play_io, &mut play_bufs, n_out, &out_planar);
                        let _ = play.pcm.start();
                    }
                }
            }
            let _ = play.pcm.drop();
            if let Some(c) = &capture {
                let _ = c.pcm.drop();
            }
            tp.timing()
        })
        .map_err(|e| format!("fil audio : {e}"))?;

    let start = Instant::now();
    let end = Duration::from_secs(opts.seconds.max(1) as u64);
    // Aide au diagnostic : `DAW_DEBUG_AUDIO=1` affiche chaque seconde le nombre de cycles faits et de dépassements (sur la sortie d'erreur).
    let debug = std::env::var_os("DAW_DEBUG_AUDIO").is_some();
    let mut next_debug = Duration::from_secs(1);
    loop {
        let elapsed = start.elapsed();
        if debug && elapsed >= next_debug {
            eprintln!("ALSA diag {:>4.0} s : cycles {} (attendus {}), dépassements {}, crête de sortie {:.3}", elapsed.as_secs_f64(), cycles.load(Ordering::Relaxed), (elapsed.as_secs_f64() * rate as f64 / period as f64) as u64, xruns.load(Ordering::Relaxed), f32::from_bits(peak.swap(0, Ordering::Relaxed)));
            next_debug += Duration::from_secs(1);
        }
        if elapsed >= end || !control(elapsed) || worker.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Un fil déjà terminé avant qu'on lui demande de s'arrêter n'a pas pu démarrer (ouverture ou premier transfert refusé).
    let died = worker.is_finished();
    stop.store(true, Ordering::Relaxed);
    let timing = worker.join().map_err(|_| "le fil audio a paniqué".to_string())?;
    if died && timing.is_none() {
        return Err(format!("« {dev_out} » : le flux n'a pas pu démarrer"));
    }
    Ok(PlaybackReport { sample_rate: rate, buffer_size: period as u32, output: dev_out, timing, xruns: xruns.load(Ordering::Relaxed), callback_allocations: alloc_guard::violations() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_devices_never_fails_and_only_offers_useful_names() {
        for d in list_devices() {
            assert!(d.name == "default" || d.name == "pipewire" || d.name == "pulse" || d.name.starts_with("hw:") || d.name.starts_with("plughw:"), "{}", d.name);
            assert!(d.playback || d.capture);
        }
    }

    #[test]
    fn an_unknown_device_is_refused_with_a_message() {
        let opts = PlayOptions { seconds: 1, device: Some("périphérique-inexistant".into()), ..PlayOptions::default() };
        let r = run_playback_ex(&opts, |_| -> (Silence, crate::playback::ControlFn) { unreachable!("le flux ne doit pas s'ouvrir") });
        assert!(r.unwrap_err().contains("impossible"));
    }

    struct Silence;
    impl StereoProcessor for Silence {
        fn process(&mut self, _: &[f32], _: &[f32], l: &mut [f32], r: &mut [f32]) {
            l.fill(0.0);
            r.fill(0.0);
        }
    }
}
