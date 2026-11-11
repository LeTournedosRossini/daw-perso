//! Marqueurs d'étirement (time stretch) : on pose des marqueurs sur un clip audio et on les déplace ; l'audio entre deux marqueurs s'étire ou
//! se comprime **sans changer la hauteur**, le reste du clip ne bouge pas (comme dans Reaper).
//!
//! **Rien n'est étiré dans le fil audio** (non déterministe, alloue) : un fil de travail rend un **fichier étiré** (`daw-align`, interface
//! `align_warp_render`, moteur au choix : Signalsmith ou Rubber Band) que le clip joue comme n'importe quel clip. Le rendu a lieu à la
//! **validation du geste** ; pose en **une seule action annulable** ; le clip garde sa longueur sur la ligne de temps (les marqueurs de
//! début et de fin de clip sont implicites et ne bougent pas).
//!
//! Chaque clip étiré garde sa **fiche** : la source (jamais modifiée), la partie de la source qu'il couvre, les marqueurs et le moteur.
//! La fiche est retrouvée par les **données du fichier rendu** (une découpe garde les mêmes données : chaque moitié reste étirable et
//! garde ses marqueurs) ; modifier les marqueurs d'une moitié rerend **depuis la source** ce que couvre cette moitié.
//! Les fiches sont enregistrées avec le projet (`UiMeta::stretches`), comme celles du calage, **pas** dans le format de la session.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use daw_align::{warp_render, Audio, WarpKey, WARP_MIN_SEGMENT, WARP_RATIO_MAX, WARP_RATIO_MIN};
use daw_engine::{AudioData, Clip};
use daw_session::peaks::Peaks;
use daw_session::ClipId;

use super::{explain, Host};
use crate::project_io::{self, UiStretch};
use crate::protocol::{Command, Event, StretchInfo};

/// Plus grand nombre de marqueurs intérieurs d'un clip.
pub(crate) const MAX_MARKERS: usize = 256;
const DEFAULT_ENGINE: &str = "signalsmith";
const ENGINES: [&str; 3] = ["signalsmith", "rubberband_r2", "rubberband_r3"];

/// Fiche d'un clip étiré : de quoi refaire le rendu.
pub(super) struct StretchRecord {
    /// Données du fichier étiré : c'est par elles qu'un clip retrouve sa fiche.
    pub(super) rendered: Arc<AudioData>,
    /// La source, jamais modifiée.
    source: Arc<AudioData>,
    /// Trame de la source où commence ce que couvre `rendered`, et nombre de trames couvertes.
    source_start: u64,
    source_len: u64,
    /// Marqueurs `(secondes de source depuis source_start, secondes de sortie depuis le début de rendered)` ; premier `(0, 0)`, dernier = fin des deux.
    keys: Vec<(f64, f64)>,
    /// Marqueurs posés par l'utilisateur `(secondes de source, secondes de sortie, lissé ?)`, sans les extrémités. `keys` en est la version
    /// développée : une courbe lissée y est échantillonnée en tronçons droits de 40 ms au plus.
    marks: Vec<(f64, f64, bool)>,
    engine: String,
}

/// Pas d'échantillonnage (secondes de source) d'un tronçon lissé.
const SMOOTH_STEP: f64 = 0.04;

