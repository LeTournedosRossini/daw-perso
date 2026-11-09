//! Modes d'écriture de l'automation (Toucher, Latch, Écriture) : l'hôte enregistre les gestes de l'utilisateur sur un paramètre
//! pendant la lecture, puis les écrit dans la courbe d'un seul coup (une action annulable).
//!
//! - **Toucher** : la courbe est lue ; tant que l'utilisateur tient le réglage, son geste est enregistré et **remplace** la courbe ;
//!   au relâchement la courbe reprend (elle rejoint en ligne droite sa valeur d'origine).
//! - **Latch** : comme Toucher, mais à partir du premier geste on **continue d'écrire la dernière valeur** jusqu'à l'arrêt.
//! - **Écriture** : on écrit en continu, dès le début de la lecture, la valeur du réglage (même sans y toucher), jusqu'à l'arrêt.
//!
//! Pendant qu'un paramètre s'écrit, sa courbe est mise en mode « désactivé » **sans historique** (le moteur obéit alors au réglage
//! manuel que l'utilisateur déplace) ; à la fin elle reprend son mode de lecture avec les nouveaux points.

use daw_core_types::NodeId;
use daw_engine::{AutoMode, AutoTarget};

use super::Host;
use crate::protocol::{Event, LaneTarget};

/// Les modes d'écriture proposés.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Touch,
    Latch,
    Write,
}

impl WriteMode {
    pub fn from_name(name: &str) -> Option<WriteMode> {
        match name {
            "touch" => Some(WriteMode::Touch),
            "latch" => Some(WriteMode::Latch),
            "write" => Some(WriteMode::Write),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            WriteMode::Touch => "touch",
            WriteMode::Latch => "latch",
            WriteMode::Write => "write",
        }
    }
}

/// Ce qui s'enregistre en ce moment pour un paramètre armé.
pub(super) struct Segment {
    pub from: u64,
    pub last: u64,
    pub gestures: Vec<(u64, f64)>,
    /// Nombre de gestes déjà envoyés à la page (aperçu en direct).
    pub sent: usize,
    /// Mode de la courbe avant l'écriture (rendu à la fin) ; `None` : la courbe n'existait pas.
    pub lane_mode: Option<AutoMode>,
}

/// Un paramètre armé pour l'écriture.
pub(super) struct WriteArm {
    pub node: u32,
    pub target: AutoTarget,
    pub mode: WriteMode,
    /// L'utilisateur tient le réglage (message `gesture`).
    pub touching: bool,
    pub seg: Option<Segment>,
}

impl Host {
    fn arm_index(&self, node: u32, target: AutoTarget) -> Option<usize> {
        self.write_arms.iter().position(|a| a.node == node && a.target == target)
    }

    /// Valeur actuelle du réglage manuel d'un paramètre (celle qu'on écrit quand l'utilisateur ne bouge rien).
    fn manual_value(&self, node: u32, target: AutoTarget) -> Option<f64> {
        match target {
            AutoTarget::Gain => self.nodes.get(node as usize).map(|n| n.gain as f64),
            AutoTarget::Pan => self.nodes.get(node as usize).map(|n| n.pan as f64),
            AutoTarget::SendGain { to } => self.session.graph().nodes.get(node as usize)?.sends.iter().find(|s| s.to == to).map(|s| s.gain as f64),
            AutoTarget::InsertParam { insert, param } => self.plugin_values.get(&(insert, param)).copied().or_else(|| self.plugin_handle(insert).and_then(|ph| ph.param_value(param))),
        }
    }

