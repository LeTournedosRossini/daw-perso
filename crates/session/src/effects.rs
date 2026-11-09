//! Chaîne d'effets d'un nœud : ajouter, retirer, réordonner (annulables) et contourner (hors historique, comme le mixage).
//!
//! Un effet retiré emporte son descriptif : l'identifiant est conservé, et l'usine peut être remplacée par une usine qui recrée
//! le plugin avec son état (l'hôte la fournit). L'annuler remet donc l'effet à sa place, tel qu'il était.

use daw_core_types::NodeId;
use daw_engine::{InsertDesc, InsertFactory, InsertId};

use crate::{Edit, Session, SessionError};

impl Session {
    /// Nœud (numéro) et place dans sa chaîne de l'effet `insert`.
    pub fn locate_insert(&self, insert: InsertId) -> Option<(usize, usize)> {
        self.graph.nodes.iter().enumerate().find_map(|(n, node)| node.inserts.iter().position(|i| i.id == insert).map(|k| (n, k)))
    }

    /// Ajoute un effet à la fin de la chaîne de `node` (annulable). Retourne son identifiant, jamais réutilisé.
    pub fn add_effect(&mut self, node: NodeId, name: &str, factory: InsertFactory) -> Result<InsertId, SessionError> {
        let index = self.graph.nodes.get(node.0 as usize).ok_or(SessionError::UnknownNode(node))?.inserts.len();
        let desc = self.graph.alloc_insert(name, factory);
        let id = desc.id;
        self.push("Ajouter un plugin", Edit::AddEffect { node, index, desc: Box::new(desc) })?;
        Ok(id)
    }

    /// Retire un effet (annulable). `keep_state` donne l'usine à garder pour l'annulation : l'hôte y met l'état courant du
    /// plugin pour qu'il revienne tel qu'il était.
    pub fn remove_effect(&mut self, insert: InsertId, keep_state: &mut dyn FnMut(&InsertDesc) -> InsertFactory) -> Result<(), SessionError> {
        let (n, index) = self.locate_insert(insert).ok_or(SessionError::UnknownInsert(insert))?;
        let mut desc = self.graph.nodes[n].inserts[index].clone();
        desc.factory = keep_state(&desc);
        self.push("Retirer un plugin", Edit::RemoveEffect { node: NodeId(n as u32), index, desc: Box::new(desc) })
    }

    /// Place l'effet juste avant `before` dans la chaîne de son nœud (à la fin si `before` est `None`).
    pub fn move_effect(&mut self, insert: InsertId, before: Option<InsertId>) -> Result<(), SessionError> {
        let (n, from) = self.locate_insert(insert).ok_or(SessionError::UnknownInsert(insert))?;
        let chain: Vec<InsertId> = self.graph.nodes[n].inserts.iter().map(|i| i.id).filter(|&i| i != insert).collect();
        let to = match before {
            None => chain.len(),
            Some(b) if b == insert => return Ok(()),
            Some(b) => chain.iter().position(|&i| i == b).ok_or(SessionError::UnknownInsert(b))?,
        };
        if to == from {
            return Ok(());
        }
        self.push("Déplacer un plugin", Edit::MoveEffect { node: NodeId(n as u32), from, to })
    }

    /// Contourne ou réactive un effet **sans historique** (comme un réglage de mixage). Faux si l'effet n'existe pas.
    pub fn set_effect_bypass_untracked(&mut self, insert: InsertId, bypass: bool) -> bool {
        let Some((n, k)) = self.locate_insert(insert) else { return false };
        self.graph.nodes[n].inserts[k].bypass = bypass;
        true
    }

    /// Source de la chaîne latérale d'un effet (`None` : aucune), **sans historique** (réglage de routage de l'effet, comme son contournement).
    /// Refusée si l'effet n'existe pas ou si le graphe la refuse (la sortie principale, le nœud lui-même, une boucle).
    pub fn set_effect_sidechain_untracked(&mut self, insert: InsertId, source: Option<NodeId>) -> Result<(), SessionError> {
        let (n, k) = self.locate_insert(insert).ok_or(SessionError::UnknownInsert(insert))?;
        let old = self.graph.nodes[n].inserts[k].sidechain;
        self.graph.nodes[n].inserts[k].sidechain = source;
        if let Err(e) = self.graph.validate() {
            self.graph.nodes[n].inserts[k].sidechain = old;
            return Err(SessionError::Routing(format!("chaîne latérale refusée : {e:?}")));
        }
        Ok(())
    }

    /// Source de la chaîne latérale d'un effet.
    pub fn effect_sidechain(&self, insert: InsertId) -> Option<NodeId> {
        let (n, k) = self.locate_insert(insert)?;
        self.graph.nodes[n].inserts[k].sidechain
    }

    pub(crate) fn apply_effect_edit(&mut self, e: &Edit) -> Result<(), SessionError> {
        match e {
            Edit::AddEffect { node, index, desc } => {
                let chain = &mut self.graph.nodes.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(*node))?.inserts;
                let at = (*index).min(chain.len());
                chain.insert(at, (**desc).clone());
            }
            Edit::RemoveEffect { node, desc, .. } => {
                let chain = &mut self.graph.nodes.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(*node))?.inserts;
                let k = chain.iter().position(|i| i.id == desc.id).ok_or(SessionError::UnknownInsert(desc.id))?;
                chain.remove(k);
            }
            Edit::MoveEffect { node, from, to } => {
                let chain = &mut self.graph.nodes.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(*node))?.inserts;
                if *from >= chain.len() {
                    return Err(SessionError::UnknownNode(*node));
                }
                let d = chain.remove(*from);
                let at = (*to).min(chain.len());
                chain.insert(at, d);
            }
            _ => {}
        }
        // Les courbes qui visaient un plugin disparu (ou revenu) suivent : voir `Session::sync_node`.
        let node = match e {
            Edit::AddEffect { node, .. } | Edit::RemoveEffect { node, .. } | Edit::MoveEffect { node, .. } => node.0 as usize,
            _ => return Ok(()),
        };
        self.sync_node(node)
    }
}
