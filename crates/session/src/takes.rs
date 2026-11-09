//! Prises composites : plusieurs prises d'un même passage, une seule active.
//!
//! Une prise inactive est un clip **muet** resté dans le graphe (le moteur l'ignore), rattaché à un groupe par `Clip::take_group`
//! (0 = aucun). Quand une prise est posée sur de l'audio existant (`record_over`), les clips dessous sont découpés aux bornes de la
//! prise : la partie recouverte devient une prise inactive du groupe, le reste reste actif. Tout est fait par les éditions
//! ordinaires de la session (une seule action annulable).

use daw_core_types::NodeId;
use daw_engine::Clip;

use crate::music::MClip;
use crate::xfade;
use crate::Edit;
use crate::{ClipId, MidiClipId, MidiTakeData, Session, SessionError};

impl Session {
    /// Clips de la piste du clip `id` qui sont dans le même groupe de prises, `id` compris (dans l'ordre de la piste).
    pub fn take_group_members(&self, id: ClipId) -> Vec<ClipId> {
        let Some((n, i)) = self.locate(id) else { return Vec::new() };
        let g = self.graph.nodes[n].clips[i].take_group;
        if g == 0 {
            return vec![id];
        }
        self.ids[n].iter().zip(&self.graph.nodes[n].clips).filter(|(_, c)| c.take_group == g).map(|(&cid, _)| cid).collect()
    }

    /// Pose `clip` (une prise) sur la piste. S'il recouvre de l'audio actif, ce dernier est découpé aux bornes de la prise et la
    /// partie recouverte devient une prise inactive du même groupe que la nouvelle, qui est la prise active.
    pub fn record_over(&mut self, node: NodeId, clip: Clip) -> Result<ClipId, SessionError> {
        self.record_over_with(node, clip, false)
    }

    /// Comme `record_over` ; avec `force_group`, la prise reçoit un groupe même s'il n'y a rien dessous (pour y ranger des prises
    /// inactives, voir `record_passes`).
    fn record_over_with(&mut self, node: NodeId, mut clip: Clip, force_group: bool) -> Result<ClipId, SessionError> {
        let n = node.0 as usize;
        if n >= self.graph.nodes.len() {
            return Err(SessionError::UnknownNode(node));
        }
        let (s, e) = (clip.start, clip.end());
        let hit: Vec<ClipId> =
            self.ids[n].iter().zip(&self.graph.nodes[n].clips).filter(|(_, c)| !c.muted && c.start < e && s < c.end()).map(|(&id, _)| id).collect();
        if hit.is_empty() && !force_group {
            return self.add_clip(node, clip);
        }
        let existing = hit.iter().filter_map(|&id| self.clip(id)).map(|c| c.take_group).find(|&g| g != 0);
        let group = existing.unwrap_or_else(|| self.next_take_group());
        clip.take_group = group;
        clip.muted = false;
        self.group("Enregistrer une prise", |sess| {
            sess.carve(hit, s, e, Some(group))?;
            let id = sess.add_clip(node, clip)?;
            sess.smooth_seams(node)?;
            Ok(id)
        })
    }

    /// Pose les passages d'un enregistrement en boucle, dans l'ordre : chacun recouvre le précédent comme `record_over`, donc tous
    /// finissent dans le même groupe de prises, le **dernier** actif et les autres dessous. Un seul passage : prise ordinaire sans
    /// groupe. Rend les identifiants dans l'ordre des passages. Une seule action (« Enregistrer »).
    pub fn record_passes(&mut self, node: NodeId, passes: Vec<Clip>) -> Result<Vec<ClipId>, SessionError> {
        self.group("Enregistrer", |sess| passes.into_iter().map(|c| sess.record_over(node, c)).collect())
    }