    /// Armement (ou désarmement avec `mode` `none`) d'un paramètre pour l'écriture. Armer met la courbe existante en mode lecture
    /// (une courbe qu'on écrit n'est pas un trim).
    pub(super) fn set_write_mode(&mut self, node: u32, target: LaneTarget, mode: &str) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "set_write_mode".into(), message }];
        let Some(nd) = self.session.graph().nodes.get(node as usize) else { return error(format!("nœud inconnu : {node}")) };
        let t = super::lane_target(target);
        let exists = match t {
            AutoTarget::SendGain { to } => nd.sends.iter().any(|s| s.to == to),
            AutoTarget::InsertParam { insert, .. } => nd.inserts.iter().any(|i| i.id == insert),
            _ => true,
        };
        if !exists {
            return error("cette cible n'existe pas sur ce nœud".into());
        }
        let mut events = Vec::new();
        if let Some(i) = self.arm_index(node, t) {
            events.extend(self.end_segment(i, self.handle.position()));
            self.write_arms.remove(i);
        }
        if mode != "none" {
            let Some(m) = WriteMode::from_name(mode) else { return error(format!("mode d'écriture inconnu : {mode}")) };
            self.write_arms.push(WriteArm { node, target: t, mode: m, touching: false, seg: None });
            let n = NodeId(node);
            if self.session.lane(n, t).is_some_and(|l| l.mode != AutoMode::Read) {
                self.session.set_lane_mode_untracked(n, t, AutoMode::Read);
                self.dirty = true;
                if self.handle.load_graph(self.session.graph()).is_err() {
                    events.push(Event::Error { command: "set_write_mode".into(), message: "le moteur a refusé le changement de mode".into() });
                }
            }
            if m == WriteMode::Write && self.playing {
                events.extend(self.begin_segment(self.write_arms.len() - 1));
            }
        }
        events.push(self.project_event());
        events
    }

    /// L'utilisateur prend (`active`) ou lâche un réglage : pour Toucher, le geste dure tant qu'on le tient.
    pub(super) fn gesture(&mut self, node: u32, target: LaneTarget, active: bool) -> Vec<Event> {
        self.gesture_auto(node, super::lane_target(target), active)
    }

    /// Même chose pour une cible déjà traduite (gestes venant de l'interface d'un plugin).
    pub(super) fn gesture_auto(&mut self, node: u32, t: AutoTarget, active: bool) -> Vec<Event> {
        let Some(i) = self.arm_index(node, t) else { return Vec::new() };
        self.write_arms[i].touching = active;
        if !active && self.write_arms[i].mode == WriteMode::Touch {
            return self.end_segment(i, self.handle.position());
        }
        Vec::new()
    }

    /// Un réglage vient d'être modifié par l'utilisateur : s'il est armé et que la lecture tourne, le geste s'enregistre.
    pub(super) fn note_value(&mut self, node: u32, target: AutoTarget, value: f64) -> Vec<Event> {
        let Some(i) = self.arm_index(node, target) else { return Vec::new() };
        if !self.playing || self.recording.is_some() {
            return Vec::new();
        }
        let mut events = Vec::new();
        if self.write_arms[i].seg.is_none() {
            events.extend(self.begin_segment(i));
        }
        let pos = self.handle.position();
        if let Some(seg) = &mut self.write_arms[i].seg {
            seg.gestures.push((pos, value));
            seg.last = pos;
        }
        events
    }

    /// Démarre l'enregistrement d'un paramètre : la courbe est suspendue (le moteur suit le réglage manuel).
    fn begin_segment(&mut self, i: usize) -> Vec<Event> {
        let (node, target) = (self.write_arms[i].node, self.write_arms[i].target);
        let pos = self.handle.position();
        let n = NodeId(node);
        let lane_mode = self.session.lane(n, target).map(|l| l.mode);
        let mut events = Vec::new();
        if lane_mode.is_some() {
            self.session.set_lane_mode_untracked(n, target, AutoMode::Off);
            if self.handle.load_graph(self.session.graph()).is_err() {
                events.push(Event::Error { command: "write_automation".into(), message: "le moteur a refusé la suspension de la courbe".into() });
            }
        }
        let first = self.manual_value(node, target);
        let mut gestures = Vec::new();
        if let Some(v) = first {
            gestures.push((pos, v));
        }
        self.write_arms[i].seg = Some(Segment { from: pos, last: pos, gestures, sent: 0, lane_mode });
        events.push(Event::Writing { active: true });
        events
    }

    /// Termine l'enregistrement d'un paramètre à la position `to` : les gestes sont allégés et écrits dans la courbe (une action
    /// annulable), qui reprend son mode d'avant.
    fn end_segment(&mut self, i: usize, to: u64) -> Vec<Event> {
        let Some(seg) = self.write_arms.get_mut(i).and_then(|a| a.seg.take()) else { return Vec::new() };
        let (node, target) = (self.write_arms[i].node, self.write_arms[i].target);
        let n = NodeId(node);
        // La courbe reprend son mode d'avant l'écriture (jamais « désactivée » : c'est nous qui l'avions suspendue).
        if let Some(mode) = seg.lane_mode {
            self.session.set_lane_mode_untracked(n, target, if mode == AutoMode::Off { AutoMode::Read } else { mode });
        }
        let to = to.max(seg.last).max(seg.from + 1);
        let (lo, hi) = seg.gestures.iter().fold((f64::MAX, f64::MIN), |(lo, hi), g| (lo.min(g.1), hi.max(g.1)));
        let tolerance = if hi >= lo { 0.01 * (hi - lo) + 1e-9 } else { 1e-9 };
        let result = self.session.write_automation(n, target, &seg.gestures, seg.from, to, tolerance);
        let mut events = self.finish_edit("write_automation", result);
        events.push(Event::WriteLive { node, target: super::lane_target_back(target), index: 0, points: Vec::new(), done: true });
        events.push(Event::Writing { active: self.write_arms.iter().any(|a| a.seg.is_some()) });
        events
    }

    /// La lecture démarre : les paramètres armés en mode Écriture commencent à écrire.
    pub(super) fn write_begin(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        for i in 0..self.write_arms.len() {
            if self.write_arms[i].mode == WriteMode::Write && self.write_arms[i].seg.is_none() {
                events.extend(self.begin_segment(i));
            }
        }
        events
    }

    /// La lecture s'arrête (ou la tête saute) : tout ce qui s'enregistre est écrit à la position `pos`.
    pub(super) fn write_end(&mut self, pos: u64) -> Vec<Event> {
        let mut events = Vec::new();
        for i in 0..self.write_arms.len() {
            events.extend(self.end_segment(i, pos));
        }
        events
    }

    /// À chaque `tick` : on prolonge ce qui s'enregistre avec la valeur courante du réglage (ce qui tient la valeur quand l'utilisateur
    /// ne bouge plus), et on termine si la tête de lecture a reculé (boucle, déplacement).
    pub(super) fn write_tick(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        if self.write_arms.is_empty() || !self.playing {
            return events;
        }
        let pos = self.handle.position();
        for i in 0..self.write_arms.len() {
            if self.write_arms[i].mode == WriteMode::Write && self.write_arms[i].seg.is_none() && !self.write_arms.iter().any(|a| a.seg.is_some() && a.mode != WriteMode::Write) {
                events.extend(self.begin_segment(i));
                continue;
            }
            let Some(last) = self.write_arms[i].seg.as_ref().map(|s| s.last) else { continue };
            if pos < last {
                events.extend(self.end_segment(i, last));
                continue;
            }
            let (node, target, mode, touching) = (self.write_arms[i].node, self.write_arms[i].target, self.write_arms[i].mode, self.write_arms[i].touching);
            if mode == WriteMode::Touch && !touching {
                continue;
            }
            if let Some(v) = self.manual_value(node, target) {
                if let Some(seg) = &mut self.write_arms[i].seg {
                    seg.gestures.push((pos, v));
                    seg.last = pos;
                }
            }
        }
        // Aperçu en direct : ce qui s'est ajouté depuis le dernier envoi.
        for a in &mut self.write_arms {
            if let Some(seg) = &mut a.seg {
                if seg.sent < seg.gestures.len() {
                    let points = seg.gestures[seg.sent..].iter().map(|&(t, v)| [t as f64, v]).collect();
                    events.push(Event::WriteLive { node: a.node, target: super::lane_target_back(a.target), index: seg.sent as u32, points, done: false });
                    seg.sent = seg.gestures.len();
                }
            }
        }
        events
    }

    /// Mode d'une courbe tel que l'utilisateur doit le voir : pendant l'écriture la courbe est suspendue (mode « désactivé » côté
    /// moteur), mais l'affichage garde son mode d'avant.
    pub(super) fn shown_lane_mode(&self, node: u32, target: AutoTarget, mode: AutoMode) -> AutoMode {
        match self.arm_index(node, target).and_then(|i| self.write_arms[i].seg.as_ref()).and_then(|s| s.lane_mode) {
            Some(original) if mode == AutoMode::Off => original,
            _ => mode,
        }
    }

    /// Armements à enregistrer avec le projet (un plugin est désigné par sa place dans la chaîne ; un armement dont le plugin a disparu
    /// n'est pas enregistré).
    pub(super) fn write_arms_for_file(&self) -> Vec<crate::project_io::UiWriteArm> {
        let graph = self.session.graph();
        self.write_arms
            .iter()
            .filter_map(|a| {
                let target = match a.target {
                    AutoTarget::Gain => "gain".to_owned(),
                    AutoTarget::Pan => "pan".to_owned(),
                    AutoTarget::SendGain { to } => format!("send:{}", to.0),
                    AutoTarget::InsertParam { insert, param } => format!("insert:{}:{param}", graph.nodes.get(a.node as usize)?.inserts.iter().position(|i| i.id == insert)?),
                };
                Some(crate::project_io::UiWriteArm { node: a.node, target, mode: a.mode.name().to_owned() })
            })
            .collect()
    }

    /// Remet les armements enregistrés d'un projet qu'on ouvre : `slots` donne, par nœud, l'identifiant du plugin recréé à chaque place
    /// de la chaîne. Ce qui ne retrouve pas sa cible est ignoré (retourne les avertissements).
    pub(super) fn restore_write_arms(&mut self, saved: &[crate::project_io::UiWriteArm], slots: &std::collections::HashMap<u32, Vec<Option<u32>>>) -> Vec<String> {
        let mut warnings = Vec::new();
        self.write_arms.clear();
        for a in saved {
            let Some(mode) = WriteMode::from_name(&a.mode) else { continue };
            let node = self.session.graph().nodes.get(a.node as usize);
            let target = match a.target.as_str() {
                "gain" => Some(AutoTarget::Gain),
                "pan" => Some(AutoTarget::Pan),
                other => {
                    if let Some(to) = other.strip_prefix("send:").and_then(|n| n.parse::<u32>().ok()) {
                        node.filter(|n| n.sends.iter().any(|s| s.to == NodeId(to))).map(|_| AutoTarget::SendGain { to: NodeId(to) })
                    } else if let Some((idx, param)) = other.strip_prefix("insert:").and_then(|r| r.split_once(':')).and_then(|(i, p)| Some((i.parse::<usize>().ok()?, p.parse::<u32>().ok()?))) {
                        slots.get(&a.node).and_then(|s| s.get(idx)).copied().flatten().map(|insert| AutoTarget::InsertParam { insert, param })
                    } else {
                        None
                    }
                }
            };
            match target {
                Some(target) if node.is_some() => self.write_arms.push(WriteArm { node: a.node, target, mode, touching: false, seg: None }),
                _ => warnings.push(format!("armement d'écriture « {} » : cible introuvable, ignoré", a.target)),
            }
        }
        warnings
    }

    /// Armements d'écriture, pour l'événement `project`.
    pub(super) fn write_arm_infos(&self) -> Vec<crate::protocol::WriteArmInfo> {
        self.write_arms.iter().map(|a| crate::protocol::WriteArmInfo { node: a.node, target: super::lane_target_back(a.target), mode: a.mode.name().to_owned() }).collect()
    }
}
