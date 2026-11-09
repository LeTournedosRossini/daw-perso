//! Sauvegarde automatique : écrite à côté du projet, jamais à sa place, reprise après une fermeture brutale.

use std::path::PathBuf;
use std::time::Duration;

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;
const EVERY: Duration = Duration::from_millis(30);
const AUTOSAVE: &str = "projet.autosave.json";

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-auto-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn host() -> Host {
    let (_, handle) = Engine::new(SR, 256);
    let (session, meta) = demo_session(SR);
    Host::new(handle, session, meta).unwrap()
}

/// Laisse passer la période puis fait un `tick` : rend les événements.
fn wait_and_tick(h: &mut Host) -> Vec<Event> {
    std::thread::sleep(EVERY * 2);
    h.tick()
}

fn gain_of(ev: &[Event], node: u32) -> f32 {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { p.nodes.iter().find(|n| n.id == node).map(|n| n.gain) } else { None }).expect("projet")
}

fn file(ev: &[Event]) -> (String, Option<String>, bool) {
    ev.iter().rev().find_map(|e| if let Event::File { name, path, dirty, .. } = e { Some((name.clone(), path.clone(), *dirty)) } else { None }).expect("événement file")
}

fn offer(ev: &[Event]) -> Option<(String, bool)> {
    ev.iter().find_map(|e| if let Event::Autosave { path, untitled, .. } = e { Some((path.clone(), *untitled)) } else { None })
}

fn save(h: &mut Host, dir: &std::path::Path) {
    let ev = h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    assert!(!file(&ev).2, "enregistré");
}

#[test]
fn the_autosave_goes_next_to_the_saved_project_without_touching_it_and_only_when_modified() {
    let root = temp("cote");
    let mut h = host();
    h.set_autosave(Some(root.join("data")), EVERY);
    save(&mut h, &root);
    let proj = root.join("P.daw");
    let saved = std::fs::read(proj.join("projet.json")).unwrap();
    // Rien à sauvegarder : pas de fichier.
    wait_and_tick(&mut h);
    assert!(!proj.join(AUTOSAVE).exists(), "projet non modifié : aucune sauvegarde automatique");
    // Une modification : la sauvegarde apparaît, le projet enregistré reste identique, il est toujours « modifié ».
    let ev = h.command_json(r#"{"type":"set_node","node":1,"gain":0.5}"#);
    assert!(file(&ev).2);
    wait_and_tick(&mut h);
    assert!(proj.join(AUTOSAVE).exists());
    assert_eq!(std::fs::read(proj.join("projet.json")).unwrap(), saved, "le projet enregistré n'est jamais touché");
    assert!(file(&h.command_json(r#"{"type":"get_project"}"#)).2, "toujours modifié");
    // Enregistrer la supprime.
    save(&mut h, &root);
    assert!(!proj.join(AUTOSAVE).exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn after_a_crash_opening_the_project_offers_the_newer_autosave_and_recovering_restores_the_edit() {
    let root = temp("reprise");
    let mut a = host();
    a.set_autosave(None, EVERY);
    save(&mut a, &root);
    let proj = root.join("P.daw");
    a.command_json(r#"{"type":"set_node","node":1,"gain":0.5}"#);
    wait_and_tick(&mut a);
    assert!(proj.join(AUTOSAVE).exists());
    drop(a); // « plantage » : ni enregistrement ni nettoyage

    let mut b = host();
    b.set_autosave(None, EVERY);
    // Ouverture normale : le projet enregistré (gain d'origine) et la proposition de reprise.
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    assert_ne!(gain_of(&ev, 1), 0.5);
    assert_eq!(offer(&ev), Some((proj.to_string_lossy().into_owned(), false)));
    assert!(!file(&ev).2);
    // Reprise : l'état de la sauvegarde, marqué modifié.
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?},"recover":true}}"#, proj.to_string_lossy()));
    assert_eq!(gain_of(&ev, 1), 0.5);
    let (name, path, dirty) = file(&ev);
    assert_eq!((name.as_str(), path.is_some(), dirty), ("P", true, true));
    // Refus : on oublie la sauvegarde ; une ouverture suivante ne propose plus rien.
    b.command_json(&format!(r#"{{"type":"discard_autosave","path":{:?}}}"#, proj.to_string_lossy()));
    assert!(!proj.join(AUTOSAVE).exists());
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    assert_eq!(offer(&ev), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_saved_project_does_not_offer_an_autosave_older_than_it() {
    let root = temp("ancienne");
    let mut a = host();
    a.set_autosave(None, EVERY);
    save(&mut a, &root);
    let proj = root.join("P.daw");
    a.command_json(r#"{"type":"set_node","node":1,"gain":0.5}"#);
    wait_and_tick(&mut a);
    let auto = std::fs::read(proj.join(AUTOSAVE)).unwrap();
    std::thread::sleep(Duration::from_millis(1100)); // la date de fichier est à la seconde près sur certains systèmes
    save(&mut a, &root); // supprime la sauvegarde ; on la remet pour simuler un reste plus ancien que le projet
    std::fs::write(proj.join(AUTOSAVE), &auto).unwrap();
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options().write(true).open(proj.join(AUTOSAVE)).unwrap().set_modified(old).unwrap();
    let mut b = host();
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    assert_eq!(offer(&ev), None, "sauvegarde plus ancienne que le projet : rien à proposer");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_untitled_project_is_autosaved_in_the_data_folder_and_offered_back_at_startup() {
    let root = temp("sans-titre");
    let data = root.join("data");
    let mut a = host();
    a.set_autosave(Some(data.clone()), EVERY);
    a.command_json(r#"{"type":"set_node","node":1,"gain":0.5}"#);
    wait_and_tick(&mut a);
    let dir = data.join("sans-titre.daw");
    assert!(dir.join(AUTOSAVE).exists());
    assert!(dir.join("audio").exists(), "les fichiers audio du projet sans titre sont rangés avec la sauvegarde");
    drop(a); // fermeture brutale

    let mut b = host();
    b.set_autosave(Some(data.clone()), EVERY);
    let ev = b.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(offer(&ev), Some((dir.to_string_lossy().into_owned(), true)));
    // Une seule proposition par démarrage.
    assert_eq!(offer(&b.command_json(r#"{"type":"get_project"}"#)), None);
    // Reprise : toujours « Sans titre », sans dossier, modifié.
    let ev = b.command_json(&format!(r#"{{"type":"open_project","path":{:?},"recover":true}}"#, dir.to_string_lossy()));
    assert_eq!(gain_of(&ev, 1), 0.5);
    let (name, path, dirty) = file(&ev);
    assert_eq!((name.as_str(), path, dirty), ("Sans titre", None, true));
    // L'enregistrer sous un nom supprime la sauvegarde du projet sans titre.
    save(&mut b, &root);
    assert!(!dir.join(AUTOSAVE).exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn without_set_autosave_nothing_is_ever_written() {
    // Sans `set_autosave`, rien n'est jamais écrit (comportement des tests et de l'aperçu navigateur).
    let root = temp("inactive");
    let mut h = host();
    h.command_json(r#"{"type":"set_node","node":1,"gain":0.5}"#);
    std::thread::sleep(Duration::from_millis(100));
    h.tick();
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    let _ = std::fs::remove_dir_all(&root);
}
