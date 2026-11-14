//! Projets récents, réouverture du dernier projet au démarrage, rassemblement des fichiers référencés en place.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daw_engine::Engine;
use daw_offline_render::export::{encode, BitDepth, Container, Format};
use daw_ui::demo::demo_session;
use daw_ui::protocol::RecentInfo;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> (Engine, Host) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    (engine, Host::new(handle, session, meta).unwrap())
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-recent-gather-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn recent(ev: &[Event]) -> Option<Vec<RecentInfo>> {
    ev.iter().rev().find_map(|e| if let Event::Recent { projects } = e { Some(projects.clone()) } else { None })
}

fn save_as(h: &mut Host, dir: &Path, name: &str) -> Vec<Event> {
    h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join(name).to_string_lossy()))
}

fn open(h: &mut Host, dir: &Path, name: &str) -> Vec<Event> {
    h.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join(format!("{name}.daw")).to_string_lossy()))
}

fn names(r: &[RecentInfo]) -> Vec<&str> {
    r.iter().map(|p| p.name.as_str()).collect()
}

#[test]
fn saved_and_opened_projects_go_to_the_top_of_the_list_and_survive_a_restart() {
    let dir = temp("list");
    let file = dir.join("recent.json");
    let (_e, mut h) = host();
    h.set_recent_path(Some(file.clone()));
    assert!(recent(&h.command_json(r#"{"type":"get_project"}"#)).unwrap().is_empty());
    let ev = save_as(&mut h, &dir, "Premier");
    assert_eq!(names(&recent(&ev).expect("la liste est renvoyée")), ["Premier"]);
    save_as(&mut h, &dir, "Deuxieme");
    let ev = open(&mut h, &dir, "Premier");
    let r = recent(&ev).unwrap();
    assert_eq!(names(&r), ["Premier", "Deuxieme"], "le projet ouvert passe en tête, sans doublon");
    assert!(r.iter().all(|p| p.exists));
    // Une simple modification ne change rien (et ne réécrit pas la liste).
    let ev = h.command_json(r#"{"type":"set_node","node":1,"gain":0.5}"#);
    assert!(recent(&ev).is_none());
    // Un autre hôte, même fichier : la liste est relue.
    let (_e2, mut h2) = host();
    h2.set_recent_path(Some(file.clone()));
    assert_eq!(names(&recent(&h2.command_json(r#"{"type":"get_recent"}"#)).unwrap()), ["Premier", "Deuxieme"]);
    // Un dossier supprimé reste listé mais signalé.
    std::fs::remove_dir_all(dir.join("Deuxieme.daw")).unwrap();
    let r = recent(&h2.command_json(r#"{"type":"get_recent"}"#)).unwrap();
    assert_eq!(r.iter().map(|p| p.exists).collect::<Vec<_>>(), [true, false]);
    // Vider la liste.
    assert!(recent(&h2.command_json(r#"{"type":"clear_recent"}"#)).unwrap().is_empty());
    assert!(daw_ui::recent::load(&file).is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_last_project_reopens_at_startup_only_when_the_option_is_on() {
    let dir = temp("startup");
    let file = dir.join("recent.json");
    {
        let (_e, mut h) = host();
        h.set_recent_path(Some(file.clone()));
        save_as(&mut h, &dir, "Ancien");
        save_as(&mut h, &dir, "Dernier");
    }
    // Option éteinte (défaut) : on démarre sur le projet de démonstration.
    let (_e, mut h) = host();
    h.set_recent_path(Some(file.clone()));
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let name = ev.iter().rev().find_map(|e| if let Event::File { name, .. } = e { Some(name.clone()) } else { None }).unwrap();
    assert_ne!(name, "Dernier");
    // Option allumée : le dernier projet s'ouvre, une seule fois.
    let (_e, mut h) = host();
    h.set_recent_path(Some(file.clone()));
    let prefs = daw_ui::prefs::Prefs { startup: daw_ui::prefs::StartupPrefs { reopen_last: true }, ..Default::default() };
    h.command(daw_ui::protocol::Command::SetPrefs { prefs });
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let name = ev.iter().rev().find_map(|e| if let Event::File { name, .. } = e { Some(name.clone()) } else { None }).unwrap();
    assert_eq!(name, "Dernier", "{ev:?}");
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })));
    // « Nouveau » puis une seconde demande du projet : on ne rouvre pas à nouveau.
    h.command_json(r#"{"type":"new_project"}"#);
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let name = ev.iter().rev().find_map(|e| if let Event::File { name, .. } = e { Some(name.clone()) } else { None }).unwrap();
    assert_ne!(name, "Dernier");
    // Dossier disparu : on démarre normalement, sans erreur.
    std::fs::remove_dir_all(dir.join("Dernier.daw")).unwrap();
    let (_e, mut h) = host();
    h.set_recent_path(Some(file.clone()));
    h.command(daw_ui::protocol::Command::SetPrefs { prefs: daw_ui::prefs::Prefs { startup: daw_ui::prefs::StartupPrefs { reopen_last: true }, ..Default::default() } });
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    std::fs::remove_dir_all(&dir).ok();
}

fn tone_file(dir: &Path, name: &str) -> PathBuf {
    let n = SR as usize / 2;
    let l: Vec<f32> = (0..n).map(|i| 0.5 * (i as f32 * std::f32::consts::TAU * 440.0 / SR as f32).sin()).collect();
    let e = encode(&l, &l, SR, Format { container: Container::Flac, depth: BitDepth::Pcm16, dither: false }, None).unwrap();
    let p = dir.join(format!("{name}.flac"));
    std::fs::write(&p, e.bytes).unwrap();
    p
}

fn import_in_place(h: &mut Host, file: &Path) {
    let mut events = h.command_json(&format!(r#"{{"type":"import_audio","files":[{:?}],"reference":true,"pos":0}}"#, file.to_string_lossy()));
    let t = Instant::now();
    loop {
        assert!(!events.iter().any(|e| matches!(e, Event::Error { .. })), "{events:?}");
        if events.iter().any(|e| matches!(e, Event::Import { active: false, .. })) {
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(60), "import trop long");
        std::thread::sleep(Duration::from_millis(5));
        events = h.tick();
    }
}

fn gathered(ev: &[Event]) -> (Vec<String>, Vec<String>) {
    ev.iter().find_map(|e| if let Event::Gathered { copied, warnings } = e { Some((copied.clone(), warnings.clone())) } else { None }).expect("événement gathered")
}

#[test]
fn gathering_copies_referenced_files_into_the_project_and_leaves_the_originals_alone() {
    let dir = temp("gather");
    let src_dir = temp("gather-src");
    let (_e, mut h) = host();
    // Sans dossier de projet : refusé, rien ne change.
    let ev = h.command_json(r#"{"type":"gather_files"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Error { message, .. } if message.contains("Enregistrer sous"))), "{ev:?}");
    save_as(&mut h, &dir, "P");
    let proj = dir.join("P.daw");
    let a = tone_file(&src_dir, "Externe");
    let b = tone_file(&src_dir, "Perdu");
    import_in_place(&mut h, &a);
    import_in_place(&mut h, &b);
    h.command_json(r#"{"type":"save_project"}"#);
    let audio_before: Vec<String> = std::fs::read_dir(proj.join("audio")).map(|d| d.map(|f| f.unwrap().file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    assert!(audio_before.iter().all(|f| !f.contains("Externe")), "référencé, pas copié : {audio_before:?}");
    assert!(std::fs::read_to_string(proj.join("projet.json")).unwrap().contains("Externe.flac"));
    // Rassembler : une copie par fichier référencé ; les originaux restent.
    let ev = h.command_json(r#"{"type":"gather_files"}"#);
    let (copied, warnings) = gathered(&ev);
    assert_eq!(copied.len(), 2, "{copied:?} {warnings:?}");
    assert!(copied.iter().any(|c| c.starts_with("audio/Externe")) && copied.iter().any(|c| c.starts_with("audio/Perdu")));
    assert!(warnings.is_empty());
    assert!(copied.iter().all(|c| proj.join(c).exists()));
    assert!(a.exists() && b.exists(), "les originaux ne sont pas touchés");
    assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: true, .. })), "le projet est modifié : il faut l'enregistrer");
    // Enregistré : le projet ne désigne plus que ses propres fichiers ; l'original peut disparaître sans conséquence.
    h.command_json(r#"{"type":"save_project"}"#);
    let json = std::fs::read_to_string(proj.join("projet.json")).unwrap();
    assert!(json.contains("audio/Externe") && !json.contains(&*src_dir.to_string_lossy().replace('\\', "\\\\")), "plus de chemin absolu : {json}");
    std::fs::remove_dir_all(&src_dir).unwrap();
    let ev = open(&mut h, &dir, "P");
    let warnings: Vec<String> = ev.iter().filter_map(|e| if let Event::File { warnings, .. } = e { Some(warnings.clone()) } else { None }).flatten().collect();
    assert!(!warnings.iter().any(|w| w.contains("manquant")), "{warnings:?}");
    // Rien à rassembler la seconde fois.
    let (copied, warnings) = gathered(&h.command_json(r#"{"type":"gather_files"}"#));
    assert!(copied.is_empty() && warnings.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_referenced_file_is_reported_and_left_as_it_is() {
    let dir = temp("gather-missing");
    let src_dir = temp("gather-missing-src");
    let (_e, mut h) = host();
    save_as(&mut h, &dir, "P");
    let a = tone_file(&src_dir, "Absent");
    import_in_place(&mut h, &a);
    h.command_json(r#"{"type":"save_project"}"#);
    std::fs::remove_file(&a).unwrap();
    open(&mut h, &dir, "P");
    let (copied, warnings) = gathered(&h.command_json(r#"{"type":"gather_files"}"#));
    assert!(copied.is_empty());
    assert!(warnings.iter().any(|w| w.contains("introuvable") && w.contains("Absent")), "{warnings:?}");
    h.command_json(r#"{"type":"save_project"}"#);
    assert!(std::fs::read_to_string(dir.join("P.daw/projet.json")).unwrap().contains("Absent.flac"), "le chemin d'origine est conservé");
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&src_dir).ok();
}
