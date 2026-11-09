//! Retours de monitoring sur le réseau local : l'hôte démarre / arrête le serveur (`crate::monitor`), publie le mix de chaque retour et
//! applique les réglages demandés par les tablettes (envois créés au besoin, niveau borné). Tout passe par les commandes ordinaires :
//! les réglages des tablettes se comportent comme ceux de la page (pas d'historique pour le niveau, état « modifié », moteur).

use std::time::{Duration, Instant};

use daw_core_types::NodeId;

use super::Host;
use crate::monitor::{Monitor, SetRequest};
use crate::protocol::{Command, Event, MonitorReturn};

const DEFAULT_PORT: u16 = 8780;
const PUBLISH_EVERY: Duration = Duration::from_millis(200);
const DB_MIN: f64 = -60.0;
const DB_MAX: f64 = 6.0;

fn gain_to_db(g: f32) -> f64 {
    if g <= 0.0 {
        DB_MIN
    } else {
        (20.0 * (g as f64).log10()).clamp(DB_MIN, DB_MAX)
    }
}

impl Host {
    /// Les retours : bus qui sortent sur la carte son.
    fn monitor_returns(&self) -> Vec<(u32, String)> {
        self.session.graph().nodes.iter().filter(|n| n.phys.is_some() && n.id.0 != 0).map(|n| (n.id.0, n.name.clone())).collect()
    }

    /// Ce que la page doit savoir : actif ou non, adresse et QR de chaque retour.
    pub(super) fn monitor_event(&self, error: Option<String>) -> Event {
        let Some(m) = &self.monitor else { return Event::Monitor { active: false, port: 0, returns: Vec::new(), error } };
        let returns = self
            .monitor_returns()
            .into_iter()
            .filter_map(|(node, name)| m.url(node).map(|url| MonitorReturn { node, name, qr: Monitor::qr_svg(&url), url, role: self.session.monitor_role(NodeId(node)), expired: self.monitor_role_expired(node) }))
            .collect();
        Event::Monitor { active: true, port: m.port(), returns, error }
    }

    pub(super) fn start_monitor(&mut self, port: Option<u16>, local_only: bool) -> Vec<Event> {
        if self.monitor.is_some() {
            return vec![self.monitor_event(None)];
        }
        match Monitor::start(port.unwrap_or(DEFAULT_PORT), local_only) {
            Ok(m) => {
                self.monitor = Some(m);
                self.monitor_generation = self.generation;
                self.monitor_sync();
                self.monitor_publish();
                vec![self.monitor_event(None)]
            }
            Err(e) => vec![self.monitor_event(Some(e))],
        }
    }

    /// Les droits de la télécommande d'un retour, expiration comprise : 0 quand ils sont périmés (`remote.elevated_hours` des préférences).
    pub(super) fn effective_role(&self, ret: u32) -> u8 {
        let role = self.session.monitor_role(NodeId(ret));
        let secs = u64::from(self.prefs.remote.elevated_hours) * 3600;
        match self.monitor_grants.get(&ret) {
            Some((granted, since)) if role != 0 && secs > 0 && *granted == role && since.elapsed().as_secs() >= secs => 0,
            _ => role,
        }
    }

    /// Vieillit les droits donnés (pour les tests : l'expiration se compte en heures).
    #[doc(hidden)]
    pub fn age_monitor_grants(&mut self, by: Duration) {
        for (_, (_, since)) in self.monitor_grants.iter_mut() {
            *since = since.checked_sub(by).unwrap_or(*since);
        }
    }

    fn monitor_role_expired(&self, ret: u32) -> bool {
        self.session.monitor_role(NodeId(ret)) != 0 && self.effective_role(ret) == 0
    }

    pub(super) fn stop_monitor(&mut self) -> Vec<Event> {
        self.monitor = None; // l'arrêt du serveur est dans son `Drop`
        self.monitor_grants.clear(); // le compte des droits repart au prochain démarrage
        vec![self.monitor_event(None)]
    }

