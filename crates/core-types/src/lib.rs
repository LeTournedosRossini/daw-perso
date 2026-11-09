//! Types de base : positions, identifiants, gains, lois de panoramique.
//!
//! Aucune dépendance système (compile en WebAssembly). Le temps musical (fractions exactes de ronde, carte
//! des tempos, chiffrages) est dans [`time`] ; le moteur, lui, ne manipule que des échantillons.

pub mod resample;
pub mod time;

/// Position ou durée en échantillons.
pub type Frames = u64;

/// Identifiant d'un nœud du graphe audio (piste, bus ou sortie principale). Le nœud `0` est
/// toujours la sortie principale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const MASTER: NodeId = NodeId(0);
}

/// Gain linéaire à partir de décibels. Sous -120 dB : silence exact.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -120.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Décibels à partir d'un gain linéaire (`-inf` pour 0, borné à -120 dB).
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 1e-6 {
        -120.0
    } else {
        20.0 * gain.log10()
    }
}

/// Façon de répartir un signal entre gauche et droite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanLaw {
    /// Source mono : puissance constante (`g_g² + g_d² = 1`), -3 dB au centre.
    EqualPower,
    /// Source stéréo : balance, gain 1 au centre, on atténue seulement le côté opposé.
    Balance,
}

/// Gains (gauche, droite) pour `pan` dans `[-1, 1]` (-1 = tout à gauche). Hors de l'intervalle,
/// la valeur est bornée. N'utilise que des opérations exactes en IEEE 754 (`+`, `*`, `sqrt`) :
/// le résultat est identique sur toutes les plateformes, ce qui garde le rendu hors ligne déterministe.
#[inline]
pub fn pan_gains(law: PanLaw, pan: f32) -> (f32, f32) {
    let p = if pan.is_nan() { 0.0 } else { pan.clamp(-1.0, 1.0) };
    match law {
        PanLaw::EqualPower => {
            let x = (p + 1.0) * 0.5;
            ((1.0 - x).sqrt(), x.sqrt())
        }
        PanLaw::Balance => ((1.0 - p).min(1.0), (1.0 + p).min(1.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_round_trip() {
        for db in [-60.0f32, -12.0, -6.0, 0.0, 6.0, 12.0] {
            assert!((gain_to_db(db_to_gain(db)) - db).abs() < 1e-3, "{db}");
        }
        assert_eq!(db_to_gain(-120.0), 0.0);
        assert_eq!(db_to_gain(-200.0), 0.0);
        assert!((db_to_gain(0.0) - 1.0).abs() < 1e-7);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-3);
        assert_eq!(gain_to_db(0.0), -120.0);
    }

    #[test]
    fn equal_power_is_constant_power() {
        for i in -100..=100 {
            let (l, r) = pan_gains(PanLaw::EqualPower, i as f32 / 100.0);
            assert!((l * l + r * r - 1.0).abs() < 1e-6, "pan {i}");
        }
        assert_eq!(pan_gains(PanLaw::EqualPower, -1.0), (1.0, 0.0));
        assert_eq!(pan_gains(PanLaw::EqualPower, 1.0), (0.0, 1.0));
        let (l, r) = pan_gains(PanLaw::EqualPower, 0.0);
        assert!((l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6 && l == r);
    }

    #[test]
    fn balance_keeps_unity_at_center() {
        assert_eq!(pan_gains(PanLaw::Balance, 0.0), (1.0, 1.0));
        assert_eq!(pan_gains(PanLaw::Balance, -1.0), (1.0, 0.0));
        assert_eq!(pan_gains(PanLaw::Balance, 1.0), (0.0, 1.0));
        assert_eq!(pan_gains(PanLaw::Balance, 0.5), (0.5, 1.0));
    }

    #[test]
    fn pan_is_clamped_and_nan_safe() {
        assert_eq!(pan_gains(PanLaw::Balance, 7.0), pan_gains(PanLaw::Balance, 1.0));
        assert_eq!(pan_gains(PanLaw::EqualPower, f32::NAN), pan_gains(PanLaw::EqualPower, 0.0));
    }
}
