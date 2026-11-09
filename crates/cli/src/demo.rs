//! Projet de démonstration : 4 pistes synthétisées (batterie, basse, nappe), 1 bus, boucle de 8 s,
//! avec quelques gestes de mixage scriptés pour entendre les paramètres lissés.

use std::f64::consts::TAU;
use std::sync::Arc;

use daw_audio_io::playback::{ControlFn, StereoProcessor};
use daw_core_types::NodeId;
use daw_clap_host::{clap_insert_factory, default_search_dirs, find_plugins, Bundle};
use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, MidiClip, NodeParams, Note, TrackFormat};

pub const LOOP_SECONDS: u32 = 8;

pub struct Demo {
    pub graph: GraphDesc,
    pub kick: NodeId,
    pub hat: NodeId,
    pub bass: NodeId,
    pub pad: NodeId,
}

fn lcg_noise(seed: u32, len: usize) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 16_777_216.0 * 2.0 - 1.0
        })
        .collect()
}

fn kick(rate: u32) -> Arc<AudioData> {
    let n = rate as usize / 2;
    let mut phase = 0.0f64;
    let data = (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            phase += TAU * (45.0 + 80.0 * (-t * 28.0).exp()) / rate as f64;
            (phase.sin() * (-t * 9.0).exp() * 0.9) as f32
        })
        .collect();
    AudioData::new(rate, vec![data]).unwrap()
}

fn hat(rate: u32) -> Arc<AudioData> {
    let n = rate as usize * 30 / 1000;
    let noise = lcg_noise(7, n);
    let data = noise.iter().enumerate().map(|(i, x)| x * (-(i as f32) / n as f32 * 6.0).exp() * 0.3).collect();
    AudioData::new(rate, vec![data]).unwrap()
}

fn bass_note(rate: u32, freq: f64) -> Arc<AudioData> {
    let n = rate as usize * 2;
    let data = (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            let env = (t / 0.005).min(1.0) * (-t * 1.1).exp();
            (((TAU * freq * t).sin() + 0.3 * (TAU * 2.0 * freq * t).sin()) * env * 0.45) as f32
        })
        .collect();
    AudioData::new(rate, vec![data]).unwrap()
}

fn pad_chord(rate: u32, freqs: &[f64]) -> Arc<AudioData> {
    let n = rate as usize * 2;
    let fade = rate as f64 * 0.15;
    let side = |detune: f64| -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f64 / rate as f64;
                let env = (i as f64 / fade).min(1.0).min((n - i) as f64 / fade).max(0.0);
                let s: f64 = freqs.iter().map(|f| (TAU * f * (1.0 + detune) * t).sin() * 0.09).sum();
                (s * env) as f32
            })
            .collect()
    };
    AudioData::new(rate, vec![side(-0.002), side(0.002)]).unwrap()
}

pub fn build(rate: u32) -> Demo {
    let mut g = GraphDesc::new();
    let drums = g.add_bus("batterie");
    let kick_t = g.add_track("grosse caisse", TrackFormat::Mono);
    let hat_t = g.add_track("charleston", TrackFormat::Mono);
    let bass_t = g.add_track("basse", TrackFormat::Mono);
    let pad_t = g.add_track("nappe", TrackFormat::Stereo);
    g.set_output(kick_t, drums);
    g.set_output(hat_t, drums);
    g.node_mut(drums).unwrap().params.gain = 0.9;
    g.node_mut(NodeId::MASTER).unwrap().params.gain = 0.5; // prudence : -6 dB

    let beat = rate as u64 / 2;
    let kick_data = kick(rate);
    let hat_data = hat(rate);
    for b in 0..(LOOP_SECONDS as u64 * 2) {
        g.add_clip(kick_t, Clip::whole(kick_data.clone(), b * beat));
    }
    for h in 0..(LOOP_SECONDS as u64 * 4) {
        // contretemps : le charleston tombe entre les temps
        g.add_clip(hat_t, Clip::whole(hat_data.clone(), h * beat / 2 + beat / 4));
    }
    let bass_freqs = [55.0, 55.0, 43.654, 48.999];
    let chords: [&[f64]; 4] = [&[220.0, 261.63, 329.63], &[220.0, 261.63, 329.63], &[174.61, 220.0, 261.63], &[196.0, 246.94, 293.66]];
    for bar in 0..4u64 {
        let start = bar * rate as u64 * 2;
        g.add_clip(bass_t, Clip::whole(bass_note(rate, bass_freqs[bar as usize]), start));
        g.add_clip(pad_t, Clip::whole(pad_chord(rate, chords[bar as usize]), start));
    }
    g.set_params(hat_t, NodeParams { gain: 1.0, pan: 0.4, ..Default::default() });
    g.set_params(bass_t, NodeParams { gain: 0.9, ..Default::default() });
    g.set_params(pad_t, NodeParams { gain: 0.8, ..Default::default() });
    Demo { graph: g, kick: kick_t, hat: hat_t, bass: bass_t, pad: pad_t }
}

