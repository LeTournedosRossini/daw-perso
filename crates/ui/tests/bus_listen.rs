//! Écoute de contrôle d'un bus ou d'un retour par le protocole : la sortie principale n'entend que lui, les sorties physiques ne changent pas.

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

/// Fait tourner le moteur et rend la dernière tranche de 64 échantillons des quatre canaux (principale G/D, retour G/D).
fn play(engine: &mut Engine, blocks: usize) -> [[f32; 64]; 4] {
    let mut bufs = [[0.0f32; 64]; 4];
    for _ in 0..blocks {
        let [a, b, c, d] = &mut bufs;
        let mut parts: [&mut [f32]; 4] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..]];
        engine.process_outputs(&[], &mut parts);
    }
    bufs
}

fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0, |m, v| m.max(v.abs()))
}

#[test]
fn listening_to_a_return_by_the_protocol_puts_it_in_the_main_output_and_leaves_the_physical_outputs() {
    let (mut engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut host = Host::new(handle, session, meta).unwrap();
    host.set_audio_outputs(8);
    let ev = host.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Marie"}"#);
    let (bus, guitar) = (node(&ev, "Retour Marie").id, node(&ev, "Guitare").id);
    host.command_json(&format!(r#"{{"type":"add_send","node":{guitar},"to":{bus}}}"#));
    host.command_json(&format!(r#"{{"type":"set_send","node":{guitar},"to":{bus},"gain":1.0,"pre":true}}"#));
    host.command_json(&format!(r#"{{"type":"set_return_output","node":{bus},"output":{{"first":3,"stereo":true,"mono":false}}}}"#));
    host.command_json(r#"{"type":"seek","pos":240000}"#);
    host.command_json(r#"{"type":"transport","action":"play"}"#);

    let normal = play(&mut engine, 300);
    assert!(peak(&normal[0]) > 0.0 && peak(&normal[2]) > 0.0, "avant l'écoute : le morceau joue et le retour aussi");
    let master_gain = project(&host.command_json(r#"{"type":"get_project"}"#)).nodes[0].gain;
    assert!(project(&host.command_json(r#"{"type":"get_project"}"#)).listen.is_none());

    let ev = host.command_json(&format!(r#"{{"type":"set_listen","node":{bus}}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(project(&ev).listen, Some(bus));
    let heard = play(&mut engine, 300);
    // La sortie principale n'a plus que le retour (guitare), au niveau du fader de la sortie principale.
    let (main_l, ret_l) = (peak(&heard[0]), peak(&heard[2]));
    assert!(ret_l > 0.0, "le musicien continue d'entendre son retour");
    assert!((main_l - ret_l * master_gain).abs() < 1e-3 + 0.02 * main_l, "principale {main_l} contre retour {ret_l} × {master_gain}");

    // Fin de l'écoute : le morceau revient.
    let ev = host.command_json(r#"{"type":"set_listen","node":null}"#);
    assert_eq!(project(&ev).listen, None);
    let back = play(&mut engine, 300);
    assert!(peak(&back[0]) > main_l * 1.5, "le mixage complet est revenu : {} contre {main_l}", peak(&back[0]));
}

#[test]
fn only_a_bus_or_a_return_can_be_listened_to_and_structure_changes_end_the_listening() {
    let (_engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut host = Host::new(handle, session, meta).unwrap();
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    let guitar = node(&ev, "Guitare").id;
    let ev = host.command_json(&format!(r#"{{"type":"set_listen","node":{guitar}}}"#));
    assert_eq!(errors(&ev).len(), 1, "une piste ne s'écoute pas : {ev:?}");
    assert!(errors(&host.command_json(r#"{"type":"set_listen","node":0}"#)).len() == 1, "la sortie principale non plus");
    assert!(errors(&host.command_json(r#"{"type":"set_listen","node":999}"#)).len() == 1);

    let ev = host.command_json(r#"{"type":"add_track","kind":"bus","name":"Sous-groupe"}"#);
    let bus = node(&ev, "Sous-groupe").id;
    let ev = host.command_json(&format!(r#"{{"type":"set_listen","node":{bus}}}"#));
    assert_eq!(project(&ev).listen, Some(bus));
    // Ajouter un nœud décale les numéros : l'écoute est défaite plutôt que de viser un autre bus.
    let ev = host.command_json(r#"{"type":"add_track","kind":"bus","name":"Autre"}"#);
    assert_eq!(project(&ev).listen, None);
    // « Nouveau » la défait aussi.
    let ev = host.command_json(r#"{"type":"add_track","kind":"bus","name":"Encore"}"#);
    let bus = node(&ev, "Encore").id;
    host.command_json(&format!(r#"{{"type":"set_listen","node":{bus}}}"#));
    let ev = host.command_json(r#"{"type":"new_project"}"#);
    assert_eq!(project(&ev).listen, None);
}
