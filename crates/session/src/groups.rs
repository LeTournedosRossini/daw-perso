//! Groupes de pistes du projet : un nom, une couleur, des membres (pistes et bus). Un nœud appartient à **un seul** groupe au plus.
//!
//! L'appartenance est stockée par nœud (tableau parallèle à `graph.nodes`, comme les couleurs) : ajouter ou retirer un nœud la
//! renumérote d'elle-même, et annuler un retrait rend le nœud à son groupe. Les groupes servent au mixeur (mute, solo et niveau
//! communs) et aux retours de monitoring (un seul fader pour « Batterie »).

use daw_core_types::NodeId;

use crate::{Edit, Session, SessionError};

/// Nombre maximal de groupes.
pub const MAX_GROUPS: usize = 64;
/// Longueur maximale du nom (en caractères).
pub const MAX_NAME: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// Identifiant stable (jamais réutilisé dans une session), 1 ou plus.
    pub id: u32,
    pub name: String,
    /// Couleur `#rrggbb` ; vide : la page choisit.
    pub color: String,
}

/// État complet des groupes : la liste et, pour chaque nœud, son groupe (0 = aucun).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupState {
    pub groups: Vec<Group>,
    pub of: Vec<u32>,
}

fn valid_color(c: &str) -> bool {
    c.is_empty() || (c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()))
}

impl Session {
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Groupe d'un nœud (0 : aucun).
    pub fn node_group(&self, node: NodeId) -> u32 {
        self.node_group.get(node.0 as usize).copied().unwrap_or(0)
    }

    /// Membres d'un groupe, dans l'ordre des numéros de nœud.
    pub fn group_members(&self, group: u32) -> Vec<NodeId> {
        self.node_group.iter().enumerate().filter(|(_, &g)| g == group).map(|(i, _)| NodeId(i as u32)).collect()
    }

    /// Réserve un identifiant de groupe neuf.
    pub fn alloc_group_id(&mut self) -> u32 {
        self.next_group_id += 1;
        self.next_group_id
    }

    /// Valide des groupes et leurs membres et en fait un état complet (appartenance par nœud). `ret` : le retour dont ce sont les
    /// groupes propres (ses membres sont alors des sources : ni la sortie principale, ni un retour) ; `None` : groupes du projet.
    fn build_group_state(&self, groups: Vec<Group>, members: &[(NodeId, u32)], ret: Option<NodeId>) -> Result<GroupState, SessionError> {
        let bad = |m: &str| SessionError::BadGroups(m.to_owned());
        if groups.len() > MAX_GROUPS {
            return Err(bad(&format!("trop de groupes ({MAX_GROUPS} au plus)")));
        }
        for (i, g) in groups.iter().enumerate() {
            if g.id == 0 || groups[..i].iter().any(|h| h.id == g.id) {
                return Err(bad("identifiant de groupe nul ou en double"));
            }
            if g.name.trim().is_empty() || g.name.chars().count() > MAX_NAME {
                return Err(bad(&format!("nom de groupe invalide (1 à {MAX_NAME} caractères)")));
            }
            if !valid_color(&g.color) {
                return Err(bad(&format!("couleur « {} » invalide (attendu #rrggbb)", g.color)));
            }
        }
        let mut of = vec![0u32; self.graph.nodes.len()];
        for &(node, g) in members {
            let i = node.0 as usize;
            if i == 0 || i >= of.len() {
                return Err(bad("membre inexistant"));
            }
            if ret.is_some() && (Some(node) == ret || self.graph.nodes[i].monitor) {
                return Err(bad("un retour n'est pas une source : il ne peut pas être membre d'un groupe de retour"));
            }
            if of[i] != 0 {
                return Err(bad("un nœud ne peut être que dans un groupe"));
            }
            if !groups.iter().any(|h| h.id == g) {
                return Err(bad("groupe inconnu"));
            }
            of[i] = g;
        }
        Ok(GroupState { groups, of })
    }

    /// Remplace tous les groupes et l'appartenance (une action annulable). `members` : (nœud, groupe) ; les nœuds absents n'ont
    /// aucun groupe. Refusée sans trace : trop de groupes, nom vide ou trop long, couleur invalide, identifiant nul ou en double,
    /// membre inexistant (ou sortie principale), groupe inconnu, nœud cité deux fois.
    pub fn set_groups(&mut self, label: &str, groups: Vec<Group>, members: &[(NodeId, u32)]) -> Result<(), SessionError> {
        let new = self.build_group_state(groups, members, None)?;
        let old = GroupState { groups: self.groups.clone(), of: self.node_group.clone() };
        if new == old {
            return Ok(());
        }
        self.push(label, Edit::SetGroups { old: Box::new(old), new: Box::new(new) })
    }

    // ---- groupes propres à un retour -----------------------------------------------------------------------------------------------

    /// Les groupes propres d'un retour, s'il en a : ils **remplacent** ceux du projet dans son mix.
    pub fn return_groups(&self, ret: NodeId) -> Option<&GroupState> {
        self.ret_groups.get(ret.0 as usize).and_then(|g| g.as_ref())
    }

    /// Les groupes qui comptent dans le mix de `ret` (ses groupes propres, sinon ceux du projet) ; `None` : ceux du projet.
    pub fn effective_group_list(&self, ret: Option<NodeId>) -> &[Group] {
        match ret.and_then(|r| self.return_groups(r)) {
            Some(g) => &g.groups,
            None => &self.groups,
        }
    }

    /// Groupe d'un nœud dans le mix de `ret` (0 : aucun).
    pub fn effective_group_of(&self, node: NodeId, ret: Option<NodeId>) -> u32 {
        match ret.and_then(|r| self.return_groups(r)) {
            Some(g) => g.of.get(node.0 as usize).copied().unwrap_or(0),
            None => self.node_group(node),
        }
    }

    /// Membres d'un groupe dans le mix de `ret`.
    pub fn effective_members(&self, group: u32, ret: Option<NodeId>) -> Vec<NodeId> {
        (1..self.graph.nodes.len() as u32).map(NodeId).filter(|&n| self.effective_group_of(n, ret) == group).collect()
    }

    /// Donne à un retour ses propres groupes (`Some`) ou le ramène à ceux du projet (`None`) : une action annulable. Refusée sans trace
    /// si le nœud n'est pas un retour de monitoring ou si les groupes sont invalides.
    pub fn set_return_groups(&mut self, label: &str, ret: NodeId, state: Option<(Vec<Group>, Vec<(NodeId, u32)>)>) -> Result<(), SessionError> {
        let node = self.graph.nodes.get(ret.0 as usize).ok_or(SessionError::UnknownNode(ret))?;
        if ret.0 == 0 || !node.monitor {
            return Err(SessionError::BadGroups("seul un retour de monitoring peut avoir ses propres groupes".into()));
        }
        let new = match state {
            Some((groups, members)) => Some(Box::new(self.build_group_state(groups, &members, Some(ret))?)),
            None => None,
        };
        let old = self.ret_groups[ret.0 as usize].clone().map(Box::new);
        if new == old {
            return Ok(());
        }
        self.push(label, Edit::SetReturnGroups { ret, old, new })
    }

    pub(crate) fn apply_groups_edit(&mut self, e: &Edit) {
        match e {
            Edit::SetGroups { new, .. } => {
                self.groups = new.groups.clone();
                self.node_group = new.of.clone();
            }
            Edit::SetReturnGroups { ret, new, .. } => {
                if let Some(slot) = self.ret_groups.get_mut(ret.0 as usize) {
                    *slot = new.as_ref().map(|b| (**b).clone());
                }
            }
            _ => {}
        }
    }
}