impl Demo {
    /// Gestes de mixage scriptés : (instant en secondes, commande, texte affiché).
    pub fn script(&self) -> Vec<(f64, Command, &'static str)> {
        vec![
            (4.0, Command::SetPan { node: self.pad.0, pan: -0.8 }, "nappe à gauche"),
            (8.0, Command::SetPan { node: self.pad.0, pan: 0.8 }, "nappe à droite"),
            (10.0, Command::SetMute { node: self.kick.0, mute: true }, "grosse caisse coupée (mute)"),
            (12.0, Command::SetMute { node: self.kick.0, mute: false }, "grosse caisse remise"),
            (13.0, Command::SetSolo { node: self.bass.0, solo: true }, "basse en solo"),
            (15.0, Command::SetSolo { node: self.bass.0, solo: false }, "solo retiré"),
            (16.0, Command::SetPan { node: self.pad.0, pan: 0.0 }, "nappe au centre"),
        ]
    }
}

/// Arpège de la démo, joué par un synthétiseur : accords Am, Am, F, G (une mesure = 2 s), croches, motif haut-bas.
pub fn synth_notes(rate: u32) -> Vec<Note> {
    let chords: [[u8; 3]; 4] = [[57, 60, 64], [57, 60, 64], [53, 57, 60], [55, 59, 62]];
    let pattern = [0usize, 1, 2, 1, 0, 1, 2, 1];
    let eighth = rate as u64 / 4;
    let mut notes = Vec::new();
    for (bar, chord) in chords.iter().enumerate() {
        for (i, &p) in pattern.iter().enumerate() {
            notes.push(Note { start: (bar as u64 * 8 + i as u64) * eighth, len: rate as u64 / 5, key: chord[p] + 12, velocity: 0.7, channel: 0 });
        }
    }
    notes
}

/// L'instrument de Jean « AnalogSynth » (éditeur « Le Tournedos Rossini »), s'il est installé sur cette machine.
pub(crate) fn find_analog_synth() -> Option<(Arc<Bundle>, String)> {
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(b) = Bundle::open(&file) else { continue };
            let Ok(list) = b.plugins() else { continue };
            if let Some(info) = list.into_iter().find(|i| i.vendor == "Le Tournedos Rossini" && i.name == "AnalogSynth") {
                return Some((Arc::new(b), info.id));
            }
        }
    }
    None
}

/// Adaptateur : le moteur comme générateur stéréo pour le callback du pilote.
pub struct EngineProc(pub Engine);

impl StereoProcessor for EngineProc {
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        self.0.process_io(in_l, in_r, out_l, out_r);
    }

    /// Toutes les entrées ouvertes par le pilote (comme l'application) : chaque piste armée écoute et enregistre son canal.
    fn process_multi(&mut self, inputs: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32]) {
        self.0.process_inputs(inputs, out_l, out_r);
    }

    /// Toutes les sorties ouvertes (la paire principale, puis les retours de monitoring).
    fn process_outputs(&mut self, inputs: &[&[f32]], outs: &mut [&mut [f32]]) {
        self.0.process_outputs(inputs, outs);
    }
}

