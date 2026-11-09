//! Messages MIDI (notes, contrôleurs, molette de hauteur, pression) : clips sur la ligne de temps et événements datés au bloc.
//!
//! Les positions sont en **échantillons** pour l'instant. Quand la carte des tempos existera (temps musical en
//! fractions exactes de ronde, comme Scorsaire), c'est elle qui convertira les notes en échantillons avant de
//! les confier au moteur.

use std::sync::Arc;

/// Message MIDI, sans allocation (copiable). Les valeurs sont celles du protocole (entiers), donc sans perte :
/// `velocity` des notes de 0 à 1 (vélocité MIDI / 127), contrôleurs et pressions de 0 à 127, molette de -8192 à 8191.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiMsg {
    NoteOn { channel: u8, key: u8, velocity: f32 },
    NoteOff { channel: u8, key: u8, velocity: f32 },
    /// Contrôleur (molette de modulation 1, volume 7, pédale de maintien 64, ...).
    ControlChange { channel: u8, controller: u8, value: u8 },
    /// Molette de hauteur : 0 = au repos, de -8192 à 8191.
    PitchBend { channel: u8, value: i16 },
    /// Pression (aftertouch) commune à tout le canal.
    ChannelPressure { channel: u8, value: u8 },
    /// Pression (aftertouch) d'une seule touche.
    PolyPressure { channel: u8, key: u8, value: u8 },
}

impl MidiMsg {
    pub fn channel(&self) -> u8 {
        match *self {
            MidiMsg::NoteOn { channel, .. }
            | MidiMsg::NoteOff { channel, .. }
            | MidiMsg::ControlChange { channel, .. }
            | MidiMsg::PitchBend { channel, .. }
            | MidiMsg::ChannelPressure { channel, .. }
            | MidiMsg::PolyPressure { channel, .. } => channel,
        }
    }
    /// Touche concernée (0 pour les messages qui n'en ont pas).
    pub fn key(&self) -> u8 {
        match *self {
            MidiMsg::NoteOn { key, .. } | MidiMsg::NoteOff { key, .. } | MidiMsg::PolyPressure { key, .. } => key,
            _ => 0,
        }
    }
    pub fn is_note(&self) -> bool {
        matches!(self, MidiMsg::NoteOn { .. } | MidiMsg::NoteOff { .. })
    }
    /// Valeurs dans les plages du protocole.
    pub fn valid(&self) -> bool {
        match *self {
            MidiMsg::NoteOn { channel, key, velocity } | MidiMsg::NoteOff { channel, key, velocity } => channel < 16 && key < 128 && (0.0..=1.0).contains(&velocity),
            MidiMsg::ControlChange { channel, controller, value } => channel < 16 && controller < 128 && value < 128,
            MidiMsg::PitchBend { channel, value } => channel < 16 && (-8192..=8191).contains(&value),
            MidiMsg::ChannelPressure { channel, value } => channel < 16 && value < 128,
            MidiMsg::PolyPressure { channel, key, value } => channel < 16 && key < 128 && value < 128,
        }
    }
}

/// Événement daté en échantillons depuis le début du bloc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiEvent {
    pub time: u32,
    pub msg: MidiMsg,
}

/// Une note dans un clip : `start` et `len` en échantillons, relatifs au début du clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub start: u64,
    pub len: u64,
    pub key: u8,
    pub velocity: f32,
    pub channel: u8,
}

/// Message qui n'est pas une note (contrôleur, molette, pression) à `start` échantillons du début du clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CtlEvent {
    pub start: u64,
    pub msg: MidiMsg,
}

/// Clip de notes posé sur une piste : `len` échantillons à partir de `start`. Une note qui dépasse la fin du clip
/// est coupée à la fin du clip. `events` : contrôleurs, molette et pressions du clip.
#[derive(Debug, Clone)]
pub struct MidiClip {
    pub start: u64,
    pub len: u64,
    pub notes: Arc<Vec<Note>>,
    pub events: Arc<Vec<CtlEvent>>,
    /// Clip muet (prise inactive) : gardé dans le graphe, jamais joué.
    pub muted: bool,
}

impl MidiClip {
    /// Clip de notes seules.
    pub fn new(start: u64, len: u64, notes: Arc<Vec<Note>>) -> MidiClip {
        MidiClip { start, len, notes, events: Arc::new(Vec::new()), muted: false }
    }

    pub(crate) fn valid(&self) -> bool {
        self.start.checked_add(self.len).is_some()
            && self.notes.iter().all(|n| {
                n.key < 128 && n.channel < 16 && n.len > 0 && n.velocity > 0.0 && n.velocity <= 1.0 && n.start.checked_add(n.len).is_some() && n.start < self.len
            })
            && self.events.iter().all(|e| !e.msg.is_note() && e.msg.valid() && e.start < self.len)
    }
}

/// Événement absolu précalculé à la compilation, trié : temps, puis fins avant débuts (re-déclenchement net), puis touche.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TimedMsg {
    pub time: u64,
    pub msg: MidiMsg,
}

/// Contrôleurs, molette et pressions des clips, en positions absolues (hors clip : déjà refusés à la compilation).
fn c_events(clips: &[MidiClip]) -> Vec<TimedMsg> {
    clips.iter().flat_map(|c| c.events.iter().map(move |e| TimedMsg { time: c.start + e.start, msg: e.msg })).collect()
}

