//! Entrée MIDI en direct et enregistrement des notes jouées.
//!
//! Le matériel (ou le pilote) fournit des octets MIDI, [`parse_midi`] les convertit en [`MidiMsg`]. Un fil de pilote
//! les dépose dans [`MidiInputTx`] avec leur décalage en échantillons **dans le bloc en cours** (JACK le donne ;
//! sinon 0 = début de bloc). Le moteur les livre à la piste à cet instant précis (aux frontières de bloc interne près).
//!
//! L'enregistrement ([`MidiRecordReceiver`]) recopie, avec sa position absolue sur la ligne de temps, tout ce qui est
//! joué en direct sur la piste, dans une file lock-free lue côté contrôle.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer};

use crate::midi::MidiMsg;

/// Message d'entrée : piste visée (`NodeId.0`), message et décalage en échantillons dans le bloc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiIn {
    pub node: u32,
    pub msg: MidiMsg,
    pub offset: u32,
}

/// Côté producteur de l'entrée MIDI, à confier au fil du pilote (une seule source à la fois).
pub struct MidiInputTx {
    pub(crate) producer: Producer<MidiIn>,
}

impl MidiInputTx {
    /// `false` si la file est pleine (message perdu). Sans allocation ni verrou : utilisable dans un callback audio.
    pub fn push(&mut self, node: u32, msg: MidiMsg, offset: u32) -> bool {
        self.producer.push(MidiIn { node, msg, offset }).is_ok()
    }
}

/// Convertit un message MIDI brut : notes (0x8n, 0x9n ; vélocité 0 = fin), contrôleurs (0xBn), pression de touche
/// (0xAn), pression de canal (0xDn, deux octets), molette de hauteur (0xEn, 14 bits). Les changements de programme,
/// messages système et octets de données invalides donnent `None`.
pub fn parse_midi(bytes: &[u8]) -> Option<MidiMsg> {
    let (&status, rest) = bytes.split_first()?;
    let channel = status & 0x0F;
    let d1 = *rest.first()?;
    if d1 > 127 {
        return None;
    }
    let data2 = || rest.get(1).copied().filter(|&v| v < 128);
    match status & 0xF0 {
        0x80 => Some(MidiMsg::NoteOff { channel, key: d1, velocity: data2()? as f32 / 127.0 }),
        0x90 => match data2()? {
            0 => Some(MidiMsg::NoteOff { channel, key: d1, velocity: 0.0 }),
            vel => Some(MidiMsg::NoteOn { channel, key: d1, velocity: vel as f32 / 127.0 }),
        },
        0xA0 => Some(MidiMsg::PolyPressure { channel, key: d1, value: data2()? }),
        0xB0 => Some(MidiMsg::ControlChange { channel, controller: d1, value: data2()? }),
        0xD0 => Some(MidiMsg::ChannelPressure { channel, value: d1 }),
        0xE0 => Some(MidiMsg::PitchBend { channel, value: (((data2()? as i16) << 7) | d1 as i16) - 8192 }),
        _ => None,
    }
}

/// Message enregistré avec sa position absolue (échantillons de la ligne de temps).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordedMidi {
    pub pos: u64,
    pub msg: MidiMsg,
}

/// État d'une prise MIDI, partagé entre le thread audio et le contrôle.
#[derive(Debug, Default)]
pub struct MidiRecordStatus {
    pub started: AtomicBool,
    /// Position de la ligne de temps au début du premier bloc traité pendant la prise.
    pub start_pos: AtomicU64,
    /// Fin du dernier bloc traité (devient la fin de la prise à la clôture).
    pub last_pos: AtomicU64,
    pub stop_pos: AtomicU64,
    /// Messages perdus parce que la file était pleine.
    pub overruns: AtomicU32,
    pub closed: AtomicBool,
}

pub(crate) struct MidiTap {
    producer: Producer<RecordedMidi>,
    status: Arc<MidiRecordStatus>,
    /// La prise ne commence qu'à cette position et seulement pendant la lecture ; `None` : dès son installation.
    start_at: Option<u64>,
    /// La prise s'arrête à cette position (exclue) et se déclare terminée toute seule (punch-out) ; `None` : sur ordre du contrôle.
    end_at: Option<u64>,
    /// Le bloc en cours est dans la prise (voir `begin`).
    active: bool,
}