/// Fabrique le moteur (prêt à jouer en boucle) et la fonction de contrôle qui déroule le script.
/// Le charleston balaie le panoramique en continu pour qu'on entende le lissage.
pub fn make(rate: u32) -> (EngineProc, ControlFn) {
    let mut demo = build(rate);
    match find_analog_synth() {
        Some((bundle, id)) => {
            let t = demo.graph.add_track("synthé", TrackFormat::Stereo);
            demo.graph.add_insert(t, "AnalogSynth", clap_insert_factory(bundle, id)).expect("la piste vient d'être créée");
            demo.graph.add_midi_clip(t, MidiClip::new(0, LOOP_SECONDS as u64 * rate as u64, Arc::new(synth_notes(rate))));
            eprintln!("AnalogSynth chargé : arpège ajouté à la démo");
        }
        None => eprintln!("AnalogSynth introuvable sur cette machine : démo sans synthétiseur"),
    }
    let (engine, mut handle) = Engine::new(rate, 256);
    handle.load_graph(&demo.graph).expect("la démo est un graphe valide");
    handle.send(Command::SetLoop(Some((0, LOOP_SECONDS as u64 * rate as u64))));
    handle.send(Command::Play);
    let script = demo.script();
    let hat = demo.hat.0;
    let mut next = 0;
    let mut last_pan = f32::NAN;
    let control: ControlFn = Box::new(move |elapsed| {
        let t = elapsed.as_secs_f64();
        while next < script.len() && script[next].0 <= t {
            handle.send(script[next].1);
            eprintln!("[{:>5.1} s] {}", t, script[next].2);
            next += 1;
        }
        let pan = (0.8 * (t * TAU / 4.0).sin()) as f32;
        if (pan - last_pan).abs() > 0.01 || last_pan.is_nan() {
            handle.send(Command::SetPan { node: hat, pan });
            last_pan = pan;
        }
        handle.collect_garbage();
        true
    });
    (EngineProc(engine), control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_offline_render::{render, RenderSpec};

    fn render_demo(rate: u32, block: usize) -> daw_offline_render::Rendered {
        let demo = build(rate);
        let mut spec = RenderSpec::new(rate, rate as u64 * LOOP_SECONDS as u64, demo.graph);
        spec.block = block;
        render(&spec).unwrap()
    }

    #[test]
    fn demo_graph_is_valid_and_audible_without_clipping() {
        for rate in [44_100, 48_000, 96_000] {
            let out = render_demo(rate, 256);
            assert!(out.peak() > 0.1, "{rate} Hz : trop faible ({})", out.peak());
            assert!(out.peak() < 0.95, "{rate} Hz : écrête ({})", out.peak());
        }
    }

    #[test]
    fn demo_render_is_deterministic_across_block_sizes() {
        assert_eq!(render_demo(48_000, 64).hash(), render_demo(48_000, 4096).hash());
    }

    #[test]
    fn synth_notes_are_valid_and_fit_the_loop() {
        for rate in [44_100, 48_000, 96_000] {
            let notes = synth_notes(rate);
            assert_eq!(notes.len(), 32);
            let loop_len = LOOP_SECONDS as u64 * rate as u64;
            assert!(notes.iter().all(|n| n.start + n.len <= loop_len && n.len > 0 && (48..100).contains(&n.key)));
            let mut g = build(rate).graph;
            let t = g.add_track("synthé", TrackFormat::Stereo);
            g.add_midi_clip(t, MidiClip::new(0, loop_len, Arc::new(notes)));
            g.compile(rate, 256).expect("les notes sont valides");
        }
    }

    #[test]
    fn script_targets_existing_nodes_in_order() {
        let d = build(48_000);
        let times: Vec<f64> = d.script().iter().map(|e| e.0).collect();
        assert!(times.windows(2).all(|w| w[0] < w[1]));
        let n = d.graph.nodes.len() as u32;
        for (_, c, _) in d.script() {
            let node = match c {
                Command::SetPan { node, .. } | Command::SetMute { node, .. } | Command::SetSolo { node, .. } => node,
                other => panic!("commande inattendue {other:?}"),
            };
            assert!(node < n);
        }
    }
}
