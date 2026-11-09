//! Dynamique d'entrée d'une piste (option) : une porte et un compresseur, sur le signal qui entre dans la carte son, avant l'écoute et,
//! au choix, avant l'enregistrement. Détecteur de crête commun aux deux canaux (l'image stéréo ne bouge pas), enveloppe à attaque et
//! relâchement séparés, gain calculé en dB. Désactivé, il ne touche à rien : le signal passe tel quel, au bit près.
//!
//! Ce traitement sert à dompter une entrée (un chanteur qui s'approche du micro, une basse inégale) : il n'est pas un effet de mixage
//! — pour celui-là, on insère un plugin sur la piste.

/// Réglages de la dynamique d'entrée d'une piste.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputDynamics {
    /// Traitement actif.
    pub enabled: bool,
    /// Seuil de la porte (dB) : sous ce niveau le signal est étouffé ; `−90` ou moins : pas de porte.
    pub gate_db: f32,
    /// Seuil du compresseur (dB) : au-dessus, le niveau est réduit.
    pub threshold_db: f32,
    /// Rapport du compresseur (1 : rien, 20 : presque un limiteur).
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    /// Gain de compensation (dB) appliqué après la compression.
    pub makeup_db: f32,
    /// Écrire dans la prise le signal **traité** (par défaut non : la prise reste brute et on peut encore tout refaire).
    pub record: bool,
}

impl Default for InputDynamics {
    fn default() -> Self {
        InputDynamics { enabled: false, gate_db: -90.0, threshold_db: -18.0, ratio: 3.0, attack_ms: 10.0, release_ms: 120.0, makeup_db: 0.0, record: false }
    }
}

impl InputDynamics {
    /// Valeurs dans leurs plages : seuil de porte −90 à −20, seuil −60 à 0, rapport 1 à 20, attaque 0,1 à 100 ms, relâchement 10 à 2 000 ms,
    /// compensation 0 à 24 dB, tout fini.
    pub fn is_valid(&self) -> bool {
        let ok = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
        ok(self.gate_db, -90.0, -20.0) && ok(self.threshold_db, -60.0, 0.0) && ok(self.ratio, 1.0, 20.0) && ok(self.attack_ms, 0.1, 100.0) && ok(self.release_ms, 10.0, 2000.0) && ok(self.makeup_db, 0.0, 24.0)
    }
}

/// État du traitement d'une piste (enveloppe du détecteur, état de la porte).
#[derive(Debug, Clone)]
pub(crate) struct DynState {
    cfg: InputDynamics,
    /// Enveloppe du niveau (linéaire).
    pub(crate) env: f32,
    /// Gain de la porte (0 à 1), lissé.
    pub(crate) gate: f32,
    pub(crate) open: bool,
    atk: f32,
    rel: f32,
    gate_open: f32,
    gate_close: f32,
    makeup: f32,
    slope: f32,
}

fn coef(ms: f32, rate: f32) -> f32 {
    (-1.0 / (ms.max(0.01) * 0.001 * rate)).exp()
}

impl DynState {
    pub(crate) fn new(cfg: InputDynamics, rate: u32) -> DynState {
        let r = rate as f32;
        DynState {
            cfg,
            env: 0.0,
            gate: 1.0,
            open: true,
            atk: coef(cfg.attack_ms, r),
            rel: coef(cfg.release_ms, r),
            gate_open: coef(2.0, r),
            gate_close: coef(80.0, r),
            makeup: 10f32.powf(cfg.makeup_db / 20.0),
            slope: 1.0 - 1.0 / cfg.ratio.max(1.0),
        }
    }

