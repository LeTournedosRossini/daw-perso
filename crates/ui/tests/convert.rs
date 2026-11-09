//! Conversion d'un projet à la fréquence du moteur : proposition, copie convertie dans un nouveau dossier, original intact.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::Engine;
use daw_offline_render::export::{encode, BitDepth, Container, Format};
use daw_recorder::decode::decode_file;
use daw_test_plugin::{entry, PARAM_GAIN, PLUGIN_ID};
use daw_ui::demo::demo_session;
use daw_ui::host::{PluginRef, PluginResolver};
use daw_ui::protocol::{ClipInfo, NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

const OLD: u32 = 44_100;
const NEW: u32 = 48_000;

fn resolver() -> PluginResolver {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let pref = PluginRef { clap_id: PLUGIN_ID.to_str().unwrap().into(), vendor: "daw-perso".into(), name: "Gain de test".into(), file: "test.clap".into() };
    Box::new(move |e| (e.clap_id == PLUGIN_ID.to_str().unwrap()).then(|| (clap_insert_factory(Arc::clone(&bundle), e.clap_id.clone()), pref.clone())))
}

fn host_at(rate: u32) -> (Engine, Host) {
    let (engine, handle) = Engine::new(rate, 64);
    let (session, meta) = demo_session(rate);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_plugin_resolver(resolver());
    (engine, h)
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-convert-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn save_as(h: &mut Host, dir: &Path, name: &str) -> PathBuf {
    h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join(name).to_string_lossy()));
    dir.join(format!("{name}.daw"))
}

/// Contenu de tous les fichiers d'un dossier (récursif) : pour prouver qu'il n'a pas bougé.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, d: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                out.insert(p.strip_prefix(base).unwrap().to_string_lossy().into_owned(), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

struct Done {
    path: Option<String>,
    cancelled: bool,
    error: Option<String>,
    progress: Vec<f64>,
    events: Vec<Event>,
}

fn convert(h: &mut Host, path: &Path) -> Done {
    let mut events = h.command_json(&format!(r#"{{"type":"convert_project","path":{:?}}}"#, path.to_string_lossy()));
    let mut all = Vec::new();
    let mut progress = Vec::new();
    let t = Instant::now();
    loop {
        let mut end: Option<Done> = None;
        for e in events.drain(..) {
            match &e {
                Event::Convert { active: true, progress: p, .. } => progress.push(*p),
                Event::Convert { active: false, path, cancelled, error, .. } => end = Some(Done { path: path.clone(), cancelled: *cancelled, error: error.clone(), progress: progress.clone(), events: Vec::new() }),
                Event::Error { message, .. } => end = Some(Done { path: None, cancelled: false, error: Some(message.clone()), progress: progress.clone(), events: Vec::new() }),
                _ => {}
            }
            all.push(e);
        }
        // Le lot qui annonce la fin contient aussi les événements de l'ouverture de la copie : on les garde.
        if let Some(d) = end {
            return Done { events: all, ..d };
        }
        assert!(t.elapsed() < Duration::from_secs(400), "conversion trop longue");
        std::thread::sleep(Duration::from_millis(5));
        events = h.tick();
    }
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(p: &'a ProjectInfo, name: &str) -> &'a NodeInfo {
    p.nodes.iter().find(|n| n.name == name).unwrap()
}

fn audio(n: &NodeInfo) -> Vec<(u64, u64, u64, bool)> {
    n.clips.iter().filter_map(|c| if let ClipInfo::Audio { start, len, offset, missing, .. } = c { Some((*start, *len, *offset, *missing)) } else { None }).collect()
}

fn scaled(x: u64) -> u64 {
    (x as u128 * NEW as u128 * 2 + OLD as u128) as u64 / 2 / OLD as u64
}

fn mismatch(ev: &[Event]) -> Option<(u32, u32)> {
    ev.iter().find_map(|e| if let Event::RateMismatch { project_rate, engine_rate, .. } = e { Some((*project_rate, *engine_rate)) } else { None })
}

#[test]
fn opening_a_project_at_another_rate_proposes_a_conversion_and_changes_nothing() {
    let (_e1, mut old) = host_at(OLD);
    let proj = save_as(&mut old, &temp("offer"), "Chanson");
    let (_e2, mut h) = host_at(NEW);
    let before = h.command_json(r#"{"type":"get_project"}"#);
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    assert_eq!(mismatch(&ev), Some((OLD, NEW)));
    assert_eq!(ev.len(), 1, "rien d'autre : ni erreur ni chargement : {ev:?}");
    let after = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(project(&after).nodes.len(), project(&before).nodes.len());
    // Une sauvegarde automatique à une autre fréquence : on ne propose pas, on dit quoi faire.
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{:?},"recover":true}}"#, proj.to_string_lossy()));
    let _ = ev; // (pas de fichier d'autosave ici : l'erreur de lecture suffit, rien ne doit planter)
}

#[test]
fn the_copy_is_converted_opened_and_the_original_is_untouched() {
    let (_e1, mut old) = host_at(OLD);
    let dir = temp("copy");
    let proj = save_as(&mut old, &dir, "Chanson");
    let original = project(&old.command_json(r#"{"type":"get_project"}"#)).clone();
    let frozen = snapshot(&proj);
    let (_e2, mut h) = host_at(NEW);
    let d = convert(&mut h, &proj);
    assert!(d.error.is_none() && !d.cancelled, "{:?}", d.error);
    assert!(d.progress.windows(2).all(|p| p[1] >= p[0]) && d.progress.iter().all(|p| (0.0..1.0).contains(p)), "{:?}", d.progress);
    let new_dir = dir.join("Chanson (48000 Hz).daw");
    assert_eq!(d.path.as_deref(), Some(new_dir.to_string_lossy().as_ref()));
    // L'original n'a pas changé d'un octet, et aucun dossier provisoire ne reste.
    assert_eq!(snapshot(&proj), frozen);
    assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".part")));
    // La copie est ouverte : mêmes pistes, à 48 kHz, positions mises à l'échelle, tempo conservé.
    let p = project(&d.events).clone();
    assert_eq!(p.sample_rate, NEW);
    assert_eq!(p.nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>(), original.nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>());
    assert_eq!(p.tempo_map, original.tempo_map);
    for (a, b) in original.nodes.iter().zip(&p.nodes) {
        let (ca, cb) = (audio(a), audio(b));
        assert_eq!(ca.len(), cb.len(), "{}", a.name);
        for (x, y) in ca.iter().zip(&cb) {
            assert_eq!((scaled(x.0), scaled(x.2)), (y.0, y.2), "{}", a.name);
            assert!(y.1.abs_diff(scaled(x.1)) <= 1, "{} : longueur {} → {}", a.name, x.1, y.1);
        }
    }
    assert!(d.events.iter().any(|e| matches!(e, Event::File { name, .. } if name == "Chanson (48000 Hz)")));
    // Les fichiers audio de la copie sont bien à 48 kHz.
    let wavs: Vec<_> = std::fs::read_dir(new_dir.join("audio")).unwrap().map(|e| e.unwrap().path()).collect();
    assert!(!wavs.is_empty());
    assert!(wavs.iter().all(|w| decode_file(w).unwrap().audio.sample_rate == NEW));
    // Une seconde conversion ne recouvre pas la première.
    let (_e3, mut h2) = host_at(NEW);
    let again = convert(&mut h2, &proj);
    assert!(again.path.as_deref().unwrap().ends_with("Chanson (48000 Hz) (2).daw"), "{:?}", again.path);
    assert_eq!(snapshot(&proj), frozen);
    // Et la copie convertie s'ouvre désormais sans rien proposer, depuis un autre hôte à 48 kHz.
    let (_e4, mut h3) = host_at(NEW);
    let ev = h3.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, new_dir.to_string_lossy()));
    assert!(mismatch(&ev).is_none() && ev.iter().any(|e| matches!(e, Event::Project(_))));
}