    /// Jetons des retours existants. Chaque retour garde le sien **dans le projet** (adresse fixe) : il n'est tiré qu'au premier besoin.
    /// `true` si la liste des adresses a changé.
    fn monitor_sync(&mut self) -> bool {
        let ids: Vec<u32> = self.monitor_returns().into_iter().map(|(id, _)| id).collect();
        let mut tokens = Vec::new();
        for id in ids {
            if self.session.monitor_token(NodeId(id)).is_empty() {
                let t = crate::monitor::new_token(id);
                self.session.set_monitor_token_untracked(NodeId(id), &t);
                self.dirty = true;
            }
            tokens.push((id, self.session.monitor_token(NodeId(id)).to_owned()));
        }
        // Droits : le compte repart quand le rôle change (ou qu'on le voit pour la première fois).
        let roles: Vec<(u32, u8)> = tokens.iter().map(|(id, _)| (*id, self.session.monitor_role(NodeId(*id)))).collect();
        self.monitor_grants.retain(|id, _| roles.iter().any(|(r, _)| r == id));
        for (id, role) in roles {
            if self.monitor_grants.get(&id).is_none_or(|(r, _)| *r != role) {
                self.monitor_grants.insert(id, (role, Instant::now()));
            }
        }
        let generation = self.generation;
        let Some(m) = self.monitor.as_mut() else { return false };
        let mut changed = false;
        if self.monitor_generation != generation {
            self.monitor_generation = generation;
            m.forget_tokens(); // un autre projet : les numéros de nœuds ne désignent plus les mêmes retours
            self.monitor_grants.clear();
            changed = true;
        }
        m.sync_returns(&tokens) || changed
    }

    /// Donne une adresse neuve à un retour (l'ancienne cesse de marcher : à refaire scanner). Pour la sécurité : un jeton est un secret,
    /// et il est maintenant enregistré dans le projet.
    pub(super) fn renew_monitor_address(&mut self, node: u32) -> Vec<Event> {
        if !self.session.graph().nodes.get(node as usize).is_some_and(|n| n.phys.is_some()) {
            return vec![Event::Error { command: "renew_monitor_address".into(), message: "seul un retour de monitoring a une adresse".into() }];
        }
        self.session.set_monitor_token_untracked(NodeId(node), "");
        self.monitor_grants.remove(&node);
        self.dirty = true;
        if self.monitor.is_some() {
            self.monitor_sync();
            self.monitor_publish();
        }
        vec![self.monitor_event(None)]
    }

    /// Le mix de chaque retour : toutes les pistes et tous les bus (sauf les retours et la sortie principale), avec le niveau de leur envoi.
    pub(super) fn monitor_publish(&mut self) {
        let returns = self.monitor_returns();
        let g = self.session.graph();
        let mut out = Vec::new();
        for (ret, name) in &returns {
            let sources: Vec<serde_json::Value> = g
                .nodes
                .iter()
                .filter(|n| n.id.0 != 0 && n.phys.is_none())
                .map(|n| {
                    let send = n.sends.iter().find(|s| s.to.0 == *ret);
                    let db = send.map(|s| gain_to_db(s.gain)).unwrap_or(DB_MIN);
                    // Témoin de signal : la crête récente de la source, après son niveau d'envoi (ce qui arrive dans ce retour) ; −120 : rien
                    // (pas d'envoi, envoi coupé, ou un autre envoi en solo).
                    let solo_elsewhere = g.nodes.iter().any(|o| o.sends.iter().any(|s| s.to.0 == *ret && s.solo));
                    let audible = send.is_some_and(|s| !s.mute && (s.solo || !solo_elsewhere));
                    let sig = match (audible, send) {
                        (true, Some(s)) => {
                            let peak = self.peak_hold.get(&n.id.0).map_or(0.0, |p| p.0) * s.gain;
                            if peak <= 1e-6 { -120.0 } else { (20.0 * f64::from(peak).log10()).max(-120.0) }
                        }
                        _ => -120.0,
                    };
                    serde_json::json!({ "id": n.id.0, "name": n.name, "db": db, "sig": (sig * 10.0).round() / 10.0, "mute": send.is_some_and(|s| s.mute), "solo": send.is_some_and(|s| s.solo), "group": self.session.effective_group_of(n.id, Some(NodeId(*ret))) })
                })
                .collect();
            let groups: Vec<serde_json::Value> = self.session.effective_group_list(Some(NodeId(*ret))).iter().map(|gr| serde_json::json!({ "id": gr.id, "name": gr.name, "color": gr.color, "offset_db": self.group_offset(gr.id, Some(*ret)) })).collect();
            let role = self.effective_role(*ret);
            let mut state = serde_json::json!({ "name": name, "sources": sources, "groups": groups, "role": role });
            if self.monitor_role_expired(*ret) {
                state["expired"] = true.into();
            }
            if role & crate::monitor::ROLE_TRANSPORT != 0 {
                state["remote"] = self.remote_state();
            }
            if role & crate::monitor::ROLE_MIXER != 0 {
                state["mix"] = self.mix_state();
            }
            out.push((*ret, state.to_string(), role));
        }
        if let Some(m) = self.monitor.as_mut() {
            for (ret, json, role) in out {
                m.publish(ret, json, role);
            }
        }
    }

