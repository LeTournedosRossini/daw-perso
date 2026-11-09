//! Export du mixage et des stems : le rendu tourne sur un fil de travail (la lecture et l'édition restent possibles), avec sa
//! propre instance du moteur ; `Host::tick` relève sa progression. Le moteur d'export est `daw_offline_render::export`.
//!
//! Le projet n'est pas modifié par un export (pas de « modifications non enregistrées »). Les fichiers sont écrits sous un nom
//! provisoire `.part` puis renommés : un export annulé ou interrompu ne laisse jamais de fichier à moitié écrit.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use daw_core_types::NodeId;
use daw_engine::{GraphDesc, NodeKind};
use daw_offline_render::Progress;
use daw_offline_render::export::{export_mix, stem_graph, BitDepth, Container, ExportError, ExportRange, ExportSpec, Format};

use super::{with_state, Host};
use crate::protocol::{Command, Event, ExportFormat, ExportMode, ExportScope, ExportedFile};

/// Ce qui remonte du fil d'export vers `Host::tick`.
enum Msg {
    Started { index: u32, name: String },
    Progress { index: u32, fraction: f64 },
    File(ExportedFile),
    Warning(String),
    Finished,
    Cancelled,
    Failed(String),
}

/// Enveloppe qui permet de confier un graphe à un autre fil.
///
/// SAFETY (invariant) : `GraphDesc` n'est pas `Send` parce que ses usines d'effets (`InsertFactory`) sont des fermetures
/// `Arc<dyn Fn>` sans garantie de fil. Celles de l'hôte (`clap_insert_factory`) ne capturent qu'un `Arc<Bundle>` et un
/// identifiant ; elles ne font que créer une instance indépendante (`clap_plugin_factory::create_plugin`, que la spécification
/// CLAP déclare *thread-safe*), qui appartient ensuite entièrement au fil d'export (créée, utilisée et détruite là ; `Plugin`
/// est lui-même `Send`). Le fil de contrôle garde les mêmes `Arc` : l'usine peut donc être appelée des deux côtés en même
/// temps, ce que `create_plugin` autorise. Aucun autre état partagé : les données audio sont des `Arc<AudioData>` immuables.
struct SendGraph(GraphDesc);
// SAFETY : voir ci-dessus.
unsafe impl Send for SendGraph {}

pub(super) struct ExportJob {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    total: u32,
    index: u32,
    fraction: f64,
    current: String,
    files: Vec<ExportedFile>,
    warnings: Vec<String>,
    dir: PathBuf,
}

impl ExportJob {
    fn event(&self, active: bool, cancelled: bool, error: Option<String>) -> Event {
        let progress = if active { ((self.index as f64 + self.fraction) / self.total.max(1) as f64).min(1.0) } else if error.is_none() && !cancelled { 1.0 } else { (self.files.len() as f64 / self.total.max(1) as f64).min(1.0) };
        Event::Export { active, progress, done: self.files.len() as u32, total: self.total, current: self.current.clone(), files: self.files.clone(), warnings: self.warnings.clone(), cancelled, error, dir: self.dir.to_string_lossy().into_owned() }
    }
}

/// Nom de fichier sûr : on ne retire que ce que les systèmes de fichiers refusent, les accents et les espaces restent.
fn file_stem(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '-' } else { c }).collect();
    let s = s.trim().trim_matches('.').trim().to_owned();
    if s.is_empty() { "export".into() } else { s }
}

/// `dir/stem.ext`, ou `dir/stem (2).ext`… si le fichier existe déjà : un export n'écrase jamais rien.
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| dir.join(format!("{stem} ({n}).{ext}"))).find(|p| !p.exists()).expect("une suite infinie")
}

fn format_of(f: ExportFormat, dither: Option<bool>) -> Format {
    let (container, depth, default_dither) = match f {
        ExportFormat::Wav16 => (Container::Wav, BitDepth::Pcm16, true),
        ExportFormat::Wav24 => (Container::Wav, BitDepth::Pcm24, false),
        ExportFormat::Wav32f => (Container::Wav, BitDepth::Float32, false),
        ExportFormat::Flac16 => (Container::Flac, BitDepth::Pcm16, true),
        ExportFormat::Flac24 => (Container::Flac, BitDepth::Pcm24, false),
    };
    Format { container, depth, dither: depth != BitDepth::Float32 && dither.unwrap_or(default_dither) }
}

struct Plan {
    graph: SendGraph,
    /// (nom de fichier sans extension, stem : le nœud, ou `None` pour le mixage)
    jobs: Vec<(String, Option<NodeId>)>,
    sample_rate: u32,
    range: ExportRange,
    tail_secs: f32,
    format: Format,
    normalize_db: Option<f32>,
    dir: PathBuf,
}

