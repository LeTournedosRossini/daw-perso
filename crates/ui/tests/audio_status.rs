//! Le pilote audio qui tourne (nom, fréquence, tampon, latences annoncées) est donné au bandeau par l'événement `project`.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::ProjectInfo;
use daw_ui::{Event, Host};

fn host() -> (Engine, Host) {
    let (engine, handle) = Engine::new(44_100, 64);
    let (session, meta) = demo_session(44_100);
    (engine, Host::new(handle, session, meta).unwrap())
}

fn project(ev: &[Event]) -> Option<&ProjectInfo> {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None })
}

#[test]
fn without_a_driver_there_is_no_audio_status() {
    let (_e, mut h) = host();
    let p = h.command_json(r#"{"type":"get_project"}"#);
    assert!(project(&p).unwrap().audio.is_none());
}

#[test]
fn the_driver_name_rate_and_buffer_are_reported_then_its_latencies_arrive_in_a_project_event() {
    let (_e, mut h) = host();
    h.set_audio_status("UMC".into(), 44_100, 64);
    let p = h.command_json(r#"{"type":"get_project"}"#);
    let a = project(&p).unwrap().audio.clone().expect("statut du pilote");
    assert_eq!((a.driver.as_str(), a.sample_rate, a.buffer, a.in_latency, a.out_latency), ("UMC", 44_100, 64, None, None), "latences pas encore connues");
    // Un tick annonce le changement une seule fois (le pilote n'a pas encore rendu ses latences : c'est le nom qui arrive).
    let first = h.tick();
    assert!(project(&first).is_some(), "le nom du pilote est envoyé");
    assert!(project(&h.tick()).is_none(), "puis plus rien tant que rien ne change");
    // Les latences arrivent plus tard (relues après le démarrage du flux) : un seul événement project.
    h.set_driver_latency(130, 66);
    let ev = h.tick();
    let a = project(&ev).expect("événement project").audio.clone().unwrap();
    assert_eq!((a.in_latency, a.out_latency), (Some(130), Some(66)));
    assert!(project(&h.tick()).is_none());
    let p = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(project(&p).unwrap().audio.as_ref().unwrap().out_latency, Some(66), "relu par get_project");
}
