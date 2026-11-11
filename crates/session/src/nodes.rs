//! Pistes et bus : ajouter, retirer, renommer, colorer, rediriger, réordonner — chaque action est un pas d'annulation.
//!
//! Dans le moteur, l'identifiant d'un nœud est sa position dans la liste : retirer un nœud renumérote les suivants et toutes
//! les références (sorties, envois). Les éditions sont donc **exactement inversibles** : un nœud retiré emporte avec lui (dans
//! un `NodeRecord`) tout ce qui lui appartient et ce que son retrait a changé ailleurs (sorties renvoyées vers la sortie
//! principale, envois supprimés), et le remettre rétablit l'état d'avant, numéros compris. L'historique étant strictement
//! empilé (dernier fait, premier défait), les numéros sont toujours ceux que l'édition attend. Les réglages de mixage des
//! autres nœuds, qui ne sont pas dans l'historique, ne sont pas touchés.

use daw_core_types::NodeId;
use daw_engine::{AutoTarget, GraphDesc, InsertDesc, InsertFactory, NodeDesc, NodeKind, SendDesc, TrackFormat};

use crate::music::{MClip, MLane};
use crate::{ClipId, Edit, MidiClipId, Session, SessionError};

/// Ce qu'une piste enregistre et écoute : un ou deux canaux d'entrée du pilote (numérotés **à partir de 0**), ou le MIDI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeInput {
    /// Entrée MIDI (les autres champs ne comptent alors pas, sauf `monitor`).
    pub midi: bool,
    /// Canal d'entrée (mono : `channels[0]`) ou paire (gauche, droite).
    pub channels: [u8; 2],
    pub stereo: bool,
    /// Écoute directe de l'entrée quand la piste est armée (sans passer par la latence du traitement).
    pub monitor: bool,
}

impl Default for NodeInput {
    fn default() -> Self {
        NodeInput { midi: false, channels: [0, 1], stereo: false, monitor: true }
    }
}

/// Un nœud avec tout ce qui lui appartient dans la session, de quoi le retirer puis le remettre à l'identique.
#[derive(Debug, Clone)]
pub struct NodeRecord {
    desc: NodeDesc,
    clip_ids: Vec<ClipId>,
    midi: Vec<(MidiClipId, MClip)>,
    lanes: Vec<MLane>,
    color: String,
    input: NodeInput,
    /// Groupe du nœud (0 : aucun).
    group: u32,
    /// Ses groupes propres s'il est un retour, et son groupe dans le mix de chaque retour qui en a (par numéro de nœud, 0 : aucun).
    ret_groups: Option<crate::groups::GroupState>,
    member_of: Vec<u32>,
    /// Jeton de l'adresse de tablette du nœud (s'il est un retour).
    monitor_token: String,
    /// Rôle de la télécommande du retour (voir `Session::monitor_role`).
    monitor_role: u8,
    /// Effets d'autres nœuds (numéro du nœud, identifiant) dont la chaîne latérale venait de lui : retirée avec lui, rendue avec lui.
    dropped_sidechains: Vec<(u32, daw_engine::InsertId)>,
    /// Ports de sortie auxiliaires d'effets d'autres nœuds qui visaient ce nœud : `(nœud, effet, port)`.
    dropped_aux: Vec<(u32, daw_engine::InsertId, usize)>,
    /// Filtre d'entrée MIDI de la piste.
    midi_filter: Vec<crate::midi_filter::MidiRule>,
    /// Place dans l'ordre d'affichage.
    display_pos: usize,
    /// Nœuds (numéros avec ce nœud présent) dont la sortie pointait vers lui : renvoyés vers la sortie principale au retrait.
    rerouted: Vec<u32>,
    /// Envois vers lui (propriétaire, place dans sa liste, envoi) : supprimés avec lui.
    dropped_sends: Vec<(u32, usize, SendDesc)>,
    /// Courbes de niveau d'envoi vers lui (propriétaire, courbe) : supprimées avec lui, rendues avec lui.
    dropped_lanes: Vec<(u32, MLane)>,
}

/// Ce qu'on peut ajouter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewNode {
    Mono,
    Stereo,
    Bus,
    /// Retour de monitoring : un bus d'un type à part (QR, onglet de mix, groupes propres), muet tant qu'il n'a pas de sortie physique.
    Return,
}

