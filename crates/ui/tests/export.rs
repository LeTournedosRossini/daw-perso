//! Export du mixage et des stems par le protocole : fil de travail, progression, annulation, nommage, plugins avec leur état.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::Engine;
use daw_recorder::decode::decode_bytes;
use daw_test_plugin::{entry, PARAM_GAIN, PLUGIN_ID};
use daw_ui::demo::demo_session;
use daw_ui::host::{PluginRef, PluginResolver};
use daw_ui::protocol::ExportedFile;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn resolver() -> PluginResolver {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let pref = PluginRef { clap_id: PLUGIN_ID.to_str().unwrap().into(), vendor: "daw-perso".into(), name: "Gain de test".into(), file: "test.clap".into() };
    Box::new(move |e| (e.clap_id == PLUGIN_ID.to_str().unwrap()).then(|| (clap_insert_factory(Arc::clone(&bundle), e.clap_id.clone()), pref.clone())))
}

fn host() -> (Engine, Host) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_plugin_resolver(resolver());
    (engine, h)
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-export-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Enregistre le projet sous `dir/P.daw` : l'export a alors un dossier par défaut (`P.daw/export`).
fn saved(h: &mut Host, dir: &Path) -> PathBuf {
    h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    dir.join("P.daw")
}

struct Done {
    files: Vec<ExportedFile>,
    warnings: Vec<String>,
    cancelled: bool,
    error: Option<String>,
    progress: Vec<f64>,
}

/// Envoie la commande puis fait tourner `tick` jusqu'à la fin de l'export.
fn export(h: &mut Host, json: &str) -> Done {
    let mut events = h.command_json(json);
    let mut progress = Vec::new();
    let start = Instant::now();
    loop {
        for e in events.drain(..) {
            match e {
                Event::Export { active: true, progress: p, .. } => progress.push(p),
                Event::Export { active: false, files, warnings, cancelled, error, .. } => return Done { files, warnings, cancelled, error, progress },
                Event::Error { message, .. } => return Done { files: vec![], warnings: vec![], cancelled: false, error: Some(message), progress },
                _ => {}
            }
        }
        assert!(start.elapsed() < Duration::from_secs(120), "export trop long");
        std::thread::sleep(Duration::from_millis(5));
        events = h.tick();
    }
}

fn wav(path: &str) -> daw_recorder::wav::WavData {
    decode_bytes(&std::fs::read(path).unwrap()).unwrap().audio
}

fn peak(w: &daw_recorder::wav::WavData) -> f32 {
    w.samples.iter().fold(0.0, |m, v| m.max(v.abs()))
}

const MIX: &str = r#"{"type":"export","scope":"project","mode":"mix","format":"wav32f","tail_secs":0}"#;

#[test]
fn the_mix_is_written_next_to_the_project_never_overwrites_and_is_not_a_modification() {
    let (_e, mut h) = host();
    let proj = saved(&mut h, &temp("mix"));
    let a = export(&mut h, MIX);
    assert!(a.error.is_none() && !a.cancelled, "{:?}", a.error);
    assert_eq!(a.files.len(), 1);
    let path = &a.files[0].path;
    assert!(Path::new(path).starts_with(proj.join("export")), "{path}");
    assert!(path.ends_with("P.wav"), "{path}");
    let w = wav(path);
    assert_eq!((w.sample_rate, w.channels), (SR, 2));
    assert_eq!(w.frames() as u64, a.files[0].frames);
    assert!(peak(&w) > 0.05 && peak(&w) <= 1.0, "pic {}", peak(&w));
    assert!(a.progress.windows(2).all(|p| p[1] >= p[0]), "la progression ne recule pas : {:?}", a.progress);
    // Deuxième export : un autre nom, le premier n'est pas touché ; et même contenu (rendu déterministe).
    let first = std::fs::read(path).unwrap();
    let b = export(&mut h, MIX);
    assert!(b.files[0].path.ends_with("P (2).wav"), "{}", b.files[0].path);
    assert_eq!(std::fs::read(&b.files[0].path).unwrap(), first);
    assert_eq!(std::fs::read(path).unwrap(), first);
    // Aucun fichier provisoire ne reste ; le projet n'est pas « modifié ».
    assert!(std::fs::read_dir(proj.join("export")).unwrap().all(|f| !f.unwrap().file_name().to_string_lossy().ends_with(".part")));
    assert!(h.tick().iter().all(|e| !matches!(e, Event::File { dirty: true, .. })));
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: false, .. })));
}

