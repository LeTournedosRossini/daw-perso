//! Niveaux de crête par nœud, du thread audio vers l'interface.
//!
//! Le thread audio dépose la crête de chaque bloc (après le fader) avec un `fetch_max` atomique ; l'interface lit et remet à
//! zéro (`take`) à son rythme (~30 Hz) : elle obtient ainsi la plus forte crête depuis sa dernière lecture, même si
//! plusieurs blocs ont passé. Les valeurs positives en `f32` se comparent comme leurs bits entiers, donc `fetch_max`
//! sur les bits est exact. Sans verrou, sans allocation côté audio.

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

/// Nombre de nœuds dont on suit le niveau (les suivants ne sont pas mesurés).
pub const MAX_METERED: usize = 256;

/// Bits du témoin d'entrée MIDI : notes, contrôleurs, molette / pressions.
pub const MIDI_NOTE: u8 = 1;
pub const MIDI_CONTROL: u8 = 2;
pub const MIDI_BEND: u8 = 4;

pub struct MeterBank {
    levels: Vec<AtomicU32>,
    /// Types de messages MIDI reçus en direct par chaque nœud depuis la dernière lecture (bits `MIDI_*`).
    midi: Vec<AtomicU8>,
    /// Vélocité brute (1 à 127) de la dernière note jouée en direct sur chaque nœud depuis la dernière lecture (0 : aucune).
    last_vel: Vec<AtomicU8>,
}

impl MeterBank {
    pub fn new() -> MeterBank {
        MeterBank { levels: (0..MAX_METERED * 2).map(|_| AtomicU32::new(0)).collect(), midi: (0..MAX_METERED).map(|_| AtomicU8::new(0)).collect(), last_vel: (0..MAX_METERED).map(|_| AtomicU8::new(0)).collect() }
    }

    /// Côté audio : mémorise la crête (valeurs absolues) du nœud `node`.
    #[inline]
    pub fn update(&self, node: usize, left: f32, right: f32) {
        if node < MAX_METERED {
            // Non fini (NaN, infini) : ignoré plutôt que de fausser l'affichage ou les comparaisons de bits.
            if left.is_finite() {
                self.levels[2 * node].fetch_max(left.abs().to_bits(), Ordering::Relaxed);
            }
            if right.is_finite() {
                self.levels[2 * node + 1].fetch_max(right.abs().to_bits(), Ordering::Relaxed);
            }
        }
    }

    /// Côté audio : note que le nœud a reçu des messages MIDI des types `bits` (sans verrou ni allocation).
    #[inline]
    pub fn note_midi(&self, node: usize, bits: u8) {
        if let Some(a) = self.midi.get(node) {
            a.fetch_or(bits, Ordering::Relaxed);
        }
    }

    /// Côté audio : vélocité brute (1 à 127) d'une note qui vient d'être jouée en direct sur le nœud.
    #[inline]
    pub fn note_velocity(&self, node: usize, velocity: u8) {
        if let Some(a) = self.last_vel.get(node) {
            a.store(velocity, Ordering::Relaxed);
        }
    }

    /// Côté interface : vélocité de la dernière note jouée depuis la dernière lecture (0 : aucune), puis remise à zéro.
    pub fn take_velocity(&self, node: usize) -> u8 {
        self.last_vel.get(node).map_or(0, |a| a.swap(0, Ordering::Relaxed))
    }

    /// Côté interface : types de messages MIDI reçus depuis la dernière lecture, puis remise à zéro.
    pub fn take_midi(&self, node: usize) -> u8 {
        self.midi.get(node).map_or(0, |a| a.swap(0, Ordering::Relaxed))
    }

    /// Côté interface : plus forte crête depuis la dernière lecture, puis remise à zéro.
    pub fn take(&self, node: usize) -> (f32, f32) {
        if node >= MAX_METERED {
            return (0.0, 0.0);
        }
        let l = f32::from_bits(self.levels[2 * node].swap(0, Ordering::Relaxed));
        let r = f32::from_bits(self.levels[2 * node + 1].swap(0, Ordering::Relaxed));
        (l, r)
    }
}

impl Default for MeterBank {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_largest_peak_until_read_then_resets() {
        let m = MeterBank::new();
        m.update(3, 0.2, -0.9);
        m.update(3, 0.7, 0.1);
        assert_eq!(m.take(3), (0.7, 0.9));
        assert_eq!(m.take(3), (0.0, 0.0));
        assert_eq!(m.take(4), (0.0, 0.0));
    }

    #[test]
    fn midi_activity_accumulates_until_read() {
        let m = MeterBank::new();
        m.note_midi(2, MIDI_NOTE);
        m.note_midi(2, MIDI_BEND);
        m.note_midi(MAX_METERED, MIDI_NOTE);
        assert_eq!(m.take_midi(2), MIDI_NOTE | MIDI_BEND);
        assert_eq!(m.take_midi(2), 0);
        assert_eq!(m.take_midi(MAX_METERED), 0);
    }

    #[test]
    fn ignores_non_finite_and_out_of_range_nodes() {
        let m = MeterBank::new();
        m.update(0, f32::NAN, f32::INFINITY);
        assert_eq!(m.take(0), (0.0, 0.0));
        m.update(MAX_METERED, 1.0, 1.0);
        assert_eq!(m.take(MAX_METERED), (0.0, 0.0));
    }
}