/// Développe des clés `[(0, 0), …, (fin, fin)]` (source, sortie en secondes) en tenant compte des marqueurs **lissés** (`smooth[i]` pour la clé
/// intérieure `i + 1`) : la vitesse d'étirement y est continue (interpolation d'Hermite monotone, pentes en moyenne harmonique des deux tronçons
/// voisins), au lieu de changer d'un coup. Un marqueur non lissé garde sa cassure. Sans marqueur lissé, les clés sont rendues telles quelles.
/// Refuse si la courbe sort des rapports permis [0,25 ; 4] quelque part.
pub(crate) fn expand_keys(keys: &[(f64, f64)], smooth: &[bool]) -> Result<Vec<(f64, f64)>, String> {
    if !smooth.iter().any(|&s| s) || keys.len() < 3 {
        return Ok(keys.to_vec());
    }
    let n = keys.len();
    let sec: Vec<f64> = keys.windows(2).map(|w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0)).collect();
    let is_smooth = |k: usize| k >= 1 && k + 1 < n && smooth.get(k - 1).copied().unwrap_or(false);
    let slope = |k: usize, seg: usize| if is_smooth(k) { 2.0 * sec[k - 1] * sec[k] / (sec[k - 1] + sec[k]) } else { sec[seg] };
    let mut out = vec![keys[0]];
    for i in 0..n - 1 {
        let (a, b) = (keys[i], keys[i + 1]);
        let (m0, m1) = (slope(i, i), slope(i + 1, i));
        if !is_smooth(i) && !is_smooth(i + 1) {
            out.push(b);
            continue;
        }
        let h = b.0 - a.0;
        let count = ((h / SMOOTH_STEP).floor() as usize).max(1);
        let mut prev = a;
        for j in 1..=count {
            let p = if j == count {
                b
            } else {
                let t = j as f64 / count as f64;
                let (t2, t3) = (t * t, t * t * t);
                let (h00, h10, h01, h11) = (2.0 * t3 - 3.0 * t2 + 1.0, t3 - 2.0 * t2 + t, -2.0 * t3 + 3.0 * t2, t3 - t2);
                (a.0 + t * h, h00 * a.1 + h * h10 * m0 + h01 * b.1 + h * h11 * m1)
            };
            let ratio = (p.1 - prev.1) / (p.0 - prev.0);
            if !(WARP_RATIO_MIN..=WARP_RATIO_MAX).contains(&ratio) {
                return Err(format!("le lissage sortirait des limites d'étirement ({:.0} %) entre les marqueurs {} et {} : rapproche les rapports ou retire le lissage", ratio * 100.0, i, i + 1));
            }
            out.push(p);
            prev = p;
        }
    }
    Ok(out)
}

/// Instant de la source (secondes) qui tombe à l'instant de sortie `out` (secondes) : interpolation linéaire entre marqueurs.
pub(crate) fn src_at(keys: &[(f64, f64)], out: f64) -> f64 {
    let Some(last) = keys.last() else { return out };
    if out <= keys[0].1 {
        return keys[0].0;
    }
    if out >= last.1 {
        return last.0;
    }
    let i = keys.partition_point(|k| k.1 <= out).clamp(1, keys.len() - 1);
    let (a, b) = (keys[i - 1], keys[i]);
    a.0 + (out - a.1) / (b.1 - a.1) * (b.0 - a.0)
}

/// Ce que montre un clip : la partie de sa source (début absolu en trames, nombre de trames) et ses marqueurs intérieurs
/// `[position dans le clip, position dans la source du clip]`, tous deux en trames depuis le début du clip.
pub(crate) struct Geometry {
    pub(crate) source_start: u64,
    pub(crate) source_len: u64,
    pub(crate) markers: Vec<[u64; 2]>,
    /// Pour chaque marqueur : lissé ou non.
    pub(crate) smooth: Vec<bool>,
}

/// Géométrie d'un clip d'après sa fiche (ou, sans fiche, celle d'un clip ordinaire : la source est son fichier, sans marqueur).
pub(crate) fn geometry(rec: Option<(&[(f64, f64)], &[(f64, f64, bool)], u64)>, clip: &Clip, rate: f64) -> Geometry {
    let Some((keys, marks, rec_source_start)) = rec else {
        return Geometry { source_start: clip.offset, source_len: clip.len, markers: Vec::new(), smooth: Vec::new() };
    };
    let (o, l) = (clip.offset as f64, clip.len as f64);
    let s0 = src_at(keys, o / rate) * rate;
    let s1 = src_at(keys, (o + l) / rate) * rate;
    let (s0r, s1r) = (s0.round(), s1.round());
    let mut markers = Vec::new();
    let mut smooth = Vec::new();
    for k in marks {
        let out = (k.1 * rate).round() - o;
        if out >= 1.0 && out <= l - 1.0 {
            let src = (k.0 * rate).round() - s0r;
            if src >= 1.0 && src <= (s1r - s0r) - 1.0 {
                markers.push([out as u64, src as u64]);
                smooth.push(k.2);
            }
        }
    }
    Geometry { source_start: rec_source_start + s0r.max(0.0) as u64, source_len: (s1r - s0r).max(1.0) as u64, markers, smooth }
}

