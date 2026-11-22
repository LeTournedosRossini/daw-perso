//! Talkback de la régie : le micro (une entrée de la carte) passe dans les retours visés, la parole est maintenue par des « je parle »
//! répétés et coupée sans nouvelle, les réglages refusés ne changent rien.

use std::time::Duration;

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::NodeInfo;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    let p = ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("project");
    p.nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de nœud « {name} »"))
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

fn talk_state(ev: &[Event]) -> (Option<u32>, Vec<u32>, bool) {
    ev.iter().rev().find_map(|e| if let Event::Talkback { channel, targets, active, .. } = e { Some((*channel, targets.clone(), *active)) } else { None }).expect("événement talkback")
}

/// Un hôte avec deux entrées de carte et un retour « Marie » sur les sorties 3-4.
fn rig() -> (Host, Engine, u32) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_audio_outputs(8);
    h.set_audio_inputs(2);
    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Marie","output":{"first":3,"stereo":true,"mono":false}}"#);
    let bus = node(&ev, "Marie").id;
    (h, engine, bus)
}

/// Fait traiter ~`blocks` blocs avec une voix constante sur l'entrée 2 ; rend la dernière valeur de la sortie 3 (gauche du retour).
fn run(engine: &mut Engine, blocks: usize) -> f32 {
    let mic = [0.2f32; 64];
    let silence = [0.0f32; 64];
    let mut outs = [[0.0f32; 64]; 4];
    for _ in 0..blocks {
        let [a, b, c, d] = &mut outs;
        let mut parts: [&mut [f32]; 4] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..]];
        engine.process_outputs(&[&silence, &mic], &mut parts);
    }
    outs[2][63]
}

#[test]
fn the_voice_passes_into_the_chosen_return_while_talking_and_stops_after() {
    let (mut h, mut engine, bus) = rig();
    let ev = h.command_json(&format!(r#"{{"type":"set_talkback","channel":2,"level_db":0,"duck_db":12,"targets":[{bus}]}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(talk_state(&ev), (Some(2), vec![bus], false));
    assert!(run(&mut engine, 20).abs() < 1e-6, "silence tant qu'on ne parle pas");
    let ev = h.command_json(r#"{"type":"talk","active":true}"#);
    assert_eq!(talk_state(&ev).2, true);
    assert!((run(&mut engine, 40) - 0.2).abs() < 1e-4, "la voix passe dans le retour");
    // Redire « je parle » ne change rien et n'émet rien.
    assert!(h.command_json(r#"{"type":"talk","active":true}"#).is_empty());
    let ev = h.command_json(r#"{"type":"talk","active":false}"#);
    assert_eq!(talk_state(&ev).2, false);
    assert!(run(&mut engine, 40).abs() < 1e-6, "la voix s'arrête");
}

#[test]
fn the_speech_is_cut_when_nobody_repeats_it_and_kept_when_repeated() {
    let (mut h, mut engine, bus) = rig();
    h.command_json(&format!(r#"{{"type":"set_talkback","channel":1,"level_db":-6,"duck_db":6,"targets":[{bus}]}}"#));
    h.command_json(r#"{"type":"talk","active":true}"#);
    std::thread::sleep(Duration::from_millis(900));
    h.command_json(r#"{"type":"talk","active":true}"#); // le doigt tient toujours
    std::thread::sleep(Duration::from_millis(900));
    let ev = h.tick();
    assert!(!ev.iter().any(|e| matches!(e, Event::Talkback { .. })), "toujours maintenu : pas d'événement");
    std::thread::sleep(Duration::from_millis(1_700));
    let ev = h.tick();
    assert_eq!(talk_state(&ev).2, false, "sans nouvelle pendant 1,5 s, la parole est coupée");
    let _ = run(&mut engine, 4);
}

#[test]
fn bad_settings_are_refused_without_effect() {
    let (mut h, _engine, bus) = rig();
    // Parler sans micro ni retour visé.
    assert!(!errors(&h.command_json(r#"{"type":"talk","active":true}"#)).is_empty());
    // Une piste n'est pas un retour ; une entrée inexistante ; valeurs hors plage.
    let guitar = node(&h.command_json(r#"{"type":"get_project"}"#), "Guitare").id;
    for body in [
        format!(r#"{{"type":"set_talkback","channel":1,"level_db":0,"duck_db":12,"targets":[{guitar}]}}"#),
        r#"{"type":"set_talkback","channel":9,"level_db":0,"duck_db":12,"targets":[]}"#.to_string(),
        r#"{"type":"set_talkback","channel":0,"level_db":0,"duck_db":12,"targets":[]}"#.to_string(),
        r#"{"type":"set_talkback","channel":1,"level_db":40,"duck_db":12,"targets":[]}"#.to_string(),
        r#"{"type":"set_talkback","channel":1,"level_db":0,"duck_db":99,"targets":[]}"#.to_string(),
        r#"{"type":"set_talkback","channel":1,"level_db":0,"duck_db":12,"targets":[0]}"#.to_string(),
    ] {
        assert!(!errors(&h.command_json(&body)).is_empty(), "{body}");
    }
    // Rien n'a été retenu.
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(talk_state(&ev), (None, vec![], false));
    let _ = bus;
}

#[test]
fn a_structure_change_ends_the_speech_and_forgets_the_targets() {
    let (mut h, _engine, bus) = rig();
    h.command_json(&format!(r#"{{"type":"set_talkback","channel":1,"level_db":0,"duck_db":12,"targets":[{bus}]}}"#));
    h.command_json(r#"{"type":"talk","active":true}"#);
    h.command_json(r#"{"type":"add_track","kind":"stereo","name":"Nouvelle"}"#);
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let (channel, targets, active) = talk_state(&ev);
    assert_eq!((channel, targets, active), (Some(1), vec![], false), "le micro reste, la parole et les cibles non");
}
