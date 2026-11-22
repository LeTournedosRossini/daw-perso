//! Talkback : la régie parle dans les retours choisis (un canal d'entrée de la carte sert de micro) ; le reste de leur mix baisse
//! pendant ce temps. Réglage de session, non enregistré dans le projet (les numéros de nœuds changent avec la structure : un
//! changement de structure efface les retours visés). Maintien (« push-to-talk ») : la page ou la tablette redit « je parle » toutes les
//! 500 ms ; passé 1,5 s sans nouvelle, la voix est coupée (page rechargée, tablette sortie du réseau, doigt levé sans message).

use std::time::{Duration, Instant};

use daw_engine::Command as EngineCommand;

use super::*;

/// Délai sans nouvelle d'une page ou d'une tablette au-delà duquel la parole est coupée.
const HOLD: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone)]
pub(super) struct Talk {
    /// Entrée du micro, numérotée à partir de 1 (comme les entrées d'enregistrement).
    pub channel: Option<u32>,
    /// Niveau de la voix dans les retours, en dB.
    pub level_db: f64,
    /// Baisse du reste du mix pendant la parole, en dB.
    pub duck_db: f64,
    pub targets: Vec<u32>,
    /// Instant jusqu'auquel la parole est maintenue (`None` : on ne parle pas).
    pub until: Option<Instant>,
}

impl Default for Talk {
    fn default() -> Self {
        Talk { channel: None, level_db: 0.0, duck_db: 12.0, targets: Vec::new(), until: None }
    }
}

impl Host {
    pub(super) fn talkback_event(&self) -> Event {
        let t = &self.talk;
        Event::Talkback { channel: t.channel, level_db: t.level_db, duck_db: t.duck_db, targets: t.targets.clone(), active: t.until.is_some() }
    }

    /// Envoie au moteur l'état courant (micro, parole, retours visés).
    pub(super) fn apply_talk(&mut self) {
        let t = &self.talk;
        let duck = 1.0 - 10f32.powf(-(t.duck_db as f32) / 20.0);
        self.handle.send(EngineCommand::SetTalkback { channel: t.channel.map_or(u32::MAX, |c| c - 1), active: t.until.is_some(), duck });
        let level = 10f32.powf(t.level_db as f32 / 20.0);
        let nodes: Vec<(u32, bool)> = self.session.graph().nodes.iter().enumerate().filter(|(_, n)| n.monitor).map(|(i, _)| (i as u32, t.targets.contains(&(i as u32)))).collect();
        for (node, target) in nodes {
            self.handle.send(EngineCommand::SetReturnTalk { node, gain: if target { level } else { 0.0 } });
        }
    }

    /// Règle le talkback : micro, niveau, baisse du mix et retours visés (tous des retours de monitoring).
    pub(super) fn set_talkback(&mut self, channel: Option<u32>, level_db: f64, duck_db: f64, mut targets: Vec<u32>) -> Vec<Event> {
        let err = |message: String| vec![Event::Error { command: "set_talkback".into(), message }];
        if let Some(c) = channel {
            if self.audio_inputs > 0 && c > self.audio_inputs {
                return err(format!("entrée {c} inexistante : le pilote en a ouvert {}", self.audio_inputs));
            }
        }
        for &n in &targets {
            if !self.session.graph().nodes.get(n as usize).is_some_and(|d| d.monitor) {
                return err(format!("le nœud {n} n'est pas un retour de monitoring"));
            }
        }
        targets.sort_unstable();
        targets.dedup();
        let was_talking = self.talk.until.is_some() && channel.is_some();
        self.talk = Talk { channel, level_db, duck_db, targets, until: self.talk.until.filter(|_| was_talking) };
        self.apply_talk();
        vec![self.talkback_event()]
    }

    /// Maintien de la parole (`active`) ou fin (`false`). Il faut un micro choisi et au moins un retour visé.
    pub(super) fn talk(&mut self, active: bool) -> Vec<Event> {
        if active {
            if self.talk.channel.is_none() || self.talk.targets.is_empty() {
                return vec![Event::Error { command: "talk".into(), message: "choisis d'abord l'entrée du micro et au moins un retour".into() }];
            }
            let was = self.talk.until.is_some();
            self.talk.until = Some(Instant::now() + HOLD);
            if was {
                return Vec::new();
            }
        } else if self.talk.until.take().is_none() {
            return Vec::new();
        }
        self.apply_talk();
        vec![self.talkback_event()]
    }

    /// À chaque `tick` : coupe la parole qui n'est plus maintenue.
    pub(super) fn talk_tick(&mut self) -> Vec<Event> {
        if self.talk.until.is_some_and(|u| Instant::now() > u) {
            self.talk.until = None;
            self.apply_talk();
            return vec![self.talkback_event()];
        }
        Vec::new()
    }

    /// Structure changée ou projet remplacé : les numéros de nœuds ne veulent plus rien dire, la parole s'arrête et les cibles sont oubliées.
    pub(super) fn clear_talk(&mut self) {
        let touched = self.talk.until.is_some() || !self.talk.targets.is_empty();
        self.talk.until = None;
        self.talk.targets.clear();
        if touched {
            self.apply_talk();
        }
    }

    /// Demande d'une tablette de régie : parler / se taire, ajouter ou retirer un retour visé. Droit relu à l'exécution.
    pub(super) fn talk_request(&mut self, ret: u32, active: Option<bool>, target: Option<(u32, bool)>) -> Vec<Event> {
        if self.effective_role(ret) & crate::monitor::ROLE_MIXER == 0 {
            return Vec::new();
        }
        let mut events = Vec::new();
        if let Some((node, on)) = target {
            let mut targets = self.talk.targets.clone();
            targets.retain(|&n| n != node);
            if on {
                targets.push(node);
            }
            events.extend(self.set_talkback(self.talk.channel, self.talk.level_db, self.talk.duck_db, targets));
        }
        if let Some(a) = active {
            events.extend(self.talk(a));
        }
        match events.iter().find(|e| matches!(e, Event::Error { .. })) {
            Some(Event::Error { message, .. }) => self.remote_msg = Some((Instant::now(), message.clone())),
            _ => self.remote_msg = None,
        }
        events
    }
}