const MAX_NAME_CHARS: usize = 60;

fn shift_up(id: NodeId, at: usize) -> NodeId {
    if id.0 as usize >= at { NodeId(id.0 + 1) } else { id }
}

fn shift_down(id: NodeId, at: usize) -> NodeId {
    if id.0 as usize > at { NodeId(id.0 - 1) } else { id }
}

impl Session {
    pub fn node_color(&self, node: NodeId) -> &str {
        self.colors.get(node.0 as usize).map_or("", String::as_str)
    }

    /// Pistes et bus dans l'ordre d'affichage (sans la sortie principale).
    pub fn display_order(&self) -> &[NodeId] {
        &self.order
    }

    /// Augmente à chaque ajout ou retrait de nœud (annulations comprises) : les numéros de nœuds ont pu changer.
    pub fn structure_version(&self) -> u64 {
        self.structure_version
    }

    pub fn node_input(&self, node: NodeId) -> NodeInput {
        self.inputs.get(node.0 as usize).copied().unwrap_or_default()
    }

    /// Entrée d'une piste **sans historique** (comme un réglage de mixage ; chargement d'un projet). Faux si le nœud n'existe pas.
    pub fn set_node_input_untracked(&mut self, node: NodeId, input: NodeInput) -> bool {
        match self.inputs.get_mut(node.0 as usize) {
            Some(i) => {
                *i = input;
                true
            }
            None => false,
        }
    }

    /// Couleur d'un nœud **sans historique** (chargement d'un projet, démonstration).
    pub fn set_node_color_untracked(&mut self, node: NodeId, color: &str) {
        if let Some(c) = self.colors.get_mut(node.0 as usize) {
            *c = color.to_owned();
        }
    }

    /// Ordre d'affichage **sans historique** (chargement d'un projet). Ignoré s'il n'est pas une permutation des pistes et bus.
    pub fn set_display_order_untracked(&mut self, order: &[NodeId]) {
        let mut want: Vec<NodeId> = (1..self.graph.nodes.len() as u32).map(NodeId).collect();
        let mut sorted = order.to_vec();
        sorted.sort_by_key(|n| n.0);
        want.sort_by_key(|n| n.0);
        if sorted == want {
            self.order = order.to_vec();
        }
    }

    fn node_index(&self, node: NodeId) -> Result<usize, SessionError> {
        let n = node.0 as usize;
        if n >= self.graph.nodes.len() {
            return Err(SessionError::UnknownNode(node));
        }
        if n == 0 {
            return Err(SessionError::MasterNode);
        }
        Ok(n)
    }

    /// Ajoute une piste ou un bus à la fin (dirigé vers la sortie principale).
    pub fn add_node(&mut self, kind: NewNode, name: &str, color: &str) -> Result<NodeId, SessionError> {
        let name = Self::clean_name(name)?;
        let index = self.graph.nodes.len();
        let new_kind = kind;
        let kind = match kind {
            NewNode::Mono => NodeKind::Track(TrackFormat::Mono),
            NewNode::Stereo => NodeKind::Track(TrackFormat::Stereo),
            NewNode::Bus | NewNode::Return => NodeKind::Bus,
        };
        let mut desc = GraphDesc::make_node(NodeId(index as u32), kind, &name);
        desc.monitor = matches!(new_kind, NewNode::Return);
        let record = NodeRecord {
            desc,
            clip_ids: Vec::new(),
            midi: Vec::new(),
            lanes: Vec::new(),
            color: color.to_owned(),
            input: NodeInput::default(),
            group: 0,
            ret_groups: None,
            member_of: Vec::new(),
            monitor_token: String::new(),
            monitor_role: 0,
            dropped_sidechains: Vec::new(),
            dropped_aux: Vec::new(),
            midi_filter: Vec::new(),
            display_pos: self.order.len(),
            rerouted: Vec::new(),
            dropped_sends: Vec::new(),
            dropped_lanes: Vec::new(),
        };
        self.push("Ajouter une piste", Edit::InsertNode { index, record: Box::new(record) })?;
        Ok(NodeId(index as u32))
    }

