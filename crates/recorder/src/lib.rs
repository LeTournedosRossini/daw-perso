//! Enregistrement : écriture disque hors du thread audio.
//!
//! Le thread audio ne fait que copier l'entrée dans une file lock-free (voir `daw_engine::RecordReceiver`) ; un fil
//! d'écriture la vide vers un fichier WAV flottant 32 bits. Ce crate contient aussi la lecture des prises et les
//! analyses utilisées pour calibrer et vérifier l'enregistrement (alignement d'impulsions, intégrité d'une tonalité).

pub mod analysis;
pub mod decode;
pub mod wav;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use daw_engine::{RecordReceiver, RecordStatus};

use wav::WavWriter;

/// Résultat d'une prise terminée.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeInfo {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
    /// Trames écrites dans le fichier.
    pub frames: u64,
    /// Position de la ligne de temps au premier échantillon (le décalage de latence n'est pas appliqué ici).
    pub start_pos: u64,
    /// Blocs perdus parce que le disque ne suivait pas : si > 0, la prise est abîmée.
    pub overruns: u32,
}

/// Prise en cours d'écriture.
pub struct Recording {
    thread: Option<JoinHandle<io::Result<u64>>>,
    status: Arc<RecordStatus>,
    path: PathBuf,
    sample_rate: u32,
    channels: u16,
}

/// Une mise à jour de l'en-tête (pour qu'un fichier interrompu reste lisible) toutes les secondes d'audio.
const HEADER_UPDATE_SECONDS: u32 = 1;

impl Recording {
    /// Crée le fichier et démarre le fil d'écriture. Il s'arrête tout seul quand le moteur a retiré la prise
    /// (`EngineHandle::stop_record`) et que la file est vide.
    pub fn start(receiver: RecordReceiver, path: impl AsRef<Path>) -> io::Result<Recording> {
        let path = path.as_ref().to_owned();
        let channels = receiver.source.channels() as u16;
        let sample_rate = receiver.sample_rate;
        let mut writer = WavWriter::create(&path, sample_rate, channels)?;
        let status = Arc::clone(&receiver.status);
        let mut consumer = receiver.consumer;
        let closed_status = Arc::clone(&status);
        let update_every = (sample_rate * HEADER_UPDATE_SECONDS) as u64 * channels as u64;
        let thread = std::thread::Builder::new().name("enregistrement".into()).spawn(move || -> io::Result<u64> {
            let mut since_update = 0u64;
            loop {
                // L'état « fermé » est lu avant la file : tout ce que le moteur a écrit avant de fermer est visible.
                let closed = closed_status.closed.load(Ordering::Acquire) || consumer.is_abandoned();
                let n = consumer.slots();
                if n > 0 {
                    let chunk = consumer.read_chunk(n).expect("n échantillons sont lisibles");
                    let (a, b) = chunk.as_slices();
                    writer.write_samples(a)?;
                    writer.write_samples(b)?;
                    chunk.commit_all();
                    since_update += n as u64;
                    if since_update >= update_every {
                        writer.update_header()?;
                        since_update = 0;
                    }
                } else if closed {
                    break;
                } else {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            writer.finish()
        })?;
        Ok(Recording { thread: Some(thread), status, path, sample_rate, channels })
    }

    pub fn status(&self) -> &Arc<RecordStatus> {
        &self.status
    }

    /// Attend la fin de l'écriture (le moteur doit avoir retiré la prise) et rend le résumé.
    pub fn finish(mut self) -> io::Result<TakeInfo> {
        let frames = self
            .thread
            .take()
            .expect("finish ne peut être appelée qu'une fois")
            .join()
            .map_err(|_| io::Error::other("le fil d'écriture a paniqué"))??;
        Ok(TakeInfo {
            path: self.path.clone(),
            sample_rate: self.sample_rate,
            channels: self.channels,
            frames,
            start_pos: self.status.start_pos.load(Ordering::Acquire),
            overruns: self.status.overruns.load(Ordering::Relaxed),
        })
    }
}