impl MidiTap {
    /// Début d'un bloc `[pos, pos + n[` traité pendant la prise.
    pub(crate) fn begin(&mut self, pos: u64, n: usize, playing: bool) {
        if self.status.closed.load(Ordering::Relaxed) {
            self.active = false;
            return;
        }
        let mut end = pos + n as u64;
        self.active = match self.start_at {
            None => true,
            Some(at) => playing && end > at,
        };
        if !self.active {
            return;
        }
        if let Some(stop) = self.end_at {
            if pos >= stop {
                // La fin tombe avant ce bloc : la prise est finie (rien n'a pu être joué après).
                self.active = false;
                self.status.last_pos.store(stop.max(self.start_at.unwrap_or(0)), Ordering::Relaxed);
                self.close();
                return;
            }
            if end >= stop {
                end = stop;
                self.status.last_pos.store(end, Ordering::Relaxed);
                if !self.status.started.load(Ordering::Relaxed) {
                    self.status.start_pos.store(self.start_at.map_or(pos, |at| at.max(pos)), Ordering::Relaxed);
                    self.status.started.store(true, Ordering::Release);
                }
                self.close();
                return;
            }
        }
        if !self.status.started.load(Ordering::Relaxed) {
            self.status.start_pos.store(self.start_at.map_or(pos, |at| at.max(pos)), Ordering::Relaxed);
            self.status.started.store(true, Ordering::Release);
        }
        self.status.last_pos.store(end, Ordering::Relaxed);
    }

    pub(crate) fn push(&mut self, pos: u64, msg: MidiMsg) {
        if !self.active || self.start_at.is_some_and(|at| pos < at) || self.end_at.is_some_and(|stop| pos >= stop) {
            return;
        }
        if self.producer.push(RecordedMidi { pos, msg }).is_err() {
            self.status.overruns.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn close(&self) {
        self.status.stop_pos.store(self.status.last_pos.load(Ordering::Relaxed), Ordering::Relaxed);
        self.status.closed.store(true, Ordering::Release);
    }
}

/// Côté contrôle d'une prise MIDI.
pub struct MidiRecordReceiver {
    pub consumer: Consumer<RecordedMidi>,
    pub status: Arc<MidiRecordStatus>,
}

impl MidiRecordReceiver {
    /// Tout ce qui est arrivé depuis le dernier appel.
    pub fn take_events(&mut self) -> Vec<RecordedMidi> {
        let mut v = Vec::with_capacity(self.consumer.slots());
        while let Ok(e) = self.consumer.pop() {
            v.push(e);
        }
        v
    }

    /// Le moteur a retiré la prise : tout ce qu'elle contient peut être lu et rien ne viendra plus.
    pub fn is_closed(&self) -> bool {
        self.status.closed.load(Ordering::Acquire)
    }

    pub fn start_pos(&self) -> u64 {
        self.status.start_pos.load(Ordering::Acquire)
    }

    /// Fin de la prise (valable une fois `is_closed()`).
    pub fn stop_pos(&self) -> u64 {
        self.status.stop_pos.load(Ordering::Acquire)
    }
}

pub(crate) fn new_midi_tap(capacity: usize, start_at: Option<u64>, end_at: Option<u64>) -> (MidiTap, MidiRecordReceiver) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity.max(16));
    let status = Arc::new(MidiRecordStatus::default());
    (MidiTap { producer, status: Arc::clone(&status), start_at, end_at, active: true }, MidiRecordReceiver { consumer, status })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notes_and_ignores_the_rest() {
        assert_eq!(parse_midi(&[0x93, 60, 127]), Some(MidiMsg::NoteOn { channel: 3, key: 60, velocity: 1.0 }));
        assert_eq!(parse_midi(&[0x90, 61, 64]), Some(MidiMsg::NoteOn { channel: 0, key: 61, velocity: 64.0 / 127.0 }));
        assert_eq!(parse_midi(&[0x90, 61, 0]), Some(MidiMsg::NoteOff { channel: 0, key: 61, velocity: 0.0 }));
        assert_eq!(parse_midi(&[0x8F, 62, 40]), Some(MidiMsg::NoteOff { channel: 15, key: 62, velocity: 40.0 / 127.0 }));
        assert_eq!(parse_midi(&[0xB2, 64, 127]), Some(MidiMsg::ControlChange { channel: 2, controller: 64, value: 127 }));
        assert_eq!(parse_midi(&[0xB0, 1, 0]), Some(MidiMsg::ControlChange { channel: 0, controller: 1, value: 0 }));
        assert_eq!(parse_midi(&[0xE0, 0, 64]), Some(MidiMsg::PitchBend { channel: 0, value: 0 })); // repos
        assert_eq!(parse_midi(&[0xE1, 0, 0]), Some(MidiMsg::PitchBend { channel: 1, value: -8192 }));
        assert_eq!(parse_midi(&[0xE0, 127, 127]), Some(MidiMsg::PitchBend { channel: 0, value: 8191 }));
        assert_eq!(parse_midi(&[0xE0, 1, 64]), Some(MidiMsg::PitchBend { channel: 0, value: 1 }));
        assert_eq!(parse_midi(&[0xD3, 90]), Some(MidiMsg::ChannelPressure { channel: 3, value: 90 })); // 2 octets
        assert_eq!(parse_midi(&[0xD3, 90, 0]), Some(MidiMsg::ChannelPressure { channel: 3, value: 90 })); // WinMM complète à 3
        assert_eq!(parse_midi(&[0xA0, 60, 33]), Some(MidiMsg::PolyPressure { channel: 0, key: 60, value: 33 }));
        assert_eq!(parse_midi(&[0xC0, 5]), None); // changement de programme : non géré
        assert_eq!(parse_midi(&[0xF8]), None); // horloge
        assert_eq!(parse_midi(&[0xB0, 7]), None); // tronqué
        assert_eq!(parse_midi(&[0x90, 60]), None); // tronqué
        assert_eq!(parse_midi(&[]), None);
        assert_eq!(parse_midi(&[0x90, 200, 10]), None); // octet de données invalide
        assert_eq!(parse_midi(&[0x90, 60, 128]), None);
    }

