//! Nouveau, ouvrir, enregistrer : projet sur le disque (dossier « Nom.daw »), modifications non enregistrées, plugins avec leur état.

use std::path::PathBuf;
use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::{Engine, GraphDesc, InsertFactory, TrackFormat};
use daw_session::Session;
use daw_test_plugin::{entry, PLUGIN_ID};
use daw_ui::demo::demo_session;
use daw_ui::host::{PluginRef, ProjectMeta};
use daw_ui::protocol::ProjectInfo;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-proj-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn demo_host(rate: u32) -> (Engine, Host) {
    let (engine, handle) = Engine::new(rate, 256);
    let (session, meta) = demo_session(rate);
    (engine, Host::new(handle, session, meta).unwrap())
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

#[derive(Debug, PartialEq)]
struct FileInfo {
    name: String,
    path: Option<String>,
    dirty: bool,
    generation: u64,
    warnings: Vec<String>,
}

fn file(ev: &[Event]) -> FileInfo {
    ev.iter().rev().find_map(|e| if let Event::File { name, path, dirty, generation, warnings } = e { Some(FileInfo { name: name.clone(), path: path.clone(), dirty: *dirty, generation: *generation, warnings: warnings.clone() }) } else { None }).expect("événement file")
}

fn error(ev: &[Event]) -> String {
    ev.iter().find_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).expect("une erreur")
}

fn can_undo(ev: &[Event]) -> bool {
    ev.iter().rev().find_map(|e| if let Event::History { can_undo, .. } = e { Some(*can_undo) } else { None }).expect("événement history")
}

