//! Données musicales du projet en **temps musical** : clips de notes et courbes d'automation positionnés en fractions
//! de ronde. Ce sont elles qui font foi ; les clips et courbes en échantillons du moteur en sont dérivés avec la carte
//! des tempos (donc suivent un changement de tempo).

use daw_core_types::time::{Frac, TempoMap};
use daw_engine::{AutoCurve, AutoLane, AutoMode, AutoPoint, AutoTarget, CtlEvent, MidiClip, MidiMsg, Note};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MidiClipId(pub u64);

/// Note d'un clip : `start` et `len` relatifs au début du clip, en fractions de ronde.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MNote {
    pub start: Frac,
    pub len: Frac,
    pub key: u8,
    pub velocity: f32,
    pub channel: u8,
    /// Voix d'affichage de la partition (1 à 4, 1 par défaut) : un choix d'écriture que le moteur audio ignore, comme le MIDI n'a pas de voix.
    pub voice: u8,
}

/// Message qui n'est pas une note : contrôleur (modulation, volume, pédale de maintien...), molette de hauteur, pressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MKind {
    ControlChange { controller: u8, value: u8 },
    /// De -8192 à 8191, 0 au repos.
    PitchBend { value: i16 },
    ChannelPressure { value: u8 },
    PolyPressure { key: u8, value: u8 },
}

/// Événement de contrôle d'un clip, relatif à son début.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MEvent {
    pub start: Frac,
    pub channel: u8,
    pub kind: MKind,
}

impl MEvent {
    pub(crate) fn msg(&self) -> MidiMsg {
        let channel = self.channel;
        match self.kind {
            MKind::ControlChange { controller, value } => MidiMsg::ControlChange { channel, controller, value },
            MKind::PitchBend { value } => MidiMsg::PitchBend { channel, value },
            MKind::ChannelPressure { value } => MidiMsg::ChannelPressure { channel, value },
            MKind::PolyPressure { key, value } => MidiMsg::PolyPressure { channel, key, value },
        }
    }

    /// `None` pour une note (qui n'est pas un événement de contrôle).
    pub fn from_msg(start: Frac, msg: MidiMsg) -> Option<MEvent> {
        let (channel, kind) = match msg {
            MidiMsg::NoteOn { .. } | MidiMsg::NoteOff { .. } => return None,
            MidiMsg::ControlChange { channel, controller, value } => (channel, MKind::ControlChange { controller, value }),
            MidiMsg::PitchBend { channel, value } => (channel, MKind::PitchBend { value }),
            MidiMsg::ChannelPressure { channel, value } => (channel, MKind::ChannelPressure { value }),
            MidiMsg::PolyPressure { channel, key, value } => (channel, MKind::PolyPressure { key, value }),
        };
        Some(MEvent { start, channel, kind })
    }

    pub(crate) fn valid(&self) -> bool {
        self.msg().valid()
    }
}

/// Clip de notes. Notes et événements sont gardés triés par les opérations de la session (les événements simultanés
/// gardent leur ordre d'arrivée).
#[derive(Debug, Clone, PartialEq)]
pub struct MClip {
    pub start: Frac,
    pub len: Frac,
    /// Début de la fenêtre visible dans la matière du clip : notes et événements sont positionnés depuis le début de la
    /// matière (comme le décalage d'un clip audio), le clip n'en montre que `[offset, offset + len[`. Rogner ou découper ne
    /// détruit donc rien.
    pub offset: Frac,
    pub notes: Vec<MNote>,
    pub events: Vec<MEvent>,
    /// Fondus de **vélocité** : les notes qui démarrent dans le fondu d'entrée (ou de sortie) voient leur vélocité réduite
    /// linéairement (jamais en dessous de 1/127 : une note ne disparaît pas, elle devient très douce).
    pub fade_in: Frac,
    pub fade_out: Frac,
    /// Prise inactive : gardée mais jamais jouée. `take_group` : groupe de prises (0 = aucun), voir `takes`.
    pub muted: bool,
    pub take_group: u32,
}

impl MClip {
    pub fn new(start: Frac, len: Frac) -> MClip {
        MClip { start, len, offset: Frac::ZERO, notes: Vec::new(), events: Vec::new(), fade_in: Frac::ZERO, fade_out: Frac::ZERO, muted: false, take_group: 0 }
    }

    pub(crate) fn valid(&self) -> bool {
        self.len != Frac::ZERO && self.fade_in <= self.len && self.fade_out <= self.len && self.notes.iter().all(|n| n.valid()) && self.events.iter().all(|e| e.valid())
    }

    pub(crate) fn sort(&mut self) {
        self.notes.sort_by(|a, b| a.start.cmp(&b.start).then(a.key.cmp(&b.key)).then(a.channel.cmp(&b.channel)));
        self.events.sort_by(|a, b| a.start.cmp(&b.start)); // tri stable
    }
}