#[test]
fn plugins_and_their_settings_come_along() {
    let (mut e1, mut old) = host_at(OLD);
    let proj = save_as(&mut old, &temp("plugin"), "Avec plugin");
    let ev = old.command_json(r#"{"type":"add_plugin","node":2,"clap_id":"com.daw-perso.test-gain","file":"test.clap"}"#);
    let id = project(&ev).nodes.iter().find(|n| n.id == 2).unwrap().inserts[0].id;
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    e1.process(&mut l, &mut r);
    old.command_json(&format!(r#"{{"type":"set_plugin_param","insert":{id},"param":{PARAM_GAIN},"value":0.5}}"#));
    e1.process(&mut l, &mut r);
    old.command_json(r#"{"type":"save_project"}"#);
    let (_e2, mut h) = host_at(NEW);
    let d = convert(&mut h, &proj);
    assert!(d.error.is_none(), "{:?}", d.error);
    let p = project(&d.events);
    let bass = node(p, "Basse");
    assert_eq!(bass.inserts.len(), 1);
    let ev = h.command_json(&format!(r#"{{"type":"get_plugin_params","insert":{}}}"#, bass.inserts[0].id));
    let value = ev.iter().find_map(|e| if let Event::PluginParams { params, .. } = e { params.iter().find(|x| x.id == PARAM_GAIN).map(|x| x.value) } else { None }).unwrap();
    assert_eq!(value, 0.5, "le réglage du plugin est repris");
}

fn tone_file(dir: &Path, name: &str, rate: u32, secs: f32) -> PathBuf {
    let n = (rate as f32 * secs) as usize;
    let x: Vec<f32> = (0..n).map(|i| (0.5 * (i as f64 * std::f64::consts::TAU * 440.0 / rate as f64).sin()) as f32).collect();
    let e = encode(&x, &x, rate, Format { container: Container::Wav, depth: BitDepth::Float32, dither: false }, None).unwrap();
    let p = dir.join(format!("{name}.wav"));
    std::fs::write(&p, e.bytes).unwrap();
    p
}

fn import(h: &mut Host, file: &Path, extra: &str) {
    let mut events = h.command_json(&format!(r#"{{"type":"import_audio","files":[{:?}],"pos":0{extra}}}"#, file.to_string_lossy()));
    let t = Instant::now();
    loop {
        if events.iter().any(|e| matches!(e, Event::Import { active: false, error: None, .. })) {
            return;
        }
        assert!(!events.iter().any(|e| matches!(e, Event::Error { .. } | Event::Import { active: false, error: Some(_), .. })), "{events:?}");
        assert!(t.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(5));
        events = h.tick();
    }
}

#[test]
fn referenced_files_become_converted_copies_and_missing_ones_stay_missing() {
    let (_e1, mut old) = host_at(OLD);
    let dir = temp("ref");
    let proj = save_as(&mut old, &dir, "Ref");
    let src = temp("ref-src");
    let found = tone_file(&src, "retrouve", OLD, 1.0);
    let lost = tone_file(&src, "perdu", OLD, 1.0);
    import(&mut old, &found, r#","reference":true"#);
    import(&mut old, &lost, r#","reference":true"#);
    old.command_json(r#"{"type":"save_project"}"#);
    std::fs::remove_file(&lost).unwrap();
    let frozen_src = std::fs::read(&found).unwrap();
    let (_e2, mut h) = host_at(NEW);
    let d = convert(&mut h, &proj);
    assert!(d.error.is_none(), "{:?}", d.error);
    let p = project(&d.events);
    // Le fichier retrouvé est maintenant une copie à 48 kHz dans le nouveau projet ; l'original n'a pas bougé.
    let new_dir = dir.join("Ref (48000 Hz).daw");
    let copy = new_dir.join("audio/retrouve.wav");
    assert!(copy.exists());
    assert_eq!(decode_file(&copy).unwrap().audio.sample_rate, NEW);
    assert_eq!(std::fs::read(&found).unwrap(), frozen_src);
    assert!(!audio(node(p, "retrouve"))[0].3, "pas manquant");
    assert_eq!(audio(node(p, "retrouve"))[0].1, NEW as u64, "1 s à 48 kHz");
    // Le fichier perdu reste référencé et manquant (le chemin survit dans le projet converti).
    assert!(audio(node(p, "perdu"))[0].3);
    let json = std::fs::read_to_string(new_dir.join("projet.json")).unwrap();
    // (Sans comparer des chemins : sous Windows le JSON échappe les barres obliques inverses.)
    assert!(json.contains("perdu.wav") && !json.contains("\"audio/perdu.wav\""), "le fichier perdu garde son chemin externe");
    assert!(json.contains("\"audio/retrouve.wav\"") && json.matches("retrouve.wav").count() == 1, "le fichier retrouvé est maintenant une copie sous audio/ seulement");
    assert!(d.events.iter().any(|e| matches!(e, Event::File { warnings, .. } if warnings.iter().any(|w| w.contains("manquant")))));
}

#[test]
fn refusals_and_cancellation_leave_nothing_behind() {
    let (_e1, mut old) = host_at(OLD);
    let dir = temp("cancel");
    let proj = save_as(&mut old, &dir, "Gros");
    // Beaucoup d'audio : la conversion dure assez pour qu'on ait le temps de l'annuler.
    let big = tone_file(&temp("cancel-src"), "long", OLD, 60.0);
    for _ in 0..3 {
        import(&mut old, &big, "");
    }
    old.command_json(r#"{"type":"save_project"}"#);
    let frozen = snapshot(&proj);
    let (_e2, mut h) = host_at(NEW);
    // Projet déjà à la bonne fréquence : rien à convertir.
    let (_e3, mut same) = host_at(OLD);
    assert!(convert(&mut same, &proj).error.unwrap().contains("déjà"));
    assert!(convert(&mut h, &dir.join("absent.daw")).error.is_some());
    assert!(matches!(&h.command_json(r#"{"type":"convert_project","path":" "}"#)[0], Event::Error { .. }));
    // Annulation : ni nouveau dossier, ni dossier provisoire ; l'original intact ; l'hôte libre ensuite.
    let start = h.command_json(&format!(r#"{{"type":"convert_project","path":{:?}}}"#, proj.to_string_lossy()));
    assert!(start.iter().any(|e| matches!(e, Event::Convert { active: true, .. })));
    assert!(matches!(&h.command_json(&format!(r#"{{"type":"convert_project","path":{:?}}}"#, proj.to_string_lossy()))[0], Event::Error { message, .. } if message.contains("déjà")));
    assert!(matches!(&h.command_json(r#"{"type":"new_project"}"#)[0], Event::Error { message, .. } if message.contains("conversion")));
    h.command_json(r#"{"type":"cancel_convert"}"#);
    let t = Instant::now();
    let end = loop {
        if let Some(e) = h.tick().into_iter().find(|e| matches!(e, Event::Convert { active: false, .. })) {
            break e;
        }
        assert!(t.elapsed() < Duration::from_secs(400));
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(end, Event::Convert { cancelled: true, .. }), "{end:?}");
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["Gros.daw"], "rien d'autre que l'original : {names:?}");
    assert_eq!(snapshot(&proj), frozen);
    assert!(convert(&mut h, &proj).error.is_none(), "l'hôte est libre de nouveau");
}
