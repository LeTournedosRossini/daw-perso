//! Projet de démonstration : cinq pistes de matière sonore synthétisée (aucun fichier à fournir), à 120 noires par minute
//! en 4/4, pour essayer l'interface avec le vrai moteur. Les couleurs et noms sont ceux du faux moteur de la page.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, default_search_dirs, find_plugins, Bundle};
use daw_core_types::time::{Meter, MeterMap, Tempo, TempoMap};
use daw_engine::{AudioData, Clip, Fade, FadeShape, GraphDesc, MidiClip, NodeParams, Note, TrackFormat};
use daw_session::{Session, Timing};

use crate::host::{PluginRef, ProjectMeta, MIDI_CLIP_BASE};

const TAU: f64 = std::f64::consts::TAU;

/// Bruit déterministe dans [-1, 1[.
fn noise(i: u64) -> f64 {
    let mut x = (i as u32) ^ 0x9e37_79b9;
    x = (x ^ (x >> 16)).wrapping_mul(0x85eb_ca6b);
    x = (x ^ (x >> 13)).wrapping_mul(0xc2b2_ae35);
    ((x ^ (x >> 16)) as f64) / 2f64.powi(31) - 1.0
}

fn render(rate: u32, seconds: f64, f: impl Fn(f64, u64) -> f64) -> Arc<AudioData> {
    let n = (rate as f64 * seconds) as usize;
    let ch: Vec<f32> = (0..n).map(|i| f(i as f64 / rate as f64, i as u64) as f32).collect();
    AudioData::new(rate, vec![ch.clone(), ch]).expect("deux canaux de même longueur")
}

fn drums(t: f64, i: u64) -> f64 {
    let beat = t * 2.0;
    let phase = beat - beat.floor();
    let kick = (-phase * 9.0).exp() * (TAU * (60.0 + 90.0 * (-phase * 25.0).exp()) * phase * 0.5).sin();
    let off = (beat * 2.0) % 1.0;
    let hat = (-off * 30.0).exp() * noise(i / 3) * 0.35;
    let snare = if (beat.floor() as i64) % 2 == 1 { (-phase * 14.0).exp() * noise(i / 4) * 0.6 } else { 0.0 };
    (kick * 0.9 + hat + snare).clamp(-1.0, 1.0) * 0.6
}

fn bass(t: f64, _: u64) -> f64 {
    let note = [55.0, 55.0, 65.4, 49.0][((t * 0.5) as usize) % 4];
    let env = 0.55 + 0.35 * (TAU * t * 2.0).sin().powi(2);
    (TAU * note * t).sin() * env * 0.5
}

fn pad(t: f64, i: u64) -> f64 {
    let env = 0.5 + 0.3 * (TAU * t * 0.4).sin();
    ((TAU * 196.0 * t).sin() * 0.45 + (TAU * 294.3 * t).sin() * 0.3 + noise(i / 5) * 0.05) * env * 0.6
}

fn voice(t: f64, i: u64) -> f64 {
    let syll = (TAU * t * 1.7).sin().max(0.0).powf(1.5);
    ((TAU * 220.0 * t).sin() * 0.5 + noise(i / 5) * 0.2) * syll * 0.6
}

/// Cherche un plugin de Jean (éditeur « Le Tournedos Rossini ») dans les dossiers CLAP du système.
fn find_user_plugin(name: &str) -> Option<(Arc<Bundle>, String, PluginRef)> {
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(bundle) = Bundle::open(&file) else { continue };
            let Ok(list) = bundle.plugins() else { continue };
            if let Some(info) = list.into_iter().find(|i| i.vendor == "Le Tournedos Rossini" && i.name == name) {
                let file = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let pref = PluginRef { clap_id: info.id.clone(), vendor: info.vendor, name: info.name, file };
                return Some((Arc::new(bundle), info.id, pref));
            }
        }
    }
    None
}

/// Projet de démonstration sans plugin : déterministe, utilisé par les tests.
pub fn demo_session(rate: u32) -> (Session, ProjectMeta) {
    build_demo(rate, false)
}

/// Comme `demo_session`, avec AnalogSynth de Jean inséré sur la piste « Claviers » s'il est installé (l'arpège sonne).
/// Retourne aussi un message à afficher/journaliser sur l'instrument.
pub fn demo_session_with_instruments(rate: u32) -> (Session, ProjectMeta, String) {
    let (s, m) = build_demo(rate, true);
    let msg = if s.graph().nodes.iter().any(|n| !n.inserts.is_empty()) { "AnalogSynth chargé sur « Claviers »" } else { "AnalogSynth introuvable : l'arpège reste muet" };
    (s, m, msg.to_owned())
}