    /// Appelé à chaque `tick` : applique les réglages des tablettes, republie les mix, signale un changement de la liste des retours.
    pub(super) fn monitor_tick(&mut self) -> Vec<Event> {
        let Some(m) = self.monitor.as_mut() else { return Vec::new() };
        let requests = m.take_requests();
        let mut events = Vec::new();
        let mut changed = self.monitor_sync();
        for r in requests {
            events.extend(self.monitor_request(r));
        }
        if changed || self.monitor_last.elapsed() >= PUBLISH_EVERY {
            self.monitor_last = Instant::now();
            self.monitor_publish();
        }
        if std::mem::take(&mut changed) {
            events.push(self.monitor_event(None));
        }
        events
    }

    /// Une demande de tablette. Seuls les retours existants et les sources (ni retour, ni sortie principale) sont acceptés.
    fn monitor_request(&mut self, r: SetRequest) -> Vec<Event> {
        match r {
            SetRequest::Level { ret, node, db } => self.monitor_apply(ret, node, db),
            SetRequest::Flags { ret, node, mute, solo } => {
                if !self.monitor_valid(ret, Some(node)) {
                    return Vec::new();
                }
                let mut events = Vec::new();
                self.monitor_ensure_send(ret, node, &mut events);
                events.extend(self.command(Command::SetSend { node, to: ret, gain: None, pre: None, mute, solo }));
                events
            }
            SetRequest::Group { ret, group, mute, solo, delta_db } => {
                if !self.monitor_valid(ret, None) {
                    return Vec::new();
                }
                // Un membre qui est lui-même un retour n'est pas une source : on ne touche qu'aux vraies sources.
                self.set_group(group, mute, solo, delta_db, Some(ret))
            }
            SetRequest::GroupOffset { ret, group, db } => {
                if !self.monitor_valid(ret, None) {
                    return Vec::new();
                }
                self.set_group_offset(group, db, Some(ret))
            }
            SetRequest::Remote { ret, cmd } => {
                if !self.monitor_valid(ret, None) {
                    return Vec::new();
                }
                self.remote_request(ret, cmd)
            }
            SetRequest::Mix { ret, cmd } => {
                if !self.monitor_valid(ret, None) {
                    return Vec::new();
                }
                self.mix_request(ret, cmd)
            }
        }
    }

    fn monitor_valid(&self, ret: u32, node: Option<u32>) -> bool {
        let g = self.session.graph();
        let ok_ret = g.nodes.get(ret as usize).map(|n| n.phys.is_some()).unwrap_or(false);
        let ok_src = node.map_or(true, |n| g.nodes.get(n as usize).map(|x| n != 0 && x.phys.is_none()).unwrap_or(false));
        ok_ret && ok_src
    }

    /// Crée l'envoi `node` → `ret` (niveau nul) s'il n'existe pas, pour qu'on puisse le couper ou le mettre en solo.
    fn monitor_ensure_send(&mut self, ret: u32, node: u32, events: &mut Vec<Event>) {
        if !self.session.graph().nodes[node as usize].sends.iter().any(|s| s.to.0 == ret) {
            events.extend(self.command(Command::AddSend { node, to: ret }));
            events.extend(self.command(Command::SetSend { node, to: ret, gain: Some(0.0), pre: None, mute: None, solo: None }));
        }
    }

    /// Règle l'envoi de `node` vers le retour `ret` à `db` (borné ; l'envoi est créé s'il n'existe pas). Une source qui n'existe pas, un retour
    /// ou la sortie principale sont refusés en silence : une tablette ne peut régler que des envois vers son propre retour.
    fn monitor_apply(&mut self, ret: u32, node: u32, db: f64) -> Vec<Event> {
        let g = self.session.graph();
        let valid_ret = g.nodes.get(ret as usize).map(|n| n.phys.is_some()).unwrap_or(false);
        let valid_src = g.nodes.get(node as usize).map(|n| node != 0 && n.phys.is_none()).unwrap_or(false);
        if !valid_ret || !valid_src {
            return Vec::new();
        }
        let has_send = g.nodes[node as usize].sends.iter().any(|s| s.to.0 == ret);
        let db = db.clamp(DB_MIN, DB_MAX);
        let gain = if db <= DB_MIN { 0.0 } else { 10f64.powf(db / 20.0) };
        let mut events = Vec::new();
        if !has_send {
            if db <= DB_MIN {
                return events; // rien à créer pour un niveau nul
            }
            events.extend(self.command(Command::AddSend { node, to: ret }));
        }
        events.extend(self.command(Command::SetSend { node, to: ret, gain: Some(gain), pre: None, mute: None, solo: None }));
        events
    }
}