/// Marqueurs → clés complètes en secondes `[(0, 0), …, (fin de la source, fin du clip)]`, ou le refus en clair. Mêmes règles que la bibliothèque
/// (temps strictement croissants, tronçon de source d'au moins 10 ms, rapport dans [0,25 ; 4]), vérifiées ici pour répondre avant de lancer un rendu.
pub(crate) fn build_keys(markers: &[[u64; 2]], len: u64, source_len: u64, rate: f64) -> Result<Vec<(f64, f64)>, String> {
    if markers.len() > MAX_MARKERS {
        return Err(format!("trop de marqueurs ({} au plus)", MAX_MARKERS));
    }
    let mut pts: Vec<(u64, u64)> = vec![(0, 0)]; // (source, sortie) en trames
    for m in markers {
        let (out, src) = (m[0], m[1]);
        if out == 0 || out >= len || src == 0 || src >= source_len {
            return Err("un marqueur doit être à l'intérieur du clip (ni au début ni à la fin)".into());
        }
        let &(ps, po) = pts.last().expect("au moins le début");
        if out <= po || src <= ps {
            return Err("les marqueurs doivent être strictement dans l'ordre, dans le clip comme dans la source".into());
        }
        pts.push((src, out));
    }
    pts.push((source_len, len));
    let min_src = (WARP_MIN_SEGMENT * rate).ceil() as u64;
    for (i, w) in pts.windows(2).enumerate() {
        let (ds, d_out) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
        if ds < min_src {
            return Err(format!("le tronçon {} de la source est trop court (au moins 10 ms)", i + 1));
        }
        let ratio = d_out as f64 / ds as f64;
        if !(WARP_RATIO_MIN..=WARP_RATIO_MAX).contains(&ratio) {
            return Err(format!("le tronçon {} serait étiré à {:.0} % : permis de {:.0} % à {:.0} %", i + 1, ratio * 100.0, WARP_RATIO_MIN * 100.0, WARP_RATIO_MAX * 100.0));
        }
    }
    Ok(pts.iter().map(|&(s, o)| (s as f64 / rate, o as f64 / rate)).collect())
}

fn engine_name(requested: Option<&str>, fallback: &str) -> Result<String, String> {
    let e = requested.unwrap_or(fallback);
    if ENGINES.contains(&e) {
        Ok(e.to_owned())
    } else {
        Err(format!("moteur inconnu : « {e} » (signalsmith, rubberband_r2 ou rubberband_r3)"))
    }
}

struct Output {
    rendered: Arc<AudioData>,
    peaks: Arc<Peaks>,
    fell_back: bool,
    /// Crête du rendu et de la partie de source rendue (valeur absolue maximale, toutes voies).
    peak: f32,
    source_peak: f32,
}

/// Longueur (secondes) du fondu enchaîné entre un tronçon recopié tel quel et ses voisins rendus.
const COPY_FADE: f64 = 0.010;

