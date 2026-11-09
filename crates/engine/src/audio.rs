//! Données audio en mémoire et clips posés sur la ligne de temps.

use std::sync::Arc;

/// Échantillons immuables, en mémoire, un tampon par canal (1 ou 2). Partagés par `Arc` : le thread
/// audio ne fait que les lire, et la dernière référence est libérée hors du thread audio.
#[derive(Debug)]
pub struct AudioData {
    sample_rate: u32,
    channels: Vec<Vec<f32>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AudioDataError {
    ChannelCount(usize),
    LengthMismatch,
}

impl AudioData {
    pub fn new(sample_rate: u32, channels: Vec<Vec<f32>>) -> Result<Arc<AudioData>, AudioDataError> {
        if channels.is_empty() || channels.len() > 2 {
            return Err(AudioDataError::ChannelCount(channels.len()));
        }
        if channels.iter().any(|c| c.len() != channels[0].len()) {
            return Err(AudioDataError::LengthMismatch);
        }
        Ok(Arc::new(AudioData { sample_rate, channels }))
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn len(&self) -> u64 {
        self.channels[0].len() as u64
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
    /// Échantillons d'un canal (0 = gauche).
    pub fn channel(&self, c: usize) -> &[f32] {
        &self.channels[c]
    }
    #[inline]
    pub(crate) fn left(&self) -> &[f32] {
        &self.channels[0]
    }
    #[inline]
    pub(crate) fn right(&self) -> &[f32] {
        self.channels.get(1).map_or(&self.channels[0], |c| c)
    }
}

/// Forme de base d'un fondu. Tout est calculé avec `+ − × /` et `sqrt` seulement (exacts, donc identiques bit à bit sur
/// toutes les plateformes et quelle que soit la taille de bloc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FadeShape {
    /// Gain linéaire : somme des gains = 1 dans un fondu enchaîné (bon pour des signaux identiques ou corrélés).
    #[default]
    Linear,
    /// Puissance constante (`sqrt` de la courbe) : somme des puissances = 1 (bon pour des signaux sans rapport).
    EqualPower,
}

impl FadeShape {
    pub const ALL: [FadeShape; 2] = [FadeShape::Linear, FadeShape::EqualPower];
}

/// Fondu d'entrée ou de sortie : `len` échantillons (0 = aucun).
///
/// La courbe se règle par trois choses, appliquées dans cet ordre à `x` (0 = bord du clip, 1 = clip pleinement audible) :
/// 1. **adoucissement en S** (`smooth`, de 0 à 1000 millièmes) : mélange la droite avec `3x² − 2x³` ;
/// 2. **courbure** (`bend`, de −1000 à +1000 millièmes) : `u − c·u·(1 − u)` — 0 = droite, +1000 = convexe `u²` (le gain monte
///    tard), −1000 = concave `2u − u²` (le gain monte tôt) ; toujours croissante ;
/// 3. **forme** : linéaire (telle quelle) ou puissance constante (racine carrée).
///
/// Deux fondus enchaînés de courbures opposées (et de même adoucissement) se complètent : la somme des gains reste 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fade {
    pub len: u64,
    pub shape: FadeShape,
    pub bend: i16,
    pub smooth: u16,
}

impl Fade {
    pub const NONE: Fade = Fade { len: 0, shape: FadeShape::Linear, bend: 0, smooth: 0 };

    pub fn new(len: u64, shape: FadeShape) -> Fade {
        Fade { len, shape, bend: 0, smooth: 0 }
    }

    /// Fondu avec courbure et adoucissement (bornés à leurs plages).
    pub fn curved(len: u64, shape: FadeShape, bend: i16, smooth: u16) -> Fade {
        Fade { len, shape, bend: bend.clamp(-1000, 1000), smooth: smooth.min(1000) }
    }

    /// Le même fondu avec une autre longueur (la courbe est conservée).
    pub fn with_len(self, len: u64) -> Fade {
        Fade { len, ..self }
    }

    /// Valeur de la courbe pour `x` entre 0 et 1.
    #[inline]
    pub fn curve(&self, x: f32) -> f32 {
        let s = self.smooth as f32 / 1000.0;
        let u = if self.smooth == 0 { x } else { x + s * (x * x * (3.0 - 2.0 * x) - x) };
        let c = self.bend as f32 / 1000.0;
        let v = if self.bend == 0 { u } else { u - c * u * (1.0 - u) };
        match self.shape {
            FadeShape::Linear => v,
            FadeShape::EqualPower => v.max(0.0).sqrt(),
        }
    }