    /// Retire un nœud avec ses clips, son automation et ses effets. Les nœuds qui y étaient dirigés repartent vers la sortie
    /// principale et ses envois sont supprimés ; annuler remet tout. `keep_state` donne, pour chaque effet retiré, l'usine qui
    /// le recréera (l'hôte y met l'état courant du plugin, pour qu'il revienne tel qu'il était).
    pub fn remove_node(&mut self, node: NodeId, keep_state: &mut dyn FnMut(&InsertDesc) -> InsertFactory) -> Result<(), SessionError> {
        let n = self.node_index(node)?;
        let mut desc = self.graph.nodes[n].clone();
        for ins in &mut desc.inserts {
            ins.factory = keep_state(ins);
        }
        let mut rerouted = Vec::new();
        let mut dropped_sends = Vec::new();
        let mut dropped_lanes = Vec::new();
        for (i, lanes) in self.lanes.iter().enumerate().filter(|(i, _)| *i != n) {
            for l in lanes.iter().filter(|l| l.target == AutoTarget::SendGain { to: node }) {
                dropped_lanes.push((i as u32, l.clone()));
            }
        }
        for (i, other) in self.graph.nodes.iter().enumerate().filter(|(i, _)| *i != n) {
            if other.output == Some(node) {
                rerouted.push(i as u32);
            }
            for (pos, s) in other.sends.iter().enumerate() {
                if s.to == node {
                    dropped_sends.push((i as u32, pos, s.clone()));
                }
            }
        }
        let mut dropped_sidechains = Vec::new();
        for (i, other) in self.graph.nodes.iter().enumerate().filter(|(i, _)| *i != n) {
            for ins in other.inserts.iter().filter(|x| x.sidechain == Some(node)) {
                dropped_sidechains.push((i as u32, ins.id));
            }
        }
        let mut dropped_aux = Vec::new();
        for (i, other) in self.graph.nodes.iter().enumerate().filter(|(i, _)| *i != n) {
            for ins in &other.inserts {
                for (port, t) in ins.aux.iter().enumerate() {
                    if *t == Some(node) {
                        dropped_aux.push((i as u32, ins.id, port));
                    }
                }
            }
        }
        let record = NodeRecord {
            dropped_aux,
            dropped_sidechains,
            desc,
            clip_ids: self.ids[n].clone(),
            midi: self.midi[n].clone(),
            lanes: self.lanes[n].clone(),
            color: self.colors[n].clone(),
            input: self.inputs[n],
            group: self.node_group[n],
            ret_groups: self.ret_groups[n].clone(),
            monitor_token: self.monitor_tokens[n].clone(),
            monitor_role: self.monitor_roles[n],
            midi_filter: self.midi_filters[n].clone(),
            member_of: self.ret_groups.iter().map(|g| g.as_ref().and_then(|g| g.of.get(n).copied()).unwrap_or(0)).collect(),
            display_pos: self.order.iter().position(|&o| o == node).unwrap_or(self.order.len()),
            rerouted,
            dropped_sends,
            dropped_lanes,
        };
        self.push("Supprimer une piste", Edit::RemoveNode { index: n, record: Box::new(record) })
    }

    fn clean_name(name: &str) -> Result<String, SessionError> {
        let t = name.trim();
        if t.is_empty() || t.chars().count() > MAX_NAME_CHARS {
            return Err(SessionError::BadName);
        }
        Ok(t.to_owned())
    }

    pub fn rename_node(&mut self, node: NodeId, name: &str) -> Result<(), SessionError> {
        let n = self.node_index(node)?;
        let new = Self::clean_name(name)?;
        let old = self.graph.nodes[n].name.clone();
        if old == new {
            return Ok(());
        }
        self.push("Renommer une piste", Edit::SetName { node, old, new })
    }

    pub fn set_node_color(&mut self, node: NodeId, color: &str) -> Result<(), SessionError> {
        let n = self.node_index(node)?;
        let old = self.colors[n].clone();
        if old == color {
            return Ok(());
        }
        self.push("Couleur d'une piste", Edit::SetColor { node, old, new: color.to_owned() })
    }

    /// Dirige la sortie d'un nœud vers la sortie principale ou un bus. Refusé si le graphe deviendrait invalide (boucle, etc.).
    pub fn set_node_output(&mut self, node: NodeId, to: NodeId) -> Result<(), SessionError> {
        let n = self.node_index(node)?;
        let old = self.graph.nodes[n].output.unwrap_or(NodeId::MASTER);
        if old == to {
            return Ok(());
        }
        let mut candidate = self.graph.clone();
        candidate.nodes[n].output = Some(to);
        candidate.validate().map_err(|e| SessionError::Routing(e.to_string()))?;
        self.push("Sortie d'une piste", Edit::SetOutput { node, old, new: to })
    }