#[test]
fn stems_are_one_file_per_track_and_bus_with_the_same_length() {
    let (_e, mut h) = host();
    saved(&mut h, &temp("stems"));
    let d = export(&mut h, r#"{"type":"export","scope":"project","mode":"stems","format":"wav24","tail_secs":0,"name":"Chanson"}"#);
    assert!(d.error.is_none(), "{:?}", d.error);
    assert!(d.files.len() >= 5, "{} fichiers", d.files.len());
    let names: Vec<String> = d.files.iter().map(|f| Path::new(&f.path).file_name().unwrap().to_string_lossy().into_owned()).collect();
    for expected in ["Chanson - Batterie.wav", "Chanson - Basse.wav", "Chanson - Voix.wav"] {
        assert!(names.contains(&expected.to_owned()), "{expected} manque : {names:?}");
    }
    assert!(d.files.iter().all(|f| f.frames == d.files[0].frames), "tous alignés et de même longueur");
    assert!(d.files.iter().any(|f| f.peak_db.is_some_and(|p| p > -40.0)));
    assert!(d.progress.last().copied().unwrap_or(0.0) <= 1.0);
    // Choix de quelques nœuds seulement.
    let some = export(&mut h, r#"{"type":"export","scope":"project","mode":"stems","format":"flac16","tail_secs":0,"nodes":[2]}"#);
    assert_eq!(some.files.len(), 1);
    assert!(some.files[0].path.ends_with("P - Basse.flac"), "{}", some.files[0].path);
}

#[test]
fn formats_zone_dither_and_normalization_reach_the_files() {
    let (_e, mut h) = host();
    saved(&mut h, &temp("fmt"));
    // Boucle : exactement la zone, sans queue.
    h.command_json(r#"{"type":"loop","start":48000,"end":96000}"#);
    let l = export(&mut h, r#"{"type":"export","scope":"loop","mode":"mix","format":"flac24","tail_secs":0}"#);
    assert_eq!(l.files[0].frames, 48_000);
    let d = decode_bytes(&std::fs::read(&l.files[0].path).unwrap()).unwrap();
    assert_eq!((d.audio.frames(), d.source_bits, d.audio.sample_rate), (48_000, 24, SR));
    // Sélection explicite.
    let r = export(&mut h, r#"{"type":"export","scope":"range","start":1000,"end":5000,"mode":"mix","format":"wav16","tail_secs":0}"#);
    let w = decode_bytes(&std::fs::read(&r.files[0].path).unwrap()).unwrap();
    assert_eq!((w.audio.frames(), w.source_bits), (4_000, 16));
    // Normalisation au pic cible.
    let n = export(&mut h, r#"{"type":"export","scope":"project","mode":"mix","format":"wav32f","tail_secs":0,"normalize_db":-1.0}"#);
    let p = peak(&wav(&n.files[0].path));
    assert!((p - 10f32.powf(-0.05)).abs() < 1e-4, "pic {p}");
    assert!(n.files[0].applied_gain_db.abs() > 0.0);
}

#[test]
fn clipping_is_reported_as_a_warning() {
    let (_e, mut h) = host();
    saved(&mut h, &temp("clip"));
    for n in 1..=5 {
        h.command_json(&format!(r#"{{"type":"set_node","node":{n},"gain":2.0}}"#));
    }
    h.command_json(r#"{"type":"set_node","node":0,"gain":3.0}"#);
    let d = export(&mut h, r#"{"type":"export","scope":"project","mode":"mix","format":"wav16","tail_secs":0}"#);
    assert!(d.files[0].clipped > 0, "il devrait y avoir de l'écrêtage");
    assert!(d.warnings.iter().any(|w| w.contains("écrêtés")), "{:?}", d.warnings);
}

#[test]
fn a_plugin_is_rendered_with_its_current_settings() {
    let (mut engine, mut h) = host();
    saved(&mut h, &temp("plugin"));
    let ev = h.command_json(r#"{"type":"add_plugin","node":2,"clap_id":"com.daw-perso.test-gain","file":"test.clap"}"#);
    let id = ev.iter().rev().find_map(|e| if let Event::Project(p) = e { p.nodes.iter().find(|n| n.id == 2).map(|n| n.inserts[0].id) } else { None }).unwrap();
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..4 {
        engine.process(&mut l, &mut r);
    }
    let stem = |h: &mut Host| export(h, r#"{"type":"export","scope":"project","mode":"stems","format":"wav32f","tail_secs":0,"nodes":[2]}"#);
    let loud = stem(&mut h);
    assert!(loud.files[0].peak_db.is_some_and(|p| p > -40.0), "la basse sonne avec le gain par défaut");
    // Gain du plugin à 0 : sa piste devient muette dans l'export — la preuve que l'état courant du plugin a été transmis.
    h.command_json(&format!(r#"{{"type":"set_plugin_param","insert":{id},"param":{PARAM_GAIN},"value":0.0}}"#));
    for _ in 0..4 {
        engine.process(&mut l, &mut r);
    }
    let silent = stem(&mut h);
    assert_eq!(silent.files[0].peak_db, None, "plugin à gain 0 : silence");
    assert!(peak(&wav(&silent.files[0].path)) == 0.0);
}

#[test]
fn refusals_leave_nothing_behind() {
    let (_e, mut h) = host();
    let err = |d: Done| d.error.expect("une erreur");
    // Projet sans dossier et sans dossier choisi.
    assert!(err(export(&mut h, MIX)).contains("dossier"));
    // Mais un dossier explicite suffit.
    let dir = temp("explicit");
    let ok = export(&mut h, &format!(r#"{{"type":"export","scope":"project","mode":"mix","format":"wav24","tail_secs":0,"dir":{:?},"name":"a/b:c"}}"#, dir.to_string_lossy()));
    assert!(ok.error.is_none(), "{:?}", ok.error);
    assert!(ok.files[0].path.ends_with("a-b-c.wav"), "{}", ok.files[0].path);
    saved(&mut h, &temp("refus"));
    h.command_json(r#"{"type":"loop","start":null,"end":null}"#);
    assert!(err(export(&mut h, r#"{"type":"export","scope":"loop","mode":"mix","format":"wav24"}"#)).contains("boucle"));
    assert!(err(export(&mut h, r#"{"type":"export","scope":"range","start":10,"end":10,"mode":"mix","format":"wav24"}"#)).contains("start < end"));
    assert!(err(export(&mut h, r#"{"type":"export","scope":"project","mode":"mix","format":"mp3"}"#)).contains("invalide"));
    assert!(err(export(&mut h, r#"{"type":"export","scope":"project","mode":"mix","format":"wav24","tail_secs":99}"#)).contains("tail_secs"));
    assert!(err(export(&mut h, r#"{"type":"export","scope":"project","mode":"mix","format":"wav24","normalize_db":3}"#)).contains("normalize_db"));
    assert!(err(export(&mut h, r#"{"type":"export","scope":"project","mode":"stems","format":"wav24","normalize_db":-1}"#)).contains("normalisation"));
    assert!(err(export(&mut h, r#"{"type":"export","scope":"project","mode":"stems","format":"wav24","nodes":[0]}"#)).contains("sortie principale"));
    assert!(err(export(&mut h, r#"{"type":"export","scope":"project","mode":"stems","format":"wav24","nodes":[99]}"#)).contains("inconnu"));
}

#[test]
fn an_export_can_be_cancelled_and_blocks_new_project_while_it_runs() {
    let (_e, mut h) = host();
    let proj = saved(&mut h, &temp("cancel"));
    let json = r#"{"type":"export","scope":"project","mode":"stems","format":"wav24","tail_secs":60}"#;
    let start = h.command_json(json);
    assert!(start.iter().any(|e| matches!(e, Event::Export { active: true, .. })));
    // Pendant l'export : un second export et « Nouveau » sont refusés.
    let again = h.command_json(json);
    assert!(matches!(&again[0], Event::Error { message, .. } if message.contains("déjà")));
    let new = h.command_json(r#"{"type":"new_project"}"#);
    assert!(matches!(&new[0], Event::Error { message, .. } if message.contains("export")));
    h.command_json(r#"{"type":"cancel_export"}"#);
    let t = Instant::now();
    let end = loop {
        if let Some(Event::Export { active: false, cancelled, files, .. }) = h.tick().into_iter().find(|e| matches!(e, Event::Export { active: false, .. })) {
            break (cancelled, files);
        }
        assert!(t.elapsed() < Duration::from_secs(60), "l'annulation doit être rapide");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(end.0, "annulé");
    assert!(end.1.is_empty(), "annulé avant le premier stem : aucun fichier");
    let left: Vec<_> = std::fs::read_dir(proj.join("export")).unwrap().collect();
    assert!(left.is_empty(), "ni fichier ni provisoire : {left:?}");
    // Et l'hôte est libre de nouveau.
    assert!(export(&mut h, MIX).error.is_none());
}
