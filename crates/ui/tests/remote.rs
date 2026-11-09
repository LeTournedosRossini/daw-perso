//! Télécommande d'une tablette de musicien : le rôle est un droit du jeton (403 sans lui), les commandes passent par l'hôte
//! (transport, section, pré-roll, métronome, enregistrement refusé avec son message), le rôle est enregistré avec le projet.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::Engine;
use daw_ui::host::{PluginRef, PluginResolver};
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
    let ev = rig.h.command_json(&format!(r#"{{"type":"set_monitor_role","node":{},"role":8}}"#, rig.bus));
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
    let ev = rig.h.command_json(&format!(r#"{{"type":"set_monitor_role","node":{},"role":8}}"#, rig.bus));
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

fn journal(rig: &mut Rig) -> Vec<(String, u16, String)> {
    let ev = rig.h.command_json(r#"{"type":"get_monitor_log"}"#);
    ev.iter()
        .find_map(|e| if let Event::MonitorLog { entries } = e { Some(entries.iter().map(|x| (x.route.clone(), x.status, x.what.clone())).collect()) } else { None })
        .expect("événement monitor_log")
}

#[test]
fn commands_are_rate_limited_per_address_and_everything_is_journaled() {
    let mut rig = Rig::new();
    rig.set_role(3);
    // Rafale de commandes de transport : le seau en laisse passer quelques-unes puis répond 429 ; la régie a sa propre limite.
    let codes: Vec<u16> = (0..20).map(|_| rig.cmd(r#"{"cmd":"click","on":true}"#)).collect();
    assert!(codes.iter().filter(|c| **c == 200).count() >= 8, "la rafale initiale passe : {codes:?}");
    assert!(codes.contains(&429), "puis la limite répond 429 : {codes:?}");
    assert_eq!(rig.mix(r#"{"node":1,"db":-3}"#), 200, "la régie a son propre seau");
    // Une commande invalide et une adresse inconnue entrent aussi au journal ; le corps brut n'y est jamais.
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(rig.cmd(r#"{"cmd":"explode"}"#), 400);
    assert_eq!(http(rig.port, "POST", "/r/0000/cmd", "{}").0, 404);
    let log = journal(&mut rig);
    assert!(log.iter().any(|(r, s, w)| r == "cmd" && *s == 200 && w.contains("Click")), "{log:?}");
    assert!(log.iter().any(|(r, s, _)| r == "cmd" && *s == 429));
    assert!(log.iter().any(|(r, s, w)| r == "mix" && *s == 200 && w.contains("node: 1")));
    assert!(log.iter().any(|(r, s, w)| r == "cmd" && *s == 400 && w == "corps invalide"));
    assert!(log.iter().any(|(r, s, _)| r == "?" && *s == 404));
    assert!(!log.iter().any(|(_, _, w)| w.contains("explode")), "pas de corps brut dans le journal");
}

#[test]
fn a_flood_on_unknown_addresses_is_blocked_but_a_valid_address_still_works() {
    let mut rig = Rig::new();
    rig.set_role(1);
    let mut blocked = false;
    for _ in 0..320 {
        if http(rig.port, "GET", "/r/deadbeef/state", "").0 == 429 {
            blocked = true;
            break;
        }
    }
    assert!(blocked, "au-delà de 300 demandes par minute à une adresse inconnue, le client est bloqué");
    assert_eq!(rig.state()["role"], 1, "l'adresse valide répond toujours");
    assert_eq!(rig.cmd(r#"{"cmd":"click","on":false}"#), 200);
}

#[test]
fn elevated_rights_expire_after_the_configured_hours_and_can_be_renewed() {
    let mut rig = Rig::new();
    rig.set_role(3);
    assert_eq!(rig.cmd(r#"{"cmd":"click","on":true}"#), 200);
    assert_eq!(rig.mix(r#"{"node":1,"db":-1}"#), 200);
    // 11 h plus tard : toujours valides ; 13 h plus tard (réglage par défaut : 12 h) : expirés.
    rig.h.age_monitor_grants(Duration::from_secs(11 * 3600));
    rig.settle();
    assert_eq!(rig.state()["role"], 3);
    rig.h.age_monitor_grants(Duration::from_secs(2 * 3600));
    rig.settle();
    let st = rig.state();
    assert_eq!(st["role"], 0, "les droits ont expiré");
    assert_eq!(st["expired"], true);
    assert!(st.get("remote").is_none() && st.get("mix").is_none());
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(rig.cmd(r#"{"cmd":"toggle"}"#), 403);
    assert_eq!(rig.mix(r#"{"node":1,"db":-2}"#), 403);
    // Le retour de la tablette reste réglable.
    assert_eq!(http(rig.port, "POST", &format!("{}/set", rig.path), r#"{"node":1,"db":-6}"#).0, 200);
    // Redonner les mêmes droits les renouvelle.
    rig.set_role(3);
    let st = rig.state();
    assert_eq!(st["role"], 3);
    assert!(st.get("expired").is_none());
    // Une heure à 0 : jamais d'expiration.
    let ev = rig.h.command_json(r#"{"type":"set_prefs","prefs":{"remote":{"elevated_hours":0}}}"#);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    rig.h.age_monitor_grants(Duration::from_secs(1000 * 3600));
    rig.settle();
    assert_eq!(rig.state()["role"], 3);
    let ev = rig.h.command_json(r#"{"type":"set_prefs","prefs":{"remote":{"elevated_hours":200}}}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })), "200 h refusé");
}

#[test]
fn the_mixing_desk_shows_the_project_groups_and_sets_mute_solo_and_a_level_that_stays_in_place() {
    let mut rig = Rig::new();
    let p = rig.h.command_json(r#"{"type":"get_project"}"#);
    let (drums, bass, guitar) = (node(&p, "Batterie").id, node(&p, "Basse").id, node(&p, "Guitare").id);
    let ev = rig.h.command_json(&format!(r##"{{"type":"create_group","name":"Rythmique","color":"#ff8800","nodes":[{drums},{bass}]}}"##));
    let gid = project(&ev).groups[0].id;
    // Sans le droit de régie : refusé (403) et l'état ne montre rien.
    assert_eq!(rig.mix(&format!(r#"{{"group":{gid},"mute":true}}"#)), 403);
    rig.set_role(2);
    let st = rig.state();
    let g = &st["mix"]["groups"][0];
    assert_eq!((g["id"].as_u64(), g["name"].as_str(), g["count"].as_u64(), g["mute"].as_bool(), g["offset_db"].as_f64()), (Some(u64::from(gid)), Some("Rythmique"), Some(2), Some(false), Some(0.0)), "{g}");
    assert_eq!(mix_node(&st, "Batterie")["group"].as_u64(), Some(u64::from(gid)));
    assert_eq!(mix_node(&st, "Guitare")["group"].as_u64(), Some(0));
    let db = |st: &serde_json::Value, n: &str| mix_node(st, n)["db"].as_f64().unwrap();
    let (d0, b0, g0) = (db(&st, "Batterie"), db(&st, "Basse"), db(&st, "Guitare"));

    // Muet et solo du groupe : tous les membres, et eux seuls.
    assert_eq!(rig.mix(&format!(r#"{{"group":{gid},"mute":true,"solo":true}}"#)), 200);
    rig.settle();
    let st = rig.state();
    assert_eq!(st["mix"]["groups"][0]["mute"], true);
    assert_eq!(st["mix"]["groups"][0]["solo"], true);
    assert_eq!((mix_node(&st, "Batterie")["mute"].as_bool(), mix_node(&st, "Basse")["mute"].as_bool(), mix_node(&st, "Guitare")["mute"].as_bool()), (Some(true), Some(true), Some(false)));
    assert_eq!(rig.mix(&format!(r#"{{"group":{gid},"mute":false,"solo":false}}"#)), 200);

    // Niveau : décalage absolu qui reste en place, écarts conservés, borné à ±12 dB ; revenir à 0 rend les niveaux d'origine.
    assert_eq!(rig.mix(&format!(r#"{{"group":{gid},"offset_db":-6}}"#)), 200);
    rig.settle();
    let st = rig.state();
    assert!((db(&st, "Batterie") - (d0 - 6.0)).abs() < 0.25 && (db(&st, "Basse") - (b0 - 6.0)).abs() < 0.25, "{st}");
    assert!((db(&st, "Guitare") - g0).abs() < 0.05, "les non-membres ne bougent pas");
    assert_eq!(st["mix"]["groups"][0]["offset_db"].as_f64(), Some(-6.0));
    assert_eq!(rig.mix(&format!(r#"{{"group":{gid},"offset_db":-6}}"#)), 200); // la même valeur ne s'applique pas deux fois
    rig.settle();
    assert!((db(&rig.state(), "Batterie") - (d0 - 6.0)).abs() < 0.25);
    assert_eq!(rig.mix(&format!(r#"{{"group":{gid},"offset_db":0}}"#)), 200);
    rig.settle();
    assert!((db(&rig.state(), "Batterie") - d0).abs() < 0.25, "retour à 0 = niveau d'origine");
    assert_eq!(rig.mix(&format!(r#"{{"group":{gid},"offset_db":99}}"#)), 200);
    rig.settle();
    assert_eq!(rig.state()["mix"]["groups"][0]["offset_db"].as_f64(), Some(12.0));

    // Invalides : 400 ; groupe inconnu : accepté par le serveur, refusé par l'hôte avec un message pour la tablette.
    for bad in [format!(r#"{{"group":{gid}}}"#), format!(r#"{{"group":{gid},"mute":"oui"}}"#), format!(r#"{{"group":{gid},"offset_db":"fort"}}"#)] {
        assert_eq!(rig.mix(&bad), 400, "{bad}");
    }
    assert_eq!(rig.mix(r#"{"group":999,"mute":true}"#), 200);
    rig.settle();
    assert_eq!(rig.state()["mix"]["groups"][0]["offset_db"].as_f64(), Some(12.0), "un groupe inconnu ne change rien");
}

fn plugin_resolver() -> PluginResolver {
    let bundle = std::sync::Arc::new(Bundle::from_static_entry(daw_test_plugin::entry()).unwrap());
    let id = daw_test_plugin::PLUGIN_ID.to_str().unwrap().to_string();
    Box::new(move |e| (e.clap_id == id).then(|| (clap_insert_factory(std::sync::Arc::clone(&bundle), e.clap_id.clone()), PluginRef { clap_id: id.clone(), vendor: "daw-perso".into(), name: "Gain de test".into(), file: "test.clap".into() })))
}

impl Rig {
    fn fx(&self, body: &str) -> u16 {
        http(self.port, "POST", &format!("{}/fx", self.path), body).0
    }
}

#[test]
fn fine_settings_are_a_separate_right_limited_to_the_chosen_parameters_of_a_plugin() {
    let mut rig = Rig::new();
    rig.h.set_plugin_resolver(plugin_resolver());
    let id = daw_test_plugin::PLUGIN_ID.to_str().unwrap();
    let ev = rig.h.command_json(&format!(r#"{{"type":"add_plugin","node":1,"clap_id":"{id}","file":"test.clap"}}"#));
    let ins = project(&ev).nodes.iter().find(|n| n.name == "Batterie").unwrap().inserts[0].id;
    rig.h.command_json(&format!(r#"{{"type":"set_control_page","plugin_id":"{id}","name":"Gain de test","params":[{{"id":{},"name":"Gain"}}]}}"#, daw_test_plugin::PARAM_GAIN));

    // Sans le droit : refusé (403), et l'état ne montre ni chaîne de plugins ni page.
    rig.set_role(2);
    assert_eq!(rig.fx(&format!(r#"{{"open":{ins}}}"#)), 403);
    let st = rig.state();
    assert!(st.get("fine").is_none());
    assert_eq!(mix_node(&st, "Batterie")["inserts"].as_array().unwrap().len(), 0, "pas de chaîne sans les réglages fins");

    // Le droit 4 seul : la tablette voit les pistes (lecture) mais ne règle rien dans la régie.
    rig.set_role(4);
    assert_eq!(rig.mix(r#"{"node":1,"db":-3}"#), 403, "le réglage de niveau de la régie reste un autre droit");
    let st = rig.state();
    let chain = mix_node(&st, "Batterie")["inserts"].clone();
    assert_eq!((chain[0]["id"].as_u64(), chain[0]["page"].as_bool(), chain[0]["bypass"].as_bool()), (Some(u64::from(ins)), Some(true), Some(false)));
    assert!(st["fine"]["page"].is_null(), "aucune page ouverte");

    // Ouvrir la page : ses paramètres, valeur et texte du plugin.
    assert_eq!(rig.fx(&format!(r#"{{"open":{ins}}}"#)), 200);
    rig.settle();
    let page = rig.state()["fine"]["page"].clone();
    assert_eq!((page["insert"].as_u64(), page["name"].as_str(), page["track"].as_str()), (Some(u64::from(ins)), Some("Gain de test"), Some("Batterie")));
    let gain = &page["params"][0];
    assert_eq!(gain["name"], "Gain");
    assert!(!gain["text"].as_str().unwrap().is_empty());

    // Régler un paramètre de la page : appliqué, relu, borné à sa plage. Un paramètre hors de la page est refusé avec un message.
    let gid = daw_test_plugin::PARAM_GAIN;
    assert_eq!(rig.fx(&format!(r#"{{"insert":{ins},"param":{gid},"value":0.25}}"#)), 200);
    rig.settle();
    assert!((rig.state()["fine"]["page"]["params"][0]["value"].as_f64().unwrap() - 0.25).abs() < 1e-9);
    assert_eq!(rig.fx(&format!(r#"{{"insert":{ins},"param":{gid},"value":1e9}}"#)), 200);
    rig.settle();
    let max = rig.state()["fine"]["page"]["params"][0]["max"].as_f64().unwrap();
    assert!((rig.state()["fine"]["page"]["params"][0]["value"].as_f64().unwrap() - max).abs() < 1e-9, "borné au maximum du paramètre");
    let other = daw_test_plugin::PARAM_EXTRA_LATENCY;
    assert_eq!(rig.fx(&format!(r#"{{"insert":{ins},"param":{other},"value":10}}"#)), 200);
    rig.settle();
    let after = rig.h.command_json(&format!(r#"{{"type":"get_plugin_params","insert":{ins}}}"#));
    let v = after.iter().find_map(|e| if let Event::PluginParams { params, .. } = e { params.iter().find(|p| p.id == other).map(|p| p.value) } else { None }).unwrap();
    assert!(v.abs() < 1e-9, "le paramètre hors de la page n'a pas bougé ({v})");

    // Toucher / lâcher et contournement : acceptés ; contourner se retrouve dans la chaîne.
    assert_eq!(rig.fx(&format!(r#"{{"insert":{ins},"param":{gid},"touch":true}}"#)), 200);
    assert_eq!(rig.fx(&format!(r#"{{"insert":{ins},"param":{gid},"touch":false}}"#)), 200);
    assert_eq!(rig.fx(&format!(r#"{{"insert":{ins},"bypass":true}}"#)), 200);
    rig.settle();
    assert_eq!(mix_node(&rig.state(), "Batterie")["inserts"][0]["bypass"], true);
    // Fermer la page.
    assert_eq!(rig.fx(r#"{"open":null}"#), 200);
    rig.settle();
    assert!(rig.state()["fine"]["page"].is_null());
    // Invalides : 400 ; plugin inconnu : accepté par le serveur, refusé par l'hôte.
    for bad in ["", "{}", r#"{"insert":1}"#, r#"{"insert":1,"param":2}"#, r#"{"insert":1,"param":2,"value":"x"}"#, r#"{"open":"a"}"#] {
        assert_eq!(rig.fx(bad), 400, "{bad:?}");
    }
    assert_eq!(rig.fx(r#"{"open":9999}"#), 200);
    // Un rôle sans le bit 4 (régie + transport) ne règle pas les plugins.
    rig.set_role(3);
    assert_eq!(rig.fx(&format!(r#"{{"insert":{ins},"bypass":false}}"#)), 403);
    // Les droits combinés vont jusqu'à 7 ; 8 est refusé (voir plus haut).
    rig.set_role(7);
    assert_eq!(rig.state()["role"], 7);
}
