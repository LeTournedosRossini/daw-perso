//! Courbe de réponse de la vélocité d'une piste (option) : entre les notes (jouées en direct ou lues dans les clips) et ce que reçoivent les
//! plugins de la piste, la vélocité des débuts de note passe par une courbe réglable de concave à convexe, avec une vélocité de sortie
//! minimale et maximale. Désactivée, elle ne touche à rien. L'enregistrement MIDI, lui, reste brut : on peut tout refaire après coup.
//!
//! La courbe est calculée côté contrôle en une table de 128 entrées ; le fil audio ne fait qu'une lecture dans la table. Mêmes formules
//! que les fondus (`+ − × /` seulement) : la table est identique sur toutes les plateformes.

/// Réglages de la courbe de vélocité d'une piste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VelocityCurve {
    /// Courbe active.
    pub enabled: bool,
    /// Courbure, de −1000 (concave : les vélocités faibles montent) à +1000 (convexe : il faut jouer fort), 0 = droite.
    pub bend: i16,
    /// Adoucissement en S, de 0 à 1000.
    pub smooth: u16,
    /// Vélocité de sortie pour une vélocité d'entrée de 1 (1 à 127).
    pub min: u8,
    /// Vélocité de sortie pour une vélocité d'entrée de 127 (1 à 127).
    pub max: u8,
    /// Vélocité fixe (1 à 127) : toutes les notes sortent à cette valeur, la courbe est ignorée. 0 : pas de vélocité fixe.
    pub fixed: u8,
}

impl Default for VelocityCurve {
    fn default() -> Self {
        VelocityCurve { enabled: false, bend: 0, smooth: 0, min: 1, max: 127, fixed: 0 }
    }
}

impl VelocityCurve {
    /// Valeurs dans leurs plages : courbure −1000 à 1000, adoucissement 0 à 1000, bornes et vélocité fixe de 1 à 127 (fixe : 0 = aucune).
    pub fn is_valid(&self) -> bool {
        (-1000..=1000).contains(&self.bend) && self.smooth <= 1000 && (1..=127).contains(&self.min) && (1..=127).contains(&self.max) && self.fixed <= 127
    }

    /// Vélocité de sortie (1 à 127) pour une vélocité d'entrée de 1 à 127 (0 est traité comme 1).
    pub fn map(&self, velocity: u8) -> u8 {
        if self.fixed > 0 {
            return self.fixed;
        }
        let i = velocity.clamp(1, 127);
        let x = (i - 1) as f32 / 126.0;
        let s = self.smooth as f32 / 1000.0;
        let u = if self.smooth == 0 { x } else { x + s * (x * x * (3.0 - 2.0 * x) - x) };
        let c = self.bend as f32 / 1000.0;
        let v = if self.bend == 0 { u } else { u - c * u * (1.0 - u) };
        let out = self.min as f32 + v * (self.max as f32 - self.min as f32);
        ((out + 0.5).floor() as i32).clamp(1, 127) as u8
    }

    /// Table `entrée (0 à 127) → vélocité de sortie` en vélocités MIDI entières ; `None` si la courbe est désactivée (rien ne change).
    pub fn table(&self) -> Option<Box<[u8; 128]>> {
        self.enabled.then(|| {
            let mut t = [1u8; 128];
            for (i, o) in t.iter_mut().enumerate() {
                *o = self.map(i as u8);
            }
            Box::new(t)
        })
    }
}

/// Applique la table à une vélocité flottante (0 à 1) : arrondie à la vélocité MIDI la plus proche, remplacée, puis remise sur 0 à 1.
#[inline]
pub(crate) fn shape(table: &[u8; 128], velocity: f32) -> f32 {
    let i = ((velocity * 127.0 + 0.5).floor() as i32).clamp(0, 127) as usize;
    table[i] as f32 / 127.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> VelocityCurve {
        VelocityCurve { enabled: true, ..VelocityCurve::default() }
    }

    #[test]
    fn straight_line_by_default_keeps_every_velocity() {
        let c = on();
        for v in 1..=127u8 {
            assert_eq!(c.map(v), v);
        }
        assert!(VelocityCurve::default().table().is_none(), "désactivée : pas de table");
    }

    #[test]
    fn handles_set_the_range_and_curve_bends_the_middle() {
        let c = VelocityCurve { min: 40, max: 100, ..on() };
        assert_eq!((c.map(1), c.map(127)), (40, 100));
        assert_eq!(c.map(64), 70);
        let convex = VelocityCurve { bend: 1000, ..on() };
        let concave = VelocityCurve { bend: -1000, ..on() };
        assert!(convex.map(64) < 64 && concave.map(64) > 64, "convexe : plus bas ; concave : plus haut ({} / {})", convex.map(64), concave.map(64));
        assert_eq!((convex.map(1), convex.map(127)), (1, 127));
    }

    #[test]
    fn output_is_monotone_and_never_zero() {
        for bend in [-1000i16, -300, 0, 500, 1000] {
            for smooth in [0u16, 400, 1000] {
                let c = VelocityCurve { bend, smooth, min: 20, max: 120, ..on() };
                let mut last = 0;
                for v in 1..=127u8 {
                    let o = c.map(v);
                    assert!(o >= 1 && o >= last, "bend {bend} smooth {smooth} v {v} → {o}");
                    last = o;
                }
            }
        }
    }

    #[test]
    fn a_fixed_velocity_ignores_the_curve_and_handles_may_be_equal() {
        let c = VelocityCurve { fixed: 90, bend: 800, ..on() };
        assert!((1..=127).all(|v| c.map(v) == 90));
        let flat = VelocityCurve { min: 64, max: 64, ..on() };
        assert!((1..=127).all(|v| flat.map(v) == 64), "poignées au même niveau = vélocité fixe");
    }

    #[test]
    fn shape_works_on_float_velocities() {
        let t = VelocityCurve { min: 64, max: 64, ..on() }.table().unwrap();
        assert_eq!(shape(&t, 1.0), 64.0 / 127.0);
        assert_eq!(shape(&t, 0.0), 64.0 / 127.0, "même une vélocité nulle sort à 1 au moins");
    }

    #[test]
    fn validity() {
        assert!(on().is_valid());
        assert!(!VelocityCurve { bend: 1001, ..on() }.is_valid());
        assert!(!VelocityCurve { min: 0, ..on() }.is_valid());
        assert!(!VelocityCurve { fixed: 128, ..on() }.is_valid());
    }
}