fn peaks(host: &mut Host, clip: u64) -> Vec<[f32; 2]> {
    match host.command_json(&format!(r#"{{"type":"get_peaks","clip":{clip},"from":0,"to":96000,"columns":64}}"#)).remove(0) {
        Event::Peaks { columns, .. } => columns,
        e => panic!("{e:?}"),
    }
}

#[test]
fn a_saved_project_reopens_identically_with_names_colors_and_audio() {
    let dir = temp("aller-retour");
    let (_, mut a) = demo_host(SR);
    a.command_json(r#"{"type":"set_auto_crossfade","enabled":false}"#);
    a.command_json(r#"{"type":"set_node","node":2,"gain":0.33,"pan":0.5,"mute":true}"#);
    let saved = a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("Chanson").to_string_lossy()));
    let f = file(&saved);
    assert_eq!((f.name.as_str(), f.dirty, f.warnings.is_empty()), ("Chanson", false, true), "{f:?}");
    assert!(f.path.unwrap().ends_with("Chanson.daw"));
    let proj = dir.join("Chanson.daw");
    assert!(proj.join("projet.json").exists());
    let mut wavs: Vec<String> = std::fs::read_dir(proj.join("audio")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    wavs.sort();
    assert_eq!(wavs, ["basse.wav", "batterie.wav", "guitare.wav", "voix.wav"], "l'audio de la démonstration est écrit sous audio/");
    assert!(!proj.join("projet.json.tmp").exists());

    let (_, mut b) = demo_host(SR);
    let opened = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    let f = file(&opened);
    assert_eq!((f.name.as_str(), f.dirty, f.generation), ("Chanson", false, 1));
    assert!(!can_undo(&opened), "l'historique n'est pas conservé");
    let before = a.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(project(&opened), project(&before), "même projet : pistes, clips, noms, couleurs, fondus, réglages");
    assert!(!project(&opened).auto_crossfade, "réglage conservé");
    assert_eq!(peaks(&mut a, 0), peaks(&mut b, 0), "même audio");
    assert_eq!(peaks(&mut a, 5), peaks(&mut b, 5));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unsaved_changes_are_reported_and_saving_in_place_keeps_a_backup() {
    let dir = temp("modifie");
    let (_, mut host) = demo_host(SR);
    assert!(!file(&host.command_json(r#"{"type":"get_project"}"#)).dirty);
    let ev = host.command_json(r#"{"type":"set_node","node":1,"gain":0.5}"#);
    assert!(file(&ev).dirty, "un réglage de piste est une modification");
    // Armer une piste n'est pas une modification du projet.
    let (_, mut calm) = demo_host(SR);
    let ev = calm.command_json(r#"{"type":"set_node","node":1,"arm":true}"#);
    assert!(!ev.iter().any(|e| matches!(e, Event::File { dirty: true, .. })));
    // Pas de dossier tant qu'on n'a pas fait « Enregistrer sous ».
    assert!(error(&host.command_json(r#"{"type":"save_project"}"#)).contains("Enregistrer sous"));
    let target = dir.join("A");
    host.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, target.to_string_lossy()));
    let ev = host.command_json(r#"{"type":"move_clip","clip":3,"start":200000}"#);
    assert!(file(&ev).dirty);
    let ev = host.command_json(r#"{"type":"save_project"}"#);
    assert!(!file(&ev).dirty);
    assert!(dir.join("A.daw/projet.json.bak").exists(), "la version précédente reste en .bak");
    // Annuler est aussi une modification par rapport à ce qui est enregistré.
    assert!(file(&host.command_json(r#"{"type":"undo"}"#)).dirty);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn bad_projects_are_refused_with_a_clear_message_and_change_nothing() {
    let dir = temp("refus");
    let (_, mut a) = demo_host(SR);
    a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    let p = dir.join("P.daw");
    let open = |host: &mut Host, path: &std::path::Path| host.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, path.to_string_lossy()));
    let (_, mut host) = demo_host(SR);
    let gen_before = file(&host.command_json(r#"{"type":"get_project"}"#)).generation;
    assert!(error(&open(&mut host, &dir.join("inexistant.daw"))).contains("lecture"));
    // Mauvais format, version trop récente, JSON abîmé.
    let json = std::fs::read_to_string(p.join("projet.json")).unwrap();
    let write = |text: &str| std::fs::write(p.join("projet.json"), text).unwrap();
    write("{ pas du json");
    assert!(error(&open(&mut host, &p)).contains("illisible"));
    write(&json.replacen("\"daw-project\"", "\"autre\"", 1));
    assert!(error(&open(&mut host, &p)).contains("n'est pas un projet"));
    write(&json.replacen("\"version\": 1,", "\"version\": 99,", 1));
    assert!(error(&open(&mut host, &p)).contains("plus récente"));
    // Chemin audio qui sort du dossier du projet.
    write(&json.replacen("audio/basse.wav", "../basse.wav", 1));
    assert!(error(&open(&mut host, &p)).contains("hors du dossier"));
    write(&json);
    // Fichier audio manquant.
    let basse = std::fs::read(p.join("audio/basse.wav")).unwrap();
    std::fs::remove_file(p.join("audio/basse.wav")).unwrap();
    assert!(error(&open(&mut host, &p)).contains("basse"));
    std::fs::write(p.join("audio/basse.wav"), basse).unwrap();
    // Fréquence d'échantillonnage différente de celle du moteur.
    let (_, mut other) = demo_host(44_100);
    // (ni erreur ni chargement : une proposition de conversion, voir `tests/convert.rs`)
    let ev = open(&mut other, &p);
    assert!(matches!(ev.as_slice(), [Event::RateMismatch { project_rate: 48_000, engine_rate: 44_100, .. }]), "{ev:?}");
    // Rien n'a changé dans l'hôte : même numéro de chargement, même projet.
    assert_eq!(file(&host.command_json(r#"{"type":"get_project"}"#)).generation, gen_before);
    assert_eq!(project(&host.command_json(r#"{"type":"get_project"}"#)).nodes.len(), 6);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_new_project_has_one_stereo_track_and_replaces_the_current_one() {
    let (_, mut host) = demo_host(SR);
    host.command_json(r#"{"type":"set_node","node":1,"gain":0.2}"#);
    let ev = host.command_json(r#"{"type":"new_project"}"#);
    let f = file(&ev);
    assert_eq!((f.name.as_str(), f.path, f.dirty, f.generation), ("Sans titre", None, false, 1));
    let p = project(&ev);
    assert_eq!(p.nodes.len(), 2);
    assert_eq!((p.nodes[1].name.as_str(), p.nodes[1].kind.as_str(), p.nodes[1].clips.len()), ("Piste 1", "stereo", 0));
    assert!(!can_undo(&ev));
    // Le nouveau projet est utilisable : on peut l'enregistrer et le rouvrir.
    let dir = temp("nouveau");
    host.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("Vide").to_string_lossy()));
    let (_, mut other) = demo_host(SR);
    let ev = other.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("Vide.daw").to_string_lossy()));
    assert_eq!(project(&ev).nodes.len(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Projet à une piste avec le plugin de test, dont l'état initial est `DTP2`, gain 0,25, latence supplémentaire 100.
fn plugin_host(resolver_knows_plugin: bool) -> Host {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let plain = clap_insert_factory(Arc::clone(&bundle), PLUGIN_ID.to_str().unwrap().into());
    let mut state = b"DTP2".to_vec();
    state.extend_from_slice(&0.25f64.to_le_bytes());
    state.extend_from_slice(&100u32.to_le_bytes());
    let with_state: InsertFactory = {
        let inner = Arc::clone(&plain);
        Arc::new(move |sr, mb, _| inner(sr, mb, Some(&state)))
    };
    let mut g = GraphDesc::new();
    let t = g.add_track("Synthé", TrackFormat::Stereo);
    let id = g.add_insert(t, "Gain de test", with_state).unwrap();
    let mut meta = ProjectMeta::default();
    meta.plugins.insert(id, pref());
    let (_, handle) = Engine::new(SR, 256);
    let mut host = Host::new(handle, Session::new_at(g, SR), meta).unwrap();
    host.set_plugin_resolver(resolver(resolver_knows_plugin));
    host
}

fn pref() -> PluginRef {
    PluginRef { clap_id: PLUGIN_ID.to_str().unwrap().into(), vendor: "daw-perso".into(), name: "Gain de test".into(), file: String::new() }
}

fn resolver(knows: bool) -> daw_ui::host::PluginResolver {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    Box::new(move |e| (knows && e.clap_id == PLUGIN_ID.to_str().unwrap()).then(|| (clap_insert_factory(Arc::clone(&bundle), e.clap_id.clone()), pref())))
}

fn latency(ev: &[Event]) -> u32 {
    project(ev).latency_samples
}

#[test]
fn plugins_come_back_with_their_state_and_a_missing_one_is_kept_for_later() {
    let dir = temp("plugins");
    let mut a = plugin_host(true);
    assert_eq!(latency(&a.command_json(r#"{"type":"get_project"}"#)), 64 + 100, "l'état initial change la latence");
    let saved = a.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("S").to_string_lossy()));
    assert!(file(&saved).warnings.is_empty(), "{:?}", file(&saved));
    let text = std::fs::read_to_string(dir.join("S.daw/projet.json")).unwrap();
    assert!(text.contains("com.daw-perso.test-gain") && text.contains("\"state\""), "plugin et état écrits");

    // Réouverture avec le plugin installé : état restauré avant l'activation, donc même latence.
    let mut b = plugin_host(true);
    b.command_json(r#"{"type":"new_project"}"#);
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("S.daw").to_string_lossy()));
    assert!(file(&ev).warnings.is_empty());
    assert_eq!(latency(&ev), 164, "l'état du plugin est revenu");
    // Enregistré de nouveau, l'état est identique.
    b.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("S2").to_string_lossy()));
    let state_of = |p: &str| serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(dir.join(p).join("projet.json")).unwrap()).unwrap()["plugins"][0]["state"].clone();
    assert_eq!(state_of("S.daw"), state_of("S2.daw"));

    // Plugin absent de la machine : projet ouvert avec un avertissement, plugin gardé dans le fichier à l'enregistrement suivant.
    let mut c = plugin_host(false);
    let ev = c.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("S.daw").to_string_lossy()));
    let w = file(&ev).warnings;
    assert!(w.len() == 1 && w[0].contains("introuvable") && w[0].contains("Gain de test"), "{w:?}");
    assert_eq!(latency(&ev), 0, "sans le plugin, aucune latence");
    c.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("S3").to_string_lossy()));
    assert_eq!(state_of("S.daw"), state_of("S3.daw"), "le plugin absent et son état sont conservés");
    // Retrouvé plus tard : tout revient.
    let mut d = plugin_host(true);
    let ev = d.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("S3.daw").to_string_lossy()));
    assert_eq!(latency(&ev), 164);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn opening_a_project_replaces_the_running_plugins_instead_of_adopting_them() {
    // Ouvrir un projet sans plugin sur un hôte qui en avait un : le plugin disparaît (plus de latence).
    let dir = temp("remplace");
    let (_, mut plain) = demo_host(SR);
    plain.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("Nu").to_string_lossy()));
    let mut with_plugin = plugin_host(true);
    assert_eq!(latency(&with_plugin.command_json(r#"{"type":"get_project"}"#)), 164);
    let ev = with_plugin.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("Nu.daw").to_string_lossy()));
    assert_eq!(latency(&ev), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}
