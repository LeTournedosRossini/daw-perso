//! Retours de monitoring dans l'hôte : un bus sort sur des canaux de la carte son, par le protocole.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::NodeInfo;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn project(ev: &[Event]) -> &daw_ui::protocol::ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de nœud « {name} »"))
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

#[test]
fn a_bus_becomes_a_return_on_the_sound_card_by_the_protocol_and_the_engine_plays_it_there() {
    let (mut engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut host = Host::new(handle, session, meta).unwrap();
    host.set_audio_outputs(8);
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(project(&ev).audio_outputs, 8);

    // Un bus « Retour Marie » ; la guitare lui envoie tout son niveau, avant le fader.
    let ev = host.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Marie"}"#);
    let (bus, guitar) = (node(&ev, "Retour Marie").id, node(&ev, "Guitare").id);
    assert!(node(&ev, "Retour Marie").phys.is_none());
    host.command_json(&format!(r#"{{"type":"add_send","node":{guitar},"to":{bus}}}"#));
    host.command_json(&format!(r#"{{"type":"set_send","node":{guitar},"to":{bus},"gain":1.0,"pre":true}}"#));

    // Sorties 3-4 en stéréo.
    let ev = host.command_json(&format!(r#"{{"type":"set_return_output","node":{bus},"output":{{"first":3,"stereo":true,"mono":false}}}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let p = node(&ev, "Retour Marie").phys.expect("le retour a une sortie");
    assert_eq!((p.first, p.stereo, p.mono), (3, true, false));
    assert!(ev.iter().any(|e| matches!(e, Event::History { undo_label: Some(l), .. } if l == "Sortie d'un retour")));

    // Le moteur joue la guitare sur les sorties 3-4 (canaux 2 et 3 du moteur) ; la sortie principale n'a pas le bus en plus.
    host.command_json(r#"{"type":"seek","pos":240000}"#);
    host.command_json(r#"{"type":"transport","action":"play"}"#);
    let mut bufs = [[0.0f32; 64]; 4];
    let mut energy = [0.0f32; 4];
    for _ in 0..400 {
        let [a, b, c, d] = &mut bufs;
        let mut parts: [&mut [f32]; 4] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..]];
        engine.process_outputs(&[], &mut parts);
        for (k, buf) in bufs.iter().enumerate() {
            energy[k] += buf.iter().map(|v| v * v).sum::<f32>();
        }
    }
    assert!(energy[2] > 0.0 && energy[3] > 0.0, "le retour sort sur les sorties 3-4 : {energy:?}");
    assert!(energy[0] > 0.0, "la sortie principale joue le reste du morceau");

    // Refus : sorties de la principale, au-delà de ce que le pilote a ouvert, une piste, un nœud inconnu ; rien ne change.
    for cmd in [
        format!(r#"{{"type":"set_return_output","node":{bus},"output":{{"first":2,"stereo":true,"mono":false}}}}"#),
        format!(r#"{{"type":"set_return_output","node":{bus},"output":{{"first":8,"stereo":true,"mono":false}}}}"#),
        format!(r#"{{"type":"set_return_output","node":{guitar},"output":{{"first":5,"stereo":false,"mono":true}}}}"#),
        r#"{"type":"set_return_output","node":99,"output":{"first":5,"stereo":false,"mono":true}}"#.to_owned(),
    ] {
        assert_eq!(errors(&host.command_json(&cmd)).len(), 1, "{cmd}");
    }
    assert_eq!(node(&host.command_json(r#"{"type":"get_project"}"#), "Retour Marie").phys.map(|p| p.first), Some(3));

    // Annuler rend le bus à la sortie principale ; rétablir lui redonne ses sorties ; retirer par `null`.
    let ev = host.command_json(r#"{"type":"undo"}"#);
    assert!(node(&ev, "Retour Marie").phys.is_none());
    host.command_json(r#"{"type":"redo"}"#);
    let ev = host.command_json(&format!(r#"{{"type":"set_return_output","node":{bus},"output":null}}"#));
    assert!(node(&ev, "Retour Marie").phys.is_none());
}

#[test]
fn the_return_survives_saving_and_reopening_the_project() {
    let dir = std::env::temp_dir().join(format!("daw-returns-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let make = || {
        let (_e, handle) = Engine::new(SR, 64);
        std::mem::forget(_e);
        let (session, meta) = demo_session(SR);
        let mut h = Host::new(handle, session, meta).unwrap();
        h.set_audio_outputs(8);
        h
    };
    let mut h = make();
    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Paul"}"#);
    let bus = node(&ev, "Retour Paul").id;
    h.command_json(&format!(r#"{{"type":"set_return_output","node":{bus},"output":{{"first":5,"stereo":false,"mono":true}}}}"#));
    let path = dir.join("Retours");
    assert!(errors(&h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, path.to_string_lossy()))).is_empty());
    let mut h2 = make();
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, path.with_extension("daw").to_string_lossy()));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let p = node(&ev, "Retour Paul").phys.expect("la sortie est revenue");
    assert_eq!((p.first, p.stereo, p.mono), (5, false, true));
    let _ = std::fs::remove_dir_all(&dir);
}

fn rig() -> Host {
    let (_e, handle) = Engine::new(SR, 64);
    std::mem::forget(_e);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_audio_outputs(8);
    h
}

#[test]
fn a_return_is_created_with_its_output_in_one_undo_step() {
    let mut h = rig();
    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Lou","output":{"first":3,"stereo":true,"mono":false}}"#);
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let n = node(&ev, "Retour Lou");
    assert!(n.is_return && n.kind == "bus");
    assert_eq!(n.phys.map(|p| (p.first, p.stereo)), Some((3, true)));
    assert!(ev.iter().any(|e| matches!(e, Event::History { undo_label: Some(l), .. } if l == "Ajouter un retour")));
    // Un seul pas : annuler retire le retour entier.
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert!(project(&ev).nodes.iter().all(|n| n.name != "Retour Lou"));
    let ev = h.command_json(r#"{"type":"redo"}"#);
    assert_eq!(node(&ev, "Retour Lou").phys.map(|p| p.first), Some(3));
    // Sans sortie : créé quand même, muet, avec son type.
    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Nono"}"#);
    let n = node(&ev, "Retour Nono");
    assert!(n.is_return && n.phys.is_none());
}

#[test]
fn a_plain_bus_can_leave_by_the_sound_card_without_becoming_a_return() {
    let mut h = rig();
    let ev = h.command_json(r#"{"type":"add_track","kind":"bus","name":"Sous-groupe","output":{"first":5,"stereo":true,"mono":false}}"#);
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let n = node(&ev, "Sous-groupe");
    assert!(!n.is_return && n.phys.map(|p| p.first) == Some(5));
    // Pas de QR, pas d'onglet de mix, pas de groupes propres.
    let bus = n.id;
    let ev = h.command_json(&format!(r#"{{"type":"customize_return_groups","ret":{bus}}}"#));
    assert_eq!(errors(&ev).len(), 1, "{ev:?}");
    // Les canaux pris par un bus ne se donnent pas à un retour (refus sans trace), et inversement.
    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour pris","output":{"first":5,"stereo":false,"mono":true}}"#);
    assert_eq!(errors(&ev).len(), 1, "{ev:?}");
    assert!(project(&h.command_json(r#"{"type":"get_project"}"#)).nodes.iter().all(|n| n.name != "Retour pris"));
    // Une piste n'a pas de sortie physique.
    let ev = h.command_json(r#"{"type":"add_track","kind":"mono","output":{"first":7,"stereo":false,"mono":true}}"#);
    assert_eq!(errors(&ev).len(), 1, "{ev:?}");
}

#[test]
fn the_type_of_a_return_is_saved_and_a_silent_return_stays_silent() {
    let dir = std::env::temp_dir().join(format!("daw-returns-type-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = rig();
    h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour muet"}"#);
    h.command_json(r#"{"type":"add_track","kind":"bus","name":"Bus nu"}"#);
    let path = dir.join("Types");
    assert!(errors(&h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, path.to_string_lossy()))).is_empty());
    let mut h2 = rig();
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, path.with_extension("daw").to_string_lossy()));
    assert!(node(&ev, "Retour muet").is_return && node(&ev, "Retour muet").phys.is_none());
    assert!(!node(&ev, "Bus nu").is_return);
    let _ = std::fs::remove_dir_all(&dir);
}
