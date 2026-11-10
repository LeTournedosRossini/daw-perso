use std::collections::HashMap;
use std::sync::Arc;

use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, FadeShape, GraphDesc, MidiClip, NodeParams, Note, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_session::peaks::Peaks;
use daw_session::project::{migrate_with, AudioLoader, Migration, ProjectError, CURRENT_VERSION};
use daw_session::Session;
use serde_json::{json, Value};

const SR: u32 = 48_000;

fn noise(seed: u32, n: usize) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 16_777_216.0 - 0.5
        })
        .collect()
}

struct Mem(HashMap<String, Arc<AudioData>>);
impl AudioLoader for Mem {
    fn load(&self, path: &str) -> Result<Arc<AudioData>, String> {
        self.0.get(path).cloned().ok_or_else(|| format!("introuvable : {path}"))
    }
}

fn build() -> (Session, Mem) {
    let d1 = AudioData::new(SR, vec![noise(1, 3_000), noise(2, 3_000)]).unwrap();
    let d2 = AudioData::new(SR, vec![noise(3, 2_000)]).unwrap();
    let mut s = Session::new({
        let mut g = GraphDesc::new();
        let bus = g.add_bus("Batterie");
        let t1 = g.add_track("Voix", TrackFormat::Stereo);
        let t2 = g.add_track("Gratte", TrackFormat::Mono);
        g.set_output(t1, bus);
        g.add_send(t2, bus, 0.25);
        g.set_params(t1, NodeParams { gain: 0.7, pan: -0.3, ..Default::default() });
        g.add_midi_clip(t2, MidiClip::new(100, 1_000, Arc::new(vec![Note { start: 10, len: 200, key: 64, velocity: 0.8, channel: 2 }])));
        g
    });
    let d1 = s.register_asset("audio/voix.wav", d1);
    let d2 = s.register_asset("audio/gratte.wav", d2);
    let a = s.add_clip(NodeId(2), Clip::whole(d1.clone(), 50)).unwrap();
    let b = s.add_clip(NodeId(2), Clip::whole(d1, 2_000)).unwrap();
    s.add_clip(NodeId(3), Clip::whole(d2.clone(), 0)).unwrap();
    s.trim_end(a, 2_900).unwrap();
    s.crossfade(a, b, FadeShape::EqualPower).unwrap();
    let mem = Mem(HashMap::from([("audio/voix.wav".into(), s.clip(a).unwrap().data.clone()), ("audio/gratte.wav".into(), d2)]));
    (s, mem)
}

fn hash(s: &Session) -> u64 {
    render(&RenderSpec::new(SR, 6_000, s.graph().clone())).unwrap().hash()
}

#[test]
fn save_then_load_gives_the_same_sound_and_the_same_file() {
    let (s, mem) = build();
    let (json1, warnings) = s.to_json(SR).unwrap();
    assert!(warnings.is_empty());
    assert!(json1.contains("\"version\": 21"));
    let (s2, rate) = Session::from_json(&json1, &mem).unwrap();
    assert_eq!(rate, SR);
    assert_eq!(hash(&s2), hash(&s));
    // Réécriture identique caractère pour caractère (sauvegarde stable, bonne pour les diffs).
    assert_eq!(s2.to_json(SR).unwrap().0, json1);
    assert_eq!(s2.graph().nodes.len(), 4);
    assert_eq!(s2.graph().nodes[1].name, "Batterie");
}

#[test]
fn loaded_project_is_editable_and_undoable() {
    let (s, mem) = build();
    let (json, _) = s.to_json(SR).unwrap();
    let (mut s2, _) = Session::from_json(&json, &mem).unwrap();
    let before = hash(&s2);
    let id = s2.clip_ids(NodeId(2))[0];
    s2.move_clip(id, 900).unwrap();
    assert_ne!(hash(&s2), before);
    s2.undo();
    assert_eq!(hash(&s2), before);
}

#[test]
fn unregistered_audio_cannot_be_saved() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let mut s = Session::new(g);
    s.add_clip(t, Clip::whole(AudioData::new(SR, vec![vec![0.0; 10]]).unwrap(), 0)).unwrap();
    assert_eq!(s.to_json(SR).err(), Some(ProjectError::UnregisteredAudio { node: t }));
}

#[test]
fn inserts_are_reported_as_not_saved() {
    use daw_engine::InsertFactory;
    let mut g = GraphDesc::new();
    let t = g.add_track("Voix", TrackFormat::Stereo);
    let f: InsertFactory = Arc::new(|_, _, _| Err("non utilisé".to_owned()));
    g.add_insert(t, "Egaliseur", f).unwrap();
    let (_, w) = Session::new(g).to_json(SR).unwrap();
    assert_eq!(w, vec!["effet « Egaliseur » sur « Voix » non enregistré".to_owned()]);
}

