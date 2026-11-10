//! Groupes de pistes : créer, modifier, supprimer (annulable) ; agir sur tous les membres d'un coup (mute, solo, niveau).

use daw_core_types::NodeId;
use daw_session::groups::Group;

use super::{explain, Host};
use crate::protocol::{Command, Event, GroupOffsetInfo};

/// Gain linéaire maximal d'un fader ou d'un envoi (+6 dB).
const GAIN_MAX: f64 = 2.0;

impl Host {
    fn group_error(command: &str, message: impl Into<String>) -> Vec<Event> {
        vec![Event::Error { command: command.into(), message: message.into() }]
    }

    /// Appelle `f` sur la liste des groupes et l'appartenance (nœud, groupe), puis remplace le tout en une action annulable. `ret` : les
    /// groupes propres de ce retour (il doit en avoir) ; `None` : les groupes du projet.
    fn edit_groups(&mut self, command: &str, label: &str, ret: Option<u32>, f: impl FnOnce(&mut Vec<Group>, &mut Vec<(NodeId, u32)>) -> Result<(), String>) -> Vec<Event> {
        let count = self.session.graph().nodes.len() as u32;
        let (mut groups, mut members) = match ret {
            None => (self.session.groups().to_vec(), (1..count).map(|i| (NodeId(i), self.session.node_group(NodeId(i)))).filter(|&(_, g)| g != 0).collect::<Vec<_>>()),
            Some(r) => {
                let Some(state) = self.session.return_groups(NodeId(r)) else {
                    return Self::group_error(command, "ce retour n'a pas de groupes propres : commence par les personnaliser");
                };
                (state.groups.clone(), state.of.iter().enumerate().filter(|(_, &g)| g != 0).map(|(i, &g)| (NodeId(i as u32), g)).collect())
            }
        };
        if let Err(m) = f(&mut groups, &mut members) {
            return Self::group_error(command, m);
        }
        let result = match ret {
            None => self.session.set_groups(label, groups, &members),
            Some(r) => self.session.set_return_groups(label, NodeId(r), Some((groups, members))),
        };
        match result {
            Ok(()) => {
                self.dirty = true;
                vec![self.project_event(), self.history_event()]
            }
            Err(e) => Self::group_error(command, explain(&e)),
        }
    }

    pub(super) fn create_group(&mut self, name: String, color: Option<String>, nodes: Vec<u32>, ret: Option<u32>) -> Vec<Event> {
        let id = self.session.alloc_group_id();
        let name = name.trim().to_owned();
        self.edit_groups("create_group", "Créer un groupe", ret, move |groups, members| {
            groups.push(Group { id, name, color: color.unwrap_or_default() });
            members.retain(|(n, _)| !nodes.contains(&n.0)); // un nœud n'est que dans un groupe : il quitte l'ancien
            members.extend(nodes.iter().map(|&n| (NodeId(n), id)));
            Ok(())
        })
    }

    pub(super) fn edit_group(&mut self, group: u32, name: Option<String>, color: Option<String>, nodes: Option<Vec<u32>>, ret: Option<u32>) -> Vec<Event> {
        self.edit_groups("edit_group", "Modifier un groupe", ret, move |groups, members| {
            let g = groups.iter_mut().find(|g| g.id == group).ok_or_else(|| format!("groupe inconnu : {group}"))?;
            if let Some(n) = name {
                g.name = n.trim().to_owned();
            }
            if let Some(c) = color {
                g.color = c;
            }
            if let Some(list) = nodes {
                members.retain(|&(n, gr)| gr != group && !list.contains(&n.0));
                members.extend(list.iter().map(|&n| (NodeId(n), group)));
            }
            Ok(())
        })
    }

    pub(super) fn delete_group(&mut self, group: u32, ret: Option<u32>) -> Vec<Event> {
        let events = self.edit_groups("delete_group", "Supprimer un groupe", ret, move |groups, members| {
            if !groups.iter().any(|g| g.id == group) {
                return Err(format!("groupe inconnu : {group}"));
            }
            groups.retain(|g| g.id != group);
            members.retain(|&(_, gr)| gr != group);
            Ok(())
        });
        self.group_offsets.retain(|(g, _), _| *g != group);
        events
    }