    /// Remplace la liste des envois de `node` après avoir vérifié que le graphe reste valide (boucles, doublons, destination).
    fn change_sends(&mut self, label: &str, node: NodeId, new: Vec<daw_engine::SendDesc>) -> Result<(), SessionError> {
        let n = self.node_index(node)?;
        let old = self.graph.nodes[n].sends.clone();
        let mut candidate = self.graph.clone();
        candidate.nodes[n].sends = new.clone();
        candidate.validate().map_err(|e| SessionError::Routing(e.to_string()))?;
        self.push(label, Edit::SetSends { node, old, new })
    }

    /// Ajoute un envoi (niveau 1, après le fader) de `node` vers un bus ou la sortie principale, **en plus** de sa sortie.
    pub fn add_send(&mut self, node: NodeId, to: NodeId) -> Result<(), SessionError> {
        let n = self.node_index(node)?;
        let mut sends = self.graph.nodes[n].sends.clone();
        sends.push(daw_engine::SendDesc { to, gain: 1.0, pre: false, mute: false, solo: false });
        self.change_sends("Ajouter un envoi", node, sends)
    }

    pub fn remove_send(&mut self, node: NodeId, to: NodeId) -> Result<(), SessionError> {
        let n = self.node_index(node)?;
        let mut sends = self.graph.nodes[n].sends.clone();
        let before = sends.len();
        sends.retain(|s| s.to != to);
        if sends.len() == before {
            return Err(SessionError::Routing(format!("pas d'envoi vers le nœud {}", to.0)));
        }
        self.change_sends("Supprimer un envoi", node, sends)
    }

    /// Envoi pris avant ou après le fader.
    pub fn set_send_pre(&mut self, node: NodeId, to: NodeId, pre: bool) -> Result<(), SessionError> {
        let n = self.node_index(node)?;
        let mut sends = self.graph.nodes[n].sends.clone();
        let s = sends.iter_mut().find(|s| s.to == to).ok_or_else(|| SessionError::Routing(format!("pas d'envoi vers le nœud {}", to.0)))?;
        if s.pre == pre {
            return Ok(());
        }
        s.pre = pre;
        self.change_sends(if pre { "Envoi avant le fader" } else { "Envoi après le fader" }, node, sends)
    }

    /// Niveau d'un envoi : hors historique (geste continu, comme le fader d'une piste). Rend `false` si l'envoi n'existe pas.
    pub fn set_send_gain_untracked(&mut self, node: NodeId, to: NodeId, gain: f32) -> bool {
        match self.graph.nodes.get_mut(node.0 as usize).and_then(|n| n.sends.iter_mut().find(|s| s.to == to)) {
            Some(s) => {
                s.gain = gain;
                true
            }
            None => false,
        }
    }

    /// Dynamique d'entrée d'une piste (porte + compresseur sur le signal qui entre).
    pub fn input_dynamics(&self, node: NodeId) -> daw_engine::InputDynamics {
        self.graph.nodes.get(node.0 as usize).map(|n| n.input_dyn).unwrap_or_default()
    }

    /// Règle la dynamique d'entrée d'une piste **sans historique** (réglage d'entrée, comme le canal audio ; l'appelant recharge le
    /// graphe). Refusée si le nœud n'est pas une piste ou si une valeur est hors de sa plage.
    pub fn set_input_dynamics_untracked(&mut self, node: NodeId, dynamics: daw_engine::InputDynamics) -> Result<(), SessionError> {
        let i = self.node_index(node)?;
        if !matches!(self.graph.nodes[i].kind, daw_engine::NodeKind::Track(_)) {
            return Err(SessionError::NotATrack(node));
        }
        if !dynamics.is_valid() {
            return Err(SessionError::BadInputDynamics);
        }
        self.graph.nodes[i].input_dyn = dynamics;
        Ok(())
    }

    /// Courbe de réponse de la vélocité d'une piste.
    pub fn velocity_curve(&self, node: NodeId) -> daw_engine::VelocityCurve {
        self.graph.nodes.get(node.0 as usize).map(|n| n.vel_curve).unwrap_or_default()
    }