fn build_demo(rate: u32, with_instruments: bool) -> (Session, ProjectMeta) {
    let bar = rate as u64 * 2; // une mesure = 4 noires à 120 par minute = 2 s
    let mut g = GraphDesc::new();
    let mut meta = ProjectMeta::default();
    meta.node_colors.insert(0, "#9aa3b2".into());
    let p = |gain, pan| NodeParams { gain, pan, ..Default::default() };
    let specs = [
        ("Batterie", TrackFormat::Stereo, "#e5484d", p(0.8, 0.0)),
        ("Basse", TrackFormat::Mono, "#f5a524", p(0.7, -0.1)),
        ("Claviers", TrackFormat::Stereo, "#4aa3ff", p(0.6, 0.15)),
        ("Guitare", TrackFormat::Stereo, "#30a46c", p(0.65, -0.35)),
        ("Voix", TrackFormat::Mono, "#a78bfa", p(0.9, 0.05)),
    ];
    let ids: Vec<_> = specs
        .iter()
        .map(|(name, fmt, color, params)| {
            let id = g.add_track(name, *fmt);
            g.set_params(id, params.clone());
            meta.node_colors.insert(id.0, (*color).into());
            id
        })
        .collect();
    let (drum_data, bass_data, pad_data, voice_data) = (render(rate, 16.0, drums), render(rate, 32.0, bass), render(rate, 9.0, pad), render(rate, 14.0, voice));
    let clip = |data: &Arc<AudioData>, start_bar: f64, len_bars: f64| {
        let len = ((len_bars * bar as f64) as u64).min(data.len());
        Clip::new(Arc::clone(data), (start_bar * bar as f64) as u64, 0, len, 1.0)
    };
    // Identifiants de clips attribués par la session dans l'ordre des nœuds puis des clips.
    let mut beat_b = clip(&drum_data, 8.0, 8.0);
    beat_b.fade_out = Fade::new(24_000.min(beat_b.len), FadeShape::Linear);
    let mut riff1 = clip(&pad_data, 2.0, 4.5);
    riff1.fade_in = Fade::new(12_000, FadeShape::Linear);
    riff1.fade_out = Fade::new(48_000, FadeShape::EqualPower);
    let mut riff2 = clip(&pad_data, 6.0, 4.0);
    riff2.fade_in = Fade::new(48_000, FadeShape::EqualPower);
    riff2.fade_out = Fade::new(12_000, FadeShape::Linear);
    let mut couplet = clip(&voice_data, 8.0, 7.0);
    couplet.fade_out = Fade::new(36_000, FadeShape::Linear);
    g.add_clip(ids[0], clip(&drum_data, 0.0, 8.0));
    g.add_clip(ids[0], beat_b);
    g.add_clip(ids[1], clip(&bass_data, 0.0, 16.0));
    g.add_clip(ids[3], riff1);
    g.add_clip(ids[3], riff2);
    g.add_clip(ids[4], couplet);
    for (id, name) in ["Beat A", "Beat B", "Basse", "Riff 1", "Riff 2", "Couplet"].iter().enumerate() {
        meta.clip_names.insert(id as u64, (*name).into());
    }

    // Clip de notes (arpège) : visible dans la timeline ; sans instrument inséré il ne produit pas de son.
    let chords: [[u8; 3]; 4] = [[57, 60, 64], [57, 60, 64], [53, 57, 60], [55, 59, 62]];
    let notes: Vec<Note> = (0..64u64)
        .map(|step| {
            let chord = chords[(step / 8) as usize % 4];
            Note { start: step * (bar / 8), len: bar / 8 - 600, key: chord[[0, 1, 2, 1][(step % 4) as usize]] + if step % 8 >= 4 { 12 } else { 0 }, velocity: 0.55 + 0.35 * ((step * 7) % 5) as f32 / 4.0, channel: 0 }
        })
        .collect();
    g.add_midi_clip(ids[2], MidiClip::new(4 * bar, 8 * bar, Arc::new(notes)));
    if with_instruments {
        if let Some((bundle, id, pref)) = find_user_plugin("AnalogSynth") {
            if let Some(insert) = g.add_insert(ids[2], "AnalogSynth", clap_insert_factory(bundle, id)) {
                meta.plugins.insert(insert, pref);
            }
        }
    }
    meta.clip_names.insert(MIDI_CLIP_BASE, "Arpège".into());

    let mut s = Session::with_timing(g, rate, Timing { tempo: TempoMap::new(Tempo::bpm(120).expect("120")), meters: MeterMap::new(Meter::new(4, 4).expect("4/4")) });
    // Fichiers audio du projet (chemins symboliques : la démo n'écrit rien sur le disque).
    for (data, name) in [(&drum_data, "batterie"), (&bass_data, "basse"), (&pad_data, "guitare"), (&voice_data, "voix")] {
        s.register_asset(&format!("demo/{name}.wav"), Arc::clone(data));
    }
    (s, meta)
}