    /// Remet à 0 tous les décalages de groupes vers `ret` (les niveaux des envois reviennent), avant de changer de groupes.
    pub(super) fn clear_return_offsets(&mut self, ret: u32) -> Vec<Event> {
        self.forget_stale_offsets();
        let groups: Vec<u32> = self.group_offsets.iter().filter(|((_, to), db)| *to == Some(ret) && **db != 0.0).map(|((g, _), _)| *g).collect();
        let mut events = Vec::new();
        for g in groups {
            events.extend(self.set_group_offset(g, 0.0, Some(ret)));
        }
        self.group_offsets.retain(|(_, to), _| *to != Some(ret));
        events
    }

    /// Donne au retour ses propres groupes : une copie de ceux du projet (nouveaux identifiants), qui les remplace dans son mix.
    pub(super) fn customize_return_groups(&mut self, ret: u32) -> Vec<Event> {
        if self.session.return_groups(NodeId(ret)).is_some() {
            return Self::group_error("customize_return_groups", "ce retour a déjà ses propres groupes");
        }
        if !self.session.graph().nodes.get(ret as usize).is_some_and(|n| n.monitor) {
            return Self::group_error("customize_return_groups", "seul un retour de monitoring peut avoir ses propres groupes");
        }
        let mut events = self.clear_return_offsets(ret);
        let project: Vec<Group> = self.session.groups().to_vec();
        let mut copy = Vec::new();
        let mut members = Vec::new();
        for g in project {
            let id = self.session.alloc_group_id();
            for m in self.session.group_members(g.id) {
                if !self.session.graph().nodes[m.0 as usize].monitor {
                    members.push((m, id));
                }
            }
            copy.push(Group { id, name: g.name, color: g.color });
        }
        events.extend(self.finish_return_groups("customize_return_groups", "Groupes propres au retour", ret, Some((copy, members))));
        events
    }

    /// Copie les groupes d'un autre retour (les siens ou ceux du projet qu'il suit) en groupes propres de `ret`, avec de nouveaux
    /// identifiants ; remplace ceux que `ret` avait.
    pub(super) fn copy_return_groups(&mut self, ret: u32, from: u32) -> Vec<Event> {
        const CMD: &str = "copy_return_groups";
        let is_return = |n: u32| self.session.graph().nodes.get(n as usize).is_some_and(|x| x.monitor);
        if ret == from {
            return Self::group_error(CMD, "choisis un autre retour que celui-ci");
        }
        if !is_return(ret) || !is_return(from) {
            return Self::group_error(CMD, "les deux nœuds doivent être des retours de monitoring");
        }
        let mut events = self.clear_return_offsets(ret);
        let old: Vec<u32> = self.session.return_groups(NodeId(ret)).map(|g| g.groups.iter().map(|x| x.id).collect()).unwrap_or_default();
        let source: Vec<Group> = self.session.effective_group_list(Some(NodeId(from))).to_vec();
        let mut copy = Vec::new();
        let mut members = Vec::new();
        for g in source {
            let id = self.session.alloc_group_id();
            for m in self.session.effective_members(g.id, Some(NodeId(from))) {
                if m.0 != ret && !self.session.graph().nodes[m.0 as usize].monitor {
                    members.push((m, id));
                }
            }
            copy.push(Group { id, name: g.name, color: g.color });
        }
        events.extend(self.finish_return_groups(CMD, "Copier les groupes d'un retour", ret, Some((copy, members))));
        self.group_offsets.retain(|(g, _), _| !old.contains(g));
        events
    }

    /// Ramène le retour aux groupes du projet.
    pub(super) fn reset_return_groups(&mut self, ret: u32) -> Vec<Event> {
        if self.session.return_groups(NodeId(ret)).is_none() {
            return Self::group_error("reset_return_groups", "ce retour suit déjà les groupes du projet");
        }
        let mut events = self.clear_return_offsets(ret);
        let old: Vec<u32> = self.session.return_groups(NodeId(ret)).map(|g| g.groups.iter().map(|x| x.id).collect()).unwrap_or_default();
        events.extend(self.finish_return_groups("reset_return_groups", "Revenir aux groupes du projet", ret, None));
        self.group_offsets.retain(|(g, _), _| !old.contains(g));
        events
    }

    fn finish_return_groups(&mut self, command: &str, label: &str, ret: u32, state: Option<(Vec<Group>, Vec<(NodeId, u32)>)>) -> Vec<Event> {
        match self.session.set_return_groups(label, NodeId(ret), state) {
            Ok(()) => {
                self.dirty = true;
                vec![self.project_event(), self.history_event()]
            }
            Err(e) => Self::group_error(command, explain(&e)),
        }
    }

