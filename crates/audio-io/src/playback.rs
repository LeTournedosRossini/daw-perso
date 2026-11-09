//! Lecture : brancher un générateur stéréo (typiquement le moteur) sur le callback d'un pilote.
//!
//! `audio-io` ne connaît pas le moteur : il appelle un [`StereoProcessor`] à chaque cycle, avec le
//! chronométrage de [`CycleTimer`] autour. Le programme appelant fournit le processeur et une fonction de
//! contrôle (exécutée sur le thread principal, hors temps réel).

use crate::timing::{Clock, CycleTimer, TimingSummary};

/// Générateur audio appelé depuis le thread temps réel. Il ne doit ni allouer, ni bloquer, ni paniquer.
pub trait StereoProcessor: Send + 'static {
    /// Reçoit l'entrée audio du pilote (`in_l`, `in_r`) et remplit `out_l` et `out_r` ; les quatre tampons ont la même
    /// longueur. Sans entrée disponible, `in_l` et `in_r` sont du silence.
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]);
    /// Comme `process`, avec **tous** les canaux d'entrée que le pilote a ouverts (un tampon par canal, au moins deux : le
    /// backend complète par du silence). Par défaut, seuls les deux premiers servent (`process`).
    fn process_multi(&mut self, inputs: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32]) {
        let a = inputs.first().copied().unwrap_or(&[]);
        let b = inputs.get(1).copied().unwrap_or(a);
        self.process(a, b, out_l, out_r);
    }
    /// Comme `process_multi`, avec **tous** les canaux de sortie que le pilote a ouverts : `outs[0]` et `outs[1]` sont la sortie
    /// principale, les suivants les retours de monitoring (au moins deux canaux ; la longueur est le nombre de sorties ouvertes). Par
    /// défaut, seule la sortie principale est remplie et les autres canaux sont mis à zéro.
    fn process_outputs(&mut self, inputs: &[&[f32]], outs: &mut [&mut [f32]]) {
        if let Some((main, rest)) = outs.split_first_mut() {
            if let Some((second, others)) = rest.split_first_mut() {
                self.process_multi(inputs, main, second);
                for o in others {
                    o.fill(0.0);
                }
            }
        }
    }
    /// Message MIDI reçu du pilote pour le bloc qui va être traité : `offset` échantillons après son début. Appelée avant
    /// `process`, dans le même thread. Par défaut, ignoré.
    fn midi(&mut self, _offset: u32, _bytes: &[u8]) {}
}

/// Enveloppe qui chronomètre les cycles autour d'un processeur.
pub struct TimedProcessor<P> {
    inner: P,
    timer: CycleTimer,
    clock: Clock,
}

impl<P: StereoProcessor> TimedProcessor<P> {
    pub fn new(inner: P, sample_rate: u32) -> Self {
        TimedProcessor { inner, timer: CycleTimer::new(sample_rate), clock: Clock::new() }
    }

    #[inline]
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        self.timer.begin(self.clock.now_ns(), out_l.len());
        self.inner.process(in_l, in_r, out_l, out_r);
        self.timer.end(self.clock.now_ns());
    }

    #[inline]
    pub fn process_outputs(&mut self, inputs: &[&[f32]], outs: &mut [&mut [f32]]) {
        let n = outs.first().map_or(0, |o| o.len());
        self.timer.begin(self.clock.now_ns(), n);
        self.inner.process_outputs(inputs, outs);
        self.timer.end(self.clock.now_ns());
    }

    #[inline]
    pub fn process_multi(&mut self, inputs: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32]) {
        self.timer.begin(self.clock.now_ns(), out_l.len());
        self.inner.process_multi(inputs, out_l, out_r);
        self.timer.end(self.clock.now_ns());
    }

    #[inline]
    pub fn midi(&mut self, offset: u32, bytes: &[u8]) {
        self.inner.midi(offset, bytes);
    }

    pub fn timing(&self) -> Option<TimingSummary> {
        self.timer.summary()
    }
}

/// Latences du pilote, connues une fois le flux démarré (spécification ASIO : elles ne sont fiables qu'après la création
/// des tampons) ; lues sans verrou depuis n'importe quel fil.
#[derive(Debug, Default)]
pub struct StreamLatency {
    input: std::sync::atomic::AtomicU32,
    output: std::sync::atomic::AtomicU32,
    ready: std::sync::atomic::AtomicBool,
}

impl StreamLatency {
    pub fn set(&self, input: u32, output: u32) {
        use std::sync::atomic::Ordering::{Relaxed, Release};
        self.input.store(input, Relaxed);
        self.output.store(output, Relaxed);
        self.ready.store(true, Release);
    }

    /// `(entrée, sortie)` en échantillons, ou `None` tant que le flux n'a pas démarré.
    pub fn get(&self) -> Option<(u32, u32)> {
        use std::sync::atomic::Ordering::{Acquire, Relaxed};
        self.ready.load(Acquire).then(|| (self.input.load(Relaxed), self.output.load(Relaxed)))
    }
}

/// Délai des convertisseurs analogique-numérique (échantillons), à ajouter aux latences annoncées par le pilote : mesuré sur la
/// UMC 1820 (latence mesurée = latence annoncée en entrée + en sortie + 65 à 80 échantillons, voir `CLAUDE.md`).
pub const CONVERTER_DELAY: u64 = 72;

/// Latence aller-retour (échantillons) à retirer d'une prise pour la ranger sur la ligne de temps : latences annoncées par le pilote
/// en entrée et en sortie, plus le délai des convertisseurs. **La même formule pour l'application et pour `daw-cli sync-test`.**
pub fn record_compensation(input: u32, output: u32) -> u64 {
    input as u64 + output as u64 + CONVERTER_DELAY
}

/// Ce que le backend sait du flux au moment où il demande le processeur.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub driver: String,
    pub sample_rate: u32,
    pub buffer_size: u32,
    /// Nombre de canaux d'entrée ouverts (0 : aucun).
    pub inputs: u32,
    /// Nombre de canaux de sortie ouverts (au moins 2 : la sortie principale ; les suivants servent aux retours de monitoring).
    pub outputs: u32,
    /// Latences du pilote, renseignées après le démarrage.
    pub latency: std::sync::Arc<StreamLatency>,
}

/// Fonction de contrôle : reçoit le temps écoulé, appelée toutes les ~10 ms hors temps réel ;
/// retourne `false` pour arrêter la lecture.
pub type ControlFn = Box<dyn FnMut(std::time::Duration) -> bool>;

#[derive(Debug)]
pub struct PlaybackReport {
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub output: String,
    pub timing: Option<TimingSummary>,
    pub xruns: u32,
    /// Allocations/libérations constatées dans le callback (doit rester 0).
    pub callback_allocations: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ramp(f32);
    impl StereoProcessor for Ramp {
        fn process(&mut self, _: &[f32], _: &[f32], l: &mut [f32], r: &mut [f32]) {
            for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                *a = self.0;
                *b = -self.0;
                self.0 += 1.0;
            }
        }
    }

    #[test]
    fn timed_processor_forwards_calls_and_times_them() {
        let mut t = TimedProcessor::new(Ramp(0.0), 48_000);
        let (mut l, mut r) = ([0.0f32; 4], [0.0f32; 4]);
        for _ in 0..40 {
            t.process(&[0.0; 4], &[0.0; 4], &mut l, &mut r);
        }
        assert_eq!(l, [156.0, 157.0, 158.0, 159.0]);
        assert_eq!(r[0], -156.0);
        assert!(t.timing().is_some());
    }
}
