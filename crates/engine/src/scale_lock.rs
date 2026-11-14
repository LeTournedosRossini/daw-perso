//! Verrou de gamme à l'entrée d'une piste : une note jouée **en direct** (clavier MIDI, `Command::Midi`) est ramenée à la gamme ou à
//! l'accord en vigueur de la piste d'accords à l'instant où elle part. Les clips de la piste ne sont jamais touchés (c'est une action
//! explicite du piano roll). Sans gamme (ou sans accord) à cet instant, rien n'est contraint.
//!
//! La fin d'une note doit rejoindre la touche **réellement** envoyée : le moteur retient, par canal et par touche jouée, la touche
//! émise (`LockMap`). Ensemble de classes de hauteur = masque de 12 bits (bit `pc`).

use crate::chords::{ChordSnapshot, FLAG_NC, KIND_CHORD, KIND_SCALE, NO_BASS};

/// Contrainte appliquée aux notes jouées en direct sur une piste.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleLock {
    /// Aucune (les notes passent telles quelles).
    #[default]
    Off,
    /// Notes de la gamme en vigueur.
    Scale,
    /// Notes de l'accord en vigueur (basse comprise).
    Chord,
}

/// Touche émise pour chaque touche jouée, par canal (`NONE` : rien de retenu).
pub type LockMap = [[u8; 128]; 16];
pub const NONE: u8 = 255;

pub fn new_map() -> Box<LockMap> {
    Box::new([[NONE; 128]; 16])
}

fn rotate(mask: u32, root: u8) -> u16 {
    let mut out = 0u16;
    for i in 0..24u32 {
        if mask >> i & 1 == 1 {
            out |= 1 << ((u32::from(root) + i) % 12);
        }
    }
    out
}

/// Classes de hauteur permises à `beat` (noires) ; `None` : tout est permis.
pub fn allowed(chords: &ChordSnapshot, beat: f64, lock: ScaleLock) -> Option<u16> {
    match lock {
        ScaleLock::Off => None,
        ScaleLock::Scale => chords.lookup(beat, KIND_SCALE).filter(|s| s.mask != 0).map(|s| rotate(s.mask, s.root)),
        ScaleLock::Chord => chords.lookup(beat, KIND_CHORD).filter(|c| c.flags & FLAG_NC == 0 && c.mask != 0).map(|c| {
            let mut set = rotate(c.mask, c.root);
            if c.bass != NO_BASS {
                set |= 1 << (c.bass % 12);
            }
            set
        }),
    }
}

/// Touche permise la plus proche de `key` (à égalité la plus grave), bornée à 0-127 ; `key` si le masque est vide.
pub fn snap_key(key: u8, set: u16) -> u8 {
    if set == 0 || set >> (key % 12) & 1 == 1 {
        return key;
    }
    for d in 1..=12i16 {
        for k in [i16::from(key) - d, i16::from(key) + d] {
            if (0..=127).contains(&k) && set >> (k % 12) & 1 == 1 {
                return k as u8;
            }
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAJOR: u32 = 0b1010_1011_0101; // 0 2 4 5 7 9 11

    #[test]
    fn nearest_allowed_key_ties_go_down() {
        let c_major = rotate(MAJOR, 0);
        assert_eq!(snap_key(60, c_major), 60);
        assert_eq!(snap_key(61, c_major), 60);
        assert_eq!(snap_key(66, c_major), 65);
        assert_eq!(snap_key(61, 0), 61);
        assert_eq!(snap_key(0, 1 << 2), 2);
        assert_eq!(snap_key(127, 1), 120);
    }

    #[test]
    fn scales_rotate_with_their_root() {
        let d_major = rotate(MAJOR, 2);
        assert!(d_major >> 1 & 1 == 1 && d_major >> 6 & 1 == 1, "ré majeur : do# et fa#");
        assert!(d_major >> 0 & 1 == 0);
    }
}