    /// Les décalages mémorisés ne valent que pour le projet en cours : un autre projet repart de zéro.
    fn forget_stale_offsets(&mut self) {
        if self.group_offsets_generation != self.generation {
            self.group_offsets_generation = self.generation;
            self.group_offsets.clear();
        }
    }

    /// Décalage actuel d'un groupe vers une destination (0 s'il n'y en a pas).
    pub(super) fn group_offset(&self, group: u32, to: Option<u32>) -> f64 {
        if self.group_offsets_generation != self.generation {
            return 0.0;
        }
        self.group_offsets.get(&(group, to)).copied().unwrap_or(0.0)
    }

    /// Décalages non nuls d'un groupe, pour l'événement `project`.
    pub(super) fn group_offset_infos(&self, group: u32) -> Vec<GroupOffsetInfo> {
        if self.group_offsets_generation != self.generation {
            return Vec::new();
        }
        let mut v: Vec<GroupOffsetInfo> = self.group_offsets.iter().filter(|((g, _), db)| *g == group && **db != 0.0).map(|((_, to), db)| GroupOffsetInfo { to: *to, db: *db }).collect();
        v.sort_by_key(|o| o.to);
        v
    }

    /// Place le décalage du groupe à `db` : applique aux membres la différence avec le décalage mémorisé (écarts conservés).
    pub(super) fn set_group_offset(&mut self, group: u32, db: f64, to: Option<u32>) -> Vec<Event> {
        if !self.session.effective_group_list(to.map(NodeId)).iter().any(|g| g.id == group) {
            return Self::group_error("set_group_offset", format!("groupe inconnu : {group}"));
        }
        self.forget_stale_offsets();
        let db = (db.clamp(-24.0, 24.0) * 100.0).round() / 100.0;
        let delta = db - self.group_offset(group, to);
        if delta.abs() < 1e-9 {
            return Vec::new();
        }
        self.group_offsets.insert((group, to), db);
        self.set_group(group, None, None, Some(delta), to)
    }

    /// Mute, solo et/ou décalage de niveau (en dB, écarts conservés) sur tous les membres. Sans `to` : sur les pistes elles-mêmes ;
    /// avec `to` : sur leurs envois vers ce nœud (les membres sans cet envoi sont ignorés). Hors historique.
    pub(super) fn set_group(&mut self, group: u32, mute: Option<bool>, solo: Option<bool>, delta_db: Option<f64>, to: Option<u32>) -> Vec<Event> {
        if !self.session.effective_group_list(to.map(NodeId)).iter().any(|g| g.id == group) {
            return Self::group_error("set_group", format!("groupe inconnu : {group}"));
        }
        let factor = delta_db.map(|d| 10f64.powf(d / 20.0));
        let mut events = Vec::new();
        for node in self.session.effective_members(group, to.map(NodeId)) {
            let n = node.0;
            match to {
                None => {
                    let gain = factor.and_then(|f| self.nodes.get(n as usize).map(|s| (f64::from(s.gain) * f).clamp(0.0, GAIN_MAX) as f32));
                    if mute.is_some() || solo.is_some() || gain.is_some() {
                        events.extend(self.command(Command::SetNode { node: n, gain, pan: None, mute, solo, arm: None, monitor: None, invert: None }));
                    }
                }
                Some(t) => {
                    let missing = !self.session.graph().nodes[n as usize].sends.iter().any(|s| s.to.0 == t);
                    if missing && (mute == Some(true) || solo == Some(true)) && n != 0 && !self.session.graph().nodes[n as usize].monitor {
                        events.extend(self.command(Command::AddSend { node: n, to: t }));
                        events.extend(self.command(Command::SetSend { node: n, to: t, gain: Some(0.0), pre: None, mute: None, solo: None }));
                    }
                    let Some(send) = self.session.graph().nodes[n as usize].sends.iter().find(|s| s.to.0 == t) else { continue };
                    let gain = factor.map(|f| (f64::from(send.gain) * f).clamp(0.0, GAIN_MAX));
                    events.extend(self.command(Command::SetSend { node: n, to: t, gain, pre: None, mute, solo }));
                }
            }
        }
        // Une seule mise à jour du projet pour toute la salve (la dernière), et les erreurs.
        let last = events.iter().rposition(|e| matches!(e, Event::Project(_)));
        events.into_iter().enumerate().filter(|(i, e)| !matches!(e, Event::Project(_)) || Some(*i) == last).map(|(_, e)| e).collect()
    }
}