    /// Règle la courbe de vélocité d'une piste **sans historique** (réglage d'entrée, comme le filtre MIDI ; l'appelant recharge le graphe).
    /// Refusée si le nœud n'est pas une piste ou si une valeur est hors de sa plage.
    pub fn set_velocity_curve_untracked(&mut self, node: NodeId, curve: daw_engine::VelocityCurve) -> Result<(), SessionError> {
        let i = self.node_index(node)?;
        if !matches!(self.graph.nodes[i].kind, daw_engine::NodeKind::Track(_)) {
            return Err(SessionError::NotATrack(node));
        }
        if !curve.is_valid() {
            return Err(SessionError::BadVelocityCurve);
        }
        self.graph.nodes[i].vel_curve = curve;
        Ok(())
    }

    /// Filtre d'entrée MIDI d'une piste (liste vide : tout accepter).
    pub fn midi_filter(&self, node: NodeId) -> &[crate::midi_filter::MidiRule] {
        self.midi_filters.get(node.0 as usize).map_or(&[], Vec::as_slice)
    }

    /// Remplace le filtre d'entrée MIDI d'une piste **sans historique** (réglage d'entrée, comme le canal audio). Refusé si le nœud n'est
    /// pas une piste ou si les règles sont invalides.
    pub fn set_midi_filter_untracked(&mut self, node: NodeId, rules: Vec<crate::midi_filter::MidiRule>) -> Result<(), SessionError> {
        let i = self.node_index(node)?;
        if !matches!(self.graph.nodes[i].kind, daw_engine::NodeKind::Track(_)) {
            return Err(SessionError::NotATrack(node));
        }
        crate::midi_filter::validate(&rules).map_err(SessionError::BadMidiFilter)?;
        self.midi_filters[i] = rules;
        Ok(())
    }

    /// Jeton secret de l'adresse de tablette d'un retour (vide : pas encore donné).
    pub fn monitor_token(&self, node: NodeId) -> &str {
        self.monitor_tokens.get(node.0 as usize).map_or("", String::as_str)
    }

    /// Donne (ou retire, avec une chaîne vide) le jeton de l'adresse d'un retour **sans historique** : ce n'est pas une édition du morceau.
    pub fn set_monitor_token_untracked(&mut self, node: NodeId, token: &str) -> bool {
        match self.monitor_tokens.get_mut(node.0 as usize) {
            Some(t) if node.0 != 0 => {
                *t = token.to_owned();
                true
            }
            _ => false,
        }
    }

    /// Droits de l'adresse d'un retour (bits) : 0 = règle son retour seulement ; 1 = + télécommande (transport, sections, pré-roll) ; 2 = + régie (mixage de toutes les pistes) ; 3 = les deux.
    pub fn monitor_role(&self, node: NodeId) -> u8 {
        self.monitor_roles.get(node.0 as usize).copied().unwrap_or(0)
    }

    /// Change les droits de l'adresse d'un retour **sans historique** (réglage d'accès, pas une édition du morceau). Refuse un rôle au-delà de 3.
    pub fn set_monitor_role_untracked(&mut self, node: NodeId, role: u8) -> bool {
        match self.monitor_roles.get_mut(node.0 as usize) {
            Some(r) if node.0 != 0 && role <= 7 => {
                *r = role;
                true
            }
            _ => false,
        }
    }

    /// Coupe / solo d'un envoi **sans historique** (comme le niveau d'un envoi). Rend `false` si l'envoi n'existe pas.
    pub fn set_send_flags_untracked(&mut self, node: NodeId, to: NodeId, mute: Option<bool>, solo: Option<bool>) -> bool {
        match self.graph.nodes.get_mut(node.0 as usize).and_then(|n| n.sends.iter_mut().find(|s| s.to == to)) {
            Some(s) => {
                if let Some(m) = mute {
                    s.mute = m;
                }
                if let Some(o) = solo {
                    s.solo = o;
                }
                true
            }
            None => false,
        }
    }

