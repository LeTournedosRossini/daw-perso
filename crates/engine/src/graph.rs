//! Description du graphe audio (côté contrôle) et sa validation.
//!
//! `GraphDesc` est une structure ordinaire, modifiable et clonable : c'est ce que l'utilisateur (ou
//! le fichier de projet) édite. `compile` la vérifie puis fabrique la version prête pour le thread audio
//! (tampons pré-alloués, ordre de calcul fixé). Cette étape peut allouer ; elle n'a jamais lieu dans le callback.

use daw_core_types::NodeId;

use crate::audio::Clip;
use crate::automation::{AutoLane, AutoTarget};
use crate::compiled::{AheadParams, CompiledGraph};
use crate::insert::{InsertDesc, InsertFactory, InsertId};
use crate::midi::MidiClip;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackFormat {
    /// Source mono, panoramique à puissance constante.
    Mono,
    /// Source stéréo, balance.
    Stereo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Track(TrackFormat),
    Bus,
    Master,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeParams {
    /// Gain linéaire (1,0 = 0 dB).
    pub gain: f32,
    /// Panoramique ou balance, de -1 (gauche) à 1 (droite).
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    /// Polarité inversée (le signal est multiplié par −1) : pour vérifier un alignement (une prise rejouée en opposition de
    /// phase s'annule contre l'original si elle est parfaitement synchrone) ou corriger une prise de micro à l'envers.
    pub invert: bool,
}

impl Default for NodeParams {
    fn default() -> Self {
        NodeParams { gain: 1.0, pan: 0.0, mute: false, solo: false, invert: false }
    }
}

/// Envoi vers un bus **ou la sortie principale**, en plus de la sortie du nœud. `pre` : pris **avant** le fader (gain, panoramique,
/// mute et solo de la piste ne le touchent pas : retours casque, mix de monitoring) ; sinon après le fader.
#[derive(Debug, Clone, PartialEq)]
pub struct SendDesc {
    pub to: NodeId,
    pub gain: f32,
    pub pre: bool,
    /// Envoi coupé (n'agit que sur cette destination : le reste du mixage ne bouge pas).
    pub mute: bool,
    /// Solo dans la destination : si au moins un envoi vers elle est en solo, seuls les envois en solo y sont entendus.
    pub solo: bool,
}

/// Sortie physique d'un bus : au lieu d'alimenter la sortie principale, le bus sort sur des canaux de la carte son (retour de
/// monitoring d'un musicien). Les canaux 0 et 1 sont ceux de la sortie principale : un bus ne peut pas les prendre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysOut {
    /// Premier canal (à partir de 0) ; au moins 2.
    pub first: u8,
    /// Deux canaux (`first` et `first + 1`) ou un seul.
    pub stereo: bool,
    /// Somme mono : (gauche + droite) / 2, sur le canal seul ou sur les deux canaux. Un canal seul est toujours en somme mono.
    pub mono: bool,
}

impl PhysOut {
    /// Nombre de canaux pris.
    pub fn channels(&self) -> u8 {
        if self.stereo { 2 } else { 1 }
    }
}

/// Plus grand nombre de canaux de sortie pris en charge (sortie principale comprise).
pub const MAX_OUT_CHANNELS: usize = 32;

