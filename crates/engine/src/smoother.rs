//! Lissage de paramètres, par échantillon et indépendant de la taille de bloc.

/// Rampe linéaire sur un nombre fixe d'échantillons : la même suite de valeurs est produite quelle que
/// soit la taille des blocs avec lesquels on la consomme (condition du rendu déterministe).
#[derive(Debug, Clone, Copy)]
pub struct Smoother {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
}

impl Smoother {
    pub fn new(value: f32) -> Self {
        Smoother { current: value, target: value, step: 0.0, remaining: 0 }
    }

    /// Vise `target` en `ramp` échantillons (0 = saut immédiat, utilisé à l'initialisation).
    pub fn set_target(&mut self, target: f32, ramp: u32) {
        if ramp == 0 || target == self.current {
            *self = Smoother::new(target);
        } else {
            self.target = target;
            self.step = (target - self.current) / ramp as f32;
            self.remaining = ramp;
        }
    }

    #[inline]
    pub fn next(&mut self) -> f32 {
        if self.remaining == 0 {
            return self.target;
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            self.current = self.target;
        } else {
            self.current += self.step;
        }
        self.current
    }

    #[inline]
    pub fn is_settled(&self) -> bool {
        self.remaining == 0
    }

    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_linearly_and_lands_exactly() {
        let mut s = Smoother::new(0.0);
        s.set_target(1.0, 4);
        let v: Vec<f32> = (0..6).map(|_| s.next()).collect();
        assert_eq!(v, vec![0.25, 0.5, 0.75, 1.0, 1.0, 1.0]);
        assert!(s.is_settled());
    }

    #[test]
    fn retarget_mid_ramp_starts_from_current_value() {
        let mut s = Smoother::new(0.0);
        s.set_target(1.0, 4);
        s.next();
        s.next(); // 0.5
        s.set_target(0.0, 2);
        assert_eq!((s.next(), s.next(), s.next()), (0.25, 0.0, 0.0));
    }

    #[test]
    fn zero_ramp_jumps() {
        let mut s = Smoother::new(0.3);
        s.set_target(0.9, 0);
        assert!(s.is_settled());
        assert_eq!(s.next(), 0.9);
    }
}
