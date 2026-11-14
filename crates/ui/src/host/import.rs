//! Import de fichiers audio (WAV, FLAC, MP3) : décodage, conversion de fréquence, copie dans `audio/` (ou référence en place) et
//! calcul des crêtes sur un fil de travail ; la pose des clips, elle, se fait ici, en une seule action annulable.
//!
//! Rien n'est ajouté au projet tant que tous les fichiers ne sont pas prêts : une annulation ne laisse ni clip, ni piste, ni fichier
//! copié dans `audio/`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use daw_core_types::resample::resample;
use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, NodeKind};
use daw_recorder::decode::{decode_file, probe_file, Format};
use daw_session::nodes::NewNode;
use daw_session::peaks::Peaks;

use super::{explain, Host, TRACK_COLORS};
use crate::project_io;
use crate::protocol::{AudioFileInfo, Command, Event, ImportedClip};

/// Un clip prêt à être posé : ses données, le chemin sous lequel le projet les enregistre, ses crêtes.
struct Ready {
    name: String,
    data: Arc<AudioData>,
    asset_path: String,
    peaks: Arc<Peaks>,
    converted: bool,
    referenced: bool,
}

enum Msg {
    Stage { index: u32, fraction: f64, name: String },
    Ready(Vec<Ready>),
    Warning(String),
    Finished,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Plan {
    Copy,
    Reference,
}

struct Job {
    path: PathBuf,
    plan: Plan,
}

pub(super) struct ImportJob {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    total: u32,
    index: u32,
    fraction: f64,
    current: String,
    ready: Vec<Ready>,
    warnings: Vec<String>,
    target: Option<NodeId>,
    pos: u64,
    /// Fichiers copiés dans `audio/` par ce travail (retirés si on annule ou si la pose échoue).
    copied: Vec<PathBuf>,
    project_dir: Option<PathBuf>,
}

pub(super) fn stem_of(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Audio".into())
}

/// `audio/<nom>.wav`, ou `audio/<nom>-2.wav`… : ni un fichier du dossier ni un chemin déjà pris par le projet.
pub(super) fn unique_audio_path(dir: &Path, stem: &str, taken: &[String]) -> String {
    let base = project_io::sanitize(stem);
    let free = |rel: &String| !taken.contains(rel) && !dir.join(rel).exists();
    let first = format!("audio/{base}.wav");
    if free(&first) {
        return first;
    }
    (2..).map(|n| format!("audio/{base}-{n}.wav")).find(free).expect("une suite infinie")
}

fn absolute(path: &Path) -> String {
    let p = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let s = p.to_string_lossy().into_owned();
    s.strip_prefix("\\\\?\\").map(str::to_owned).unwrap_or(s)
}

struct Work {
    jobs: Vec<Job>,
    rate: u32,
    multichannel: Option<String>,
    /// Dossier du projet (les copies vont dans son sous-dossier `audio/`).
    project_dir: Option<PathBuf>,
    taken: Vec<String>,
}

fn deinterleave(samples: &[f32], channels: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|c| samples.iter().skip(c).step_by(channels).copied().collect()).collect()
}

