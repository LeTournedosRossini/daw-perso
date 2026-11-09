//! Prises d'enregistrement : copie de l'entrée audio d'une piste vers une file lock-free lue par un fil d'écriture.
//!
//! Côté audio, [`RecordTap::push`] écrit dans la file sans allocation ni verrou. Côté contrôle, le récepteur
//! [`RecordReceiver`] donne la file à lire (fil disque) et l'état partagé ([`RecordStatus`]).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer};

/// Nombre maximal de canaux d'entrée du pilote que le moteur accepte en un appel.
pub const MAX_INPUTS: usize = 32;

/// Quel(s) canal(aux) d'entrée enregistrer ou écouter. Les canaux sont numérotés **à partir de 0** parmi les entrées que le
/// pilote a ouvertes ; un canal qui n'existe pas donne du silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordSource {
    /// Premier canal d'entrée, en mono (= `Channel(0)`).
    Left,
    /// Second canal d'entrée, en mono (= `Channel(1)`).
    Right,
    /// Les deux premiers, en stéréo (= `Pair(0, 1)`).
    Stereo,
    /// Un canal d'entrée au choix, en mono.
    Channel(u8),
    /// Deux canaux d'entrée au choix (gauche, droite), en stéréo.
    Pair(u8, u8),
}

impl RecordSource {
    pub fn channels(self) -> usize {
        if matches!(self, RecordSource::Stereo | RecordSource::Pair(..)) {
            2
        } else {
            1
        }
    }

    /// Canaux d'entrée utilisés (gauche, droite) ; en mono, les deux sont le même canal.
    pub fn indices(self) -> (usize, usize) {
        match self {
            RecordSource::Left => (0, 0),
            RecordSource::Right => (1, 1),
            RecordSource::Stereo => (0, 1),
            RecordSource::Channel(c) => (c as usize, c as usize),
            RecordSource::Pair(a, b) => (a as usize, b as usize),
        }
    }
}

/// Échantillon `j` du canal d'entrée `ch` ; silence si le canal n'existe pas.
#[inline]
pub(crate) fn sample(inputs: &[&[f32]], ch: usize, j: usize) -> f32 {
    inputs.get(ch).and_then(|s| s.get(j)).copied().unwrap_or(0.0)
}

/// État d'un enregistrement, partagé entre le thread audio (qui écrit) et le contrôle / le fil disque (qui lisent).
#[derive(Debug, Default)]
pub struct RecordStatus {
    /// Le premier bloc a été reçu.
    pub started: AtomicBool,
    /// Position de la ligne de temps (échantillons) au premier échantillon enregistré.
    pub start_pos: AtomicU64,
    /// Trames passées dans la prise (y compris celles perdues par débordement).
    pub frames: AtomicU64,
    /// Blocs perdus parce que la file était pleine (le fil disque ne suit pas) : la prise est alors abîmée.
    pub overruns: AtomicU32,
    /// Le moteur a retiré la prise : plus rien ne sera écrit dans la file.
    pub closed: AtomicBool,
}

pub(crate) struct RecordTap {
    producer: Producer<f32>,
    status: Arc<RecordStatus>,
    source: RecordSource,
    /// La prise ne commence qu'à cette position de la ligne de temps, et seulement pendant la lecture (pré-roll et pré-compte
    /// ne sont pas enregistrés) ; `None` : elle enregistre tous les blocs dès son installation.
    start_at: Option<u64>,
    /// La prise s'arrête à cette position (exclue) : le bloc qui la contient est coupé à l'échantillon près et la prise est déclarée
    /// terminée (`RecordStatus::closed`) sans attendre un ordre du contrôle (punch-out) ; `None` : jusqu'à l'ordre d'arrêt.
    end_at: Option<u64>,
}

impl RecordTap {
    /// Copie `n` trames de l'entrée vers la file. `inputs` : un tampon par canal d'entrée du pilote.
    pub(crate) fn push(&mut self, pos: u64, inputs: &[&[f32]], n: usize, playing: bool) {
        let (ia, ib) = self.source.indices();
        self.push_with(pos, n, playing, |which, j| sample(inputs, if which == 0 { ia } else { ib }, j));
    }