#[derive(Debug, Clone)]
pub struct NodeDesc {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,
    pub params: NodeParams,
    /// Destination principale (un bus ou la sortie principale) ; `None` pour la sortie principale seule.
    pub output: Option<NodeId>,
    /// Sortie physique (bus seulement) : le bus sort sur la carte son et **n'alimente plus** sa destination principale (ses envois
    /// restent possibles). `None` : comme avant.
    pub phys: Option<PhysOut>,
    pub sends: Vec<SendDesc>,
    /// Clips (pistes seulement).
    pub clips: Vec<Clip>,
    /// Clips de notes (pistes seulement) : leurs notes sont envoyées aux effets insérés qui en acceptent.
    pub midi_clips: Vec<MidiClip>,
    /// Effets insérés, dans l'ordre de traitement (avant le fader).
    pub inserts: Vec<InsertDesc>,
    /// Courbes d'automation (au plus une par cible).
    pub automation: Vec<AutoLane>,
    /// Dynamique d'entrée (pistes seulement) : porte et compresseur sur le signal qui entre, avant l'écoute et, au choix, la prise.
    pub input_dyn: crate::dynamics::InputDynamics,
    /// Courbe de réponse de la vélocité des notes (pistes seulement) : appliquée entre les notes (en direct ou des clips) et les plugins.
    pub vel_curve: crate::velocity::VelocityCurve,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GraphError {
    /// Le nœud 0 n'est pas la sortie principale, ou il y en a plusieurs.
    BadMaster,
    UnknownNode(NodeId),
    /// La destination d'un nœud n'est ni un bus ni la sortie principale.
    OutputNotBus { node: NodeId, to: NodeId },
    /// Un envoi doit viser un bus.
    SendNotBus { node: NodeId, to: NodeId },
    /// Sortie physique invalide : pas un bus, canal 0 ou 1 (sortie principale), au-delà de `MAX_OUT_CHANNELS`, ou canal déjà pris par un autre bus.
    BadPhysOut(NodeId),
    SelfRouting(NodeId),
    DuplicateSend { node: NodeId, to: NodeId },
    /// La chaîne latérale d'un effet vise la sortie principale ou son propre nœud.
    BadSidechain { node: NodeId, insert: InsertId },
    /// Les nœuds listés forment (ou dépendent de) une boucle.
    Cycle(Vec<NodeId>),
    SampleRateMismatch { node: NodeId, clip_rate: u32, graph_rate: u32 },
    ClipOutOfRange { node: NodeId },
    ClipOnNonTrack(NodeId),
    /// Note invalide (touche > 127, canal > 15, durée nulle, vélocité hors de ]0, 1], début hors du clip).
    BadNote(NodeId),
    NonFiniteParam(NodeId),
    /// Courbe d'automation invalide (sans point, temps non croissants, valeur non finie ou négative pour un gain,
    /// cible inexistante ou en double).
    BadAutomation(NodeId),
    InvalidConfig,
    /// L'usine de l'effet a échoué à fabriquer une instance.
    InsertFailed { node: NodeId, insert: InsertId, reason: String },
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for GraphError {}

#[derive(Debug, Clone)]
pub struct GraphDesc {
    pub nodes: Vec<NodeDesc>,
    /// Compensation de latence des plugins (PDC) : activée par défaut. Désactivée, chaque chemin garde sa propre latence.
    pub pdc: bool,
    /// Carte des tempos et des chiffrages en échantillons, donnée aux plugins par le transport. Remplie par la session ; par défaut
    /// 120 noires par minute en 4/4.
    pub time_map: std::sync::Arc<crate::TimeMap>,
    /// Piste d'accords donnée aux plugins (extension d'hôte) : instantané immuable fabriqué par la session. Vide par défaut.
    pub chords: std::sync::Arc<crate::chords::ChordSnapshot>,
    next_insert: InsertId,
}

impl Default for GraphDesc {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeDesc {
    /// La destination principale **qui compte pour le routage** : aucune pour un bus qui sort sur la carte son.
    pub fn main_output(&self) -> Option<NodeId> {
        if self.phys.is_some() { None } else { self.output }
    }
}

impl GraphDesc {
    /// Graphe ne contenant que la sortie principale (nœud 0).
    pub fn new() -> Self {
        GraphDesc {
            nodes: vec![NodeDesc {
                id: NodeId::MASTER,
                kind: NodeKind::Master,
                name: "Master".to_owned(),
                params: NodeParams::default(),
                output: None,
                phys: None,
                sends: Vec::new(),
                clips: Vec::new(),
                midi_clips: Vec::new(),
                inserts: Vec::new(),
                automation: Vec::new(),
                input_dyn: Default::default(),
                vel_curve: Default::default(),
            }],
            pdc: true,
            time_map: Default::default(),
            chords: Default::default(),
            next_insert: 0,
        }
    }

    /// Un nœud neuf (piste ou bus) dirigé vers la sortie principale, sans clip ni effet.
    pub fn make_node(id: NodeId, kind: NodeKind, name: &str) -> NodeDesc {
        NodeDesc {
            id,
            kind,
            name: name.to_owned(),
            params: NodeParams::default(),
            output: Some(NodeId::MASTER),
            phys: None,
            sends: Vec::new(),
            clips: Vec::new(),
            midi_clips: Vec::new(),
            inserts: Vec::new(),
            automation: Vec::new(),
            input_dyn: Default::default(),
            vel_curve: Default::default(),
        }
    }

    fn push(&mut self, kind: NodeKind, name: &str) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(GraphDesc::make_node(id, kind, name));
        id
    }

    /// Nouvelle piste, dirigée vers la sortie principale.
    pub fn add_track(&mut self, name: &str, format: TrackFormat) -> NodeId {
        self.push(NodeKind::Track(format), name)
    }

    /// Nouveau bus, dirigé vers la sortie principale.
    pub fn add_bus(&mut self, name: &str) -> NodeId {
        self.push(NodeKind::Bus, name)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut NodeDesc> {
        self.nodes.get_mut(id.0 as usize)
    }

    pub fn set_output(&mut self, node: NodeId, to: NodeId) {
        if let Some(n) = self.node_mut(node) {
            n.output = Some(to);
        }
    }

    pub fn add_send(&mut self, node: NodeId, to: NodeId, gain: f32) {
        if let Some(n) = self.node_mut(node) {
            n.sends.push(SendDesc { to, gain, pre: false, mute: false, solo: false });
        }
    }

    pub fn add_clip(&mut self, track: NodeId, clip: Clip) {
        if let Some(n) = self.node_mut(track) {
            n.clips.push(clip);
        }
    }

    pub fn add_midi_clip(&mut self, track: NodeId, clip: MidiClip) {
        if let Some(n) = self.node_mut(track) {
            n.midi_clips.push(clip);
        }
    }

    /// Ajoute un effet à la fin de la chaîne d'inserts du nœud ; retourne son identifiant stable.
    pub fn add_insert(&mut self, node: NodeId, name: &str, factory: InsertFactory) -> Option<InsertId> {
        let id = self.next_insert;
        let n = self.nodes.get_mut(node.0 as usize)?;
        n.inserts.push(InsertDesc { id, name: name.to_owned(), factory, bypass: false, sidechain: None });
        self.next_insert += 1;
        Some(id)
    }

    /// Un effet neuf avec un identifiant frais (jamais réutilisé), à placer dans un nœud par l'appelant.
    pub fn alloc_insert(&mut self, name: &str, factory: InsertFactory) -> InsertDesc {
        let id = self.next_insert;
        self.next_insert += 1;
        InsertDesc { id, name: name.to_owned(), factory, bypass: false, sidechain: None }
    }

    /// Pose une courbe d'automation sur le nœud (remplace celle qui vise la même cible).
    pub fn set_automation(&mut self, node: NodeId, lane: AutoLane) {
        if let Some(n) = self.node_mut(node) {
            match n.automation.iter_mut().find(|l| l.target == lane.target) {
                Some(l) => *l = lane,
                None => n.automation.push(lane),
            }
        }
    }

    pub fn set_params(&mut self, node: NodeId, params: NodeParams) {
        if let Some(n) = self.node_mut(node) {
            n.params = params;
        }
    }

    fn kind_of(&self, id: NodeId) -> Result<NodeKind, GraphError> {
        self.nodes.get(id.0 as usize).map(|n| n.kind).ok_or(GraphError::UnknownNode(id))
    }

    /// Vérifie la structure : un seul nœud principal en position 0, destinations valides, pas de boucle.
    /// Retourne l'ordre de calcul (sources d'abord ; à égalité, plus petit identifiant d'abord, pour que
    /// l'ordre de sommation, donc le résultat au bit près, soit toujours le même).
    /// Pour chaque nœud, ceux qu'il alimente : sa sortie, ses envois, et les nœuds dont un effet l'a pour **chaîne latérale**. C'est ce qui
    /// fixe l'ordre de calcul, les niveaux et la compensation de latence.
    pub(crate) fn out_edges(&self) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = self.nodes.iter().map(|n| n.main_output().into_iter().chain(n.sends.iter().map(|s| s.to)).map(|t| t.0 as usize).collect()).collect();
        for (d, node) in self.nodes.iter().enumerate() {
            for src in node.inserts.iter().filter_map(|i| i.sidechain) {
                if let Some(list) = out.get_mut(src.0 as usize) {
                    list.push(d);
                }
            }
        }
        out
    }

    pub fn validate(&self) -> Result<Vec<usize>, GraphError> {
        let n = self.nodes.len();
        if n == 0 || self.nodes[0].kind != NodeKind::Master {
            return Err(GraphError::BadMaster);
        }
        for (i, node) in self.nodes.iter().enumerate() {
            if node.id.0 as usize != i || (i > 0 && node.kind == NodeKind::Master) {
                return Err(GraphError::BadMaster);
            }
            let p = &node.params;
            if !p.gain.is_finite() || !p.pan.is_finite() || node.sends.iter().any(|s| !s.gain.is_finite()) {
                return Err(GraphError::NonFiniteParam(node.id));
            }
            match (node.kind, node.output) {
                (NodeKind::Master, None) => {}
                (NodeKind::Master, Some(_)) => return Err(GraphError::BadMaster),
                (_, None) => return Err(GraphError::UnknownNode(node.id)),
                (_, Some(to)) => {
                    if to == node.id {
                        return Err(GraphError::SelfRouting(node.id));
                    }
                    match self.kind_of(to)? {
                        NodeKind::Bus | NodeKind::Master => {}
                        NodeKind::Track(_) => return Err(GraphError::OutputNotBus { node: node.id, to }),
                    }
                }
            }
            if let Some(p) = node.phys {
                let last = usize::from(p.first) + usize::from(p.channels());
                let taken = self.nodes.iter().any(|o| o.id != node.id && o.phys.is_some_and(|q| p.first < q.first + q.channels() && q.first < p.first + p.channels()));
                if node.kind != NodeKind::Bus || p.first < 2 || last > MAX_OUT_CHANNELS || taken {
                    return Err(GraphError::BadPhysOut(node.id));
                }
            }
            if node.kind == NodeKind::Master && !node.sends.is_empty() {
                return Err(GraphError::SendNotBus { node: node.id, to: node.id });
            }
            for (k, s) in node.sends.iter().enumerate() {
                if s.to == node.id {
                    return Err(GraphError::SelfRouting(node.id));
                }
                if !matches!(self.kind_of(s.to)?, NodeKind::Bus | NodeKind::Master) {
                    return Err(GraphError::SendNotBus { node: node.id, to: s.to });
                }
                if node.sends[..k].iter().any(|o| o.to == s.to) {
                    return Err(GraphError::DuplicateSend { node: node.id, to: s.to });
                }
            }
            if (!node.clips.is_empty() || !node.midi_clips.is_empty()) && !matches!(node.kind, NodeKind::Track(_)) {
                return Err(GraphError::ClipOnNonTrack(node.id));
            }
            for ins in &node.inserts {
                if let Some(src) = ins.sidechain {
                    // La source d'une chaîne latérale est une piste ou un bus, pas la sortie principale ni le nœud lui-même.
                    if src == node.id || matches!(self.kind_of(src)?, NodeKind::Master) {
                        return Err(GraphError::BadSidechain { node: node.id, insert: ins.id });
                    }
                }
            }
        }

        // Tri topologique (Kahn), plus petit identifiant d'abord.
        let edges = self.out_edges();
        let mut indegree = vec![0usize; n];
        for list in &edges {
            for &to in list {
                indegree[to] += 1;
            }
        }
        let mut done = vec![false; n];
        let mut order = Vec::with_capacity(n);
        while let Some(next) = (0..n).find(|&i| !done[i] && indegree[i] == 0) {
            done[next] = true;
            order.push(next);
            for &to in &edges[next] {
                indegree[to] -= 1;
            }
        }
        if order.len() < n {
            return Err(GraphError::Cycle((0..n).filter(|&i| !done[i]).map(|i| NodeId(i as u32)).collect()));
        }
        Ok(order)
    }

    /// Fabrique le graphe exécutable, tous les effets étant neufs. Alloue ; à appeler hors du thread audio.
    pub fn compile(&self, sample_rate: u32, max_block: usize) -> Result<CompiledGraph, GraphError> {
        self.compile_with(sample_rate, max_block, &|_| None, &|_| None)
    }

    /// Comme `compile`, mais `live(id)` indique la latence d'un effet déjà vivant côté audio (il sera adopté depuis le
    /// graphe précédent au moment de l'échange, sans nouvelle instance).
    pub fn compile_with(
        &self,
        sample_rate: u32,
        max_block: usize,
        live: &dyn Fn(InsertId) -> Option<u32>,
        restore: &dyn Fn(InsertId) -> Option<Vec<u8>>,
    ) -> Result<CompiledGraph, GraphError> {
        self.compile_with_ahead(sample_rate, max_block, live, restore, None)
    }

    /// Comme `compile_with`, en préparant les jumelles des pistes pour le calcul en avance (voir `AheadSystem`).
    pub fn compile_with_ahead(
        &self,
        sample_rate: u32,
        max_block: usize,
        live: &dyn Fn(InsertId) -> Option<u32>,
        restore: &dyn Fn(InsertId) -> Option<Vec<u8>>,
        ahead: Option<AheadParams>,
    ) -> Result<CompiledGraph, GraphError> {
        if sample_rate == 0 || max_block == 0 {
            return Err(GraphError::InvalidConfig);
        }
        let order = self.validate()?;
        for node in &self.nodes {
            for c in &node.clips {
                if c.data.sample_rate() != sample_rate {
                    return Err(GraphError::SampleRateMismatch { node: node.id, clip_rate: c.data.sample_rate(), graph_rate: sample_rate });
                }
                let in_data = c.offset.checked_add(c.len).is_some_and(|e| e <= c.data.len());
                if !in_data || c.start.checked_add(c.len).is_none() || !c.gain.is_finite() || !c.fades_valid() {
                    return Err(GraphError::ClipOutOfRange { node: node.id });
                }
            }
            if node.midi_clips.iter().any(|m| !m.valid()) {
                return Err(GraphError::BadNote(node.id));
            }
            for (k, lane) in node.automation.iter().enumerate() {
                let target_exists = match lane.target {
                    AutoTarget::InsertParam { insert, .. } => node.inserts.iter().any(|i| i.id == insert),
                    AutoTarget::SendGain { to } => node.sends.iter().any(|s| s.to == to),
                    _ => true,
                };
                if !lane.valid() || !target_exists || node.automation[..k].iter().any(|l| l.target == lane.target) {
                    return Err(GraphError::BadAutomation(node.id));
                }
            }
        }
        CompiledGraph::build(self, order, sample_rate, max_block, live, restore, ahead)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioData;

    fn ids(v: &[usize]) -> Vec<u32> {
        v.iter().map(|&i| i as u32).collect()
    }

    #[test]
    fn empty_graph_is_valid() {
        assert_eq!(GraphDesc::new().validate().unwrap(), vec![0]);
    }

    #[test]
    fn order_puts_sources_first_and_master_last() {
        let mut g = GraphDesc::new();
        let t1 = g.add_track("t1", TrackFormat::Mono); // 1
        let t2 = g.add_track("t2", TrackFormat::Stereo); // 2
        let b = g.add_bus("b"); // 3
        g.set_output(t1, b);
        g.add_send(t2, b, 0.5);
        assert_eq!(ids(&g.validate().unwrap()), vec![1, 2, 3, 0]);
    }

    #[test]
    fn order_is_deterministic_by_id_when_independent() {
        let mut g = GraphDesc::new();
        let _ = (g.add_bus("a"), g.add_track("t", TrackFormat::Mono), g.add_bus("c"));
        assert_eq!(ids(&g.validate().unwrap()), vec![1, 2, 3, 0]);
    }

    #[test]
    fn detects_cycle_between_buses() {
        let mut g = GraphDesc::new();
        let a = g.add_bus("a");
        let b = g.add_bus("b");
        g.set_output(a, b);
        g.set_output(b, a);
        match g.validate() {
            Err(GraphError::Cycle(ids)) => assert_eq!(ids, vec![a, b]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn detects_cycle_through_a_send() {
        let mut g = GraphDesc::new();
        let t = g.add_track("t", TrackFormat::Mono);
        let a = g.add_bus("a");
        let b = g.add_bus("b");
        g.set_output(t, a);
        g.set_output(a, b);
        g.add_send(b, a, 0.3); // b -> a -> b
        assert!(matches!(g.validate(), Err(GraphError::Cycle(_))));
    }

    #[test]
    fn rejects_bad_routing() {
        let mut g = GraphDesc::new();
        let t = g.add_track("t", TrackFormat::Mono);
        let u = g.add_track("u", TrackFormat::Mono);
        let b = g.add_bus("b");
        let mut h = g.clone();
        h.set_output(t, u);
        assert_eq!(h.validate(), Err(GraphError::OutputNotBus { node: t, to: u }));
        let mut h = g.clone();
        h.set_output(b, b);
        assert_eq!(h.validate(), Err(GraphError::SelfRouting(b)));
        let mut h = g.clone();
        h.add_send(t, u, 1.0);
        assert_eq!(h.validate(), Err(GraphError::SendNotBus { node: t, to: u }));
        let mut h = g.clone();
        h.add_send(t, NodeId::MASTER, 1.0); // la sortie principale est une destination d'envoi valide
        assert!(h.validate().is_ok());
        let mut h = g.clone();
        h.add_send(t, b, 1.0);
        h.add_send(t, b, 0.5);
        assert_eq!(h.validate(), Err(GraphError::DuplicateSend { node: t, to: b }));
        let mut h = g.clone();
        h.set_output(t, NodeId(99));
        assert_eq!(h.validate(), Err(GraphError::UnknownNode(NodeId(99))));
        let mut h = g.clone();
        h.node_mut(t).unwrap().params.gain = f32::NAN;
        assert_eq!(h.validate(), Err(GraphError::NonFiniteParam(t)));
    }

    #[test]
    fn rejects_clip_problems() {
        let data = AudioData::new(48_000, vec![vec![0.0; 100]]).unwrap();
        let mut g = GraphDesc::new();
        let t = g.add_track("t", TrackFormat::Mono);
        g.add_clip(t, Clip::whole(data.clone(), 0));
        assert!(g.compile(48_000, 64).is_ok());
        assert!(matches!(g.compile(44_100, 64), Err(GraphError::SampleRateMismatch { .. })));
        let mut h = GraphDesc::new();
        let t = h.add_track("t", TrackFormat::Mono);
        h.add_clip(t, Clip::new(data, 0, 50, 51, 1.0));
        assert_eq!(h.compile(48_000, 64).err(), Some(GraphError::ClipOutOfRange { node: t }));
        let mut h = GraphDesc::new();
        let b = h.add_bus("b");
        h.node_mut(b).unwrap().clips.push(Clip::whole(AudioData::new(48_000, vec![vec![0.0; 4]]).unwrap(), 0));
        assert_eq!(h.compile(48_000, 64).err(), Some(GraphError::ClipOnNonTrack(b)));
    }

    #[test]
    fn rejects_invalid_config() {
        assert_eq!(GraphDesc::new().compile(0, 64).err(), Some(GraphError::InvalidConfig));
        assert_eq!(GraphDesc::new().compile(48_000, 0).err(), Some(GraphError::InvalidConfig));
    }
}
