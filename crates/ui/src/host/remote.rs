//! Télécommande d'une tablette de musicien : un retour dont le rôle est « transport » permet, depuis sa page, de lancer / arrêter la lecture, de
//! démarrer l'enregistrement, de revenir au début ou à une section de la forme, de régler le pré-roll en mesures et le métronome. Le serveur
//! (`crate::monitor`) ne fait que transmettre : chaque commande passe par les commandes ordinaires de l'hôte (mêmes garde-fous, mêmes refus,
//! mêmes événements pour la page principale). Les droits sont ceux du **jeton** (rôle choisi par l'utilisateur) ; le serveur refuse (403) toute
//! commande d'un jeton de simple musicien avant même de la lire.

use std::time::Instant;

use daw_core_types::NodeId;

use super::Host;
use crate::monitor::{RemoteCmd, ROLE_TRANSPORT};
use crate::prefs::PreRollUnit;
use crate::protocol::{Command, Event, TransportAction};

/// Combien de temps le dernier refus reste affiché sur la tablette.
const MSG_SECONDS: u64 = 8;

impl Host {
    /// Le bloc `remote` de l'état d'une page à rôle « transport » : ce qu'elle affiche (état, mesure, sections, pré-roll, métronome, dernier refus).
    pub(super) fn remote_state(&self) -> serde_json::Value {
        let rate = self.session.sample_rate();
        let pos = self.handle.position();
        let (tm, mm) = (self.session.tempo_map(), self.session.meter_map());
        let bar_of = |f| mm.locate(f).map(|(b, _)| b + 1).unwrap_or(1);
        let bar = tm.to_frac(pos, rate).map(bar_of).unwrap_or(1);
        let sections: Vec<serde_json::Value> = self.session.structure().iter().map(|s| serde_json::json!({ "name": s.name, "bar": bar_of(s.at), "color": s.color })).collect();
        let armed = self.nodes.iter().filter(|n| n.arm).count();
        let msg = self.remote_msg.as_ref().filter(|(t, _)| t.elapsed().as_secs() < MSG_SECONDS).map(|(_, m)| m.clone());
        serde_json::json!({
            "playing": self.playing,
            "recording": self.recording.is_some(),
            "armed": armed,
            "bar": bar,
            "sections": sections,
            "pre_roll_bars": if self.metro.pre_roll_unit == PreRollUnit::Bars { self.metro.pre_roll_bars } else { 0 },
            "click": self.metro.enabled,
            "msg": msg,
        })
    }

    /// Une commande d'une tablette autorisée. Les refus de l'hôte (aucune piste armée, enregistrement en cours…) sont gardés pour la tablette.
    pub(super) fn remote_request(&mut self, ret: u32, cmd: RemoteCmd) -> Vec<Event> {
        // Le rôle est relu ici aussi : il a pu changer entre la requête et ce tick.
        if self.session.monitor_role(NodeId(ret)) < ROLE_TRANSPORT {
            return Vec::new();
        }
        let events = match cmd {
            RemoteCmd::Toggle => self.command(Command::Transport { action: TransportAction::Toggle }),
            RemoteCmd::Stop => self.command(Command::Transport { action: TransportAction::Stop }),
            RemoteCmd::Start => self.command(Command::Seek { pos: 0 }),
            RemoteCmd::Record => self.command(Command::Record),
            RemoteCmd::Section(i) => {
                let Some(sec) = self.session.structure().get(i as usize) else {
                    self.remote_msg = Some((Instant::now(), "section inconnue".into()));
                    return Vec::new();
                };
                let rate = self.session.sample_rate();
                match self.session.tempo_map().to_samples(sec.at, rate) {
                    Some(pos) => self.command(Command::Seek { pos }),
                    None => Vec::new(),
                }
            }
            RemoteCmd::PreRoll(bars) => {
                let m = &self.metro;
                self.command(Command::SetMetronome { enabled: m.enabled, only_recording: m.only_recording, volume: m.volume, count_in_bars: m.count_in_bars, pre_roll: m.pre_roll, post_roll: m.post_roll, pre_roll_bars: bars, pre_roll_unit: PreRollUnit::Bars })
            }
            RemoteCmd::Click(on) => {
                let m = &self.metro;
                self.command(Command::SetMetronome { enabled: on, only_recording: m.only_recording, volume: m.volume, count_in_bars: m.count_in_bars, pre_roll: m.pre_roll, post_roll: m.post_roll, pre_roll_bars: m.pre_roll_bars, pre_roll_unit: m.pre_roll_unit })
            }
        };
        if let Some(Event::Error { message, .. }) = events.iter().find(|e| matches!(e, Event::Error { .. })) {
            self.remote_msg = Some((Instant::now(), message.clone()));
        } else {
            self.remote_msg = None;
        }
        events
    }

    /// Change le rôle de la télécommande d'un retour (0 : son retour seulement ; 1 : aussi le transport). Sans historique, compte comme modification.
    pub(super) fn set_monitor_role(&mut self, node: u32, role: u8) -> Vec<Event> {
        if !self.session.graph().nodes.get(node as usize).is_some_and(|n| n.phys.is_some()) {
            return vec![Event::Error { command: "set_monitor_role".into(), message: "seul un retour de monitoring a un rôle de télécommande".into() }];
        }
        if !self.session.set_monitor_role_untracked(NodeId(node), role) {
            return vec![Event::Error { command: "set_monitor_role".into(), message: "rôle inconnu (0 : retour seulement, 1 : retour + transport)".into() }];
        }
        self.dirty = true;
        if self.monitor.is_some() {
            self.monitor_publish();
        }
        vec![self.monitor_event(None)]
    }

    /// Arrêt automatique (option des réglages) : arrête le transport quand la lecture ou la prise a dépassé la fin du projet de trop longtemps.
    /// Une prise s'arrête comme à un arrêt normal (fichier fermé, clips posés). Ne s'applique ni en boucle, ni pendant un aperçu ou une écoute.
    pub(super) fn auto_stop_tick(&mut self) -> Vec<Event> {
        let a = self.prefs.auto_stop;
        if !a.enabled || !self.playing || self.loop_range.is_some() || self.preview.is_some() || self.listen.is_some() {
            return Vec::new();
        }
        let rate = u64::from(self.session.sample_rate());
        let end = daw_offline_render::export::project_end(self.session.graph());
        // Pendant une prise, la référence est la fin du dernier élément ou, s'il n'y a rien derrière, le début de la prise.
        let (base, secs) = match &self.recording {
            Some(r) => (end.max(r.start), a.recording_secs),
            None => (end, a.after_end_secs),
        };
        if self.handle.position() < base + u64::from(secs) * rate {
            return Vec::new();
        }
        let message = if self.recording.is_some() { format!("Arrêt automatique : {} min sans rien derrière pendant l'enregistrement.", secs.div_ceil(60)) } else { format!("Arrêt automatique : {} s après la fin du projet.", secs) };
        self.remote_msg = Some((Instant::now(), message.clone()));
        let mut events = self.command(Command::Transport { action: TransportAction::Stop });
        events.push(Event::Error { command: "auto_stop".into(), message });
        events
    }
}