    /// Place `node` juste avant `before` dans l'ordre d'affichage (à la fin si `before` est `None`).
    pub fn move_node(&mut self, node: NodeId, before: Option<NodeId>) -> Result<(), SessionError> {
        self.node_index(node)?;
        let mut new = self.order.clone();
        new.retain(|&o| o != node);
        let at = match before {
            None => new.len(),
            Some(b) if b == node => return Ok(()),
            Some(b) => new.iter().position(|&o| o == b).ok_or(SessionError::UnknownNode(b))?,
        };
        new.insert(at, node);
        if new == self.order {
            return Ok(());
        }
        let old = self.order.clone();
        self.push("Déplacer une piste", Edit::SetOrder { old, new })
    }

    pub(crate) fn apply_node_edit(&mut self, e: &Edit) -> Result<(), SessionError> {
        match e {
            Edit::SetName { node, new, .. } => {
                self.graph.nodes.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(*node))?.name = new.clone();
            }
            Edit::SetColor { node, new, .. } => {
                *self.colors.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(*node))? = new.clone();
            }
            Edit::SetOutput { node, new, .. } => {
                self.graph.nodes.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(*node))?.output = Some(*new);
            }
            Edit::SetSends { node, new, .. } => {
                self.graph.nodes.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(*node))?.sends = new.clone();
                self.sync_node(node.0 as usize)?; // une courbe de niveau d'envoi dont l'envoi a disparu (ou est revenu) suit
            }
            Edit::SetOrder { new, .. } => self.order = new.clone(),
            Edit::SetPhys { node, new, .. } => self.graph.nodes.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(*node))?.phys = *new,
            Edit::InsertNode { index, record } => self.insert_node(*index, record)?,
            Edit::RemoveNode { index, .. } => self.remove_node_at(*index)?,
            _ => {}
        }
        Ok(())
    }

    fn insert_node(&mut self, at: usize, r: &NodeRecord) -> Result<(), SessionError> {
        if at == 0 || at > self.graph.nodes.len() {
            return Err(SessionError::UnknownNode(NodeId(at as u32)));
        }
        for n in &mut self.graph.nodes {
            n.output = n.output.map(|o| shift_up(o, at));
            for s in &mut n.sends {
                s.to = shift_up(s.to, at);
            }
            for i in &mut n.inserts {
                i.sidechain = i.sidechain.map(|s| shift_up(s, at));
                for a in &mut i.aux {
                    *a = a.map(|t| shift_up(t, at));
                }
            }
        }
        for lanes in &mut self.lanes {
            for l in lanes {
                if let AutoTarget::SendGain { to } = &mut l.target {
                    *to = shift_up(*to, at);
                }
            }
        }
        self.graph.nodes.insert(at, r.desc.clone());
        for (i, n) in self.graph.nodes.iter_mut().enumerate() {
            n.id = NodeId(i as u32);
        }
        for &o in &r.rerouted {
            if let Some(n) = self.graph.nodes.get_mut(o as usize) {
                n.output = Some(NodeId(at as u32));
            }
        }
        for (o, id) in &r.dropped_sidechains {
            if let Some(ins) = self.graph.nodes.get_mut(*o as usize).and_then(|n| n.inserts.iter_mut().find(|i| i.id == *id)) {
                ins.sidechain = Some(NodeId(at as u32));
            }
        }
        for (o, id, port) in &r.dropped_aux {
            if let Some(ins) = self.graph.nodes.get_mut(*o as usize).and_then(|n| n.inserts.iter_mut().find(|i| i.id == *id)) {
                if let Some(slot) = ins.aux.get_mut(*port) {
                    *slot = Some(NodeId(at as u32));
                }
            }
        }
        for (o, pos, s) in &r.dropped_sends {
            if let Some(n) = self.graph.nodes.get_mut(*o as usize) {
                let p = (*pos).min(n.sends.len());
                n.sends.insert(p, s.clone());
            }
        }
        self.ids.insert(at, r.clip_ids.clone());
        self.midi.insert(at, r.midi.clone());
        self.lanes.insert(at, r.lanes.clone());
        for (o, l) in &r.dropped_lanes {
            if let Some(list) = self.lanes.get_mut(*o as usize) {
                list.push(l.clone());
            }
        }
        self.colors.insert(at, r.color.clone());
        self.inputs.insert(at, r.input);
        self.node_group.insert(at, r.group);
        self.ret_groups.insert(at, r.ret_groups.clone());
        self.monitor_tokens.insert(at, r.monitor_token.clone());
        self.monitor_roles.insert(at, r.monitor_role);
        self.midi_filters.insert(at, r.midi_filter.clone());
        for (j, slot) in self.ret_groups.iter_mut().enumerate() {
            if j == at {
                continue; // ses propres groupes reviennent tels qu'ils étaient (de la bonne taille)
            }
            if let Some(g) = slot {
                g.of.insert(at, r.member_of.get(j).copied().unwrap_or(0));
            }
        }
        for o in &mut self.order {
            *o = shift_up(*o, at);
        }
        let p = r.display_pos.min(self.order.len());
        self.order.insert(p, NodeId(at as u32));
        self.structure_version += 1;
        self.sync_all()
    }

    fn remove_node_at(&mut self, at: usize) -> Result<(), SessionError> {
        if at == 0 || at >= self.graph.nodes.len() {
            return Err(SessionError::UnknownNode(NodeId(at as u32)));
        }
        let gone = NodeId(at as u32);
        for (i, n) in self.graph.nodes.iter_mut().enumerate() {
            if i == at {
                continue;
            }
            if n.output == Some(gone) {
                n.output = Some(NodeId::MASTER);
            }
            n.sends.retain(|s| s.to != gone);
            for ins in &mut n.inserts {
                if ins.sidechain == Some(gone) {
                    ins.sidechain = None;
                }
                for a in &mut ins.aux {
                    if *a == Some(gone) {
                        *a = None;
                    }
                }
            }
        }
        for lanes in &mut self.lanes {
            lanes.retain(|l| l.target != AutoTarget::SendGain { to: gone });
            for l in lanes {
                if let AutoTarget::SendGain { to } = &mut l.target {
                    *to = shift_down(*to, at);
                }
            }
        }
        self.graph.nodes.remove(at);
        for (i, n) in self.graph.nodes.iter_mut().enumerate() {
            n.id = NodeId(i as u32);
            n.output = n.output.map(|o| shift_down(o, at));
            for s in &mut n.sends {
                s.to = shift_down(s.to, at);
            }
            for i in &mut n.inserts {
                i.sidechain = i.sidechain.map(|s| shift_down(s, at));
                for a in &mut i.aux {
                    *a = a.map(|t| shift_down(t, at));
                }
            }
        }
        self.ids.remove(at);
        self.midi.remove(at);
        self.lanes.remove(at);
        self.colors.remove(at);
        self.inputs.remove(at);
        self.node_group.remove(at);
        self.ret_groups.remove(at);
        self.monitor_tokens.remove(at);
        self.monitor_roles.remove(at);
        self.midi_filters.remove(at);
        for g in self.ret_groups.iter_mut().flatten() {
            if at < g.of.len() {
                g.of.remove(at);
            }
        }
        self.order.retain(|&o| o != gone);
        for o in &mut self.order {
            *o = shift_down(*o, at);
        }
        self.structure_version += 1;
        self.sync_all() // les courbes de niveau d'envoi des autres nœuds ont été renumérotées ou retirées
    }
}

