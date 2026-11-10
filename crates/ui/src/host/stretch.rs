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
    engine: String,
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
}

/// Géométrie d'un clip d'après sa fiche (ou, sans fiche, celle d'un clip ordinaire : la source est son fichier, sans marqueur).
pub(crate) fn geometry(rec: Option<(&[(f64, f64)], u64)>, clip: &Clip, rate: f64) -> Geometry {
    let Some((keys, rec_source_start)) = rec else {
        return Geometry { source_start: clip.offset, source_len: clip.len, markers: Vec::new() };
    };
    let (o, l) = (clip.offset as f64, clip.len as f64);
    let s0 = src_at(keys, o / rate) * rate;
    let s1 = src_at(keys, (o + l) / rate) * rate;
    let (s0r, s1r) = (s0.round(), s1.round());
    let mut markers = Vec::new();
    for k in keys {
        let out = (k.1 * rate).round() - o;
        if out >= 1.0 && out <= l - 1.0 {
            let src = (k.0 * rate).round() - s0r;
            if src >= 1.0 && src <= (s1r - s0r) - 1.0 {
                markers.push([out as u64, src as u64]);
            }
        }
    }
    Geometry { source_start: rec_source_start + s0r.max(0.0) as u64, source_len: (s1r - s0r).max(1.0) as u64, markers }
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
        Ok((out, info)) => match AudioData::new(rate, deinterleave(&out, ch)) {
            Ok(rendered) => {
                let peaks = Arc::new(Peaks::build(&rendered));
                let _ = tx.send(Msg::Done(Output { rendered, peaks, fell_back: info.fell_back }));
            }
            Err(e) => {
                let _ = tx.send(Msg::Failed(format!("{e:?}")));
            }
        },
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
        let g = geometry(Some((&rec.keys, rec.source_start)), clip, f64::from(self.session.sample_rate()));
        Some(StretchInfo { engine: rec.engine.clone(), src_len: g.source_len, markers: g.markers })
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
    pub(super) fn start_stretch(&mut self, clip: u64, markers: Vec<[u64; 2]>, engine: Option<String>) -> Vec<Event> {
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
        let g = geometry(rec.map(|r| (r.keys.as_slice(), r.source_start)), &before, rate);
        let (source, fallback_engine) = match rec {
            Some(r) => (Arc::clone(&r.source), r.engine.clone()),
            None => (Arc::clone(&before.data), DEFAULT_ENGINE.to_owned()),
        };
        let engine = match engine_name(engine.as_deref(), &fallback_engine) {
            Ok(e) => e,
            Err(m) => return Self::stretch_error(CMD, m),
        };
        let keys = match build_keys(&markers, before.len, g.source_len, rate) {
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
        self.stretch = Some(StretchJob { rx, cancel, thread, clip: id, name: name.clone(), before, pending: Pending { source, source_start: g.source_start, source_len: g.source_len, keys, engine }, fraction: 0.0 });
        vec![Self::stretch_event(true, 0.0, clip, &name, false, None, false)]
    }

    pub(super) fn cancel_stretch(&mut self) -> Vec<Event> {
        if let Some(job) = &self.stretch {
            job.cancel.store(true, Ordering::Relaxed);
        }
        Vec::new()
    }

    fn stretch_event(active: bool, progress: f64, clip: u64, name: &str, cancelled: bool, error: Option<String>, fell_back: bool) -> Event {
        Event::Stretch { active, progress, clip, name: name.to_owned(), cancelled, error, fell_back }
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
        self.stretches.push(StretchRecord { rendered: out.rendered, source: p.source, source_start: p.source_start, source_len: p.source_len, keys: p.keys, engine: p.engine });
        let mut events = self.finish_edit("set_stretch", Ok(()));
        events.push(Self::stretch_event(false, 1.0, clip, &name, false, None, out.fell_back));
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
            self.stretches.push(StretchRecord { rendered, source, source_start: a.source_start, source_len: a.source_len, keys, engine: a.engine.clone() });
        }
        warnings
    }

    /// Dispatch des commandes du protocole.
    pub(super) fn stretch_command(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::SetStretch { clip, markers, engine } => self.start_stretch(clip, markers, engine),
            Command::CancelStretch => self.cancel_stretch(),
            _ => Vec::new(),
        }
    }
}