    /// Gain du `k`-ième échantillon d'un fondu d'entrée (`k < len`), croissant strictement entre 0 et 1.
    /// Un fondu de sortie utilise `gain(m)` avec `m` = nombre d'échantillons restants après le courant :
    /// les deux fondus d'un enchaîné se complètent alors exactement.
    #[inline]
    pub fn gain(&self, k: u64) -> f32 {
        self.curve((k as f32 + 0.5) / self.len as f32)
    }
}

/// Erreurs des opérations d'édition d'un clip.
#[derive(Debug, PartialEq, Eq)]
pub enum EditError {
    /// Position hors du clip ou donnant un clip vide.
    OutOfRange,
    /// Les clips ne se chevauchent pas comme il faut pour un fondu enchaîné.
    NoOverlap,
}

/// Portion de `data` posée sur la ligne de temps : `len` échantillons à partir de `offset` dans les
/// données, placés à `start` (en échantillons depuis le début du projet), multipliés par `gain` et par les fondus.
#[derive(Debug, Clone)]
pub struct Clip {
    pub data: Arc<AudioData>,
    pub start: u64,
    pub offset: u64,
    pub len: u64,
    pub gain: f32,
    pub fade_in: Fade,
    pub fade_out: Fade,
    /// Clip muet : conservé dans le graphe (placement, fondus, découpe) mais jamais joué. Sert aux prises inactives d'un groupe
    /// de prises et à « couper un clip ».
    pub muted: bool,
    /// Groupe de prises (0 = aucun) : métadonnée portée par le clip pour que découpe, annulation et copie la conservent ; le moteur
    /// ne la lit pas. Voir `session::takes`.
    pub take_group: u32,
}

impl Clip {
    pub fn new(data: Arc<AudioData>, start: u64, offset: u64, len: u64, gain: f32) -> Clip {
        Clip { data, start, offset, len, gain, fade_in: Fade::NONE, fade_out: Fade::NONE, muted: false, take_group: 0 }
    }

    /// Clip couvrant toutes les données, gain unité.
    pub fn whole(data: Arc<AudioData>, start: u64) -> Clip {
        let len = data.len();
        Clip::new(data, start, 0, len, 1.0)
    }

    pub fn end(&self) -> u64 {
        self.start + self.len
    }

    /// Les fondus tiennent dans le clip.
    pub(crate) fn fades_valid(&self) -> bool {
        self.fade_in.len <= self.len && self.fade_out.len <= self.len
    }

    /// Gain total (clip × fondus) de l'échantillon `rel` (position relative au début du clip, `rel < len`).
    #[inline]
    pub(crate) fn gain_at(&self, rel: u64) -> f32 {
        let mut g = self.gain;
        if rel < self.fade_in.len {
            g *= self.fade_in.gain(rel);
        }
        let left = self.len - 1 - rel;
        if left < self.fade_out.len {
            g *= self.fade_out.gain(left);
        }
        g
    }

    /// Le même clip posé ailleurs.
    pub fn moved(&self, start: u64) -> Clip {
        Clip { start, ..self.clone() }
    }

    /// Découpe en `at` (position absolue, strictement à l'intérieur) : la partie gauche garde le fondu d'entrée, la droite
    /// le fondu de sortie. Les deux moitiés rejouées ensemble sonnent comme le clip d'origine, sauf dans les fondus.
    pub fn split(&self, at: u64) -> Result<(Clip, Clip), EditError> {
        if at <= self.start || at >= self.end() {
            return Err(EditError::OutOfRange);
        }
        let d = at - self.start;
        let mut left = Clip { len: d, fade_out: Fade::NONE, ..self.clone() };
        let mut right = Clip { start: at, offset: self.offset + d, len: self.len - d, fade_in: Fade::NONE, ..self.clone() };
        left.fade_in.len = left.fade_in.len.min(left.len);
        right.fade_out.len = right.fade_out.len.min(right.len);
        Ok((left, right))
    }

    /// Déplace le début vers `new_start` (le contenu ne bouge pas sur la ligne de temps) : vers la droite raccourcit,
    /// vers la gauche rallonge en découvrant les données précédentes (refusé s'il n'y en a pas assez).
    pub fn trim_start(&self, new_start: u64) -> Result<Clip, EditError> {
        let end = self.end();
        if new_start >= end {
            return Err(EditError::OutOfRange);
        }
        let mut c = self.clone();
        if new_start >= self.start {
            let d = new_start - self.start;
            c.offset += d;
        } else {
            let d = self.start - new_start;
            if d > self.offset {
                return Err(EditError::OutOfRange);
            }
            c.offset -= d;
        }
        c.start = new_start;
        c.len = end - new_start;
        c.fade_in.len = c.fade_in.len.min(c.len);
        c.fade_out.len = c.fade_out.len.min(c.len);
        Ok(c)
    }

    /// Déplace la fin vers `new_end` (absolue) : rallonger exige assez de données après le clip.
    pub fn trim_end(&self, new_end: u64) -> Result<Clip, EditError> {
        if new_end <= self.start {
            return Err(EditError::OutOfRange);
        }
        let len = new_end - self.start;
        if self.offset.checked_add(len).is_none_or(|e| e > self.data.len()) {
            return Err(EditError::OutOfRange);
        }
        let mut c = Clip { len, ..self.clone() };
        c.fade_in.len = c.fade_in.len.min(c.len);
        c.fade_out.len = c.fade_out.len.min(c.len);
        Ok(c)
    }
}

/// Fondu enchaîné entre deux clips d'une même piste : `a` commence avant `b` et finit après le début de `b`.
/// La zone commune devient le fondu de sortie de `a` et le fondu d'entrée de `b` (même forme).
pub fn crossfade(a: &Clip, b: &Clip, shape: FadeShape) -> Result<(Clip, Clip), EditError> {
    if !(a.start < b.start && b.start < a.end() && a.end() <= b.end()) {
        return Err(EditError::NoOverlap);
    }
    let overlap = a.end() - b.start;
    Ok((Clip { fade_out: Fade::new(overlap, shape), ..a.clone() }, Clip { fade_in: Fade::new(overlap, shape), ..b.clone() }))
}