fn work(plan: Plan, tx: Sender<Msg>, cancel: Arc<AtomicBool>) {
    let Plan { graph, jobs, sample_rate, range, tail_secs, format, normalize_db, dir } = plan;
    let graph = graph.0;
    for (index, (name, node)) in jobs.into_iter().enumerate() {
        let index = index as u32;
        let _ = tx.send(Msg::Started { index, name: name.clone() });
        let g = match node {
            Some(n) => match stem_graph(&graph, n) {
                Ok(g) => g,
                Err(e) => return drop(tx.send(Msg::Failed(format!("« {name} » : {e}")))),
            },
            None => graph.clone(),
        };
        let mut spec = ExportSpec::new(sample_rate, g, range, format);
        spec.tail_secs = tail_secs;
        spec.normalize_db = if node.is_none() { normalize_db } else { None };
        let (ptx, pcancel, last) = (tx.clone(), cancel.clone(), AtomicU64::new(u64::MAX));
        spec.progress = Some(Progress(Arc::new(move |done, total| {
            let per_mille = done * 1000 / total.max(1);
            if last.swap(per_mille, Ordering::Relaxed) != per_mille {
                let _ = ptx.send(Msg::Progress { index, fraction: per_mille as f64 / 1000.0 });
            }
            !pcancel.load(Ordering::Relaxed)
        })));
        let exported = match export_mix(&spec) {
            Ok(e) => e,
            Err(ExportError::Cancelled) => return drop(tx.send(Msg::Cancelled)),
            Err(e) => return drop(tx.send(Msg::Failed(format!("« {name} » : {e}")))),
        };
        if cancel.load(Ordering::Relaxed) {
            return drop(tx.send(Msg::Cancelled));
        }
        let path = unique_path(&dir, &file_stem(&name), format.extension());
        let part = path.with_extension(format!("{}.part", format.extension()));
        if let Err(e) = std::fs::write(&part, &exported.bytes).and_then(|_| std::fs::rename(&part, &path)) {
            let _ = std::fs::remove_file(&part);
            return drop(tx.send(Msg::Failed(format!("« {name} » : écriture de {} impossible : {e}", path.display()))));
        }
        let r = &exported.report;
        if r.clipped_samples > 0 {
            let _ = tx.send(Msg::Warning(format!("« {name} » : {} échantillon(s) au-delà de la pleine échelle (écrêtés) — baisser le volume, ou exporter en 32 bits flottant", r.clipped_samples)));
        }
        let peak_db = (r.peak > 0.0).then(|| 20.0 * (r.peak as f64).log10());
        let _ = tx.send(Msg::File(ExportedFile { path: path.to_string_lossy().into_owned(), frames: r.frames, peak_db, clipped: r.clipped_samples, applied_gain_db: r.applied_gain_db as f64 }));
    }
    let _ = tx.send(Msg::Finished);
}

impl Host {
    pub(super) fn refuse_while_exporting(&self, command: &str) -> Option<Vec<Event>> {
        self.export.as_ref().map(|_| vec![Event::Error { command: command.into(), message: "un export est en cours : attends sa fin ou annule-le".into() }])
    }