    /// Comme `push`, avec le signal **déjà traité** (dynamique d'entrée) : `a` et `b` sont les canaux gauche et droit de la prise (le
    /// second est ignoré pour une prise mono).
    pub(crate) fn push_processed(&mut self, pos: u64, a: &[f32], b: &[f32], n: usize, playing: bool) {
        self.push_with(pos, n, playing, |which, j| if which == 0 { a.get(j).copied().unwrap_or(0.0) } else { b.get(j).copied().unwrap_or(0.0) });
    }

    /// Corps commun : `get(canal de la prise, trame du bloc)` donne les échantillons (0 = gauche ou mono, 1 = droite).
    fn push_with(&mut self, pos: u64, n: usize, playing: bool, get: impl Fn(usize, usize) -> f32) {
        if self.status.closed.load(Ordering::Relaxed) {
            return;
        }
        // Prise « à partir de » : rien avant la position demandée ni hors lecture ; le bloc qui la contient est coupé à
        // l'échantillon près.
        let skip = match self.start_at {
            None => 0,
            Some(at) => {
                if !playing || pos + n as u64 <= at {
                    return;
                }
                at.saturating_sub(pos) as usize
            }
        };
        let (pos, mut n) = (pos + skip as u64, n - skip);
        let mut ended = false;
        if let Some(end) = self.end_at {
            if pos >= end {
                self.close();
                return;
            }
            if pos + n as u64 >= end {
                n = (end - pos) as usize;
                ended = true;
            }
        }
        if !self.status.started.load(Ordering::Relaxed) {
            self.status.start_pos.store(pos, Ordering::Relaxed);
            self.status.started.store(true, Ordering::Release);
        }
        let need = n * self.source.channels();
        match self.producer.write_chunk(need) {
            Ok(mut chunk) => {
                let (a, b) = chunk.as_mut_slices();
                let stereo = self.source.channels() == 2;
                let mut k = 0usize;
                for slot in a.iter_mut().chain(b.iter_mut()) {
                    *slot = if stereo { get(k % 2, skip + k / 2) } else { get(0, skip + k) };
                    k += 1;
                }
                chunk.commit_all();
            }
            Err(_) => {
                self.status.overruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.status.frames.fetch_add(n as u64, Ordering::Relaxed);
        if ended {
            self.close();
        }
    }

    /// Appelé quand le moteur retire la prise.
    pub(crate) fn close(&self) {
        self.status.closed.store(true, Ordering::Release);
    }
}

/// Côté contrôle d'une prise : la file à vider (échantillons entrelacés), l'état et le format.
pub struct RecordReceiver {
    pub consumer: Consumer<f32>,
    pub status: Arc<RecordStatus>,
    pub source: RecordSource,
    pub sample_rate: u32,
}

#[cfg(test)]
pub(crate) fn new_tap(source: RecordSource, capacity_frames: usize, sample_rate: u32) -> (RecordTap, RecordReceiver) {
    new_tap_at(source, capacity_frames, sample_rate, None, None)
}

pub(crate) fn new_tap_at(source: RecordSource, capacity_frames: usize, sample_rate: u32, start_at: Option<u64>, end_at: Option<u64>) -> (RecordTap, RecordReceiver) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity_frames.max(1) * source.channels());
    let status = Arc::new(RecordStatus::default());
    (RecordTap { producer, status: Arc::clone(&status), source, start_at, end_at }, RecordReceiver { consumer, status, source, sample_rate })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_by_source_and_tracks_position() {
        for (source, expected) in [(RecordSource::Left, vec![1.0, 2.0]), (RecordSource::Right, vec![-1.0, -2.0]), (RecordSource::Stereo, vec![1.0, -1.0, 2.0, -2.0])] {
            let (mut tap, mut rx) = new_tap(source, 16, 48_000);
            tap.push(777, &[&[1.0, 2.0], &[-1.0, -2.0]], 2, true);
            assert!(rx.status.started.load(Ordering::Acquire));
            assert_eq!(rx.status.start_pos.load(Ordering::Relaxed), 777);
            let chunk = rx.consumer.read_chunk(rx.consumer.slots()).unwrap();
            let (a, b) = chunk.as_slices();
            assert_eq!([a, b].concat(), expected);
        }
    }