#[test]
fn damaged_or_foreign_files_are_refused_cleanly() {
    let (s, mem) = build();
    let (json, _) = s.to_json(SR).unwrap();
    let load = |j: &str| Session::from_json(j, &mem).err();
    assert!(matches!(load("pas du json"), Some(ProjectError::Parse(_))));
    assert_eq!(load("{}"), Some(ProjectError::MissingVersion));
    assert_eq!(load(&json.replace("\"version\": 21", "\"version\": 99")), Some(ProjectError::TooNew { found: 99, supported: CURRENT_VERSION }));
    assert!(matches!(load(&json.replace("audio/voix.wav", "audio/absent.wav")), Some(ProjectError::Audio { .. })));
    assert!(matches!(load(&json.replace("\"equal_power\"", "\"zigzag\"")), Some(ProjectError::Parse(_))));
    // Un envoi vers un nœud qui n'existe pas.
    let mut v: Value = serde_json::from_str(&json).unwrap();
    v["nodes"][3]["sends"][0]["to"] = json!(42);
    assert!(matches!(load(&v.to_string()), Some(ProjectError::BadGraph(_))));
    // Un clip qui pointe un fichier audio inexistant dans le projet.
    let mut v: Value = serde_json::from_str(&json).unwrap();
    v["nodes"][2]["clips"][0]["asset"] = json!(9);
    assert!(matches!(load(&v.to_string()), Some(ProjectError::Parse(_))));
}

#[test]
fn audio_at_another_sample_rate_is_refused() {
    let (s, mut mem) = build();
    let (json, _) = s.to_json(SR).unwrap();
    mem.0.insert("audio/gratte.wav".into(), AudioData::new(44_100, vec![vec![0.0; 2_000]]).unwrap());
    assert!(matches!(Session::from_json(&json, &mem).err(), Some(ProjectError::SampleRate { found: 44_100, expected: SR, .. })));
}

#[test]
fn migrations_run_in_order_from_the_file_version() {
    fn v1_to_v2(v: &mut Value) -> Result<(), String> {
        let g = v["gain_db"].take();
        v["gain"] = json!(10f64.powf(g.as_f64().ok_or("gain_db manquant")? / 20.0));
        Ok(())
    }
    fn v2_to_v3(v: &mut Value) -> Result<(), String> {
        v["name"] = v["title"].take();
        Ok(())
    }
    let ms: &[Migration] = &[v1_to_v2, v2_to_v3];
    let out = migrate_with(json!({"version": 1, "gain_db": -20.0, "title": "x"}), ms).unwrap();
    assert_eq!((out["version"].clone(), out["name"].clone()), (json!(3), json!("x")));
    assert!((out["gain"].as_f64().unwrap() - 0.1).abs() < 1e-12);
    // Un fichier déjà en version 2 ne passe que par la seconde.
    let out = migrate_with(json!({"version": 2, "title": "y"}), ms).unwrap();
    assert_eq!((out["version"].clone(), out["name"].clone()), (json!(3), json!("y")));
    assert_eq!(migrate_with(json!({"version": 4}), ms).err(), Some(ProjectError::TooNew { found: 4, supported: 3 }));
    assert!(matches!(migrate_with(json!({"version": 1}), ms).err(), Some(ProjectError::Migration { from: 1, .. })));
}

#[test]
fn peaks_bound_the_true_extremes_at_every_zoom() {
    let n = 100_000;
    let data = AudioData::new(SR, vec![noise(9, n), noise(10, n)]).unwrap();
    let p = Peaks::build(&data);
    assert_eq!((p.channels(), p.frames()), (2, n as u64));
    assert!(p.resolutions().len() >= 3 && p.resolutions()[0] == 64);
    for (start, end, cols) in [(0, n as u64, 100), (0, n as u64, 1_000), (12_345, 54_321, 37), (500, 600, 10), (99_000, 200_000, 8)] {
        let q = p.query(1, start, end, cols);
        assert_eq!(q.len(), cols);
        let end = end.min(n as u64);
        for (k, &(lo, hi)) in q.iter().enumerate() {
            let a = (start + (end - start) * k as u64 / cols as u64) as usize;
            let b = ((start + (end - start) * (k as u64 + 1) / cols as u64) as usize).max(a + 1).min(n);
            let real_lo = data.channel(1)[a..b].iter().cloned().fold(f32::INFINITY, f32::min);
            let real_hi = data.channel(1)[a..b].iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            assert!(lo <= real_lo && hi >= real_hi, "colonne {k} de {cols} : [{lo},{hi}] ne couvre pas [{real_lo},{real_hi}]");
        }
    }
}

#[test]
fn peaks_of_a_known_signal_are_exact_and_edge_cases_are_safe() {
    // Rampe : le maximum d'une colonne de largeur multiple de la crête est exact.
    let ramp: Vec<f32> = (0..6_400).map(|i| i as f32).collect();
    let p = Peaks::build(&AudioData::new(SR, vec![ramp]).unwrap());
    assert_eq!(p.query(0, 0, 6_400, 100)[1], (64.0, 127.0));
    assert!(p.query(0, 10, 10, 5).is_empty() && p.query(0, 0, 100, 0).is_empty() && p.query(3, 0, 100, 5).is_empty());
    let tiny = Peaks::build(&AudioData::new(SR, vec![vec![0.5; 3]]).unwrap());
    assert_eq!(tiny.query(0, 0, 3, 2).len(), 2);
    let empty = Peaks::build(&AudioData::new(SR, vec![vec![]]).unwrap());
    assert!(empty.query(0, 0, 10, 4).is_empty());
}

