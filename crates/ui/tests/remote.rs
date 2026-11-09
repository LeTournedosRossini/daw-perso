//! Télécommande d'une tablette de musicien : le rôle est un droit du jeton (403 sans lui), les commandes passent par l'hôte
//! (transport, section, pré-roll, métronome, enregistrement refusé avec son message), le rôle est enregistré avec le projet.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::NodeInfo;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connexion");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = write!(s, "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    let status = raw.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

fn project(ev: &[Event]) -> &daw_ui::protocol::ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de nœud « {name} »"))
}

struct Rig {
    h: Host,
    engine: Engine,
    port: u16,
    path: String,
    bus: u32,
}

impl Rig {
    fn new() -> Rig {
        let (engine, handle) = Engine::new(SR, 64);
        let (session, meta) = demo_session(SR);
        let mut h = Host::new(handle, session, meta).unwrap();
        h.set_audio_outputs(8);
        let ev = h.command_json(r#"{"type":"add_track","kind":"bus","name":"Retour Marie"}"#);
        let bus = node(&ev, "Retour Marie").id;
        h.command_json(&format!(r#"{{"type":"set_return_output","node":{bus},"output":{{"first":3,"stereo":true,"mono":false}}}}"#));
        h.command_json(r#"{"type":"set_structure","sections":[{"at":[0,1],"dur":[4,1],"name":"Intro"},{"at":[4,1],"dur":[4,1],"name":"Refrain"}]}"#);
        let ev = h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#);
        let (port, url) = ev
            .iter()
            .rev()
            .find_map(|e| if let Event::Monitor { port, returns, .. } = e { returns.first().map(|r| (*port, r.url.clone())) } else { None })
            .expect("événement monitor");
        let path = format!("/{}", url.splitn(4, '/').nth(3).unwrap());
        Rig { h, engine, port, path, bus }
    }

    /// Laisse le serveur répondre, l'hôte appliquer et republier, et le moteur traiter quelques blocs.
    fn settle(&mut self) -> Vec<Event> {
        std::thread::sleep(Duration::from_millis(120));
        let mut ev = self.h.tick();
        let (mut l, mut r) = ([0f32; 64], [0f32; 64]);
        for _ in 0..8 {
            self.engine.process(&mut l, &mut r);
        }
        std::thread::sleep(Duration::from_millis(250));
        ev.extend(self.h.tick());
        ev
    }

    fn cmd(&self, body: &str) -> u16 {
        http(self.port, "POST", &format!("{}/cmd", self.path), body).0
    }

    fn mix(&self, body: &str) -> u16 {
        http(self.port, "POST", &format!("{}/mix", self.path), body).0
    }

    fn set_role(&mut self, role: u8) {
        let ev = self.h.command_json(&format!(r#"{{"type":"set_monitor_role","node":{},"role":{role}}}"#, self.bus));
        assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
        self.settle();
    }

    fn state(&self) -> serde_json::Value {
        let (status, body) = http(self.port, "GET", &format!("{}/state", self.path), "");
        assert_eq!(status, 200);
        serde_json::from_str(&body).unwrap()
    }
}

#[test]
fn a_plain_musician_address_is_refused_every_remote_command() {
    let mut rig = Rig::new();
    rig.settle();
    for body in [r#"{"cmd":"toggle"}"#, r#"{"cmd":"record"}"#, r#"{"cmd":"stop"}"#, r#"{"cmd":"section","index":0}"#] {
        assert_eq!(rig.cmd(body), 403, "{body}");
    }
    let st = rig.state();
    assert_eq!(st["role"], 0);
    assert!(st.get("remote").is_none(), "pas de télécommande dans l'état d'un simple musicien");
    let ev = rig.settle();
    assert!(!ev.iter().any(|e| matches!(e, Event::Position { playing: true, .. })), "rien n'a démarré");
}

#[test]
fn a_transport_address_drives_playback_sections_preroll_and_click() {
    let mut rig = Rig::new();
    let ev = rig.h.command_json(&format!(r#"{{"type":"set_monitor_role","node":{},"role":1}}"#, rig.bus));
    assert!(ev.iter().any(|e| matches!(e, Event::Monitor { returns, .. } if returns.iter().any(|r| r.role == 1))));
    rig.settle();
    let st = rig.state();
    assert_eq!(st["role"], 1);
    assert_eq!(st["remote"]["playing"], false);
    let secs = st["remote"]["sections"].as_array().unwrap();
    assert_eq!((secs[0]["name"].as_str(), secs[0]["bar"].as_u64()), (Some("Intro"), Some(1)));
    assert_eq!((secs[1]["name"].as_str(), secs[1]["bar"].as_u64()), (Some("Refrain"), Some(5)), "4 rondes à 4/4 = la mesure 5");

    // Lecture puis arrêt.
    assert_eq!(rig.cmd(r#"{"cmd":"toggle"}"#), 200);
    rig.settle();
    assert_eq!(rig.state()["remote"]["playing"], true);
    assert_eq!(rig.cmd(r#"{"cmd":"stop"}"#), 200);
    rig.settle();
    assert_eq!(rig.state()["remote"]["playing"], false);

    // Aller au Refrain : 4 rondes à 120 bpm = 8 s ; le moteur est déplacé à cet échantillon.
    assert_eq!(rig.cmd(r#"{"cmd":"section","index":1}"#), 200);
    let ev = rig.settle();
    assert!(ev.iter().any(|e| matches!(e, Event::Position { pos, .. } if *pos == 8 * SR as u64)), "tête sur le Refrain : {:?}", ev.iter().filter(|e| matches!(e, Event::Position { .. })).count());
    assert_eq!(rig.state()["remote"]["bar"], 5);
    assert_eq!(rig.cmd(r#"{"cmd":"start"}"#), 200);
    rig.settle();
    assert_eq!(rig.state()["remote"]["bar"], 1);

    // Pré-roll en mesures et métronome.
    assert_eq!(rig.cmd(r#"{"cmd":"preroll","bars":2}"#), 200);
    assert_eq!(rig.cmd(r#"{"cmd":"click","on":true}"#), 200);
    assert_eq!(rig.cmd(r#"{"cmd":"playpreroll","on":true}"#), 200);
    rig.settle();
    let st = rig.state();
    assert_eq!(st["remote"]["pre_roll_bars"], 2);
    assert_eq!(st["remote"]["click"], true);
    assert_eq!(st["remote"]["play_pre_roll"], true);
}

#[test]
fn bad_remote_requests_and_refusals_are_reported_without_effect() {
    let mut rig = Rig::new();
    rig.h.command_json(&format!(r#"{{"type":"set_monitor_role","node":{},"role":1}}"#, rig.bus));
    rig.settle();
    for bad in [r#"{"cmd":"explode"}"#, r#"{"cmd":"preroll","bars":9}"#, r#"{"cmd":"section"}"#, r#"{"cmd":"click"}"#, "pas du json", ""] {
        assert_eq!(rig.cmd(bad), 400, "{bad:?}");
    }
    // Une section qui n'existe pas : acceptée par le serveur, refusée par l'hôte avec un message pour la tablette.
    assert_eq!(rig.cmd(r#"{"cmd":"section","index":99}"#), 200);
    rig.settle();
    assert_eq!(rig.state()["remote"]["msg"], "section inconnue");
    // Enregistrer sans piste armée : l'hôte refuse, la tablette lit pourquoi, rien ne s'enregistre.
    assert_eq!(rig.cmd(r#"{"cmd":"record"}"#), 200);
    rig.settle();
    let st = rig.state();
    assert_eq!(st["remote"]["recording"], false);
    assert!(st["remote"]["msg"].as_str().is_some_and(|m| !m.is_empty()), "un message de refus : {st}");
}

#[test]
fn the_role_is_a_right_of_the_address_saved_with_the_project_and_revocable() {
    let mut rig = Rig::new();
    // Rôle inconnu et retour inexistant : refusés.
    let ev = rig.h.command_json(&format!(r#"{{"type":"set_monitor_role","node":{},"role":7}}"#, rig.bus));
    assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })));
    let ev = rig.h.command_json(r#"{"type":"set_monitor_role","node":1,"role":1}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })), "une piste n'est pas un retour");

    rig.h.command_json(&format!(r#"{{"type":"set_monitor_role","node":{},"role":1}}"#, rig.bus));
    rig.settle();
    assert_eq!(rig.cmd(r#"{"cmd":"toggle"}"#), 200);

    // Enregistré avec le projet, relu.
    let dir = std::env::temp_dir().join(format!("daw-remote-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("r.daw");
    let p = serde_json::to_string(&path.to_string_lossy()).unwrap();
    rig.h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#));
    let json = std::fs::read_to_string(path.join("projet.json")).unwrap();
    assert!(json.contains("monitor_role"), "le rôle est écrit dans le projet");
    let (engine2, handle2) = Engine::new(SR, 64);
    let (session2, meta2) = demo_session(SR);
    let mut h2 = Host::new(handle2, session2, meta2).unwrap();
    h2.set_audio_outputs(8);
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    let ev = h2.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Monitor { returns, .. } if returns.iter().any(|r| r.role == 1))), "rôle relu à l'ouverture");
    drop(engine2);
    let _ = std::fs::remove_dir_all(&dir);

    // Révoqué : les commandes sont de nouveau refusées.
    rig.h.command_json(&format!(r#"{{"type":"set_monitor_role","node":{},"role":0}}"#, rig.bus));
    rig.settle();
    assert_eq!(rig.cmd(r#"{"cmd":"toggle"}"#), 403);
    assert!(rig.state().get("remote").is_none());
}

fn mix_node(st: &serde_json::Value, name: &str) -> serde_json::Value {
    st["mix"]["nodes"].as_array().unwrap().iter().find(|n| n["name"] == name).unwrap_or_else(|| panic!("pas de « {name} » dans la régie : {st}")).clone()
}

#[test]
fn rights_combine_transport_and_mixing_are_separate_bits_and_both_can_be_given() {
    let mut rig = Rig::new();
    // Transport seulement : la régie est refusée et absente de l'état.
    rig.set_role(1);
    assert_eq!(rig.mix(r#"{"node":1,"db":-6}"#), 403);
    assert!(rig.state().get("mix").is_none());
    // Régie seulement : la télécommande est refusée, la régie marche.
    rig.set_role(2);
    assert_eq!(rig.cmd(r#"{"cmd":"toggle"}"#), 403);
    assert!(rig.state().get("remote").is_none());
    assert_eq!(rig.mix(r#"{"node":1,"db":-6}"#), 200);
    // Les deux : musicien + transport + régie avec la même adresse.
    rig.set_role(3);
    assert_eq!(rig.cmd(r#"{"cmd":"toggle"}"#), 200);
    assert_eq!(rig.mix(r#"{"node":1,"db":-6}"#), 200);
    let st = rig.state();
    assert_eq!(st["role"], 3);
    assert!(st.get("remote").is_some() && st.get("mix").is_some() && st["sources"].is_array(), "son retour, le transport et la régie ensemble");
    // Au-delà des droits connus : refusé.
    let ev = rig.h.command_json(&format!(r#"{{"type":"set_monitor_role","node":{},"role":4}}"#, rig.bus));
    assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })));
    // Une adresse de simple musicien n'a ni l'un ni l'autre.
    rig.set_role(0);
    assert_eq!((rig.cmd(r#"{"cmd":"toggle"}"#), rig.mix(r#"{"node":1,"db":-6}"#)), (403, 403));
}

#[test]
fn the_mixing_desk_sets_level_pan_mute_solo_and_arming_of_any_track_with_the_usual_refusals() {
    let mut rig = Rig::new();
    rig.set_role(2);
    let st = rig.state();
    let names: Vec<String> = st["mix"]["nodes"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap().to_string()).collect();
    for want in ["Batterie", "Guitare", "Retour Marie", "Master"] {
        assert!(names.iter().any(|n| n == want), "{want} dans {names:?}");
    }
    assert_eq!(mix_node(&st, "Retour Marie")["kind"], "return");
    assert_eq!(mix_node(&st, "Master")["kind"], "master");
    assert_eq!(mix_node(&st, "Guitare")["kind"], "track");
    let guitar = mix_node(&st, "Guitare")["id"].as_u64().unwrap();

    // Niveau, panoramique, muet, solo : appliqués par l'hôte, relus dans l'état.
    assert_eq!(rig.mix(&format!(r#"{{"node":{guitar},"db":-6,"pan":-0.5,"mute":true}}"#)), 200);
    assert_eq!(rig.mix(&format!(r#"{{"node":{guitar},"solo":true}}"#)), 200);
    rig.settle();
    let g = mix_node(&rig.state(), "Guitare");
    assert!((g["db"].as_f64().unwrap() + 6.0).abs() < 0.2, "{g}");
    assert!((g["pan"].as_f64().unwrap() + 0.5).abs() < 1e-3, "{g}");
    assert_eq!((g["mute"].as_bool(), g["solo"].as_bool()), (Some(true), Some(true)));
    // Borné : +40 dB devient +6 dB, −90 dB coupe, un panoramique hors de ±1 est ramené.
    assert_eq!(rig.mix(&format!(r#"{{"node":{guitar},"db":40,"pan":9}}"#)), 200);
    rig.settle();
    let g = mix_node(&rig.state(), "Guitare");
    assert!((g["db"].as_f64().unwrap() - 6.0).abs() < 0.2 && g["pan"].as_f64() == Some(1.0), "{g}");
    // Armement d'une piste : accepté (sans entrée choisie il reste possible). Pour un bus ou la sortie principale : refusé avec un message.
    assert_eq!(rig.mix(&format!(r#"{{"node":{guitar},"arm":true}}"#)), 200);
    rig.settle();
    assert_eq!(mix_node(&rig.state(), "Guitare")["arm"], true);
    let ret = mix_node(&rig.state(), "Retour Marie")["id"].as_u64().unwrap();
    assert_eq!(rig.mix(&format!(r#"{{"node":{ret},"arm":true}}"#)), 200);
    rig.settle();
    assert_eq!(mix_node(&rig.state(), "Retour Marie")["arm"], false, "un retour ne s'arme pas");
    assert_eq!(rig.mix(r#"{"node":0,"pan":0.3}"#), 200);
    rig.settle();
    assert_eq!(mix_node(&rig.state(), "Master")["pan"].as_f64(), Some(0.0), "la sortie principale n'a pas de panoramique réglable");
    // Le niveau de la sortie principale se règle ; son panoramique non.
    assert_eq!(rig.mix(r#"{"node":0,"db":-3}"#), 200);
    rig.settle();
    assert!((mix_node(&rig.state(), "Master")["db"].as_f64().unwrap() + 3.0).abs() < 0.2);
    // Demandes invalides : 400 sans effet.
    for bad in ["", "pas du json", "{}", r#"{"node":1}"#, r#"{"db":-3}"#, r#"{"node":1,"db":"fort"}"#, r#"{"node":1,"mute":"oui"}"#, r#"{"node":-1,"db":0}"#] {
        assert_eq!(rig.mix(bad), 400, "{bad:?}");
    }
    // Un nœud inconnu : accepté par le serveur, refusé par l'hôte avec un message pour la tablette.
    assert_eq!(rig.mix(r#"{"node":999,"db":0}"#), 200);
    rig.settle();
    assert_eq!(rig.state()["mix"]["nodes"].as_array().unwrap().len(), names.len(), "rien n'a changé");
}