// ---- retours de monitoring : sortie physique d'un bus ------------------------------------------------------------------------------

impl Session {
    /// Donne (ou retire) une sortie physique à un bus : il sort sur la carte son et n'alimente plus la sortie principale. Une action
    /// annulable ; refusée sans trace si le graphe la refuse (pas un bus, canaux 0 et 1, canal déjà pris...).
    pub fn set_node_phys(&mut self, node: NodeId, phys: Option<daw_engine::PhysOut>) -> Result<(), SessionError> {
        let old = self.graph.nodes.get(node.0 as usize).ok_or(SessionError::UnknownNode(node))?.phys;
        if old == phys {
            return Ok(());
        }
        // Essai sur le graphe lui-même, défait en cas de refus : la validation du moteur fait foi.
        self.graph.nodes[node.0 as usize].phys = phys;
        let valid = self.graph.validate();
        self.graph.nodes[node.0 as usize].phys = old;
        valid.map_err(|_| SessionError::BadPhysOut(node))?;
        let label = match (self.graph.nodes[node.0 as usize].monitor, phys.is_some()) {
            (true, true) => "Sortie d'un retour",
            (true, false) => "Retirer la sortie d'un retour",
            (false, true) => "Sortie d'un bus",
            (false, false) => "Retirer la sortie d'un bus",
        };
        self.push(label, Edit::SetPhys { node, old, new: phys })
    }
}