    pub(super) fn start_export(&mut self, cmd: Command) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "export".into(), message }];
        let Command::Export { scope, start, end, mode, nodes, format, dither, tail_secs, normalize_db, dir, name } = cmd else { return Vec::new() };
        if self.export.is_some() {
            return error("un export est déjà en cours".into());
        }
        if let Some(refused) = self.refuse_while_recording("export") {
            return refused;
        }
        // Un aperçu de fondu en cours a posé des solos dans la session : ils ne doivent pas se retrouver dans le fichier.
        let mut events = self.end_preview(true, false);
        let dir = match (dir, &self.project_dir) {
            (Some(d), _) => PathBuf::from(d),
            (None, Some(p)) => p.join("export"),
            (None, None) => return error("le projet n'a pas encore de dossier : choisis un dossier d'export, ou enregistre d'abord le projet".into()),
        };
        let range = match scope {
            ExportScope::Project => ExportRange::whole_project(self.session.graph()),
            ExportScope::Loop => match self.loop_range {
                Some(l) => ExportRange { start: l.start, end: l.end },
                None => return error("aucune boucle : pose une boucle (Maj + glisser sur la règle) ou choisis une autre zone".into()),
            },
            ExportScope::Range => ExportRange { start: start.unwrap_or(0), end: end.unwrap_or(0) },
        };
        if range.end <= range.start {
            return error(if scope == ExportScope::Project { "le projet est vide : rien à exporter".into() } else { "zone d'export vide".into() });
        }
        let g = self.session.graph();
        let base = file_stem(&name.unwrap_or_else(|| self.name.clone()));
        let jobs: Vec<(String, Option<NodeId>)> = match mode {
            ExportMode::Mix => vec![(base, None)],
            ExportMode::Stems => {
                let chosen: Vec<NodeId> = match nodes {
                    Some(list) => list.into_iter().map(NodeId).collect(),
                    None => g.nodes.iter().filter(|n| !matches!(n.kind, NodeKind::Master)).map(|n| n.id).collect(),
                };
                let mut jobs = Vec::new();
                for id in chosen {
                    match g.nodes.iter().find(|n| n.id == id) {
                        Some(n) if !matches!(n.kind, NodeKind::Master) => jobs.push((format!("{base} - {}", n.name), Some(id))),
                        _ => return error(format!("nœud {} inconnu ou sans stem", id.0)),
                    }
                }
                if jobs.is_empty() {
                    return error("aucune piste ni bus à exporter".into());
                }
                jobs
            }
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return error(format!("dossier d'export {} impossible : {e}", dir.display()));
        }
        // Le graphe d'export porte l'état courant de chaque plugin : le rendu doit sonner comme le projet à cet instant.
        let mut graph = g.clone();
        for node in &mut graph.nodes {
            for ins in &mut node.inserts {
                if let Some(state) = self.handle.insert_control(ins.id).and_then(|c| c.save_state()) {
                    ins.factory = with_state(ins.factory.clone(), Some(state));
                }
            }
        }
        let plan = Plan { graph: SendGraph(graph), sample_rate: self.session.sample_rate(), range, tail_secs: tail_secs as f32, format: format_of(format, dither), normalize_db: normalize_db.map(|n| n as f32), dir: dir.clone(), jobs: jobs.clone() };
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread = {
            let cancel = cancel.clone();
            match std::thread::Builder::new().name("export".into()).spawn(move || work(plan, tx, cancel)) {
                Ok(t) => t,
                Err(e) => return error(format!("fil d'export impossible : {e}")),
            }
        };
        let job = ExportJob { rx, cancel, thread: Some(thread), total: jobs.len() as u32, index: 0, fraction: 0.0, current: jobs[0].0.clone(), files: Vec::new(), warnings: Vec::new(), dir };
        events.push(job.event(true, false, None));
        self.export = Some(job);
        events
    }

    pub(super) fn cancel_export(&mut self) -> Vec<Event> {
        if let Some(job) = &self.export {
            job.cancel.store(true, Ordering::Relaxed);
        }
        Vec::new()
    }

    /// Relève la progression du fil d'export (appelé par `tick`) : au plus un événement par appel.
    pub(super) fn export_tick(&mut self) -> Vec<Event> {
        let Some(job) = self.export.as_mut() else { return Vec::new() };
        let mut changed = false;
        let mut end: Option<(bool, Option<String>)> = None; // (annulé, erreur)
        loop {
            match job.rx.try_recv() {
                Ok(Msg::Started { index, name }) => {
                    job.index = index;
                    job.fraction = 0.0;
                    job.current = name;
                    changed = true;
                }
                Ok(Msg::Progress { index, fraction }) => {
                    job.index = index;
                    job.fraction = fraction;
                    changed = true;
                }
                Ok(Msg::File(f)) => {
                    job.files.push(f);
                    changed = true;
                }
                Ok(Msg::Warning(w)) => {
                    job.warnings.push(w);
                    changed = true;
                }
                Ok(Msg::Finished) => {
                    end = Some((false, None));
                }
                Ok(Msg::Cancelled) => end = Some((true, None)),
                Ok(Msg::Failed(m)) => end = Some((false, Some(m))),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if end.is_none() {
                        end = Some((false, Some("l'export s'est arrêté de façon inattendue (plugin défaillant ?)".into())));
                    }
                    break;
                }
            }
            if end.is_some() {
                break;
            }
        }
        match end {
            Some((cancelled, error)) => {
                let job = self.export.take().expect("export en cours");
                if let Some(t) = job.thread {
                    let _ = t.join();
                }
                let (c, e) = (cancelled, error);
                let ev = Event::Export {
                    active: false,
                    progress: if c || e.is_some() { (job.files.len() as f64 / job.total.max(1) as f64).min(1.0) } else { 1.0 },
                    done: job.files.len() as u32,
                    total: job.total,
                    current: job.current,
                    files: job.files,
                    warnings: job.warnings,
                    cancelled: c,
                    error: e,
                    dir: job.dir.to_string_lossy().into_owned(),
                };
                vec![ev]
            }
            None if changed => vec![job.event(true, false, None)],
            None => Vec::new(),
        }
    }
}
