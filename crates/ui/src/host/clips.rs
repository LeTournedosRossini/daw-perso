//! Opérations sur plusieurs clips à la fois : déplacer, supprimer, copier / couper / coller, dupliquer.
//!
//! Audio et clips de notes se traitent ensemble. Chaque opération est **une seule action annulable** et laisse le projet intact
//! si l'un des clips la refuse. Le presse-papiers vit dans l'hôte (hors historique, survit à la suppression des sources).

use daw_core_types::time::Frac;
use daw_core_types::NodeId;
use daw_engine::{Clip, NodeKind};
use daw_session::{MClip, Session, SessionError};

use super::{auto_crossfades, ClipRef, Host, MIDI_CLIP_BASE};
use crate::protocol::Event;

/// Un clip copié : sa piste d'origine, ses données et son nom.
#[derive(Clone)]
pub(super) enum ClipSnap {
    Audio { node: u32, clip: Clip, name: Option<String> },
    Midi { node: u32, clip: MClip, name: Option<String> },
}

fn musical_at(s: &Session, sample: u64) -> Result<Frac, SessionError> {
    use daw_core_types::time::TempoMap;
    let exact = s.tempo_map().to_frac(sample, s.sample_rate()).ok_or(SessionError::TimeOverflow)?;
    let ticks = TempoMap::to_ticks(exact, daw_session::RECORD_PPQ).ok_or(SessionError::TimeOverflow)?;
    TempoMap::from_ticks(ticks, daw_session::RECORD_PPQ).ok_or(SessionError::TimeOverflow)
}

fn samples_of(s: &Session, f: Frac) -> Result<u64, SessionError> {
    s.tempo_map().to_samples(f, s.sample_rate()).ok_or(SessionError::TimeOverflow)
}

impl ClipSnap {
    fn node(&self) -> u32 {
        match self {
            ClipSnap::Audio { node, .. } | ClipSnap::Midi { node, .. } => *node,
        }
    }

    fn start(&self, s: &Session) -> Result<u64, SessionError> {
        match self {
            ClipSnap::Audio { clip, .. } => Ok(clip.start),
            ClipSnap::Midi { clip, .. } => samples_of(s, clip.start),
        }
    }

    fn end(&self, s: &Session) -> Result<u64, SessionError> {
        match self {
            ClipSnap::Audio { clip, .. } => Ok(clip.end()),
            ClipSnap::Midi { clip, .. } => samples_of(s, clip.start.checked_add(clip.len).ok_or(SessionError::TimeOverflow)?),
        }
    }
}

impl Host {
    /// Les pistes (ni bus ni sortie principale) dans l'ordre d'affichage : c'est dans cet ordre qu'un groupe « descend d'une piste ».
    fn track_list(&self) -> Vec<u32> {
        let g = self.session.graph();
        self.session.display_order().iter().map(|n| n.0).filter(|&n| matches!(g.nodes.get(n as usize).map(|x| x.kind), Some(NodeKind::Track(_)))).collect()
    }

    /// Piste d'arrivée de chaque clip quand le groupe est décalé de `delta` pistes (borné pour que tous restent sur une piste) ;
    /// `strict` : refuse au lieu de borner.
    fn shifted_tracks(&self, origins: &[u32], delta: i32, strict: bool) -> Result<Vec<u32>, String> {
        let tracks = self.track_list();
        let idx: Vec<i64> = origins.iter().map(|o| tracks.iter().position(|t| t == o).map(|i| i as i64).ok_or(format!("le nœud {o} n'est pas une piste"))).collect::<Result<_, _>>()?;
        let (lo, hi) = (-idx.iter().copied().min().unwrap_or(0), tracks.len() as i64 - 1 - idx.iter().copied().max().unwrap_or(0));
        let d = delta as i64;
        if strict && (d < lo || d > hi) {
            return Err("pas assez de pistes pour coller tous les clips à partir de cette piste".into());
        }
        let d = d.clamp(lo, hi);
        Ok(idx.iter().map(|i| tracks[(i + d) as usize]).collect())
    }

