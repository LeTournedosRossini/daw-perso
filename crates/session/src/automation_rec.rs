//! Enregistrement de l'automation : les gestes (valeur d'un paramètre à des instants successifs) deviennent une courbe
//! d'automation allégée, qui remplace la zone enregistrée de la courbe existante.
//!
//! Ce module ne lit rien lui-même : l'interface (ou le CLI) horodate les gestes avec la position de lecture du moteur
//! (`EngineHandle::position`, précise à un bloc audio près) et les donne à [`Session::write_automation`].

use daw_core_types::time::{Frac, TempoMap};
use daw_core_types::NodeId;
use daw_engine::{AutoCurve, AutoMode, AutoTarget};

use crate::music::{MLane, MPoint};
use crate::{Session, SessionError, RECORD_PPQ};

/// Simplifie une suite de points `(échantillon, valeur)` en gardant ceux qui comptent : la courbe linéaire entre les
/// points conservés ne s'écarte jamais de plus de `tolerance` des points d'origine. Les extrémités sont toujours gardées.
/// Deux points au même instant : le dernier gagne. Algorithme de Douglas-Peucker, itératif.
pub fn thin(points: &[(u64, f64)], tolerance: f64) -> Vec<(u64, f64)> {
    let mut p: Vec<(u64, f64)> = Vec::with_capacity(points.len());
    for &(t, v) in points {
        match p.last_mut() {
            Some(last) if last.0 == t => *last = (t, v),
            _ => p.push((t, v)),
        }
    }
    if p.len() <= 2 {
        return p;
    }
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    keep[p.len() - 1] = true;
    let mut stack = vec![(0usize, p.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (ta, va, tb, vb) = (p[a].0 as f64, p[a].1, p[b].0 as f64, p[b].1);
        let (mut worst, mut at) = (0.0f64, None);
        for i in a + 1..b {
            let line = va + (vb - va) * ((p[i].0 as f64 - ta) / (tb - ta));
            let d = (p[i].1 - line).abs();
            if d > worst {
                worst = d;
                at = Some(i);
            }
        }
        if let (Some(i), true) = (at, worst > tolerance) {
            keep[i] = true;
            stack.push((a, i));
            stack.push((i, b));
        }
    }
    p.into_iter().zip(keep).filter_map(|(pt, k)| k.then_some(pt)).collect()
}

impl Session {
    /// Écrit dans la courbe de `target` ce que l'utilisateur a fait entre les échantillons `from` et `to` :
    /// `recorded` = gestes `(échantillon, valeur)`, allégés à `tolerance` près puis arrondis au tick (960 par noire).
    /// Dans la zone `[from, to]` la courbe existante est **remplacée** ; avant et après, elle est conservée. Si la zone
    /// se termine avant la fin des gestes enregistrés (point de sortie), la courbe rejoint en ligne droite sa valeur
    /// d'origine à `to`. Une seule action annulable. Gestes hors de `[from, to]` : ignorés. Aucun geste : rien n'est fait.
    pub fn write_automation(&mut self, node: NodeId, target: AutoTarget, recorded: &[(u64, f64)], from: u64, to: u64, tolerance: f64) -> Result<(), SessionError> {
        let mut gestures: Vec<(u64, f64)> = recorded.iter().copied().filter(|&(t, v)| t >= from && t <= to && v.is_finite()).collect();
        if gestures.is_empty() || from >= to {
            return Ok(());
        }
        gestures.sort_by_key(|g| g.0); // tri stable : à instant égal, le dernier reçu gagne dans `thin`
        let thinned = thin(&gestures, tolerance.max(0.0));
        let (tm, sr) = (&self.timing.tempo, self.sample_rate);
        let to_frac = |sample: u64| -> Result<Frac, SessionError> {
            let exact = tm.to_frac(sample, sr).ok_or(SessionError::TimeOverflow)?;
            let ticks = TempoMap::to_ticks(exact, RECORD_PPQ).ok_or(SessionError::TimeOverflow)?;
            TempoMap::from_ticks(ticks, RECORD_PPQ).ok_or(SessionError::TimeOverflow)
        };
        let (from_f, to_f) = (to_frac(from)?, to_frac(to)?);
        let old = self.lane(node, target).cloned();
        let mut points: Vec<MPoint> = old.iter().flat_map(|l| l.points.iter().filter(|p| p.pos < from_f).copied()).collect();
        let first_new = points.len();
        for (t, v) in thinned {
            let pos = to_frac(t)?;
            let pt = MPoint { pos, value: v, curve: AutoCurve::Linear };
            match points.last_mut() {
                Some(last) if last.pos == pos => *last = pt, // deux gestes sur le même tick
                _ => points.push(pt),
            }
        }
        if let Some(old) = &old {
            let last_pos = points.last().map_or(Frac::ZERO, |p| p.pos);
            if points.len() > first_new && last_pos < to_f {
                points.push(MPoint { pos: to_f, value: old.value_at(to_f), curve: AutoCurve::Linear });
            }
            let resume = points.last().map_or(Frac::ZERO, |p| p.pos);
            points.extend(old.points.iter().filter(|p| p.pos > to_f && p.pos > resume).copied());
        }
        self.set_lane(node, MLane { target, points, mode: self.lane(node, target).map_or(AutoMode::Read, |l| l.mode) })
    }
}