    #[test]
    fn start_position_is_that_of_the_first_block_only() {
        let (mut tap, rx) = new_tap(RecordSource::Left, 64, 48_000);
        tap.push(100, &[&[0.0; 4], &[0.0; 4]], 4, true);
        tap.push(104, &[&[0.0; 4], &[0.0; 4]], 4, true);
        assert_eq!(rx.status.start_pos.load(Ordering::Relaxed), 100);
        assert_eq!(rx.status.frames.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn a_tap_with_an_end_position_is_cut_to_the_sample_and_closes_itself() {
        let (mut tap, mut rx) = new_tap_at(RecordSource::Left, 64, 48_000, Some(1_000), Some(1_010));
        tap.push(996, &[&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]], 8, true); // 996..1004 : on garde 1000..1004
        assert!(!rx.status.closed.load(Ordering::Acquire));
        tap.push(1_004, &[&[10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0]], 8, true); // 1004..1012 : on garde 1004..1010
        assert!(rx.status.closed.load(Ordering::Acquire), "fermée dès que la fin est atteinte");
        tap.push(1_012, &[&[99.0; 8]], 8, true); // plus rien après
        assert_eq!(rx.status.start_pos.load(Ordering::Relaxed), 1_000);
        assert_eq!(rx.status.frames.load(Ordering::Relaxed), 10);
        let chunk = rx.consumer.read_chunk(rx.consumer.slots()).unwrap();
        let (a, b) = chunk.as_slices();
        assert_eq!([a, b].concat(), vec![5.0, 6.0, 7.0, 8.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0]);
    }

    #[test]
    fn an_end_exactly_on_a_block_boundary_closes_at_that_block() {
        let (mut tap, rx) = new_tap_at(RecordSource::Left, 64, 48_000, None, Some(104));
        tap.push(100, &[&[1.0; 4]], 4, true);
        assert!(rx.status.closed.load(Ordering::Acquire));
        assert_eq!(rx.status.frames.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn a_tap_with_a_start_position_cuts_the_block_that_contains_it_and_waits_for_playback() {
        let (mut tap, mut rx) = new_tap_at(RecordSource::Left, 64, 48_000, Some(1_000), None);
        tap.push(900, &[&[9.0; 100]], 100, true); // avant la position : rien
        tap.push(1_000, &[&[5.0; 3]], 3, false); // arrêté : rien
        assert!(!rx.status.started.load(Ordering::Acquire));
        let ramp: Vec<f32> = (0..8).map(|k| k as f32).collect();
        tap.push(996, &[&ramp], 8, true); // contient 1 000 : on garde à partir du quatrième échantillon
        assert_eq!(rx.status.start_pos.load(Ordering::Relaxed), 1_000);
        let chunk = rx.consumer.read_chunk(rx.consumer.slots()).unwrap();
        let (a, b) = chunk.as_slices();
        assert_eq!([a, b].concat(), vec![4.0, 5.0, 6.0, 7.0]);
        assert_eq!(rx.status.frames.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn overrun_drops_whole_blocks_and_is_counted() {
        let (mut tap, rx) = new_tap(RecordSource::Stereo, 4, 48_000);
        tap.push(0, &[&[1.0; 4], &[1.0; 4]], 4, true); // remplit exactement la file
        tap.push(4, &[&[2.0; 2], &[2.0; 2]], 2, true); // pas de place
        assert_eq!(rx.status.overruns.load(Ordering::Relaxed), 1);
        assert_eq!(rx.status.frames.load(Ordering::Relaxed), 6);
    }
}
