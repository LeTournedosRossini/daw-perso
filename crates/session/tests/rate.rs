//! Conversion de la fréquence d'échantillonnage à l'ouverture d'un projet (`Session::from_json_at_rate`).

use std::collections::HashMap;
use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, Fade, FadeShape, GraphDesc, MidiClip, Note, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_session::project::{AudioLoader, ProjectError};
use daw_session::Session;

const OLD: u32 = 44_100;
const NEW: u32 = 48_000;

struct Mem(HashMap<String, Arc<AudioData>>);
impl AudioLoader for Mem {
    fn load(&self, path: &str) -> Result<Arc<AudioData>, String> {
        self.0.get(path).cloned().ok_or_else(|| format!("introuvable : {path}"))
    }
}

fn tone(rate: u32, secs: f32, freq: f32, amp: f32) -> Arc<AudioData> {
    let n = (rate as f32 * secs) as usize;
    // Calculé en f64 : un sinus en f32 porte un bruit d'arrondi (~ −80 dB) qui fausserait la mesure de fidélité.
    let ch: Vec<f32> = (0..n).map(|i| (amp as f64 * (i as f64 * std::f64::consts::TAU * freq as f64 / rate as f64).sin()) as f32).collect();
    AudioData::new(rate, vec![ch.clone(), ch]).unwrap()
}

/// Projet à 44,1 kHz : un clip audio (décalage, fondu d'entrée, gain), un clip de notes. Renvoie la session et son loader.
fn project() -> (Session, Mem, Arc<AudioData>) {
    let data = tone(OLD, 2.0, 440.0, 0.5);
    let mut g = GraphDesc::new();
    let t = g.add_track("Son", TrackFormat::Stereo);
    let m = g.add_track("Notes", TrackFormat::Stereo);
    g.add_midi_clip(m, MidiClip::new(OLD as u64, 22_050, Arc::new(vec![Note { start: 1_000, len: 5_000, key: 64, velocity: 0.8, channel: 0 }])));
    let mut s = Session::new_at(g, OLD);
    let d = s.register_asset("audio/ton.wav", Arc::clone(&data));
    let mut c = Clip::new(d, 10_000, 2_000, 60_000, 0.8);
    c.fade_in = Fade::new(4_410, FadeShape::Linear);
    s.add_clip(t, c).unwrap();
    let _ = m;
    (s, Mem(HashMap::from([("audio/ton.wav".to_owned(), data.clone())])), data)
}

fn json_at(s: &Session, rate: u32) -> String {
    s.to_json(rate).unwrap().0
}

fn old_json(s: &Session) -> String {
    json_at(s, OLD)
}

#[test]
fn audio_positions_are_scaled_and_musical_content_follows_the_new_rate() {
    let (s, mem, _) = project();
    let json = old_json(&s);
    let (c, rate) = Session::from_json_at_rate(&json, &mem, Some(NEW), &mut |_, _| true).unwrap();
    assert_eq!(rate, NEW);
    assert_eq!(c.sample_rate(), NEW);
    let ids = c.clip_ids(NodeId(1));
    let clip = c.clip(ids[0]).unwrap();
    // 10 000 × 48 000 / 44 100 = 10 884,35 ; 2 000 → 2 176,87 ; (2 000 + 60 000) → 67 482,99 ; 4 410 → 4 800.
    assert_eq!(clip.start, 10_884);
    assert_eq!(clip.offset, 2_177);
    assert_eq!(clip.len, 67_483 - 2_177);
    assert_eq!(clip.fade_in.len, 4_800);
    assert_eq!(clip.gain, 0.8);
    assert_eq!(clip.data.sample_rate(), NEW);
    assert_eq!(clip.data.len(), 96_000, "2 s à 48 kHz");
    assert!(clip.offset + clip.len <= clip.data.len());
    // Les notes sont en temps musical : le clip qui commençait à 1 s commence toujours à 1 s (= 48 000 trames).
    let mids = c.midi_clip_ids(NodeId(2));
    let g = c.graph();
    assert_eq!(g.nodes[2].midi_clips[0].start, 48_000);
    let note = &g.nodes[2].midi_clips[0].notes[0];
    assert!((1_087..=1_089).contains(&note.start), "note à {}", note.start);
    assert_eq!(mids.len(), 1);
}

#[test]
fn the_converted_project_sounds_like_the_original() {
    let (s, mem, _) = project();
    let json = old_json(&s);
    let (c, _) = Session::from_json_at_rate(&json, &mem, Some(NEW), &mut |_, _| true).unwrap();
    let out = render(&RenderSpec::new(NEW, 80_000, c.graph().clone())).unwrap();
    let clip = c.clip(c.clip_ids(NodeId(1))[0]).unwrap();
    // Au milieu du clip (hors fondu et bords), le signal est le sinus de 440 Hz × 0,5 × 0,8, à l'échantillon près.
    let (lo, hi) = (clip.start as usize + 5_500, (clip.start + clip.len) as usize - 700);
    let (mut sq, mut n) = (0.0f64, 0.0f64);
    for i in lo..hi {
        let data_index = i as f64 - clip.start as f64 + clip.offset as f64;
        let ideal = 0.4 * (data_index * std::f64::consts::TAU * 440.0 / NEW as f64).sin();
        sq += (out.left[i] as f64 - ideal).powi(2);
        n += 1.0;
    }
    let err_db = 20.0 * ((sq / n).sqrt() / 0.4).log10();
    assert!(err_db < -90.0, "écart {err_db:.1} dB");
    // Rien avant le clip.
    assert!(out.left[..clip.start as usize].iter().all(|&v| v == 0.0));
}