    /// Les clips désignés, tels qu'ils sont maintenant. Un identifiant inconnu refuse tout.
    fn snapshot_clips(&self, ids: &[u64]) -> Result<Vec<ClipSnap>, String> {
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            let name = self.meta.clip_names.get(&id).cloned();
            match self.find_clip(id) {
                Some(ClipRef::Audio(cid)) => {
                    let node = self.owner_of_audio(cid).ok_or(format!("clip inconnu : {id}"))?;
                    let clip = self.session.clip(cid).ok_or(format!("clip inconnu : {id}"))?.clone();
                    out.push(ClipSnap::Audio { node: node.0, clip, name });
                }
                Some(ClipRef::Midi(mid)) => {
                    let node = self.owner_of_midi(mid).ok_or(format!("clip inconnu : {id}"))?;
                    let clip = self.session.midi_clip(mid).ok_or(format!("clip inconnu : {id}"))?.clone();
                    out.push(ClipSnap::Midi { node: node.0, clip, name });
                }
                None => return Err(format!("clip inconnu : {id}")),
            }
        }
        Ok(out)
    }

    pub(super) fn move_clips(&mut self, name: &str, ids: &[u64], delta: i64, node_delta: i32) -> Vec<Event> {
        let error = |m: String| vec![Event::Error { command: name.into(), message: m }];
        let snaps = match self.snapshot_clips(ids) {
            Ok(s) => s,
            Err(m) => return error(m),
        };
        let auto = self.auto_crossfade;
        let origins: Vec<u32> = snaps.iter().map(|c| c.node()).collect();
        let targets = match self.shifted_tracks(&origins, node_delta, false) {
            Ok(t) => t,
            Err(m) => return error(m),
        };
        let result = self.session.group("Déplacer des clips", |s| {
            // Le groupe s'arrête à 0 plutôt que de refuser : on borne le décalage par le clip le plus à gauche.
            let earliest = snaps.iter().map(|c| c.start(s)).collect::<Result<Vec<_>, _>>()?.into_iter().min().unwrap_or(0);
            let delta = delta.max(-(earliest.min(i64::MAX as u64) as i64));
            let mut nodes = Vec::new();
            for (k, snap) in snaps.iter().enumerate() {
                let id = ids[k];
                let new_start = snap.start(s)?.checked_add_signed(delta).ok_or(SessionError::TimeOverflow)?;
                let dest = NodeId(targets[k]);
                match snap {
                    ClipSnap::Audio { .. } => s.move_clip_to(daw_session::ClipId(id), dest, new_start)?,
                    ClipSnap::Midi { .. } => {
                        let f = musical_at(s, new_start)?;
                        s.move_midi_clip_to(daw_session::MidiClipId(id - MIDI_CLIP_BASE), dest, f)?
                    }
                }
                if !nodes.contains(&dest.0) {
                    nodes.push(dest.0);
                }
            }
            if auto {
                for n in nodes {
                    auto_crossfades(s, NodeId(n))?;
                }
            }
            Ok(())
        });
        self.finish_edit(name, result)
    }

    pub(super) fn delete_clips(&mut self, name: &str, ids: &[u64]) -> Vec<Event> {
        let error = |m: String| vec![Event::Error { command: name.into(), message: m }];
        if let Err(m) = self.snapshot_clips(ids) {
            return error(m);
        }
        let result = self.session.group("Supprimer des clips", |s| {
            for &id in ids {
                if id >= MIDI_CLIP_BASE {
                    s.remove_midi_clip(daw_session::MidiClipId(id - MIDI_CLIP_BASE))?;
                } else {
                    s.remove_clip(daw_session::ClipId(id))?;
                }
            }
            Ok(())
        });
        self.finish_edit(name, result)
    }

    /// Découpe en `at` tous les clips désignés qui contiennent cette position (les autres sont laissés tels quels) : une seule action.
    pub(super) fn split_clips(&mut self, name: &str, ids: &[u64], at: u64) -> Vec<Event> {
        let error = |m: String| vec![Event::Error { command: name.into(), message: m }];
        let snaps = match self.snapshot_clips(ids) {
            Ok(s) => s,
            Err(m) => return error(m),
        };
        let mut todo: Vec<(u64, Option<String>)> = Vec::new();
        for (k, snap) in snaps.iter().enumerate() {
            match (snap.start(&self.session), snap.end(&self.session)) {
                (Ok(a), Ok(b)) if a < at && at < b => todo.push((ids[k], self.meta.clip_names.get(&ids[k]).cloned())),
                (Err(e), _) | (_, Err(e)) => return error(super::explain(&e)),
                _ => {}
            }
        }
        if todo.is_empty() {
            return error("aucun clip sélectionné ne contient cette position".into());
        }
        let mut pieces: Vec<(u64, u64, Option<String>)> = Vec::new(); // (gauche, droite, nom)
        let result = self.session.group("Découper", |s| {
            for (id, n) in &todo {
                if *id >= MIDI_CLIP_BASE {
                    let f = musical_at(s, at)?;
                    let (l, r) = s.split_midi_clip(daw_session::MidiClipId(id - MIDI_CLIP_BASE), f)?;
                    pieces.push((MIDI_CLIP_BASE + l.0, MIDI_CLIP_BASE + r.0, n.clone()));
                } else {
                    let (l, r) = s.split(daw_session::ClipId(*id), at)?;
                    pieces.push((l.0, r.0, n.clone()));
                }
            }
            Ok(())
        });
        let ok = result.is_ok();
        let mut events = self.finish_edit(name, result);
        if ok && !events.iter().any(|e| matches!(e, Event::Error { .. })) {
            for (_, r, n) in &pieces {
                if let Some(n) = n {
                    self.meta.clip_names.insert(*r, n.clone());
                }
            }
            events.push(self.project_event());
            // Les deux moitiés restent sélectionnées (comme le clip découpé seul gardait sa moitié gauche) : on peut enchaîner.
            events.push(Event::Selection { clips: pieces.iter().flat_map(|p| [p.0, p.1]).collect() });
        }
        events
    }

    pub(super) fn copy_clips(&mut self, name: &str, ids: &[u64]) -> Vec<Event> {
        match self.snapshot_clips(ids) {
            Ok(s) => {
                let count = s.len();
                self.clipboard = s;
                vec![Event::Clipboard { count }]
            }
            Err(m) => vec![Event::Error { command: name.into(), message: m }],
        }
    }

    pub(super) fn paste_clips(&mut self, name: &str, at: u64, node: Option<u32>) -> Vec<Event> {
        let error = |m: String| vec![Event::Error { command: name.into(), message: m }];
        if self.clipboard.is_empty() {
            return error("le presse-papiers est vide".into());
        }
        let snaps = self.clipboard.clone();
        let origins: Vec<u32> = snaps.iter().map(|c| c.node()).collect();
        // Sans piste choisie : chaque clip retrouve sa piste. Avec une piste : la plus haute des pistes copiées vient sur elle, les autres
        // gardent leur écart (refusé s'il manque des pistes en dessous).
        let targets = match node {
            None => origins.clone(),
            Some(n) => {
                let tracks = self.track_list();
                let Some(dest) = tracks.iter().position(|t| *t == n) else { return error(format!("le nœud {n} n'est pas une piste : un clip ne peut être posé que sur une piste")) };
                let top = origins.iter().filter_map(|o| tracks.iter().position(|t| t == o)).min().unwrap_or(dest);
                match self.shifted_tracks(&origins, dest as i32 - top as i32, true) {
                    Ok(t) => t,
                    Err(m) => return error(m),
                }
            }
        };
        let base = match snaps.iter().map(|c| c.start(&self.session)).collect::<Result<Vec<_>, _>>() {
            Ok(v) => v.into_iter().min().unwrap_or(0),
            Err(e) => return error(super::explain(&e)),
        };
        self.place_clips(name, "Coller des clips", &snaps, base, at, &targets)
    }

    pub(super) fn duplicate_clips(&mut self, name: &str, ids: &[u64], delta: Option<i64>, node_delta: i32) -> Vec<Event> {
        let snaps = match self.snapshot_clips(ids) {
            Ok(s) => s,
            Err(m) => return vec![Event::Error { command: name.into(), message: m }],
        };
        let spans: Result<Vec<(u64, u64)>, SessionError> = snaps.iter().map(|c| Ok((c.start(&self.session)?, c.end(&self.session)?))).collect();
        let spans = match spans {
            Ok(s) => s,
            Err(e) => return vec![Event::Error { command: name.into(), message: super::explain(&e) }],
        };
        let base = spans.iter().map(|s| s.0).min().unwrap_or(0);
        let end = spans.iter().map(|s| s.1).max().unwrap_or(base);
        let target = match delta {
            Some(d) => base.checked_add_signed(d).unwrap_or(0),
            None => end,
        };
        let origins: Vec<u32> = snaps.iter().map(|c| c.node()).collect();
        let targets = match self.shifted_tracks(&origins, node_delta, false) {
            Ok(t) => t,
            Err(m) => return vec![Event::Error { command: name.into(), message: m }],
        };
        self.place_clips(name, "Dupliquer des clips", &snaps, base, target, &targets)
    }

    /// Pose des copies : le clip qui commençait en `base` commence en `at`, les autres gardent leurs écarts.
    fn place_clips(&mut self, name: &str, label: &str, snaps: &[ClipSnap], base: u64, at: u64, targets: &[u32]) -> Vec<Event> {
        let auto = self.auto_crossfade;
        let mut created: Vec<(u64, Option<String>)> = Vec::new();
        let result = self.session.group(label, |s| {
            let mut nodes = Vec::new();
            for (k, snap) in snaps.iter().enumerate() {
                let target = NodeId(targets[k]);
                let new_start = snap.start(s)?.checked_sub(base).and_then(|d| at.checked_add(d)).ok_or(SessionError::TimeOverflow)?;
                match snap {
                    ClipSnap::Audio { clip, name, .. } => {
                        let id = s.add_clip(target, clip.moved(new_start))?;
                        created.push((id.0, name.clone()));
                    }
                    ClipSnap::Midi { clip, name, .. } => {
                        let mut c = clip.clone();
                        c.start = musical_at(s, new_start)?;
                        let id = s.add_midi_clip(target, c)?;
                        created.push((MIDI_CLIP_BASE + id.0, name.clone()));
                    }
                }
                if !nodes.contains(&target.0) {
                    nodes.push(target.0);
                }
            }
            if auto {
                for n in nodes {
                    auto_crossfades(s, NodeId(n))?;
                }
            }
            Ok(())
        });
        let ok = result.is_ok();
        let mut events = self.finish_edit(name, result);
        if ok && !events.iter().any(|e| matches!(e, Event::Error { .. })) {
            for (id, n) in &created {
                if let Some(n) = n {
                    self.meta.clip_names.insert(*id, n.clone());
                }
            }
            // Les noms ont changé après l'événement `project` : on le renvoie à jour.
            events.push(self.project_event());
            events.push(Event::Selection { clips: created.iter().map(|c| c.0).collect() });
        }
        events
    }
}
