//! Outils communs aux tests de rendu.
#![allow(dead_code)]

use std::sync::Arc;

use daw_engine::{AudioData, Clip, GraphDesc, Insert, InsertFactory};
use daw_offline_render::{render, RenderSpec, Rendered};

pub const SR: u32 = 48_000;

/// Signal pseudo-aléatoire reproductible dans [-0.5, 0.5[ (congruentiel linéaire : opérations entières seulement).
pub fn noise(seed: u32, len: usize) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 16_777_216.0 - 0.5
        })
        .collect()
}

pub fn mono(data: Vec<f32>) -> Arc<AudioData> {
    AudioData::new(SR, vec![data]).unwrap()
}

pub fn stereo(l: Vec<f32>, r: Vec<f32>) -> Arc<AudioData> {
    AudioData::new(SR, vec![l, r]).unwrap()
}

pub fn clip(data: &Arc<AudioData>, start: u64) -> Clip {
    Clip::whole(data.clone(), start)
}

pub fn render_simple(graph: GraphDesc, frames: u64) -> Rendered {
    render(&RenderSpec::new(SR, frames, graph)).unwrap()
}

/// Effet de test : retard pur de `latency` échantillons, multiplié par un gain (paramètre 0, défaut 1).
pub struct DelayGain {
    l: Vec<f32>,
    r: Vec<f32>,
    pos: usize,
    latency: u32,
    gain: f32,
}

impl DelayGain {
    pub fn new(latency: u32) -> Self {
        DelayGain { l: vec![0.0; latency as usize], r: vec![0.0; latency as usize], pos: 0, latency, gain: 1.0 }
    }
}

impl Insert for DelayGain {
    fn latency(&self) -> u32 {
        self.latency
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        for j in 0..in_l.len() {
            if self.l.is_empty() {
                out_l[j] = in_l[j] * self.gain;
                out_r[j] = in_r[j] * self.gain;
            } else {
                out_l[j] = self.l[self.pos] * self.gain;
                out_r[j] = self.r[self.pos] * self.gain;
                self.l[self.pos] = in_l[j];
                self.r[self.pos] = in_r[j];
                self.pos = (self.pos + 1) % self.l.len();
            }
        }
    }
    fn set_param(&mut self, param: u32, value: f64) {
        if param == 0 {
            self.gain = value as f32;
        }
    }
    fn reset(&mut self) {
        self.l.fill(0.0);
        self.r.fill(0.0);
        self.pos = 0;
    }
}

pub fn delay_factory(latency: u32) -> InsertFactory {
    std::sync::Arc::new(move |_, _, _| Ok(Box::new(DelayGain::new(latency)) as Box<dyn Insert>))
}

/// Piste stéréo dont le seul contenu est une impulsion de 1,0 à `at`.
pub fn impulse_track(g: &mut GraphDesc, name: &str, at: u64, len: usize) -> daw_core_types::NodeId {
    let mut x = vec![0.0f32; len];
    x[at as usize] = 1.0;
    let t = g.add_track(name, daw_engine::TrackFormat::Stereo);
    g.add_clip(t, clip(&stereo(x.clone(), x), 0));
    t
}