pub(crate) fn expand(clips: &[MidiClip]) -> Vec<TimedMsg> {
    let mut v = Vec::new();
    for c in clips.iter().filter(|c| !c.muted) {
        let end = c.start + c.len;
        for n in c.notes.iter() {
            let on = c.start + n.start;
            let off = (on + n.len).min(end);
            v.push(TimedMsg { time: on, msg: MidiMsg::NoteOn { channel: n.channel, key: n.key, velocity: n.velocity } });
            v.push(TimedMsg { time: off, msg: MidiMsg::NoteOff { channel: n.channel, key: n.key, velocity: 0.0 } });
        }
    }
    for e in c_events(clips) {
        v.push(e);
    }
    // À instant égal : fins de notes, puis contrôleurs (pédale, molette : déjà en place quand la note démarre), puis débuts.
    let class = |m: &MidiMsg| match m {
        MidiMsg::NoteOff { .. } => 0u8,
        MidiMsg::NoteOn { .. } => 2,
        _ => 1,
    };
    v.sort_by_key(|m: &TimedMsg| (m.time, class(&m.msg), m.msg.channel(), m.msg.key()));
    v
}

/// Suivi des notes en cours (16 canaux × 128 touches) et des états qui feraient durer le son (pédale de maintien
/// enfoncée, molette de hauteur déviée), pour pouvoir tout relâcher.
#[derive(Clone)]
pub(crate) struct ActiveNotes {
    notes: [u128; 16],
    /// Un bit par canal : pédale de maintien (contrôleur 64) enfoncée.
    sustain: u16,
    /// Un bit par canal : molette de hauteur hors de sa position de repos.
    bend: u16,
}

impl ActiveNotes {
    pub fn new() -> Self {
        ActiveNotes { notes: [0; 16], sustain: 0, bend: 0 }
    }
    pub fn apply(&mut self, msg: MidiMsg) {
        let (c, k) = ((msg.channel() & 15) as usize, msg.key() & 127);
        let bit = 1u16 << c;
        match msg {
            MidiMsg::NoteOn { .. } => self.notes[c] |= 1u128 << k,
            MidiMsg::NoteOff { .. } => self.notes[c] &= !(1u128 << k),
            MidiMsg::ControlChange { controller: 64, value, .. } => {
                if value >= 64 {
                    self.sustain |= bit;
                } else {
                    self.sustain &= !bit;
                }
            }
            MidiMsg::PitchBend { value, .. } => {
                if value != 0 {
                    self.bend |= bit;
                } else {
                    self.bend &= !bit;
                }
            }
            _ => {}
        }
    }
    /// Appelle `f(canal, touche)` pour chaque note active.
    pub fn for_each(&self, mut f: impl FnMut(u8, u8)) {
        for (c, bits) in self.notes.iter().enumerate() {
            let mut b = *bits;
            while b != 0 {
                let k = b.trailing_zeros();
                f(c as u8, k as u8);
                b &= b - 1;
            }
        }
    }
    /// Appelle `f(canal)` pour chaque canal dont la pédale de maintien est enfoncée.
    pub fn for_each_sustain(&self, mut f: impl FnMut(u8)) {
        (0..16u8).filter(|c| self.sustain & (1 << c) != 0).for_each(&mut f);
    }
    /// Appelle `f(canal)` pour chaque canal dont la molette de hauteur est déviée.
    pub fn for_each_bend(&self, mut f: impl FnMut(u8)) {
        (0..16u8).filter(|c| self.bend & (1 << c) != 0).for_each(&mut f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: u64, len: u64, key: u8) -> Note {
        Note { start, len, key, velocity: 0.8, channel: 0 }
    }

    #[test]
    fn expansion_is_sorted_with_offs_before_ons() {
        let clip = MidiClip::new(100, 1000, Arc::new(vec![note(0, 50, 60), note(50, 50, 60), note(10, 20, 64)]));
        let ev = expand(&[clip]);
        let times: Vec<u64> = ev.iter().map(|e| e.time).collect();
        assert_eq!(times, vec![100, 110, 130, 150, 150, 200]);
        // à 150 : la fin de la première note (60) avant le début de la seconde (60)
        assert!(matches!(ev[3].msg, MidiMsg::NoteOff { key: 60, .. }));
        assert!(matches!(ev[4].msg, MidiMsg::NoteOn { key: 60, .. }));
    }

    #[test]
    fn notes_are_cut_at_the_end_of_the_clip() {
        let clip = MidiClip::new(0, 100, Arc::new(vec![note(90, 50, 60)]));
        let ev = expand(&[clip]);
        assert_eq!(ev[1].time, 100);
    }

    #[test]
    fn validation() {
        let ok = MidiClip::new(0, 100, Arc::new(vec![note(0, 10, 60)]));
        assert!(ok.valid());
        for bad in [note(0, 0, 60), note(0, 10, 128), note(100, 10, 60), Note { velocity: 0.0, ..note(0, 10, 60) }, Note { channel: 16, ..note(0, 10, 60) }] {
            assert!(!MidiClip::new(0, 100, Arc::new(vec![bad])).valid(), "{bad:?}");
        }
    }

    #[test]
    fn active_notes_tracking() {
        let mut a = ActiveNotes::new();
        a.apply(MidiMsg::NoteOn { channel: 0, key: 60, velocity: 1.0 });
        a.apply(MidiMsg::NoteOn { channel: 3, key: 127, velocity: 1.0 });
        a.apply(MidiMsg::NoteOn { channel: 0, key: 61, velocity: 1.0 });
        a.apply(MidiMsg::NoteOff { channel: 0, key: 61, velocity: 0.0 });
        let mut seen = Vec::new();
        a.for_each(|c, k| seen.push((c, k)));
        assert_eq!(seen, vec![(0, 60), (3, 127)]);
        a = ActiveNotes::new();
        a.for_each(|_, _| panic!("vide"));
    }
}
