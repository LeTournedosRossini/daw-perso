//! Préférences par le protocole : lecture, enregistrement sur disque, métronome appliqué et mémorisé, redémarrage nécessaire.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::prefs::{self, Effective, MetroPrefs, Prefs, Runtime};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn rig() -> (Engine, Host) {
    let (engine, handle) = Engine::new(SR, 256);
    let (session, meta) = demo_session(SR);
    (engine, Host::new(handle, session, meta).unwrap())
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-prefs-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn prefs_event(ev: &[Event]) -> (Prefs, Option<Runtime>, bool, Option<String>) {
    ev.iter().rev().find_map(|e| if let Event::Prefs { prefs, running, restart_needed, path } = e { Some((prefs.clone(), running.clone(), *restart_needed, path.clone())) } else { None }).expect("événement prefs")
}

fn runtime(driver: &str, buffer: u32, threads: u32, ahead_ms: u32) -> Runtime {
    Runtime { effective: Effective { driver: driver.into(), buffer, threads, ahead_ms, ahead_threads: 2, forced: Vec::new() }, sample_rate: 44_100, inputs: 10, drivers: vec!["UMC ASIO Driver".into(), "ASIO4ALL v2".into()], cores: 4, audio: true }
}

#[test]
fn get_project_and_get_prefs_report_defaults_with_no_file() {
    let (_e, mut host) = rig();
    let (p, running, restart, path) = prefs_event(&host.command_json(r#"{"type":"get_project"}"#));
    assert_eq!(p, Prefs::default());
    assert!(running.is_none() && !restart && path.is_none());
    assert_eq!(prefs_event(&host.command_json(r#"{"type":"get_prefs"}"#)).0, Prefs::default());
}

#[test]
fn set_prefs_writes_the_file_applies_the_metronome_and_flags_a_restart() {
    let dir = temp("set");
    let file = dir.join("preferences.json");
    let (_e, mut host) = rig();
    host.set_preferences(Prefs::default(), Some(file.clone()));
    host.set_runtime(runtime("UMC", 64, 1, 0));
    let cmd = r#"{"type":"set_prefs","prefs":{"version":1,"driver":"ASIO4ALL v2","buffer":32,"threads":3,"ahead_ms":50,"ahead_threads":2,
        "metronome":{"enabled":true,"only_recording":false,"volume":1.0,"count_in_bars":2,"pre_roll":1.5,"post_roll":0.5}}}"#;
    let ev = host.command_json(cmd);
    let (p, running, restart, path) = prefs_event(&ev);
    assert_eq!((p.driver.as_deref(), p.buffer, p.threads, p.ahead_ms), (Some("ASIO4ALL v2"), Some(32), 3, 50));
    assert!(running.is_some());
    assert!(restart, "le pilote, le tampon, les fils et l'avance enregistrés diffèrent de ce qui tourne");
    assert_eq!(path.as_deref(), file.to_str());
    // Le métronome est appliqué tout de suite (événement `metronome`) et le fichier est écrit.
    assert!(ev.iter().any(|e| matches!(e, Event::Metronome { enabled: true, count_in_bars: 2, .. })));
    assert_eq!(prefs::load(&file, &Prefs::default()), (p, None));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_restart_when_saved_audio_settings_are_the_running_ones() {
    let (_e, mut host) = rig();
    host.set_runtime(runtime("UMC", 64, 1, 0));
    let (_, _, restart, _) = prefs_event(&host.command_json(r#"{"type":"get_prefs"}"#));
    assert!(!restart);
    let ev = host.command_json(r#"{"type":"set_prefs","prefs":{"buffer":64,"driver":"UMC"}}"#);
    assert!(!prefs_event(&ev).2);
    // Le même pilote désigné par son nom complet n'est pas un changement.
    let ev = host.command_json(r#"{"type":"set_prefs","prefs":{"buffer":64,"driver":"UMC ASIO Driver"}}"#);
    assert!(!prefs_event(&ev).2);
    let ev = host.command_json(r#"{"type":"set_prefs","prefs":{"buffer":128}}"#);
    assert!(prefs_event(&ev).2);
}

#[test]
fn invalid_prefs_are_refused_without_changing_anything() {
    let dir = temp("bad");
    let file = dir.join("p.json");
    let (_e, mut host) = rig();
    host.set_preferences(Prefs { buffer: Some(256), ..Prefs::default() }, Some(file.clone()));
    for bad in [r#"{"type":"set_prefs","prefs":{"buffer":3}}"#, r#"{"type":"set_prefs","prefs":{"threads":0}}"#, r#"{"type":"set_prefs","prefs":{"ahead_ms":7}}"#, r#"{"type":"set_prefs","prefs":{"metronome":{"volume":9}}}"#, r#"{"type":"set_prefs","prefs":{"buffer":"beaucoup"}}"#] {
        let ev = host.command_json(bad);
        assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })), "{bad}");
    }
    assert_eq!(prefs_event(&host.command_json(r#"{"type":"get_prefs"}"#)).0.buffer, Some(256));
    assert!(!file.exists(), "rien d'écrit après des refus");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn metronome_changes_are_remembered_in_the_file_after_a_moment() {
    let dir = temp("metro");
    let file = dir.join("preferences.json");
    let (_e, mut host) = rig();
    host.set_preferences(Prefs::default(), Some(file.clone()));
    host.command_json(r#"{"type":"set_metronome","enabled":true,"only_recording":true,"volume":0.5,"count_in_bars":1,"pre_roll":2.0,"post_roll":0.0}"#);
    assert!(!file.exists(), "écrit un instant plus tard, pas à chaque geste");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    host.tick();
    let (p, w) = prefs::load(&file, &Prefs::default());
    assert!(w.is_none());
    assert_eq!(p.metronome, MetroPrefs { enabled: true, only_recording: true, volume: 0.5, count_in_bars: 1, pre_roll: 2.0, post_roll: 0.0, pre_roll_bars: 0, pre_roll_unit: daw_ui::prefs::PreRollUnit::Seconds });
    // Un autre démarrage avec ce fichier retrouve le métronome tel quel.
    let (_e2, mut host2) = rig();
    host2.set_preferences(p, Some(file.clone()));
    let ev = host2.command_json(r#"{"type":"get_project"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Metronome { enabled: true, only_recording: true, count_in_bars: 1, .. })));
    let _ = std::fs::remove_dir_all(&dir);
}