impl MNote {
    pub(crate) fn valid(&self) -> bool {
        self.key < 128 && self.channel < 16 && (1..=4).contains(&self.voice) && self.len != Frac::ZERO && self.velocity > 0.0 && self.velocity <= 1.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MPoint {
    pub pos: Frac,
    pub value: f64,
    pub curve: AutoCurve,
}

/// Courbe d'automation en temps musical (points triés, positions distinctes).
#[derive(Debug, Clone, PartialEq)]
pub struct MLane {
    pub target: AutoTarget,
    pub points: Vec<MPoint>,
    /// Lecture, trim (le fader s'ajoute à la courbe) ou courbe ignorée.
    pub mode: AutoMode,
}

impl MLane {
    /// Valeur de la courbe à la position `pos` (avant le premier point : sa valeur ; après le dernier : la sienne).
    pub fn value_at(&self, pos: Frac) -> f64 {
        let i = self.points.partition_point(|p| p.pos <= pos);
        if i == 0 {
            return self.points[0].value;
        }
        if i == self.points.len() {
            return self.points[i - 1].value;
        }
        let (a, b) = (&self.points[i - 1], &self.points[i]);
        match a.curve {
            AutoCurve::Step => a.value,
            AutoCurve::Linear => {
                let (span, at) = (b.pos.checked_sub(a.pos).map_or(1.0, |f| f.as_f64()), pos.checked_sub(a.pos).map_or(0.0, |f| f.as_f64()));
                a.value + (b.value - a.value) * (at / span)
            }
        }
    }

    pub(crate) fn valid(&self) -> bool {
        !self.points.is_empty() && self.points.windows(2).all(|w| w[0].pos < w[1].pos) && self.points.iter().all(|p| p.value.is_finite())
    }
}

/// Facteur de vélocité des fondus d'un clip pour une note qui démarre à `rel` (depuis le début de la fenêtre visible).
fn fade_gain(c: &MClip, rel: Frac) -> f64 {
    let mut g = 1.0;
    if c.fade_in != Frac::ZERO && rel < c.fade_in {
        g *= rel.as_f64() / c.fade_in.as_f64();
    }
    if c.fade_out != Frac::ZERO {
        if let Some(remaining) = c.len.checked_sub(rel) {
            if remaining < c.fade_out {
                g *= remaining.as_f64() / c.fade_out.as_f64();
            }
        }
    }
    g
}

/// Clip de notes du moteur (en échantillons) pour un clip musical : seules les notes et événements de la fenêtre visible
/// `[offset, offset + len[` y figurent, avec les fondus de vélocité appliqués.
pub(crate) fn clip_to_engine(c: &MClip, tm: &TempoMap, sr: u32) -> Option<MidiClip> {
    let s0 = tm.to_samples(c.start, sr)?;
    let len = tm.to_samples(c.start.checked_add(c.len)?, sr)?.saturating_sub(s0).max(1);
    let window_end = c.offset.checked_add(c.len)?;
    let mut notes = Vec::with_capacity(c.notes.len());
    for n in &c.notes {
        if n.start < c.offset || n.start >= window_end {
            continue;
        }
        let rel = n.start.checked_sub(c.offset)?;
        let on = c.start.checked_add(rel)?;
        let a = tm.to_samples(on, sr)?;
        let b = tm.to_samples(on.checked_add(n.len)?, sr)?;
        let velocity = ((n.velocity as f64 * fade_gain(c, rel)) as f32).max(1.0 / 127.0).min(n.velocity.max(1.0 / 127.0));
        notes.push(Note { start: (a - s0).min(len - 1), len: b.saturating_sub(a).max(1), key: n.key, velocity, channel: n.channel });
    }
    let mut events = Vec::with_capacity(c.events.len());
    for e in &c.events {
        if e.start < c.offset || e.start >= window_end {
            continue;
        }
        let at = tm.to_samples(c.start.checked_add(e.start.checked_sub(c.offset)?)?, sr)?;
        events.push(CtlEvent { start: (at - s0).min(len - 1), msg: e.msg() });
    }
    let mut clip = MidiClip::new(s0, len, Arc::new(notes));
    clip.events = Arc::new(events);
    clip.muted = c.muted;
    Some(clip)
}

/// Courbe du moteur : deux points qui tombent sur le même échantillon ne gardent que le dernier.
pub(crate) fn lane_to_engine(l: &MLane, tm: &TempoMap, sr: u32) -> Option<AutoLane> {
    let mut points: Vec<AutoPoint> = Vec::with_capacity(l.points.len());
    for p in &l.points {
        let time = tm.to_samples(p.pos, sr)?;
        let pt = AutoPoint { time, value: p.value, curve: p.curve };
        match points.last_mut() {
            Some(last) if last.time == time => *last = pt,
            _ => points.push(pt),
        }
    }
    Some(AutoLane { target: l.target, points, mode: l.mode })
}

/// Conversion inverse, exacte, d'un clip du moteur (pour les projets construits en échantillons).
pub(crate) fn clip_from_engine(c: &MidiClip, tm: &TempoMap, sr: u32) -> Option<MClip> {
    let start = tm.to_frac(c.start, sr)?;
    let len = tm.to_frac(c.start.checked_add(c.len)?, sr)?.checked_sub(start)?;
    let mut notes = Vec::with_capacity(c.notes.len());
    for n in c.notes.iter() {
        let a = tm.to_frac(c.start.checked_add(n.start)?, sr)?;
        let b = tm.to_frac(c.start.checked_add(n.start)?.checked_add(n.len)?, sr)?;
        notes.push(MNote { start: a.checked_sub(start)?, len: b.checked_sub(a)?, key: n.key, velocity: n.velocity, channel: n.channel, voice: 1 });
    }
    let mut events = Vec::with_capacity(c.events.len());
    for e in c.events.iter() {
        let at = tm.to_frac(c.start.checked_add(e.start)?, sr)?;
        events.push(MEvent::from_msg(at.checked_sub(start)?, e.msg)?);
    }
    Some(MClip { start, len, offset: Frac::ZERO, notes, events, fade_in: Frac::ZERO, fade_out: Frac::ZERO, muted: false, take_group: 0 })
}

pub(crate) fn lane_from_engine(l: &AutoLane, tm: &TempoMap, sr: u32) -> Option<MLane> {
    let points = l.points.iter().map(|p| Some(MPoint { pos: tm.to_frac(p.time, sr)?, value: p.value, curve: p.curve })).collect::<Option<Vec<_>>>()?;
    Some(MLane { target: l.target, points, mode: l.mode })
}

/// Construit un clip de notes à partir de messages datés en ticks absolus (`ppq` ticks par noire) : appariement des débuts
/// et fins de notes, notes tenues terminées à `t_end`, même touche rejouée = la précédente est terminée, fin sans début
/// ignorée, durée nulle = 1 tick ; les autres messages deviennent des événements de contrôle. Le clip commence à `t0`.
/// `Ok(None)` si rien à mettre dedans ; `None` en cas de dépassement d'entier.
pub(crate) fn build_clip(events: &[(u64, MidiMsg)], t0: u64, t_end: u64, ppq: u32) -> Option<Option<MClip>> {
    let mut open: [[Option<(u64, f32)>; 128]; 16] = [[None; 128]; 16];
    let mut notes: Vec<(u64, u64, u8, f32, u8)> = Vec::new();
    let mut ctl: Vec<(u64, MidiMsg)> = Vec::new();
    let close = |slot: &mut Option<(u64, f32)>, at: u64, ch: usize, key: usize, notes: &mut Vec<(u64, u64, u8, f32, u8)>| {
        if let Some((start, vel)) = slot.take() {
            notes.push((start, at.max(start + 1), key as u8, vel, ch as u8));
        }
    };
    for &(tick, msg) in events {
        let at = tick.clamp(t0, t_end.max(t0 + 1));
        match msg {
            MidiMsg::NoteOn { channel, key, velocity } => {
                let (c, k) = ((channel & 15) as usize, (key & 127) as usize);
                close(&mut open[c][k], at, c, k, &mut notes);
                open[c][k] = Some((at, velocity.clamp(f32::MIN_POSITIVE, 1.0)));
            }
            MidiMsg::NoteOff { channel, key, .. } => {
                let (c, k) = ((channel & 15) as usize, (key & 127) as usize);
                close(&mut open[c][k], at, c, k, &mut notes);
            }
            other => ctl.push((at, other)),
        }
    }
    for (c, row) in open.iter_mut().enumerate() {
        for (k, slot) in row.iter_mut().enumerate() {
            close(slot, t_end, c, k, &mut notes);
        }
    }
    if notes.is_empty() && ctl.is_empty() {
        return Some(None);
    }
    let frac = |ticks: u64| TempoMap::from_ticks(ticks, ppq);
    let last_ctl = ctl.iter().map(|c| c.0 + 1).max().unwrap_or(0);
    let clip_end = notes.iter().map(|n| n.1).max().unwrap_or(t0 + 1).max(t_end).max(t0 + 1).max(last_ctl);
    let mut clip = MClip::new(frac(t0)?, frac(clip_end - t0)?);
    for (start, end, key, velocity, channel) in notes {
        clip.notes.push(MNote { start: frac(start - t0)?, len: frac(end - start)?, key, velocity, channel, voice: 1 });
    }
    for (at, msg) in ctl {
        clip.events.push(MEvent::from_msg(frac(at - t0)?, msg)?);
    }
    clip.sort();
    Some(Some(clip))
}