/// Recopie la **source telle quelle** sur les tronçons dont le rapport vaut exactement 1 (le moteur d'étirement les altère un peu, Signalsmith
/// d'environ −30 dB) ; fondu enchaîné linéaire de 10 ms avec le rendu aux bords intérieurs (pas aux bords du clip). `out` et `src` sont
/// entrelacés à `ch` voies ; `keys` = clés `(source, sortie)` en secondes.
pub(crate) fn copy_unity_runs(out: &mut [f32], src: &[f32], ch: usize, keys: &[(f64, f64)], rate: f64) {
    if ch == 0 || keys.len() < 2 {
        return;
    }
    let (out_frames, src_frames) = (out.len() / ch, src.len() / ch);
    // Tronçons de rapport 1, regroupés quand ils se touchent : (début sortie, fin sortie, début source, bord gauche intérieur, bord droit intérieur).
    let mut runs: Vec<(usize, usize, usize, bool, bool)> = Vec::new();
    let last = keys.len() - 2;
    for (i, w) in keys.windows(2).enumerate() {
        let (ds, d_out) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
        if ds <= 0.0 || (d_out / ds - 1.0).abs() > 1e-6 {
            continue;
        }
        let (o0, o1) = ((w[0].1 * rate).round() as usize, (w[1].1 * rate).round() as usize);
        let s0 = (w[0].0 * rate).round() as usize;
        match runs.last_mut() {
            Some(r) if r.1 == o0 => r.1 = o1,
            _ => runs.push((o0, o1, s0, i != 0, false)),
        }
        if let Some(r) = runs.last_mut() {
            r.4 = i != last;
        }
    }
    let fade = (COPY_FADE * rate).round().max(1.0);
    for (o0, o1, s0, left_inner, right_inner) in runs {
        let o1 = o1.min(out_frames);
        for i in o0..o1 {
            let si = s0 + (i - o0);
            if si >= src_frames {
                break;
            }
            let mut w = 1.0f64;
            if left_inner {
                w = w.min(((i - o0) as f64 + 0.5) / fade);
            }
            if right_inner {
                w = w.min(((o1 - 1 - i) as f64 + 0.5) / fade);
            }
            let w = w.min(1.0) as f32;
            for c in 0..ch {
                out[i * ch + c] = out[i * ch + c] * (1.0 - w) + src[si * ch + c] * w;
            }
        }
    }
}

fn peak_of(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

enum Msg {
    Stage(f64),
    Done(Output),
    Failed(String),
    Cancelled,
}

/// Un rendu en cours (un seul à la fois).
pub(super) struct StretchJob {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    clip: ClipId,
    name: String,
    /// Le clip tel qu'il était au départ : s'il a changé pendant le rendu, l'étirement est abandonné.
    before: Clip,
    pending: Pending,
    fraction: f64,
}

/// Ce que deviendra la fiche une fois le rendu posé.
struct Pending {
    source: Arc<AudioData>,
    source_start: u64,
    source_len: u64,
    keys: Vec<(f64, f64)>,
    marks: Vec<(f64, f64, bool)>,
    engine: String,
}

fn deinterleave(samples: &[f32], channels: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|c| samples.iter().skip(c).step_by(channels).copied().collect()).collect()
}

/// Audio entrelacé de `[start, start + len)` de la source (silence au-delà de ses données).
fn source_window(source: &AudioData, start: u64, len: u64) -> (Vec<f32>, usize) {
    let ch = source.channel_count();
    let mut out = vec![0.0f32; len as usize * ch];
    let total = source.len();
    for t in 0..len {
        let i = start + t;
        if i >= total {
            break;
        }
        for c in 0..ch {
            out[t as usize * ch + c] = source.channel(c)[i as usize];
        }
    }
    (out, ch)
}