    /// Traite un échantillon stéréo (détecteur commun) : rend les deux canaux traités.
    #[inline]
    pub(crate) fn process(&mut self, a: f32, b: f32) -> (f32, f32) {
        let level = a.abs().max(b.abs());
        let c = if level > self.env { self.atk } else { self.rel };
        self.env = level + c * (self.env - level);
        let env_db = 20.0 * self.env.max(1e-9).log10();
        // Porte avec hystérésis de 4 dB : elle s'ouvre vite (2 ms) et se referme doucement (80 ms), sans claquement.
        let mut gate = 1.0;
        if self.cfg.gate_db > -90.0 {
            if env_db > self.cfg.gate_db {
                self.open = true;
            } else if env_db < self.cfg.gate_db - 4.0 {
                self.open = false;
            }
            let (target, c) = if self.open { (1.0, self.gate_open) } else { (0.0, self.gate_close) };
            self.gate = target + c * (self.gate - target);
            gate = self.gate;
        }
        let over = env_db - self.cfg.threshold_db;
        let comp = if over > 0.0 { 10f32.powf(-over * self.slope / 20.0) } else { 1.0 };
        let g = comp * self.makeup * gate;
        (a * g, b * g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn sine(level_db: f32, n: usize) -> Vec<f32> {
        let a = 10f32.powf(level_db / 20.0);
        (0..n).map(|i| a * (std::f32::consts::TAU * 440.0 * i as f32 / SR as f32).sin()).collect()
    }

    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
    }

    fn run(cfg: InputDynamics, x: &[f32]) -> Vec<f32> {
        let mut s = DynState::new(cfg, SR);
        x.iter().map(|&v| s.process(v, v).0).collect()
    }

    #[test]
    fn defaults_are_valid_and_off_and_out_of_range_values_are_rejected() {
        let d = InputDynamics::default();
        assert!(d.is_valid() && !d.enabled && !d.record);
        for bad in [InputDynamics { ratio: 0.5, ..d }, InputDynamics { threshold_db: 5.0, ..d }, InputDynamics { attack_ms: 0.0, ..d }, InputDynamics { makeup_db: f32::NAN, ..d }, InputDynamics { gate_db: -10.0, ..d }, InputDynamics { release_ms: 5000.0, ..d }] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
    }

    #[test]
    fn above_the_threshold_the_level_follows_the_ratio() {
        // Régime établi : −8 dB avec un seuil à −20 dB et un rapport de 4 → 12 dB au-dessus, réduits de 12 × (1 − 1/4) = 9 dB → −17 dB.
        let cfg = InputDynamics { enabled: true, threshold_db: -20.0, ratio: 4.0, attack_ms: 1.0, release_ms: 50.0, ..Default::default() };
        let y = run(cfg, &sine(-8.0, SR as usize));
        let out_db = 20.0 * peak(&y[SR as usize / 2..]).log10();
        assert!((out_db + 17.0).abs() < 0.6, "niveau de sortie {out_db:.2} dB (attendu −17)");
        // Sous le seuil : rien ne bouge.
        let quiet = sine(-30.0, SR as usize);
        let y = run(cfg, &quiet);
        assert!((peak(&y[SR as usize / 2..]) / peak(&quiet) - 1.0).abs() < 0.01);
    }

    #[test]
    fn makeup_gain_adds_after_compression() {
        let base = InputDynamics { enabled: true, threshold_db: -20.0, ratio: 4.0, attack_ms: 1.0, release_ms: 50.0, ..Default::default() };
        let loud = sine(-8.0, SR as usize);
        let a = peak(&run(base, &loud)[SR as usize / 2..]);
        let b = peak(&run(InputDynamics { makeup_db: 6.0, ..base }, &loud)[SR as usize / 2..]);
        assert!((20.0 * (b / a).log10() - 6.0).abs() < 0.1);
    }

    #[test]
    fn the_gate_closes_on_a_quiet_signal_and_reopens_on_a_loud_one() {
        let cfg = InputDynamics { enabled: true, gate_db: -50.0, threshold_db: 0.0, ratio: 1.0, ..Default::default() };
        let mut x = sine(-70.0, SR as usize / 2); // un souffle sous le seuil
        x.extend(sine(-20.0, SR as usize / 2)); // puis du signal
        let y = run(cfg, &x);
        let hiss = peak(&y[SR as usize / 4..SR as usize / 2]);
        assert!(hiss < 0.1 * peak(&x[SR as usize / 4..SR as usize / 2]), "le souffle est étouffé : {hiss}");
        let tail = peak(&y[SR as usize * 3 / 4..]);
        assert!((tail / peak(&x[SR as usize * 3 / 4..]) - 1.0).abs() < 0.05, "le signal repasse en entier");
    }

    #[test]
    fn an_attack_lets_the_first_milliseconds_through_and_a_release_recovers() {
        let cfg = InputDynamics { enabled: true, threshold_db: -30.0, ratio: 20.0, attack_ms: 20.0, release_ms: 200.0, ..Default::default() };
        let mut x = sine(-60.0, 2_000);
        x.extend(sine(-3.0, 9_600)); // un coup fort de 200 ms
        x.extend(sine(-60.0, 72_000));
        let y = run(cfg, &x);
        let first = peak(&y[2_000..2_100]); // 2 ms après le coup : l'attaque n'a pas encore agi
        let later = peak(&y[2_000 + 6_000..2_000 + 9_600]);
        assert!(first > 3.0 * later, "attaque : {first} contre {later}");
        // Après le coup, le gain remonte : le signal faible retrouve presque son niveau (relâchement de 200 ms : l'enveloppe repasse sous le seuil après 0,2 × ln(22) ≈ 0,6 s, on regarde 1,4 s plus tard).
        let end = peak(&y[x.len() - 4_800..]);
        assert!((end / peak(&x[x.len() - 4_800..]) - 1.0).abs() < 0.15, "récupération : {end}");
    }
}
