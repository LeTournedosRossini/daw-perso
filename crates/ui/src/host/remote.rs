//! Télécommande d'une tablette de musicien : un retour dont le rôle est « transport » permet, depuis sa page, de lancer / arrêter la lecture, de
//! démarrer l'enregistrement, de revenir au début ou à une section de la forme, de régler le pré-roll en mesures et le métronome. Le serveur
//! (`crate::monitor`) ne fait que transmettre : chaque commande passe par les commandes ordinaires de l'hôte (mêmes garde-fous, mêmes refus,
//! mêmes événements pour la page principale). Les droits sont ceux du **jeton** (rôle choisi par l'utilisateur) ; le serveur refuse (403) toute
//! commande d'un jeton de simple musicien avant même de la lire.

use std::time::Instant;

use daw_core_types::NodeId;

use super::Host;
use crate::monitor::{FxCmd, FxTarget, MixCmd, RemoteCmd, ROLE_ALL, ROLE_FINE, ROLE_MIXER, ROLE_TRANSPORT};
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
            "play_pre_roll": self.prefs.transport.play_pre_roll,
            "msg": msg,
        })
    }

    /// Une commande d'une tablette autorisée. Les refus de l'hôte (aucune piste armée, enregistrement en cours…) sont gardés pour la tablette.
    pub(super) fn remote_request(&mut self, ret: u32, cmd: RemoteCmd) -> Vec<Event> {
        // Le rôle est relu ici aussi : il a pu changer entre la requête et ce tick.
        if self.effective_role(ret) & ROLE_TRANSPORT == 0 {
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
            RemoteCmd::PlayPreRoll(on) => {
                let mut prefs = self.prefs.clone();
                prefs.transport.play_pre_roll = on;
                self.command(Command::SetPrefs { prefs })
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

    /// La régie d'une tablette : état de chaque nœud dans l'ordre d'affichage (niveau, panoramique, muet, solo, armement, crête récente).
    pub(super) fn mix_state(&self, fine: bool) -> serde_json::Value {
        let g = self.session.graph();
        // L'ordre d'affichage du projet ; la sortie principale n'en fait pas partie : elle vient en dernier.
        let mut order: Vec<NodeId> = self.session.display_order().to_vec();
        if !order.contains(&NodeId(0)) {
            order.push(NodeId(0));
        }
        let nodes: Vec<serde_json::Value> = order
            .iter()
            .filter_map(|id| {
                let n = g.nodes.get(id.0 as usize)?;
                let st = self.nodes.get(id.0 as usize)?;
                let kind = match n.kind {
                    daw_engine::NodeKind::Master => "master",
                    daw_engine::NodeKind::Bus if n.phys.is_some() => "return",
                    daw_engine::NodeKind::Bus => "bus",
                    daw_engine::NodeKind::Track(_) => "track",
                };
                let peak = self.peak_hold.get(&id.0).map_or(0.0, |p| p.0);
                let peak_db = if peak <= 1e-6 { -120.0 } else { (20.0 * f64::from(peak).log10()).max(-120.0) };
                let color = match self.session.node_color(*id) {
                    "" => "#9aa3b2",
                    c => c,
                };
                let group = self.session.node_group(*id);
                Some(serde_json::json!({
                    "id": id.0, "name": n.name, "kind": kind, "color": color, "group": group,
                    "db": (f64::from(daw_core_types::gain_to_db(st.gain)).clamp(-60.0, 6.0) * 10.0).round() / 10.0,
                    "pan": st.pan, "mute": st.mute, "solo": st.solo, "arm": st.arm,
                    "peak": (peak_db * 10.0).round() / 10.0,
                    // Réglages fins : la chaîne de plugins de la piste (un plugin a une page de contrôle s'il en a une dans la banque).
                    "auto": if fine { serde_json::json!({ "gain": self.auto_mode_name(id.0, daw_engine::AutoTarget::Gain), "pan": self.auto_mode_name(id.0, daw_engine::AutoTarget::Pan) }) } else { serde_json::Value::Null },
                    "inserts": if fine { n.inserts.iter().map(|i| serde_json::json!({ "id": i.id, "name": i.name, "bypass": i.bypass, "page": self.insert_has_page(i.id) })).collect::<Vec<_>>() } else { Vec::new() },
                }))
            })
            .collect();
        // Groupes du projet (ceux de la page, pas ceux d'un retour) : état muet / solo des membres et décalage de niveau en cours.
        let groups: Vec<serde_json::Value> = self
            .session
            .effective_group_list(None)
            .iter()
            .map(|gr| {
                let members = self.session.effective_members(gr.id, None);
                let all = |f: &dyn Fn(&super::NodeState) -> bool| !members.is_empty() && members.iter().all(|m| self.nodes.get(m.0 as usize).is_some_and(f));
                let any_solo = members.iter().any(|m| self.nodes.get(m.0 as usize).is_some_and(|s| s.solo));
                serde_json::json!({ "id": gr.id, "name": gr.name, "color": gr.color, "count": members.len(), "mute": all(&|s| s.mute), "solo": any_solo, "offset_db": self.group_offset(gr.id, None) })
            })
            .collect();
        serde_json::json!({ "nodes": nodes, "groups": groups })
    }

    /// Le mode d'automation d'une cible : l'armement d'écriture (`touch`, `latch`, `write`) s'il y en a un, sinon le mode de sa courbe (`read`, `trim`, `off`), sinon `none`.
    pub(super) fn auto_mode_name(&self, node: u32, target: daw_engine::AutoTarget) -> &'static str {
        if let Some(a) = self.write_arms.iter().find(|a| a.node == node && a.target == target) {
            return a.mode.name();
        }
        match self.session.lane(NodeId(node), target) {
            Some(l) => super::lane_mode_name(l.mode),
            None => "none",
        }
    }

    fn insert_has_page(&self, insert: u32) -> bool {
        self.meta.plugins.get(&insert).is_some_and(|p| self.bank.get(&p.clap_id).is_some_and(|e| !e.params.is_empty()))
    }

    /// La page de contrôle ouverte par la tablette du retour `ret` : les paramètres choisis du plugin avec leurs valeurs et leurs textes.
    pub(super) fn fine_state(&self, ret: u32) -> serde_json::Value {
        let Some(&insert) = self.remote_open.get(&ret) else { return serde_json::json!({ "page": null }) };
        let Some(Event::InsertPage { configured, params, missing, .. }) = self.insert_page_event(insert).into_iter().next() else { return serde_json::json!({ "page": null }) };
        let owner = self.session.graph().nodes.iter().find(|n| n.inserts.iter().any(|i| i.id == insert));
        let name = owner.and_then(|n| n.inserts.iter().find(|i| i.id == insert)).map(|i| i.name.clone()).unwrap_or_default();
        serde_json::json!({ "page": { "insert": insert, "name": name, "track": owner.map(|n| n.name.clone()), "configured": configured, "missing": missing,
            "node": owner.map(|n| n.id.0),
            "params": params.iter().map(|p| serde_json::json!({ "id": p.id, "name": p.name, "min": p.min, "max": p.max, "default": p.default, "value": p.value, "text": p.text, "auto": owner.map_or("none", |n| self.auto_mode_name(n.id.0, daw_engine::AutoTarget::InsertParam { insert, param: p.id })) })).collect::<Vec<_>>() } })
    }

    /// Un réglage fin d'une tablette : la page d'un plugin s'ouvre, ses paramètres **choisis** se règlent (aucun autre), il se contourne. Le droit est relu ici.
    pub(super) fn fx_request(&mut self, ret: u32, cmd: FxCmd) -> Vec<Event> {
        if self.effective_role(ret) & ROLE_FINE == 0 {
            return Vec::new();
        }
        let refuse = |this: &mut Self, m: &str| {
            this.remote_msg = Some((Instant::now(), m.into()));
            Vec::new()
        };
        let in_page = |this: &Self, insert: u32, param: u32| matches!(this.insert_page_event(insert).into_iter().next(), Some(Event::InsertPage { params, .. }) if params.iter().any(|p| p.id == param));
        match cmd {
            FxCmd::Open(None) => {
                self.remote_open.remove(&ret);
                Vec::new()
            }
            FxCmd::Open(Some(insert)) => {
                if self.plugin_handle(insert).is_none() {
                    return refuse(self, "plugin inconnu");
                }
                self.remote_open.insert(ret, insert);
                self.remote_msg = None;
                Vec::new()
            }
            FxCmd::Param { insert, param, value } => {
                if !in_page(self, insert, param) {
                    return refuse(self, "ce paramètre n'est pas dans la page de contrôle du plugin");
                }
                // Borne à la plage du paramètre.
                let ph = self.plugin_handle(insert).expect("plugin déjà vérifié par la page");
                let value = ph.params().iter().find(|p| p.id == param).map_or(value, |p| value.clamp(p.min.min(p.max), p.max.max(p.min)));
                self.remote_msg = None;
                self.command(Command::SetPluginParam { insert, param, value })
            }
            FxCmd::Touch { insert, param, active } => {
                if !in_page(self, insert, param) {
                    return refuse(self, "ce paramètre n'est pas dans la page de contrôle du plugin");
                }
                let Some(node) = self.session.graph().nodes.iter().position(|n| n.inserts.iter().any(|i| i.id == insert)) else { return refuse(self, "plugin inconnu") };
                self.gesture_auto(node as u32, daw_engine::AutoTarget::InsertParam { insert, param }, active)
            }
            FxCmd::Auto { node, target, mode } => {
                if self.session.graph().nodes.get(node as usize).is_none() {
                    return refuse(self, "piste inconnue");
                }
                let lane = match target {
                    FxTarget::Gain => crate::protocol::LaneTarget::Gain,
                    FxTarget::Pan => crate::protocol::LaneTarget::Pan,
                    FxTarget::Insert { insert, param } => {
                        // Seulement un paramètre de la page de contrôle, d'un plugin de cette piste.
                        let on_node = self.session.graph().nodes[node as usize].inserts.iter().any(|i| i.id == insert);
                        if !on_node || !in_page(self, insert, param) {
                            return refuse(self, "ce paramètre n'est pas dans la page de contrôle d'un plugin de cette piste");
                        }
                        crate::protocol::LaneTarget::Insert { insert, param }
                    }
                };
                self.remote_msg = None;
                let mut events = match mode.as_str() {
                    "touch" | "latch" | "write" | "none" => return self.command(Command::SetWriteMode { node, target: lane, mode }),
                    // Les modes de la courbe : on désarme d'abord l'écriture (sinon elle remet la courbe en lecture).
                    _ => self.command(Command::SetWriteMode { node, target: lane, mode: "none".into() }),
                };
                let more = self.command(Command::SetLaneMode { node, target: lane, mode });
                if let Some(Event::Error { message, .. }) = more.iter().find(|e| matches!(e, Event::Error { .. })) {
                    self.remote_msg = Some((Instant::now(), message.clone()));
                }
                events.extend(more);
                events
            }
            FxCmd::Bypass { insert, on } => {
                if self.plugin_handle(insert).is_none() {
                    return refuse(self, "plugin inconnu");
                }
                self.remote_msg = None;
                self.command(Command::SetPluginBypass { insert, bypass: on })
            }
        }
    }

    /// Un réglage de groupe de la régie : passe par les mêmes fonctions que la page (écarts de niveau conservés). Le droit est relu ici aussi.
    pub(super) fn mix_group_request(&mut self, ret: u32, group: u32, mute: Option<bool>, solo: Option<bool>, offset_db: Option<f64>) -> Vec<Event> {
        if self.effective_role(ret) & ROLE_MIXER == 0 {
            return Vec::new();
        }
        let mut events = Vec::new();
        if mute.is_some() || solo.is_some() {
            events.extend(self.set_group(group, mute, solo, None, None));
        }
        if let Some(db) = offset_db {
            events.extend(self.set_group_offset(group, db, None));
        }
        match events.iter().find(|e| matches!(e, Event::Error { .. })) {
            Some(Event::Error { message, .. }) => self.remote_msg = Some((Instant::now(), message.clone())),
            _ => self.remote_msg = None,
        }
        events
    }

    /// Un réglage de régie, avec le droit relu à l'exécution. Passe par `set_node` (mêmes refus que la page : armement d'un bus, pendant une prise…).
    pub(super) fn mix_request(&mut self, ret: u32, cmd: MixCmd) -> Vec<Event> {
        if self.effective_role(ret) & ROLE_MIXER == 0 {
            return Vec::new();
        }
        let Some(n) = self.session.graph().nodes.get(cmd.node as usize) else {
            self.remote_msg = Some((Instant::now(), "piste inconnue".into()));
            return Vec::new();
        };
        let master = cmd.node == 0;
        let is_track = matches!(n.kind, daw_engine::NodeKind::Track(_));
        if (master && (cmd.pan.is_some() || cmd.solo.is_some() || cmd.arm.is_some())) || (cmd.arm.is_some() && !is_track) {
            self.remote_msg = Some((Instant::now(), "ce réglage n'existe pas pour ce nœud".into()));
            return Vec::new();
        }
        let gain = cmd.db.map(|db| if db <= -60.0 { 0.0 } else { daw_core_types::db_to_gain(db.clamp(-60.0, 6.0) as f32) });
        let pan = cmd.pan.map(|p| p.clamp(-1.0, 1.0) as f32);
        let events = self.command(Command::SetNode { node: cmd.node, gain, pan, mute: cmd.mute, solo: cmd.solo, arm: cmd.arm, monitor: None, invert: None });
        match events.iter().find(|e| matches!(e, Event::Error { .. })) {
            Some(Event::Error { message, .. }) => self.remote_msg = Some((Instant::now(), message.clone())),
            _ => self.remote_msg = None,
        }
        events
    }

    /// Change le rôle de la télécommande d'un retour (0 : son retour seulement ; 1 : aussi le transport). Sans historique, compte comme modification.
    pub(super) fn set_monitor_role(&mut self, node: u32, role: u8) -> Vec<Event> {
        if !self.session.graph().nodes.get(node as usize).is_some_and(|n| n.phys.is_some()) {
            return vec![Event::Error { command: "set_monitor_role".into(), message: "seul un retour de monitoring a un rôle de télécommande".into() }];
        }
        if !self.session.set_monitor_role_untracked(NodeId(node), role) {
            return vec![Event::Error { command: "set_monitor_role".into(), message: format!("rôle inconnu (0 à {ROLE_ALL} : 1 = transport, 2 = régie, 4 = réglages fins, additionnés)") }];
        }
        self.dirty = true;
        self.monitor_grants.insert(node, (role, Instant::now())); // redonner les mêmes droits les renouvelle
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