fn work(source: Arc<AudioData>, start: u64, len: u64, keys: Vec<(f64, f64)>, engine: String, rate: u32, tx: Sender<Msg>, cancel: Arc<AtomicBool>) {
    let (samples, ch) = source_window(&source, start, len);
    let settings = format!(r#"{{"general":{{"engine":"{engine}"}}}}"#);
    let marks: Vec<WarpKey> = keys.iter().map(|k| WarpKey { source: k.0, output: k.1 }).collect();
    let mut cb = |f: f64| {
        let _ = tx.send(Msg::Stage(f));
        cancel.load(Ordering::Relaxed)
    };
    match warp_render(Audio { samples: &samples, channels: ch, sample_rate: f64::from(rate) }, &marks, Some(&settings), Some(&mut cb)) {
        Ok((mut out, info)) => {
            copy_unity_runs(&mut out, &samples, ch, &keys, f64::from(rate));
            let (peak, source_peak) = (peak_of(&out), peak_of(&samples));
            match AudioData::new(rate, deinterleave(&out, ch)) {
            Ok(rendered) => {
                let peaks = Arc::new(Peaks::build(&rendered));
                let _ = tx.send(Msg::Done(Output { rendered, peaks, fell_back: info.fell_back, peak, source_peak }));
            }
            Err(e) => {
                let _ = tx.send(Msg::Failed(format!("{e:?}")));
            }
            }
        }
        Err(daw_align::AlignError::Cancelled) => {
            let _ = tx.send(Msg::Cancelled);
        }
        Err(e) => {
            let _ = tx.send(Msg::Failed(e.to_string()));
        }
    }
}

impl Host {
    pub(super) fn refuse_while_stretching(&self, command: &str) -> Option<Vec<Event>> {
        self.stretch.as_ref().map(|_| vec![Event::Error { command: command.into(), message: "un étirement est en cours : attends sa fin ou annule-le".into() }])
    }

    fn stretch_error(command: &str, message: impl Into<String>) -> Vec<Event> {
        vec![Event::Error { command: command.into(), message: message.into() }]
    }

    fn stretch_record_of(&self, clip: &Clip) -> Option<&StretchRecord> {
        self.stretches.iter().find(|r| Arc::ptr_eq(&r.rendered, &clip.data))
    }

    /// Ce que la page montre d'un clip étiré (`None` : clip ordinaire).
    pub(super) fn stretch_info(&self, clip: &Clip) -> Option<StretchInfo> {
        let rec = self.stretch_record_of(clip)?;
        let g = geometry(Some((&rec.keys, &rec.marks, rec.source_start)), clip, f64::from(self.session.sample_rate()));
        Some(StretchInfo { engine: rec.engine.clone(), src_len: g.source_len, markers: g.markers, smooth: g.smooth })
    }

    /// Chemin `audio/<nom>-etire.wav` (ou `-etire-2`…) : ni un fichier du dossier du projet ni un chemin déjà pris.
    fn stretch_asset_path(&self, stem: &str) -> String {
        let base = project_io::sanitize(stem);
        let taken: Vec<&str> = self.session.assets().iter().map(|(_, p)| p.as_str()).collect();
        let free = |rel: &String| !taken.contains(&rel.as_str()) && self.project_dir.as_ref().is_none_or(|d| !d.join(rel).exists());
        let first = format!("audio/{base}-etire.wav");
        if free(&first) {
            return first;
        }
        (2..).map(|n| format!("audio/{base}-etire-{n}.wav")).find(free).expect("une suite infinie")
    }

    /// `set_stretch` : remplace les marqueurs d'un clip (les marqueurs de début et de fin sont implicites). Sans marqueur et sans étirement
    /// d'ensemble, le clip redevient un clip ordinaire sur sa source (aucun rendu).
    pub(super) fn start_stretch(&mut self, clip: u64, markers: Vec<[u64; 2]>, smooth: Vec<bool>, engine: Option<String>) -> Vec<Event> {
        const CMD: &str = "set_stretch";
        if let Some(refused) = self.refuse_while_recording(CMD) {
            return refused;
        }
        if self.stretch.is_some() {
            return Self::stretch_error(CMD, "un étirement est déjà en cours");
        }
        if self.align.is_some() {
            return Self::stretch_error(CMD, "un calage est en cours : attends sa fin ou annule-le");
        }
        let id = ClipId(clip);
        let Some(before) = self.session.clip(id).cloned() else { return Self::stretch_error(CMD, format!("clip audio inconnu : {clip}")) };
        let rate = f64::from(self.session.sample_rate());
        let rec = self.stretch_record_of(&before);
        let g = geometry(rec.map(|r| (r.keys.as_slice(), r.marks.as_slice(), r.source_start)), &before, rate);
        let (source, fallback_engine) = match rec {
            Some(r) => (Arc::clone(&r.source), r.engine.clone()),
            None => (Arc::clone(&before.data), DEFAULT_ENGINE.to_owned()),
        };
        let engine = match engine_name(engine.as_deref(), &fallback_engine) {
            Ok(e) => e,
            Err(m) => return Self::stretch_error(CMD, m),
        };
        if !smooth.is_empty() && smooth.len() != markers.len() {
            return Self::stretch_error(CMD, "smooth : un indicateur par marqueur");
        }
        let smooth: Vec<bool> = if smooth.is_empty() { vec![false; markers.len()] } else { smooth };
        let user_keys = match build_keys(&markers, before.len, g.source_len, rate) {
            Ok(k) => k,
            Err(m) => return Self::stretch_error(CMD, m),
        };
        let marks: Vec<(f64, f64, bool)> = user_keys[1..user_keys.len() - 1].iter().zip(&smooth).map(|(k, &s)| (k.0, k.1, s)).collect();
        let keys = match expand_keys(&user_keys, &smooth) {
            Ok(k) => k,
            Err(m) => return Self::stretch_error(CMD, m),
        };
        // Aucun marqueur et aucun étirement d'ensemble : retour au clip ordinaire sur la source, sans rendu.
        if markers.is_empty() && g.source_len == before.len {
            if rec.is_none() {
                return Self::stretch_error(CMD, "ce clip n'a pas de marqueur : rien à étirer");
            }
            let mut plain = before.clone();
            plain.data = source;
            plain.offset = g.source_start;
            let result = self.session.group("Retirer l'étirement", |s| s.set_clip(id, plain));
            return self.finish_edit(CMD, result);
        }
        let name = self.audio_clip_name(id.0, &before.data).unwrap_or_else(|| format!("Clip {}", id.0));
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread = {
            let (src, cancel, keys, engine) = (Arc::clone(&source), Arc::clone(&cancel), keys.clone(), engine.clone());
            let (start, len, sample_rate) = (g.source_start, g.source_len, self.session.sample_rate());
            std::thread::Builder::new().name("etirement".into()).spawn(move || work(src, start, len, keys, engine, sample_rate, tx, cancel)).ok()
        };
        if thread.is_none() {
            return Self::stretch_error(CMD, "le fil d'étirement n'a pas pu démarrer");
        }
        self.stretch = Some(StretchJob { rx, cancel, thread, clip: id, name: name.clone(), before, pending: Pending { source, source_start: g.source_start, source_len: g.source_len, keys, marks, engine }, fraction: 0.0 });
        vec![Self::stretch_event(true, 0.0, clip, &name, false, None, false)]
    }

    /// `detect_onsets` : les attaques de l'audio du clip tel qu'il joue (partie visible, fondus et gain non appliqués).
    pub(super) fn detect_onsets(&mut self, clip: u64, sensitivity: Option<f64>) -> Vec<Event> {
        const CMD: &str = "detect_onsets";
        let Some(c) = self.session.clip(ClipId(clip)) else { return Self::stretch_error(CMD, format!("clip audio inconnu : {clip}")) };
        let (start, len) = (c.offset as usize, c.len as usize);
        let total = c.data.len() as usize;
        let end = (start + len).min(total);
        let chans: Vec<&[f32]> = (0..c.data.channel_count()).map(|k| &c.data.channel(k)[start.min(end)..end]).collect();
        let rate = f64::from(self.session.sample_rate());
        let positions = crate::onsets::detect(&chans, rate, sensitivity.unwrap_or(0.5)).into_iter().map(|p| p as u64).collect();
        vec![Event::Onsets { clip, positions }]
    }

    pub(super) fn cancel_stretch(&mut self) -> Vec<Event> {
        if let Some(job) = &self.stretch {
            job.cancel.store(true, Ordering::Relaxed);
        }
        Vec::new()
    }

    fn stretch_event(active: bool, progress: f64, clip: u64, name: &str, cancelled: bool, error: Option<String>, fell_back: bool) -> Event {
        Event::Stretch { active, progress, clip, name: name.to_owned(), cancelled, error, fell_back, warning: None }
    }

    /// Relève le fil de rendu (appelé par `tick`) ; à la fin, pose le clip étiré en une seule action annulable.
    pub(super) fn stretch_tick(&mut self) -> Vec<Event> {
        let Some(job) = self.stretch.as_mut() else { return Vec::new() };
        let mut progressed = false;
        let mut end: Option<Msg> = None;
        loop {
            match job.rx.try_recv() {
                Ok(Msg::Stage(f)) => {
                    job.fraction = f;
                    progressed = true;
                }
                Ok(m) => {
                    end = Some(m);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    end = Some(Msg::Failed("l'étirement s'est arrêté de façon inattendue".into()));
                    break;
                }
            }
        }
        let Some(end) = end else {
            return if progressed { vec![Self::stretch_event(true, job.fraction, job.clip.0, &job.name, false, None, false)] } else { Vec::new() };
        };
        let mut job = self.stretch.take().expect("étirement en cours");
        if let Some(t) = job.thread.take() {
            let _ = t.join();
        }
        let (clip, name) = (job.clip.0, job.name.clone());
        match end {
            Msg::Cancelled => vec![Self::stretch_event(false, 1.0, clip, &name, true, None, false)],
            Msg::Failed(m) => vec![Self::stretch_event(false, 1.0, clip, &name, false, Some(m), false)],
            Msg::Done(o) => self.finish_stretch(job, o),
            Msg::Stage(_) => Vec::new(),
        }
    }

    fn finish_stretch(&mut self, job: StretchJob, out: Output) -> Vec<Event> {
        let (clip, name) = (job.clip.0, job.name.clone());
        let Some(now) = self.session.clip(job.clip) else {
            return vec![Self::stretch_event(false, 1.0, clip, &name, false, Some("le clip a disparu pendant le rendu : étirement abandonné".into()), false)];
        };
        let same = Arc::ptr_eq(&now.data, &job.before.data) && now.start == job.before.start && now.offset == job.before.offset && now.len == job.before.len;
        if !same {
            return vec![Self::stretch_event(false, 1.0, clip, &name, false, Some("le clip a changé pendant le rendu : étirement abandonné".into()), false)];
        }
        let path = self.stretch_asset_path(&format!("{} étiré", name));
        let mut new = job.before.clone();
        new.data = Arc::clone(&out.rendered);
        new.offset = 0;
        let rendered = Arc::clone(&out.rendered);
        let result = self.session.group("Étirer l'audio", |s| {
            s.register_asset(&path, Arc::clone(&rendered));
            s.set_clip(job.clip, new)
        });
        if let Err(e) = result {
            return vec![Self::stretch_event(false, 1.0, clip, &name, false, Some(format!("l'étirement n'a pas pu être posé : {}", explain(&e))), false)];
        }
        self.peaks.insert(Arc::as_ptr(&out.rendered) as usize, Arc::clone(&out.peaks));
        let p = job.pending;
        self.stretches.push(StretchRecord { rendered: out.rendered, source: p.source, source_start: p.source_start, source_len: p.source_len, keys: p.keys, marks: p.marks, engine: p.engine });
        let mut events = self.finish_edit("set_stretch", Ok(()));
        let mut done = Self::stretch_event(false, 1.0, clip, &name, false, None, out.fell_back);
        if out.peak > 1.0 && out.peak > out.source_peak + 1e-3 {
            if let Event::Stretch { warning, .. } = &mut done {
                *warning = Some(format!("le rendu dépasse 0 dBFS (+{:.1} dB) : le moteur d'étirement a gonflé les crêtes ; baisse le gain du clip si ça sature", 20.0 * f64::from(out.peak).log10()));
            }
        }
        events.push(done);
        events
    }

    // ---- enregistrement dans le projet ----

    /// Les fiches telles qu'on les écrit (chaque fichier audio désigné par son chemin dans le projet).
    pub(super) fn stretches_for_file(&self) -> Vec<UiStretch> {
        self.stretches
            .iter()
            .filter_map(|r| {
                Some(UiStretch {
                    asset: self.session.asset_path(&r.rendered)?.to_owned(),
                    source_asset: self.session.asset_path(&r.source)?.to_owned(),
                    source_start: r.source_start,
                    source_len: r.source_len,
                    keys: r.keys.iter().map(|k| [k.0, k.1]).collect(),
                    marks: r.marks.iter().map(|m| [m.0, m.1, if m.2 { 1.0 } else { 0.0 }]).collect(),
                    engine: r.engine.clone(),
                })
            })
            .collect()
    }

    /// Relit les fiches d'un projet ouvert ; une fiche dont un fichier manque est ignorée (avertissement).
    pub(super) fn restore_stretches(&mut self, saved: &[UiStretch]) -> Vec<String> {
        let mut warnings = Vec::new();
        let find = |s: &Self, path: &str| s.session.assets().iter().find(|(_, p)| p == path).map(|(d, _)| Arc::clone(d));
        for a in saved {
            let (Some(rendered), Some(source)) = (find(self, &a.asset), find(self, &a.source_asset)) else {
                warnings.push(format!("étirement de « {} » : fichier introuvable, fiche ignorée (le clip reste étiré mais ses marqueurs sont perdus)", a.asset));
                continue;
            };
            let keys: Vec<(f64, f64)> = a.keys.iter().map(|k| (k[0], k[1])).collect();
            if keys.len() < 2 || !ENGINES.contains(&a.engine.as_str()) {
                warnings.push(format!("étirement de « {} » : fiche illisible, ignorée", a.asset));
                continue;
            }
            // Fiche d'avant le lissage : les marqueurs sont les clés intérieures, aucun n'est lissé.
            let marks: Vec<(f64, f64, bool)> = if a.marks.is_empty() { keys[1..keys.len() - 1].iter().map(|k| (k.0, k.1, false)).collect() } else { a.marks.iter().map(|m| (m[0], m[1], m[2] > 0.5)).collect() };
            self.stretches.push(StretchRecord { rendered, source, source_start: a.source_start, source_len: a.source_len, keys, marks, engine: a.engine.clone() });
        }
        warnings
    }

    /// Dispatch des commandes du protocole.
    pub(super) fn stretch_command(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::SetStretch { clip, markers, smooth, engine } => self.start_stretch(clip, markers, smooth, engine),
            Command::CancelStretch => self.cancel_stretch(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<(f64, f64)> {
        // Source de 4 s : 0-2 s comprimé à 50 %, 2-4 s étiré à 150 % ; clip de 4 s.
        vec![(0.0, 0.0), (2.0, 1.0), (4.0, 4.0)]
    }

    #[test]
    fn unity_runs_are_copied_from_the_source_with_short_fades_at_inner_edges() {
        let rate = 1000.0; // 1 trame = 1 ms
        // Source 0..300 ; tronçon 1 (0..100) à rapport 1, tronçon 2 (100..300 → 100..200) à 0,5, tronçon 3 (300..? ) n'existe pas.
        let keys = vec![(0.0, 0.0), (0.1, 0.1), (0.3, 0.2)];
        let src: Vec<f32> = (0..300).map(|i| i as f32).collect();
        let mut out = vec![-1.0f32; 200];
        copy_unity_runs(&mut out, &src, 1, &keys, rate);
        // Bord gauche = bord du clip : recopie directe ; bord droit intérieur : fondu de 10 trames vers le rendu (-1).
        assert_eq!(&out[0..89], &src[0..89]);
        assert!(out[99] < out[95] && out[95] < src[95] && out[99] < 10.0, "fondu vers le rendu : {} {}", out[95], out[99]);
        assert!(out[100..].iter().all(|&v| v == -1.0), "le tronçon étiré n'est pas touché");
    }

    #[test]
    fn without_smoothing_keys_are_unchanged() {
        assert_eq!(expand_keys(&keys(), &[false]).unwrap(), keys());
    }

    #[test]
    fn smoothing_keeps_ends_and_marker_and_changes_speed_gradually() {
        let k = keys();
        let d = expand_keys(&k, &[true]).unwrap();
        assert_eq!(d.first(), Some(&k[0]));
        assert_eq!(d.last(), Some(&k[2]));
        assert!(d.contains(&k[1]), "le marqueur lui-même reste une clé");
        assert!(d.len() > 50, "courbe échantillonnée : {} clés", d.len());
        let ratios: Vec<f64> = d.windows(2).map(|w| (w[1].1 - w[0].1) / (w[1].0 - w[0].0)).collect();
        assert!(ratios.iter().all(|r| (WARP_RATIO_MIN..=WARP_RATIO_MAX).contains(r)));
        // La plus grande variation entre deux rapports voisins est bien plus petite que le saut de 0,5 à 1,5 d'un coup.
        let jump = ratios.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f64::max);
        assert!(jump < 0.2, "variation maximale {jump}");
        // Le rapport passe progressivement de ~0,5 à ~1,5 et reste monotone entre les deux.
        assert!((ratios.first().unwrap() - 0.5).abs() < 0.1 && (ratios.last().unwrap() - 1.5).abs() < 0.1, "{:?} {:?}", ratios.first(), ratios.last());
        // Temps strictement croissants.
        assert!(d.windows(2).all(|w| w[1].0 > w[0].0 && w[1].1 > w[0].1));
    }
}
