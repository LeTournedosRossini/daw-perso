//! Automation : courbes de paramètres évaluées à l'échantillon près.
//!
//! Une courbe ([`AutoLane`]) vise un paramètre d'un nœud et se compose de points datés en **échantillons**
//! (le temps musical est converti côté contrôle avec `TempoMap`). La valeur à l'échantillon absolu `t` ne dépend que
//! de `t` : le rendu est donc identique quelle que soit la taille des blocs.
//!
//! - Gain et panoramique : la courbe **remplace** le réglage manuel (mute et solo continuent d'agir) et s'applique
//!   échantillon par échantillon.
//! - Paramètres d'effets : la valeur est envoyée au plugin au début de chaque tranche de [`AUTO_GRID`] échantillons
//!   alignée sur la ligne de temps absolue (pas sur le bloc), puis maintenue : c'est ce qui rend le résultat
//!   indépendant de la taille de bloc. À 48 kHz, 32 échantillons = 0,67 ms.

use crate::insert::InsertId;
use daw_core_types::NodeId;

/// Pas de la grille d'envoi des paramètres d'effets, en échantillons (aligné sur la ligne de temps absolue).
pub const AUTO_GRID: u64 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoTarget {
    /// Gain linéaire de la piste ou du bus (≥ 0).
    Gain,
    /// Panoramique de -1 à 1 (borné à l'application).
    Pan,
    /// Paramètre `param` de l'effet inséré `insert` (valeur dans l'unité du plugin).
    InsertParam { insert: InsertId, param: u32 },
    /// Niveau (gain linéaire ≥ 0) de l'envoi du nœud vers `to`.
    SendGain { to: NodeId },
}

/// Ce que fait la courbe d'une voie d'automation (comme les modes de Reaper).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoMode {
    /// La courbe commande le paramètre ; le réglage manuel (fader, panoramique, niveau d'envoi) n'a plus d'effet.
    #[default]
    Read,
    /// La courbe est lue **et** le réglage manuel s'y ajoute : le fader devient un « trim » de toute la courbe (gain : produit ;
    /// panoramique : somme, bornée à ±1 ; niveau d'envoi : produit). Sans effet sur les paramètres de plugins (lus comme `Read`).
    Trim,
    /// La courbe est conservée mais ignorée : le réglage manuel commande seul.
    Off,
}

/// Façon de passer d'un point au suivant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoCurve {
    #[default]
    Linear,
    /// Garde la valeur du point jusqu'au suivant (palier).
    Step,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoPoint {
    /// Position en échantillons depuis le début du projet.
    pub time: u64,
    pub value: f64,
    /// Courbe du point **vers le suivant** (sans effet sur le dernier).
    pub curve: AutoCurve,
}

impl AutoPoint {
    pub fn new(time: u64, value: f64) -> AutoPoint {
        AutoPoint { time, value, curve: AutoCurve::Linear }
    }
    pub fn step(time: u64, value: f64) -> AutoPoint {
        AutoPoint { time, value, curve: AutoCurve::Step }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutoLane {
    pub target: AutoTarget,
    /// Strictement croissants en temps, valeurs finies, au moins un point.
    pub points: Vec<AutoPoint>,
    pub mode: AutoMode,
}

impl AutoLane {
    pub fn new(target: AutoTarget, points: Vec<AutoPoint>) -> AutoLane {
        AutoLane { target, points, mode: AutoMode::Read }
    }

    pub fn with_mode(mut self, mode: AutoMode) -> AutoLane {
        self.mode = mode;
        self
    }

    pub(crate) fn valid(&self) -> bool {
        !self.points.is_empty()
            && self.points.windows(2).all(|w| w[0].time < w[1].time)
            && self.points.iter().all(|p| p.value.is_finite() && (!matches!(self.target, AutoTarget::Gain | AutoTarget::SendGain { .. }) || p.value >= 0.0))
    }

    /// Valeur à l'échantillon `t` : avant le premier point, la valeur du premier ; après le dernier, celle du dernier.
    /// N'utilise que `+ - × /` en `f64` (exacts, donc identiques sur toutes les plateformes).
    #[inline]
    pub fn value_at(&self, t: u64) -> f64 {
        let p = &self.points;
        let i = p.partition_point(|x| x.time <= t);
        if i == 0 {
            return p[0].value;
        }
        if i == p.len() {
            return p[i - 1].value;
        }
        let (a, b) = (&p[i - 1], &p[i]);
        match a.curve {
            AutoCurve::Step => a.value,
            AutoCurve::Linear => a.value + (b.value - a.value) * ((t - a.time) as f64 / (b.time - a.time) as f64),
        }
    }
}
