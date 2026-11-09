//! Métronome : un clic synchronisé sur la carte des tempos, et un pré-compte.
//!
//! Le contrôle (qui connaît tempo et chiffrage) calcule la position de chaque temps en échantillons et fabrique les deux formes
//! d'onde du clic ; le fil audio ne fait que **copier** ces échantillons au bon moment (aucune allocation, aucun calcul de
//! sinus). Les temps sont en positions absolues de la ligne de temps : déplacement, boucle et arrêt sont donc corrects sans
//! rien d'autre. Le pré-compte est un cas à part : la tête de lecture est immobile, les clics se mesurent depuis son début, et la
//! lecture démarre toute seule à la fin.

/// Un temps de la ligne de temps : sa position absolue (échantillons) et s'il est accentué (premier temps de la mesure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Click {
    pub pos: u64,
    pub accent: bool,
}

/// Les temps du projet (triés par position) et les deux sons du clic, déjà au niveau 1,0 (le volume se règle à part).
#[derive(Debug, Clone)]
pub struct ClickTrack {
    pub clicks: Vec<Click>,
    pub normal: Vec<f32>,
    pub accent: Vec<f32>,
}

impl ClickTrack {
    /// Un clic de bois bref : sinus de 1 000 Hz (1 500 Hz accentué), 25 ms, décroissance exponentielle.
    pub fn tone(sample_rate: u32, accent: bool) -> Vec<f32> {
        let rate = sample_rate.max(1) as f32;
        let (freq, len) = (if accent { 1_500.0f32 } else { 1_000.0 }, (rate * 0.025) as usize);
        (0..len.max(1))
            .map(|k| {
                let t = k as f32 / rate;
                0.8 * (2.0 * std::f32::consts::PI * freq * t).sin() * (-t / 0.006).exp()
            })
            .collect()
    }

    pub fn new(sample_rate: u32, clicks: Vec<Click>) -> ClickTrack {
        ClickTrack { clicks, normal: Self::tone(sample_rate, false), accent: Self::tone(sample_rate, true) }
    }
}

/// Pré-compte : les clics (décalages en échantillons depuis son début, et accent) et sa durée totale.
#[derive(Debug, Clone)]
pub struct CountIn {
    pub clicks: Vec<(u64, bool)>,
    pub frames: u64,
}

const VOICES: usize = 8;

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    accent: bool,
    /// Échantillons à attendre avant de commencer (début situé plus loin dans le bloc en cours).
    wait: usize,
    /// Prochain échantillon de la forme d'onde.
    idx: usize,
}

/// État du métronome côté audio.
pub(crate) struct Metronome {
    track: Option<Box<ClickTrack>>,
    /// Pré-compte en cours : le pré-compte, ce qui en est écoulé et le prochain clic à lancer.
    count_in: Option<(Box<CountIn>, u64, usize)>,
    enabled: bool,
    gain: f32,
    voices: [Voice; VOICES],
}

impl Metronome {
    pub(crate) fn new() -> Metronome {
        Metronome { track: None, count_in: None, enabled: false, gain: 0.5, voices: [Voice::default(); VOICES] }
    }

    pub(crate) fn set(&mut self, enabled: bool, gain: f32) {
        if self.enabled && !enabled {
            // Éteint : les clics en cours de son s'arrêtent tout de suite (pas de queue qui traîne).
            self.voices = [Voice::default(); VOICES];
        }
        self.enabled = enabled;
        if gain.is_finite() {
            self.gain = gain.clamp(0.0, 4.0);
        }
    }

    /// Remplace les temps ; rend les anciens pour qu'ils soient libérés hors du fil audio.
    pub(crate) fn set_track(&mut self, track: Option<Box<ClickTrack>>) -> Option<Box<ClickTrack>> {
        std::mem::replace(&mut self.track, track)
    }

    /// Démarre un pré-compte (le précédent, s'il y en avait un, est rendu pour être libéré).
    pub(crate) fn set_count_in(&mut self, c: Option<Box<CountIn>>) -> Option<Box<CountIn>> {
        let old = self.count_in.take().map(|(c, _, _)| c);
        self.count_in = c.map(|c| (c, 0, 0));
        old
    }

    /// Échantillons restants du pré-compte, s'il y en a un.
    pub(crate) fn count_in_remaining(&self) -> Option<u64> {
        self.count_in.as_ref().map(|(c, done, _)| c.frames.saturating_sub(*done))
    }

    fn start(&mut self, accent: bool, wait: usize) {
        if let Some(v) = self.voices.iter_mut().find(|v| !v.active) {
            *v = Voice { active: true, accent, wait, idx: 0 };
        }
    }

    /// Mélange le clic dans `out_l` / `out_r` pour `n` échantillons à partir de la position `pos`. `playing` : la tête avance (les
    /// temps du projet ne sonnent que dans ce cas). Retourne vrai quand le pré-compte vient de se terminer.
    pub(crate) fn render(&mut self, pos: u64, n: usize, playing: bool, out_l: &mut [f32], out_r: &mut [f32]) -> bool {
        let mut finished = false;
        if let Some((c, done, next)) = self.count_in.as_mut() {
            let end = *done + n as u64;
            let mut starts: [(usize, bool); VOICES] = [(0, false); VOICES];
            let mut count = 0;
            while *next < c.clicks.len() && c.clicks[*next].0 < end && count < VOICES {
                let (at, accent) = c.clicks[*next];
                if at >= *done {
                    starts[count] = ((at - *done) as usize, accent);
                    count += 1;
                }
                *next += 1;
            }
            *done = end;
            finished = *done >= c.frames;
            for &(wait, accent) in &starts[..count] {
                self.start(accent, wait);
            }
            if finished {
                self.count_in = None;
            }
        } else if self.enabled && playing {
            if let Some(track) = self.track.as_ref() {
                let end = pos + n as u64;
                let first = track.clicks.partition_point(|c| c.pos < pos);
                let mut starts: [(usize, bool); VOICES] = [(0, false); VOICES];
                let mut count = 0;
                for c in track.clicks[first..].iter().take_while(|c| c.pos < end).take(VOICES) {
                    starts[count] = ((c.pos - pos) as usize, c.accent);
                    count += 1;
                }
                for &(wait, accent) in &starts[..count] {
                    self.start(accent, wait);
                }
            }
        }
        self.mix(n, out_l, out_r);
        finished
    }

    fn mix(&mut self, n: usize, out_l: &mut [f32], out_r: &mut [f32]) {
        let Some(track) = self.track.as_ref() else {
            self.voices = [Voice::default(); VOICES];
            return;
        };
        let gain = self.gain;
        for v in self.voices.iter_mut().filter(|v| v.active) {
            let wave = if v.accent { &track.accent } else { &track.normal };
            for j in v.wait.min(n)..n {
                match wave.get(v.idx) {
                    Some(s) => {
                        out_l[j] += s * gain;
                        out_r[j] += s * gain;
                        v.idx += 1;
                    }
                    None => {
                        v.active = false;
                        break;
                    }
                }
            }
            v.wait = v.wait.saturating_sub(n);
            if v.idx >= wave.len() {
                v.active = false;
            }
        }
    }
}