    /// Raccords entre prises : chaque paire de clips **actifs** de la piste qui se touchent exactement (fin de l'un = début de l'autre),
    /// dont l'un au moins appartient à un groupe de prises, reçoit un petit fondu enchaîné centré sur la jonction (`xfade::seam`,
    /// longueur `seam_ms`). Dans l'action en cours (un seul pas d'annulation). Sans effet si `seam_ms` vaut 0.
    pub(crate) fn smooth_seams(&mut self, node: NodeId) -> Result<(), SessionError> {
        let n = node.0 as usize;
        if self.seam_ms == 0 || n >= self.graph.nodes.len() {
            return Ok(());
        }
        let target = u64::from(self.sample_rate) * u64::from(self.seam_ms) / 1000;
        let mut active: Vec<(ClipId, Clip)> = self.ids[n].iter().copied().zip(self.graph.nodes[n].clips.iter().cloned()).filter(|(_, c)| !c.muted).collect();
        active.sort_by_key(|(_, c)| c.start);
        let junctions: Vec<(ClipId, ClipId)> = active
            .windows(2)
            .filter(|w| w[0].1.end() == w[1].1.start && (w[0].1.take_group != 0 || w[1].1.take_group != 0))
            .map(|w| (w[0].0, w[1].0))
            .collect();
        // Un clip peut border deux jonctions (une prise au milieu) : on relit chaque clip avant de le modifier.
        for (ia, ib) in junctions {
            let (a, b) = (self.clip(ia).ok_or(SessionError::UnknownClip(ia))?.clone(), self.clip(ib).ok_or(SessionError::UnknownClip(ib))?.clone());
            if let Some((na, nb)) = xfade::seam(&a, &b, target) {
                self.push("Raccord entre prises", Edit::ReplaceClip { id: ia, old: a, new: na })?;
                self.push("Raccord entre prises", Edit::ReplaceClip { id: ib, old: b, new: nb })?;
            }
        }
        Ok(())
    }

    /// Découpe chaque clip de `ids` aux bornes `[s, e)` et rend muette la partie qui y tombe (rattachée à `group` si donné) ; ce qui
    /// dépasse de part et d'autre reste tel quel.
    fn carve(&mut self, ids: Vec<ClipId>, s: u64, e: u64, group: Option<u32>) -> Result<(), SessionError> {
        for id in ids {
            let mut cur = id;
            if self.clip(cur).map_or(false, |c| c.start < s) {
                cur = self.split(cur, s)?.1;
            }
            if self.clip(cur).map_or(false, |c| c.end() > e) {
                self.split(cur, e)?;
            }
            let mut under = self.clip(cur).ok_or(SessionError::UnknownClip(cur))?.clone();
            under.muted = true;
            if let Some(g) = group {
                under.take_group = g;
            }
            self.set_clip(cur, under)?;
        }
        Ok(())
    }

    /// Composer : active seulement la plage `[s, e)` de la prise `id` (bornée à la prise). La prise est coupée aux bords de la plage
    /// si besoin, et les autres prises actives du groupe sont coupées pour lui laisser la place (voir `set_active_take`). Rend le
    /// morceau activé. Une seule action annulable (« Composer une prise »).
    pub fn activate_take_range(&mut self, id: ClipId, s: u64, e: u64) -> Result<ClipId, SessionError> {
        let c = self.clip(id).ok_or(SessionError::UnknownClip(id))?.clone();
        let (s, e) = (s.max(c.start), e.min(c.end()));
        if c.take_group == 0 || s >= e {
            return Err(SessionError::UnknownClip(id));
        }
        self.group("Composer une prise", |sess| {
            let mut cur = id;
            if c.start < s {
                cur = sess.split(cur, s)?.1;
            }
            if sess.clip(cur).map_or(false, |k| k.end() > e) {
                sess.split(cur, e)?;
            }
            sess.set_active_take(cur)?;
            Ok(cur)
        })
    }

    /// Rend active la prise `id` : les autres clips de son groupe qui la recouvrent dans le temps deviennent muets. Une seule action
    /// annulable (aucune si `id` est déjà la seule prise active). Refusé pour un clip qui n'est dans aucun groupe.
    pub fn set_active_take(&mut self, id: ClipId) -> Result<(), SessionError> {
        let chosen = self.clip(id).ok_or(SessionError::UnknownClip(id))?.clone();
        if chosen.take_group == 0 {
            return Err(SessionError::UnknownClip(id));
        }
        let others: Vec<ClipId> = self
            .take_group_members(id)
            .into_iter()
            .filter(|&o| o != id)
            .filter(|&o| self.clip(o).map_or(false, |c| !c.muted && c.start < chosen.end() && chosen.start < c.end()))
            .collect();
        self.group("Changer de prise", |sess| {
            // Les prises actives qui la recouvrent partiellement sont coupées : seule la partie recouverte devient muette.
            sess.carve(others, chosen.start, chosen.end(), None)?;
            let owner = sess.locate(id).map(|(n, _)| NodeId(n as u32));
            if chosen.muted {
                let mut c = chosen;
                c.muted = false;
                sess.set_clip(id, c)?;
            }
            if let Some(n) = owner {
                sess.smooth_seams(n)?;
            }
            Ok(())
        })
    }

    /// Coupe ou rétablit un clip **sans historique** (écoute provisoire : l'appelant remet l'état d'avant et recharge le graphe).
    /// Un clip inconnu est ignoré.
    pub fn set_clip_muted_untracked(&mut self, id: ClipId, muted: bool) {
        if let Some((n, i)) = self.locate(id) {
            self.graph.nodes[n].clips[i].muted = muted;
        }
    }