fn work(w: Work, tx: Sender<Msg>, cancel: Arc<AtomicBool>) {
    let Work { jobs, rate, multichannel, project_dir, mut taken } = w;
    let stop = || cancel.load(Ordering::Relaxed);
    let mut written: Vec<PathBuf> = Vec::new();
    let undo = |written: &[PathBuf]| written.iter().for_each(|f| drop(std::fs::remove_file(f)));
    for (i, job) in jobs.iter().enumerate() {
        let index = i as u32;
        let name = stem_of(&job.path);
        let stage = |fraction: f64| tx.send(Msg::Stage { index, fraction, name: name.clone() }).is_ok();
        stage(0.0);
        let decoded = match decode_file(&job.path) {
            Ok(d) => d,
            Err(e) => {
                let _ = tx.send(Msg::Warning(format!("« {name} » : {e}")));
                continue;
            }
        };
        if stop() {
            undo(&written);
            return drop(tx.send(Msg::Cancelled));
        }
        stage(0.4);
        let (src_rate, n_ch) = (decoded.audio.sample_rate, decoded.audio.channels as usize);
        let mut chans = deinterleave(&decoded.audio.samples, n_ch);
        let mut names = vec![name.clone()];
        if n_ch > 2 {
            match multichannel.as_deref() {
                Some("split") => names = (1..=n_ch).map(|c| format!("{name} {c}")).collect(),
                Some("stereo") => {
                    let side = |parity: usize| -> Vec<f32> {
                        let picked: Vec<&Vec<f32>> = chans.iter().enumerate().filter(|(c, _)| c % 2 == parity).map(|(_, v)| v).collect();
                        (0..chans[0].len()).map(|k| picked.iter().map(|v| v[k]).sum::<f32>() / picked.len() as f32).collect()
                    };
                    chans = vec![side(0), side(1)];
                }
                _ => {
                    let _ = tx.send(Msg::Warning(format!("« {name} » : {n_ch} canaux — choisis « une piste par canal » ou « mixage stéréo »")));
                    continue;
                }
            }
        }
        let converted = src_rate != rate;
        if converted {
            chans = chans.iter().map(|c| resample(c, src_rate, rate)).collect();
        }
        if stop() {
            undo(&written);
            return drop(tx.send(Msg::Cancelled));
        }
        stage(0.7);
        // Une piste par canal en `split`, sinon un seul fichier mono ou stéréo.
        let groups: Vec<Vec<Vec<f32>>> = if names.len() > 1 { chans.into_iter().map(|c| vec![c]).collect() } else { vec![chans] };
        let mut ready = Vec::new();
        for (group, clip_name) in groups.into_iter().zip(names) {
            let data = match AudioData::new(rate, group) {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(Msg::Warning(format!("« {clip_name} » : {e:?}")));
                    continue;
                }
            };
            let referenced = job.plan == Plan::Reference;
            let asset_path = if referenced {
                absolute(&job.path)
            } else {
                let dir = project_dir.as_ref().expect("une copie a un dossier");
                let rel = unique_audio_path(dir, &clip_name, &taken);
                let target = dir.join(&rel);
                if let Err(e) = std::fs::create_dir_all(target.parent().unwrap_or(dir)).and_then(|_| project_io::write_wav_file(&target, &data)) {
                    undo(&written);
                    return drop(tx.send(Msg::Failed(format!("« {clip_name} » : écriture de {} impossible : {e}", target.display()))));
                }
                written.push(target);
                taken.push(rel.clone());
                rel
            };
            let peaks = Arc::new(Peaks::build(&data));
            ready.push(Ready { name: clip_name, data, asset_path, peaks, converted, referenced });
        }
        stage(1.0);
        let _ = tx.send(Msg::Ready(ready));
    }
    let _ = tx.send(Msg::Finished);
}

impl Host {
    pub(super) fn refuse_while_importing(&self, command: &str) -> Option<Vec<Event>> {
        self.import.as_ref().map(|_| vec![Event::Error { command: command.into(), message: "un import est en cours : attends sa fin ou annule-le".into() }])
    }

    pub(super) fn probe_audio(&self, files: &[String]) -> Vec<Event> {
        let rate = self.session.sample_rate();
        let list = files
            .iter()
            .map(|f| {
                let path = Path::new(f);
                let name = stem_of(path);
                match probe_file(path) {
                    Ok(p) => AudioFileInfo {
                        path: f.clone(),
                        name,
                        format: Some(match p.format { Format::Wav => "wav", Format::Flac => "flac", Format::Mp3 => "mp3" }.into()),
                        sample_rate: p.sample_rate,
                        channels: p.channels,
                        frames: p.frames,
                        needs_conversion: p.sample_rate != rate,
                        error: None,
                    },
                    Err(e) => AudioFileInfo { path: f.clone(), name, format: None, sample_rate: 0, channels: 0, frames: None, needs_conversion: false, error: Some(e.to_string()) },
                }
            })
            .collect();
        vec![Event::AudioInfo { files: list }]
    }