#[test]
fn polarity_is_saved_and_a_version_6_project_loads_with_it_off() {
    use daw_engine::{AudioData, Clip, GraphDesc, TrackFormat};
    let data = AudioData::new(48_000, vec![vec![0.5; 100], vec![0.5; 100]]).unwrap();
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    let b = g.add_track("B", TrackFormat::Stereo);
    g.add_clip(a, Clip::whole(data.clone(), 0));
    g.nodes[b.0 as usize].params.invert = true;
    let mut s = Session::new(g);
    s.register_asset("a.wav", data.clone());
    let (json, _) = s.to_json(48_000).unwrap();
    struct L(std::sync::Arc<AudioData>);
    impl AudioLoader for L {
        fn load(&self, _: &str) -> Result<std::sync::Arc<AudioData>, String> {
            Ok(self.0.clone())
        }
    }
    let (back, _) = Session::from_json(&json, &L(data.clone())).unwrap();
    assert_eq!((back.graph().nodes[1].params.invert, back.graph().nodes[2].params.invert), (false, true));
    // Un projet de la version 6 (sans le champ) se relit, polarité normale partout.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["version"] = serde_json::Value::from(6);
    for n in v["nodes"].as_array_mut().unwrap() {
        n.as_object_mut().unwrap().remove("invert");
    }
    let (old, _) = Session::from_json(&v.to_string(), &L(data)).unwrap();
    assert!(old.graph().nodes.iter().all(|n| !n.params.invert));
}

#[test]
fn external_files_are_kept_by_absolute_path_and_a_missing_one_plays_silence_without_losing_its_clips() {
    let data = AudioData::new(SR, vec![noise(5, 4_000), noise(6, 4_000)]).unwrap();
    let mut s = Session::new({
        let mut g = GraphDesc::new();
        g.add_track("Import", TrackFormat::Stereo);
        g
    });
    for path in ["/media/samples/boucle.flac", "C:\\Sons\\boucle.mp3", "\\\\serveur\\sons\\x.wav", "audio/interne.wav"] {
        assert_eq!(daw_session::project::is_external_path(path), !path.starts_with("audio"), "{path}");
    }
    let d = s.register_asset("/media/samples/boucle.flac", data.clone());
    let id = s.add_clip(NodeId(1), Clip::new(d, 1_000, 500, 2_000, 0.8)).unwrap();
    let (json, _) = s.to_json(SR).unwrap();
    assert!(json.contains("\"frames\": 4000") && json.contains("\"channels\": 2"), "{json}");

    // Le fichier est là : rien de manquant, même son.
    let there = Mem(HashMap::from([("/media/samples/boucle.flac".to_owned(), data)]));
    let (ok, _) = Session::from_json(&json, &there).unwrap();
    assert!(ok.missing_assets().is_empty());
    assert_eq!(hash(&ok), hash(&s));

    // Le fichier a disparu : le projet s'ouvre, le clip est à sa place, le son est du silence, le fichier est signalé.
    let gone = Mem(HashMap::new());
    let (lost, rate) = Session::from_json(&json, &gone).unwrap();
    assert_eq!(rate, SR);
    assert_eq!(lost.missing_assets(), ["/media/samples/boucle.flac"]);
    let ids = lost.clip_ids(NodeId(1));
    assert_eq!(ids.len(), 1);
    let c = lost.clip(ids[0]).unwrap();
    assert_eq!((c.start, c.offset, c.len, c.gain), (1_000, 500, 2_000, 0.8));
    assert!(lost.is_missing(&c.data));
    assert_eq!(c.data.len(), 2_500, "seulement la partie utilisée par le clip");
    assert_eq!(c.data.channel_count(), 2);
    assert_eq!(render(&RenderSpec::new(SR, 4_000, lost.graph().clone())).unwrap().peak(), 0.0);
    // Réenregistré, il référence toujours le même chemin (le fichier peut revenir) ; un chemin relatif absent reste une erreur.
    let (again, _) = lost.to_json(SR).unwrap();
    assert!(again.contains("/media/samples/boucle.flac"));
    let _ = id;
    let (rel, _) = build();
    let (rel_json, _) = rel.to_json(SR).unwrap();
    assert!(matches!(Session::from_json(&rel_json, &gone).err(), Some(ProjectError::Audio { .. })));
}

#[test]
fn a_version_9_file_without_the_new_fields_still_loads() {
    let (s, mem) = build();
    let (json, _) = s.to_json(SR).unwrap();
    let v9 = json.replace("\"version\": 21", "\"version\": 9").replace(",\n      \"frames\":", ",\n      \"x_frames\":");
    // Les champs `frames` / `channels` sont facultatifs : un fichier qui ne les connaît pas se relit à l'identique.
    let (back, _) = Session::from_json(&v9, &mem).unwrap();
    assert_eq!(hash(&back), hash(&s));
    assert_eq!(CURRENT_VERSION, 21);
}