    /// Coupe ou rétablit un clip (hors groupe de prises).
    pub fn set_clip_muted(&mut self, id: ClipId, muted: bool) -> Result<(), SessionError> {
        let mut c = self.clip(id).ok_or(SessionError::UnknownClip(id))?.clone();
        if c.muted != muted {
            c.muted = muted;
            self.set_clip(id, c)?;
        }
        Ok(())
    }
}

// ---- prises de notes ---------------------------------------------------------------------------------------------------------
//
// Un clip de notes muet est un conteneur de prise inactif (le moteur l'ignore). Pas de découpe aux bords ni de composition par
// plage pour les notes : on active ou non un conteneur entier (les conteneurs d'un même groupe qui se recouvrent s'excluent).

impl Session {
    /// Premier numéro de groupe de prises libre (audio et notes confondus).
    pub(crate) fn next_take_group(&self) -> u32 {
        let audio = self.graph.nodes.iter().flat_map(|n| n.clips.iter()).map(|c| c.take_group).max().unwrap_or(0);
        let midi = self.midi.iter().flat_map(|v| v.iter()).map(|(_, c)| c.take_group).max().unwrap_or(0);
        audio.max(midi) + 1
    }

    /// Conteneurs du même groupe que `id` sur sa piste, `id` compris.
    pub fn midi_take_group_members(&self, id: MidiClipId) -> Vec<MidiClipId> {
        let Some((n, i)) = self.locate_midi(id) else { return Vec::new() };
        let g = self.midi[n][i].1.take_group;
        if g == 0 {
            return vec![id];
        }
        self.midi[n].iter().filter(|(_, c)| c.take_group == g).map(|(cid, _)| *cid).collect()
    }

    /// Pose les passages d'un enregistrement de notes en boucle : un conteneur par passage **qui contient quelque chose** (un
    /// passage vide ne crée rien). Un seul passage non vide : un clip ordinaire. Plusieurs : un groupe de prises, le **dernier** actif,
    /// les autres muets. Une seule action annulable (« Enregistrer »). Rend les identifiants créés, dans l'ordre des passages.
    pub fn add_recorded_passes(&mut self, node: NodeId, passes: &[MidiTakeData], latency_comp: u64) -> Result<Vec<MidiClipId>, SessionError> {
        let mut clips = Vec::new();
        for p in passes {
            if let Some(c) = self.recorded_clip(p, latency_comp)? {
                clips.push(c);
            }
        }
        let group = if clips.len() > 1 { self.next_take_group() } else { 0 };
        let last = clips.len().saturating_sub(1);
        self.group("Enregistrer", |sess| {
            let mut ids = Vec::new();
            for (k, mut c) in clips.into_iter().enumerate() {
                c.take_group = group;
                c.muted = group != 0 && k != last;
                ids.push(sess.add_midi_clip(node, c)?);
            }
            Ok(ids)
        })
    }

    /// Rend actif le conteneur `id` : les autres du groupe qui le recouvrent dans le temps deviennent muets. Une action annulable.
    pub fn set_active_midi_take(&mut self, id: MidiClipId) -> Result<(), SessionError> {
        let chosen = self.midi_clip(id).ok_or(SessionError::UnknownMidiClip(id))?.clone();
        if chosen.take_group == 0 {
            return Err(SessionError::UnknownMidiClip(id));
        }
        let chosen_end = chosen.start.checked_add(chosen.len).ok_or(SessionError::TimeOverflow)?;
        let others: Vec<MidiClipId> = self
            .midi_take_group_members(id)
            .into_iter()
            .filter(|&o| o != id)
            .filter(|&o| {
                self.midi_clip(o).is_some_and(|c| !c.muted && c.start < chosen_end && c.start.checked_add(c.len).is_some_and(|e| chosen.start < e))
            })
            .collect();
        self.group("Changer de prise", |sess| {
            for o in others {
                sess.set_midi_muted(o, true)?;
            }
            sess.set_midi_muted(id, false)
        })
    }

    /// Coupe ou rétablit un clip de notes.
    pub fn set_midi_muted(&mut self, id: MidiClipId, muted: bool) -> Result<(), SessionError> {
        let c = self.midi_clip(id).ok_or(SessionError::UnknownMidiClip(id))?;
        if c.muted == muted {
            return Ok(());
        }
        self.replace_midi(if muted { "Couper un clip de notes" } else { "Rétablir un clip de notes" }, id, |c| Ok(MClip { muted, ..c.clone() }))
    }
}