    pub(super) fn start_import(&mut self, cmd: Command) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "import_audio".into(), message }];
        let Command::ImportAudio { files, node, pos, reference, multichannel } = cmd else { return Vec::new() };
        if self.import.is_some() {
            return error("un import est déjà en cours".into());
        }
        if let Some(refused) = self.refuse_while_recording("import_audio") {
            return refused;
        }
        let rate = self.session.sample_rate();
        let target = match node {
            Some(n) => match self.session.graph().nodes.get(n as usize) {
                Some(nd) if matches!(nd.kind, NodeKind::Track(_)) => Some(NodeId(n)),
                _ => return error(format!("nœud {n} : ce n'est pas une piste")),
            },
            None => None,
        };
        // Pour chaque fichier : copier, ou référencer en place (seulement mono / stéréo déjà à la fréquence du projet).
        let mut jobs = Vec::new();
        let mut warnings = Vec::new();
        for f in &files {
            let path = PathBuf::from(f);
            let plan = if reference {
                match probe_file(&path) {
                    Ok(p) if p.sample_rate == rate && p.channels <= 2 => Plan::Reference,
                    Ok(p) => {
                        let why = if p.channels > 2 { format!("{} canaux", p.channels) } else { format!("{} Hz au lieu de {rate} Hz", p.sample_rate) };
                        warnings.push(format!("« {} » : copié dans le projet au lieu d'être référencé ({why})", stem_of(&path)));
                        Plan::Copy
                    }
                    Err(_) => Plan::Copy, // l'erreur sera dite par le décodage
                }
            } else {
                Plan::Copy
            };
            jobs.push(Job { path, plan });
        }
        let project_dir = self.project_dir.clone();
        if jobs.iter().any(|j| j.plan == Plan::Copy) && project_dir.is_none() {
            return error("le projet n'a pas encore de dossier : enregistre-le d'abord, ou choisis « référencer en place »".into());
        }
        let mut events = self.end_preview(true, false);
        let taken: Vec<String> = self.session.assets().iter().map(|(_, p)| p.clone()).collect();
        let start = pos.unwrap_or_else(|| self.handle.position());
        let total = jobs.len() as u32;
        let first = stem_of(&jobs[0].path);
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let work_args = Work { jobs, rate, multichannel, project_dir: project_dir.clone(), taken };
        let thread = {
            let cancel = cancel.clone();
            match std::thread::Builder::new().name("import".into()).spawn(move || work(work_args, tx, cancel)) {
                Ok(t) => t,
                Err(e) => return error(format!("fil d'import impossible : {e}")),
            }
        };
        let job = ImportJob { rx, cancel, thread: Some(thread), total, index: 0, fraction: 0.0, current: first, ready: Vec::new(), warnings, target, pos: start, copied: Vec::new(), project_dir };
        events.push(job.event(true, false, None, Vec::new()));
        self.import = Some(job);
        events
    }

    pub(super) fn cancel_import(&mut self) -> Vec<Event> {
        if let Some(job) = &self.import {
            job.cancel.store(true, Ordering::Relaxed);
        }
        Vec::new()
    }

    /// Relève le fil d'import (appelé par `tick`) ; à la fin, pose les clips d'un seul coup.
    pub(super) fn import_tick(&mut self) -> Vec<Event> {
        let Some(job) = self.import.as_mut() else { return Vec::new() };
        let mut changed = false;
        let mut end: Option<Result<(), (bool, String)>> = None; // Ok = terminé ; Err((annulé, message))
        loop {
            match job.rx.try_recv() {
                Ok(Msg::Stage { index, fraction, name }) => {
                    job.index = index;
                    job.fraction = fraction;
                    job.current = name;
                    changed = true;
                }
                Ok(Msg::Ready(items)) => {
                    for r in &items {
                        if !r.referenced {
                            if let Some(d) = &job.project_dir {
                                job.copied.push(d.join(&r.asset_path));
                            }
                        }
                    }
                    job.ready.extend(items);
                    changed = true;
                }
                Ok(Msg::Warning(w)) => {
                    job.warnings.push(w);
                    changed = true;
                }
                Ok(Msg::Finished) => end = Some(Ok(())),
                Ok(Msg::Cancelled) => end = Some(Err((true, String::new()))),
                Ok(Msg::Failed(m)) => end = Some(Err((false, m))),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    end.get_or_insert(Err((false, "l'import s'est arrêté de façon inattendue".into())));
                }
            }
            if end.is_some() {
                break;
            }
        }
        let Some(result) = end else {
            return if changed { vec![job.event(true, false, None, Vec::new())] } else { Vec::new() };
        };
        let mut job = self.import.take().expect("import en cours");
        if let Some(t) = job.thread.take() {
            let _ = t.join();
        }
        let remove_copies = |job: &ImportJob| job.copied.iter().for_each(|f| drop(std::fs::remove_file(f)));
        match result {
            Err((cancelled, message)) => {
                remove_copies(&job);
                vec![job.event(false, cancelled, (!cancelled).then_some(message), Vec::new())]
            }
            Ok(()) if job.ready.is_empty() => {
                let message = job.warnings.last().cloned().unwrap_or_else(|| "aucun fichier importable".into());
                vec![job.event(false, false, Some(message), Vec::new())]
            }
            Ok(()) => self.finish_import(job),
        }
    }

    fn finish_import(&mut self, job: ImportJob) -> Vec<Event> {
        let (pos, target) = (job.pos, job.target);
        let result = self.session.group("Importer de l'audio", |s| {
            let mut placed = Vec::new();
            let mut first = true;
            for r in &job.ready {
                s.register_asset(&r.asset_path, Arc::clone(&r.data));
                let node = match target {
                    Some(n) if first => n,
                    _ => {
                        let kind = if r.data.channel_count() == 2 { NewNode::Stereo } else { NewNode::Mono };
                        let color = TRACK_COLORS[s.graph().nodes.len().saturating_sub(1) % TRACK_COLORS.len()];
                        s.add_node(kind, &r.name, color)?
                    }
                };
                first = false;
                let id = s.add_clip(node, Clip::whole(Arc::clone(&r.data), pos))?;
                placed.push((id, s.graph().nodes[node.0 as usize].name.clone()));
            }
            Ok(placed)
        });
        match result {
            Ok(placed) => {
                let mut clips = Vec::new();
                for (r, (id, track)) in job.ready.iter().zip(placed) {
                    self.meta.clip_names.insert(id.0, r.name.clone());
                    self.peaks.insert(Arc::as_ptr(&r.data) as usize, Arc::clone(&r.peaks));
                    clips.push(ImportedClip { name: r.name.clone(), track, converted: r.converted, referenced: r.referenced });
                }
                let mut events = self.finish_edit("import_audio", Ok(()));
                events.push(job.event(false, false, None, clips));
                events
            }
            Err(e) => {
                job.copied.iter().for_each(|f| drop(std::fs::remove_file(f)));
                vec![job.event(false, false, Some(format!("les clips n'ont pas pu être posés : {}", explain(&e))), Vec::new())]
            }
        }
    }
}

impl ImportJob {
    fn event(&self, active: bool, cancelled: bool, error: Option<String>, clips: Vec<ImportedClip>) -> Event {
        let progress = if active { ((self.index as f64 + self.fraction) / self.total.max(1) as f64).min(1.0) } else if error.is_none() && !cancelled { 1.0 } else { 0.0 };
        let done = if active { self.index } else if error.is_none() && !cancelled { self.total } else { 0 };
        Event::Import { active, progress, done, total: self.total, current: self.current.clone(), clips, warnings: self.warnings.clone(), cancelled, error }
    }
}

