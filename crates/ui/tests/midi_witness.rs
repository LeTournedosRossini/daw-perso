//! Témoin d'entrée MIDI : une piste qui reçoit un message en direct s'allume (par type), puis s'éteint après la retenue.

use std::collections::BTreeMap;
use std::time::Duration;

use daw_engine::{Engine, MidiMsg};
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn velocity(events: &[Event]) -> Option<(u32, u8)> {
    events.iter().find_map(|e| if let Event::NoteVelocity { node, input } = e { Some((*node, *input)) } else { None })
}

fn activity(events: &[Event]) -> Option<BTreeMap<String, u8>> {
    events.iter().find_map(|e| if let Event::MidiActivity { active } = e { Some(active.clone()) } else { None })
}

#[test]
fn a_track_lights_up_by_message_type_then_goes_dark() {
    let (mut engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut host = Host::new(handle, session, meta).unwrap();
    host.command_json(r#"{"type":"new_project"}"#);
    host.command_json(r#"{"type":"set_track_input","node":1,"input":{"kind":"midi"}}"#);
    host.command_json(r#"{"type":"set_node","node":1,"arm":true}"#);
    let mut midi = host.take_midi_input().expect("entrée MIDI");
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let mut run = |engine: &mut Engine| engine.process(&mut l, &mut r);

    run(&mut engine);
    assert_eq!(activity(&host.tick()), None, "rien reçu : rien à signaler");

    midi.push(1, MidiMsg::NoteOn { channel: 0, key: 60, velocity: 0.8 }, 0);
    midi.push(1, MidiMsg::PitchBend { channel: 0, value: 100 }, 0);
    run(&mut engine);
    let ev = host.tick();
    assert_eq!(velocity(&ev), Some((1, 102)), "vélocité brute 0,8 → 102");
    let on = activity(&ev).expect("allumée");
    assert_eq!(on.get("1"), Some(&(1 | 4)), "notes et molette : {on:?}");
    assert_eq!(activity(&host.tick()), None, "inchangé : pas de nouvel événement");

    midi.push(1, MidiMsg::ControlChange { channel: 0, controller: 1, value: 64 }, 0);
    run(&mut engine);
    let more = activity(&host.tick()).expect("le contrôleur s'ajoute");
    assert_eq!(more.get("1"), Some(&(1 | 2 | 4)));

    std::thread::sleep(Duration::from_millis(200));
    run(&mut engine);
    assert_eq!(activity(&host.tick()), Some(BTreeMap::new()), "éteinte après la retenue");
}