#[test]
fn same_rate_is_untouched_and_a_round_trip_returns_within_one_sample() {
    let (s, mem, data) = project();
    let json = old_json(&s);
    let (plain, r) = Session::from_json_at_rate(&json, &mem, None, &mut |_, _| true).unwrap();
    let (classic, _) = Session::from_json(&json, &mem).unwrap();
    assert_eq!(r, OLD);
    let (a, b) = (plain.clip(plain.clip_ids(NodeId(1))[0]).unwrap(), classic.clip(classic.clip_ids(NodeId(1))[0]).unwrap());
    assert_eq!((a.start, a.offset, a.len, a.fade_in.len), (b.start, b.offset, b.len, b.fade_in.len));
    assert_eq!((a.start, a.offset, a.len), (10_000, 2_000, 60_000));
    // 44,1 → 48 → 44,1 : les positions reviennent à un échantillon près ; les données aussi (longueur).
    let (up, _) = Session::from_json_at_rate(&json, &mem, Some(NEW), &mut |_, _| true).unwrap();
    let json48 = json_at(&up, NEW);
    let back_mem = Mem(HashMap::from([("audio/ton.wav".to_owned(), up.clip(up.clip_ids(NodeId(1))[0]).unwrap().data.clone())]));
    let (down, rate) = Session::from_json_at_rate(&json48, &back_mem, Some(OLD), &mut |_, _| true).unwrap();
    assert_eq!(rate, OLD);
    let d = down.clip(down.clip_ids(NodeId(1))[0]).unwrap();
    assert!(d.start.abs_diff(10_000) <= 1 && d.offset.abs_diff(2_000) <= 1 && d.len.abs_diff(60_000) <= 1, "{} {} {}", d.start, d.offset, d.len);
    assert_eq!(d.data.len(), data.len());
}

#[test]
fn clips_stay_inside_their_converted_data_and_never_become_empty() {
    // Clip qui prend tout le fichier, clip d'un seul échantillon, clip qui finit exactement au bout : aucun ne dépasse après arrondi.
    let data = tone(OLD, 0.05, 1_000.0, 0.5); // 2 205 trames
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let mut s = Session::new_at(g, OLD);
    let d = s.register_asset("audio/c.wav", Arc::clone(&data));
    s.add_clip(t, Clip::whole(d.clone(), 0)).unwrap();
    s.add_clip(t, Clip::new(d.clone(), 100, 2_204, 1, 1.0)).unwrap();
    s.add_clip(t, Clip::new(d, 5_000, 1_000, 1_205, 1.0)).unwrap();
    let json = old_json(&s);
    let mem = Mem(HashMap::from([("audio/c.wav".to_owned(), data)]));
    for target in [NEW, 96_000, 22_050, 8_000, 44_101] {
        let (c, _) = Session::from_json_at_rate(&json, &mem, Some(target), &mut |_, _| true).unwrap_or_else(|e| panic!("{target} Hz : {e:?}"));
        for &id in c.clip_ids(NodeId(1)) {
            let k = c.clip(id).unwrap();
            assert!(k.len >= 1 && k.offset + k.len <= k.data.len(), "{target} Hz : offset {} len {} données {}", k.offset, k.len, k.data.len());
        }
    }
}

#[test]
fn a_missing_external_file_stays_missing_after_conversion() {
    let data = tone(OLD, 1.0, 300.0, 0.5);
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let mut s = Session::new_at(g, OLD);
    let d = s.register_asset("/media/x/ext.flac", Arc::clone(&data));
    s.add_clip(t, Clip::new(d, 1_000, 500, 20_000, 1.0)).unwrap();
    let json = old_json(&s);
    let (c, _) = Session::from_json_at_rate(&json, &Mem(HashMap::new()), Some(NEW), &mut |_, _| true).unwrap();
    assert_eq!(c.missing_assets(), ["/media/x/ext.flac"]);
    let k = c.clip(c.clip_ids(NodeId(1))[0]).unwrap();
    assert_eq!(k.data.sample_rate(), NEW);
    assert!(k.offset + k.len <= k.data.len());
    assert_eq!(k.start, 1_088);
}

#[test]
fn the_conversion_can_be_cancelled_and_reports_its_progress() {
    let (s, mem, _) = project();
    let json = old_json(&s);
    let mut seen = Vec::new();
    Session::from_json_at_rate(&json, &mem, Some(NEW), &mut |done, total| {
        seen.push((done, total));
        true
    })
    .unwrap();
    assert_eq!(seen, [(0, 1)]);
    let r = Session::from_json_at_rate(&json, &mem, Some(NEW), &mut |_, _| false);
    assert_eq!(r.err(), Some(ProjectError::Cancelled));
}