    #[test]
    fn tap_tracks_start_end_and_overruns() {
        let (mut tap, mut rx) = new_midi_tap(16, None, None);
        tap.begin(1_000, 64, true);
        tap.push(1_010, MidiMsg::NoteOn { channel: 0, key: 60, velocity: 1.0 });
        tap.begin(1_064, 64, true);
        tap.close();
        assert!(rx.is_closed());
        assert_eq!((rx.start_pos(), rx.stop_pos()), (1_000, 1_128));
        assert_eq!(rx.take_events(), vec![RecordedMidi { pos: 1_010, msg: MidiMsg::NoteOn { channel: 0, key: 60, velocity: 1.0 } }]);
        for k in 0..20 {
            tap.push(k, MidiMsg::NoteOff { channel: 0, key: 60, velocity: 0.0 });
        }
        assert_eq!(rx.status.overruns.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn a_tap_with_an_end_stops_itself_and_ignores_later_messages() {
        let (mut tap, mut rx) = new_midi_tap(16, Some(1_000), Some(1_100));
        tap.begin(960, 64, true); // 960..1024 : commence à 1000
        tap.push(1_010, MidiMsg::NoteOn { channel: 0, key: 60, velocity: 1.0 });
        tap.push(990, MidiMsg::NoteOn { channel: 0, key: 61, velocity: 1.0 }); // avant le début : ignoré
        tap.begin(1_024, 64, true); // 1024..1088 : dans la prise
        assert!(!rx.is_closed());
        tap.begin(1_088, 64, true); // 1088..1152 : la fin (1100) tombe dans ce bloc
        tap.push(1_099, MidiMsg::NoteOff { channel: 0, key: 60, velocity: 0.0 });
        tap.push(1_100, MidiMsg::NoteOn { channel: 0, key: 62, velocity: 1.0 }); // à la fin : exclu
        tap.push(1_120, MidiMsg::NoteOn { channel: 0, key: 63, velocity: 1.0 }); // après : ignoré
        assert!(rx.is_closed(), "terminée dès que la fin est atteinte");
        assert_eq!((rx.start_pos(), rx.stop_pos()), (1_000, 1_100));
        tap.begin(1_152, 64, true);
        tap.push(1_160, MidiMsg::NoteOn { channel: 0, key: 64, velocity: 1.0 });
        let keys: Vec<u64> = rx.take_events().iter().map(|e| e.pos).collect();
        assert_eq!(keys, vec![1_010, 1_099]);
    }
}
