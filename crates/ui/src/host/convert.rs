//! Conversion d'un projet à la fréquence d'échantillonnage du moteur : copie convertie dans un nouveau dossier, sur un fil de travail
//! (l'audio est rééchantillonné et réécrit : cela peut prendre du temps), puis ouverture de la copie. L'original n'est jamais modifié.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;

use super::Host;
use crate::project_io::{self, ConvertError};
use crate::protocol::Event;

enum Msg {
    Progress { done: usize, total: usize },
    Finished(Vec<String>),
    Cancelled,
    Failed(String),
}

pub(super) struct ConvertJob {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    dst: PathBuf,
    done: usize,
    total: usize,
}

/// `dossier/Nom (48000 Hz).daw`, ou `Nom (48000 Hz) (2).daw`… : jamais un dossier qui existe déjà.
fn target_dir(src: &Path, rate: u32) -> PathBuf {
    let stem = project_io::project_name(src);
    let parent = src.parent().map(Path::to_path_buf).unwrap_or_default();
    let first = parent.join(format!("{stem} ({rate} Hz).{}", project_io::EXTENSION));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| parent.join(format!("{stem} ({rate} Hz) ({n}).{}", project_io::EXTENSION))).find(|p| !p.exists()).expect("une suite infinie")
}

impl ConvertJob {
    fn event(&self, active: bool, cancelled: bool, error: Option<String>, path: Option<String>, warnings: Vec<String>) -> Event {
        // Chargement et rééchantillonnage des fichiers : 0 à 90 % ; l'écriture du nouveau projet, qui suit, fait le reste.
        let progress = if !active {
            if error.is_none() && !cancelled { 1.0 } else { 0.0 }
        } else {
            0.9 * self.done as f64 / self.total.max(1) as f64
        };
        Event::Convert { active, progress, done: self.done as u32, total: self.total as u32, current: project_io::project_name(&self.dst), path, warnings, cancelled, error }
    }
}

impl Host {
    pub(super) fn refuse_while_converting(&self, command: &str) -> Option<Vec<Event>> {
        self.convert.as_ref().map(|_| vec![Event::Error { command: command.into(), message: "une conversion de projet est en cours : attends sa fin ou annule-la".into() }])
    }

    pub(super) fn start_convert(&mut self, path: &str) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "convert_project".into(), message }];
        if self.convert.is_some() {
            return error("une conversion est déjà en cours".into());
        }
        if let Some(refused) = self.refuse_while_recording("convert_project").or_else(|| self.refuse_while_importing("convert_project")) {
            return refused;
        }
        let src = project_io::project_dir(path);
        let rate = self.session.sample_rate();
        match project_io::peek_sample_rate(&src, project_io::PROJECT_FILE) {
            Ok(r) if r == rate => return error(format!("ce projet est déjà à {rate} Hz : l'ouvrir suffit")),
            Ok(_) => {}
            Err(e) => return error(e),
        }
        let dst = target_dir(&src, rate);
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (worker_dst, worker_cancel) = (dst.clone(), cancel.clone());
        let thread = match std::thread::Builder::new().name("conversion".into()).spawn(move || {
            let tx2 = tx.clone();
            let result = project_io::convert_project(&src, &worker_dst, rate, &mut |done, total| {
                let _ = tx2.send(Msg::Progress { done, total });
                !worker_cancel.load(Ordering::Relaxed)
            });
            let _ = tx.send(match result {
                Ok(w) => Msg::Finished(w),
                Err(ConvertError::Cancelled) => Msg::Cancelled,
                Err(ConvertError::Failed(m)) => Msg::Failed(m),
            });
        }) {
            Ok(t) => t,
            Err(e) => return error(format!("fil de conversion impossible : {e}")),
        };
        let job = ConvertJob { rx, cancel, thread: Some(thread), dst, done: 0, total: 1 };
        let ev = job.event(true, false, None, None, Vec::new());
        self.convert = Some(job);
        vec![ev]
    }

    pub(super) fn cancel_convert(&mut self) -> Vec<Event> {
        if let Some(job) = &self.convert {
            job.cancel.store(true, Ordering::Relaxed);
        }
        Vec::new()
    }

    /// Relève le fil de conversion (appelé par `tick`) ; à la fin, ouvre la copie convertie.
    pub(super) fn convert_tick(&mut self) -> Vec<Event> {
        let Some(job) = self.convert.as_mut() else { return Vec::new() };
        let mut changed = false;
        let mut end: Option<Msg> = None;
        loop {
            match job.rx.try_recv() {
                Ok(Msg::Progress { done, total }) => {
                    job.done = done;
                    job.total = total.max(1);
                    changed = true;
                }
                Ok(m @ (Msg::Finished(_) | Msg::Cancelled | Msg::Failed(_))) => {
                    end = Some(m);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    end = Some(Msg::Failed("la conversion s'est arrêtée de façon inattendue".into()));
                    break;
                }
            }
        }
        let Some(end) = end else {
            return if changed { vec![job.event(true, false, None, None, Vec::new())] } else { Vec::new() };
        };
        let mut job = self.convert.take().expect("conversion en cours");
        if let Some(t) = job.thread.take() {
            let _ = t.join();
        }
        match end {
            Msg::Finished(warnings) => {
                let path = job.dst.to_string_lossy().into_owned();
                job.done = job.total;
                let mut events = vec![job.event(false, false, None, Some(path.clone()), warnings)];
                events.extend(self.open_project(&path, false));
                events
            }
            Msg::Cancelled => vec![job.event(false, true, None, None, Vec::new())],
            Msg::Failed(m) => vec![job.event(false, false, Some(m), None, Vec::new())],
            Msg::Progress { .. } => Vec::new(),
        }
    }
}
