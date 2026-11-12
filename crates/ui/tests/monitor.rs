//! Serveur web local des retours de monitoring : adresse secrète par retour, mix lu et réglé par HTTP, refus de tout le reste.
//! Le serveur écoute `127.0.0.1` (option `local_only`) ; les requêtes sont de vrais échanges HTTP sur une prise TCP.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::NodeInfo;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn try_http_once(port: u16, method: &str, path: &str, body: &str) -> Option<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    // Le serveur peut refuser et fermer avant la fin d'un très gros corps : une écriture coupée n'est pas une erreur du test.
    let _ = write!(s, "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    // Lecture par morceaux : si la connexion est coupée (RST sous Windows) on garde ce qui est déjà arrivé.
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
        }
    }
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let status = raw.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Some((status, body))
}

/// Un GET sans réponse (connexion coupée avant tout octet) est rejoué : il est sans effet, et les exécuteurs de CI lents le provoquent.
fn try_http(port: u16, method: &str, path: &str, body: &str) -> Option<(u16, String)> {
    let mut r = try_http_once(port, method, path, body);
    if method == "GET" {
        for _ in 0..3 {
            if matches!(r, Some((0, _))) {
                std::thread::sleep(Duration::from_millis(200));
                r = try_http_once(port, method, path, body);
            }
        }
    }
    r
}

fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    try_http(port, method, path, body).unwrap_or_else(|| panic!("connexion impossible : {method} {path}"))
}

fn project(ev: &[Event]) -> &daw_ui::protocol::ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de nœud « {name} »"))
}

fn monitor(ev: &[Event]) -> Option<(bool, u16, Vec<(u32, String, String, String)>, Option<String>)> {
    ev.iter().rev().find_map(|e| if let Event::Monitor { active, port, returns, error } = e { Some((*active, *port, returns.iter().map(|r| (r.node, r.name.clone(), r.url.clone(), r.qr.clone())).collect(), error.clone())) } else { None })
}

fn host() -> Host {
    let (_engine, handle) = Engine::new(SR, 64);
    std::mem::forget(_engine);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_audio_outputs(8);
    h
}

/// Un hôte avec un retour « Retour Marie » (sorties 3-4) ; rend aussi les identifiants du retour, de la guitare et de la batterie.
fn with_return() -> (Host, u32, u32, u32) {
    let mut h = host();
    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Marie"}"#);
    let (bus, guitar, drums) = (node(&ev, "Retour Marie").id, node(&ev, "Guitare").id, node(&ev, "Batterie").id);
    h.command_json(&format!(r#"{{"type":"set_return_output","node":{bus},"output":{{"first":3,"stereo":true,"mono":false}}}}"#));
    (h, bus, guitar, drums)
}

fn path_of(url: &str) -> String {
    format!("/{}", url.splitn(4, '/').nth(3).expect("adresse complète"))
}

#[test]
fn a_tablet_reads_and_sets_its_own_mix_over_http() {
    let (mut h, bus, guitar, drums) = with_return();
    // Le serveur n'existe pas tant qu'on ne le démarre pas.
    assert_eq!(monitor(&h.command_json(r#"{"type":"get_project"}"#)).map(|m| m.0).unwrap_or(false), false);
    let ev = h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#);
    let (active, port, returns, error) = monitor(&ev).expect("événement monitor");
    assert!(active && error.is_none(), "{error:?}");
    assert_eq!(returns.len(), 1);
    let (node_id, name, url, qr) = &returns[0];
    assert_eq!((*node_id, name.as_str()), (bus, "Retour Marie"));
    assert!(qr.contains("<svg"), "un code QR en SVG");
    let path = path_of(url);
    assert!(path.starts_with("/r/") && path.len() == 3 + 32, "jeton de 128 bits : {path}");

    // Page et état : la guitare n'a pas d'envoi (−60), la page ne liste ni le retour ni la sortie principale.
    h.tick();
    let (code, page) = http(port, "GET", &path, "");
    assert_eq!(code, 200);
    assert!(page.contains("<title>Mon retour</title>"));
    let (code, state) = http(port, "GET", &format!("{path}/state"), "");
    assert_eq!(code, 200);
    let st: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert_eq!(st["name"], "Retour Marie");
    let names: Vec<&str> = st["sources"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Guitare") && names.contains(&"Batterie"));
    assert!(!names.contains(&"Retour Marie") && !names.contains(&"Sortie"), "{names:?}");
    assert!(st["sources"].as_array().unwrap().iter().all(|s| s["db"] == -60.0));

    // Réglage : l'envoi est créé, au niveau demandé (−6 dB), après le tick de l'hôte.
    let (code, _) = http(port, "POST", &format!("{path}/set"), &format!(r#"{{"node":{guitar},"db":-6.0}}"#));
    assert_eq!(code, 200);
    std::thread::sleep(Duration::from_millis(100));
    let ev = h.tick();
    let send = h.command_json(r#"{"type":"get_project"}"#);
    let g = node(&send, "Guitare").sends.iter().find(|s| s.to == bus).expect("envoi créé").gain;
    assert!((g - 10f32.powf(-6.0 / 20.0)).abs() < 1e-4, "gain {g}");
    assert!(ev.iter().any(|e| matches!(e, Event::Project(_))), "la page principale est informée de l'envoi créé");

    // La tablette relit son mix à jour.
    std::thread::sleep(Duration::from_millis(250));
    h.tick();
    let (_, state) = http(port, "GET", &format!("{path}/state"), "");
    let st: serde_json::Value = serde_json::from_str(&state).unwrap();
    let gtr = st["sources"].as_array().unwrap().iter().find(|s| s["id"] == guitar).unwrap();
    assert!((gtr["db"].as_f64().unwrap() + 6.0).abs() < 0.05, "{gtr}");

    // Niveau hors plage : borné ; −60 : l'envoi reste mais muet.
    http(port, "POST", &format!("{path}/set"), &format!(r#"{{"node":{drums},"db":40}}"#));
    std::thread::sleep(Duration::from_millis(100));
    h.tick();
    let p = h.command_json(r#"{"type":"get_project"}"#);
    let gd = node(&p, "Batterie").sends.iter().find(|s| s.to == bus).unwrap().gain;
    assert!((gd - 10f32.powf(6.0 / 20.0)).abs() < 1e-3, "borné à +6 dB : {gd}");

    h.command_json(r#"{"type":"stop_monitor"}"#);
}

#[test]
fn everything_else_is_refused() {
    let (mut h, bus, guitar, _) = with_return();
    let ev = h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#);
    let (_, port, returns, _) = monitor(&ev).unwrap();
    let path = path_of(&returns[0].2);
    h.tick();

    // Mauvais jeton, chemins inconnus, méthode inadaptée : 404.
    for (m, p) in [("GET", "/"), ("GET", "/r/0000"), ("GET", "/r/0000/state"), ("POST", "/r/0000/set"), ("GET", "/admin"), ("GET", &format!("{path}/autre")), ("DELETE", &path)] {
        let (code, _) = http(port, m, p, "{}");
        assert_eq!(code, 404, "{m} {p}");
    }
    // Corps illisible ou incomplet : 400, rien ne change.
    for body in ["", "pas du json", r#"{"node":1}"#, r#"{"db":0}"#, r#"{"node":-1,"db":0}"#, r#"{"node":1,"db":"fort"}"#] {
        let (code, _) = http(port, "POST", &format!("{path}/set"), body);
        assert_eq!(code, 400, "corps : {body}");
    }
    // Sources interdites : le retour lui-même, la sortie principale, un nœud inexistant : acceptées par HTTP mais sans aucun effet.
    for node in [bus, 0, 9999] {
        http(port, "POST", &format!("{path}/set"), &format!(r#"{{"node":{node},"db":0}}"#));
    }
    // Niveau nul sur une source sans envoi : aucun envoi n'est créé.
    http(port, "POST", &format!("{path}/set"), &format!(r#"{{"node":{guitar},"db":-60}}"#));
    std::thread::sleep(Duration::from_millis(150));
    let ev = h.tick();
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })));
    let p = h.command_json(r#"{"type":"get_project"}"#);
    assert!(project(&p).nodes.iter().all(|n| n.sends.iter().all(|s| s.to != bus)), "aucun envoi créé");
    // Corps énorme : lu seulement jusqu'à la limite, refus propre.
    let (code, _) = http(port, "POST", &format!("{path}/set"), &"x".repeat(100_000));
    assert!(code == 400 || code == 0, "refusé (400) ou connexion coupée par le serveur : {code}");

    // Arrêt : le port ne répond plus.
    let ev = h.command_json(r#"{"type":"stop_monitor"}"#);
    assert_eq!(monitor(&ev).map(|m| m.0), Some(false));
    // (Un autre test peut avoir reçu le même port libre entre-temps : on vérifie donc que NOTRE adresse ne répond plus, pas que le port est fermé.)
    let alive = try_http(port, "GET", &format!("{path}/state"), "").map(|r| r.0 == 200).unwrap_or(false);
    assert!(!alive, "le serveur est arrêté");
}

#[test]
fn a_new_return_gets_its_own_secret_and_a_new_project_forgets_the_old_ones() {
    let (mut h, bus, _, _) = with_return();
    let ev = h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#);
    let (_, port, returns, _) = monitor(&ev).unwrap();
    let first = path_of(&returns[0].2);

    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Paul"}"#);
    let paul = node(&ev, "Retour Paul").id;
    let ev = h.command_json(&format!(r#"{{"type":"set_return_output","node":{paul},"output":{{"first":5,"stereo":true,"mono":false}}}}"#));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    let ev = h.tick();
    let (_, _, returns, _) = monitor(&ev).expect("la liste des retours a changé : l'événement est renvoyé");
    assert_eq!(returns.len(), 2);
    assert_ne!(path_of(&returns[0].2), path_of(&returns[1].2), "un jeton par retour");
    assert_eq!(path_of(&returns.iter().find(|r| r.0 == bus).unwrap().2), first, "le jeton d'un retour existant ne change pas");
    h.tick();
    // Chaque tablette ne voit que son retour.
    let (_, s1) = http(port, "GET", &format!("{first}/state"), "");
    assert!(s1.contains("Retour Marie") && !s1.contains("Retour Paul"));

    // Un nouveau projet : les anciennes adresses ne fonctionnent plus.
    h.command_json(r#"{"type":"new_project"}"#);
    h.tick();
    std::thread::sleep(Duration::from_millis(100));
    let (code, _) = http(port, "GET", &format!("{first}/state"), "");
    assert_eq!(code, 404);
    h.command_json(r#"{"type":"stop_monitor"}"#);
}

#[test]
fn a_return_keeps_the_same_address_across_saving_and_reopening_the_project_until_it_is_renewed() {
    let (mut h, bus, _, _) = with_return();
    let dir = std::env::temp_dir().join(format!("daw-address-{}.daw", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let p = serde_json::to_string(&dir.to_string_lossy()).unwrap();
    let ev = h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#);
    let first = path_of(&monitor(&ev).unwrap().2[0].2);
    // L'adresse est tirée une fois : un second démarrage ou un arrêt ne la change pas.
    h.command_json(r#"{"type":"stop_monitor"}"#);
    let again = path_of(&monitor(&h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#)).unwrap().2[0].2);
    assert_eq!(first, again);
    h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#));
    h.command_json(r#"{"type":"stop_monitor"}"#);
    // Un autre projet, puis ce projet rouvert : même adresse (et un autre projet a la sienne).
    h.command_json(r#"{"type":"new_project"}"#);
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    let reopened = path_of(&monitor(&h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#)).unwrap().2[0].2);
    assert_eq!(first, reopened, "l'adresse est enregistrée dans le projet");
    // Renouveler : une adresse neuve, l'ancienne ne répond plus.
    let (_, port, _, _) = monitor(&h.command_json(r#"{"type":"get_project"}"#)).unwrap_or((true, 0, vec![], None));
    let _ = port;
    let ev = h.command_json(&format!(r#"{{"type":"renew_monitor_address","node":{bus}}}"#));
    let (_, port, returns, error) = monitor(&ev).expect("événement monitor");
    assert!(error.is_none());
    let renewed = path_of(&returns[0].2);
    assert_ne!(renewed, first);
    h.tick();
    assert_eq!(http(port, "GET", &renewed, "").0, 200);
    assert_eq!(http(port, "GET", &first, "").0, 404, "l'ancienne adresse ne marche plus");
    // Pas un retour : refusé.
    let p = h.command_json(r#"{"type":"get_project"}"#);
    let guitar = node(&p, "Guitare").id;
    assert!(h.command_json(&format!(r#"{{"type":"renew_monitor_address","node":{guitar}}}"#)).iter().any(|e| matches!(e, Event::Error { .. })));
    h.command_json(r#"{"type":"stop_monitor"}"#);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tablet_gets_a_signal_indicator_for_what_arrives_in_its_return() {
    // Un vrai moteur qui tourne : les crêtes des pistes passent par l'hôte jusqu'à l'état de la tablette.
    let (mut engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_audio_outputs(8);
    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Marie"}"#);
    let (bus, drums, bass) = (node(&ev, "Retour Marie").id, node(&ev, "Batterie").id, node(&ev, "Basse").id);
    h.command_json(&format!(r#"{{"type":"set_return_output","node":{bus},"output":{{"first":3,"stereo":true,"mono":false}}}}"#));
    for n in [drums, bass] {
        h.command_json(&format!(r#"{{"type":"add_send","node":{n},"to":{bus}}}"#));
    }
    h.command_json(&format!(r#"{{"type":"set_send","node":{bass},"to":{bus},"mute":true}}"#));
    let ev = h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#);
    let (port, url) = ev.iter().rev().find_map(|e| if let Event::Monitor { port, returns, .. } = e { Some((*port, returns[0].url.clone())) } else { None }).unwrap();
    let path = path_of(&url);
    h.command_json(r#"{"type":"transport","action":"play"}"#);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..400 {
        engine.process(&mut l, &mut r);
        h.tick();
    }
    std::thread::sleep(Duration::from_millis(250));
    h.tick();
    let st: serde_json::Value = serde_json::from_str(&http(port, "GET", &format!("{path}/state"), "").1).unwrap();
    let sig = |id: u32| st["sources"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap()["sig"].as_f64().unwrap();
    assert!(sig(drums) > -60.0 && sig(drums) < 6.0, "la batterie joue dans le retour : {}", sig(drums));
    assert_eq!(sig(bass), -120.0, "envoi coupé : rien n'arrive dans ce retour");
    let guitar = node(&h.command_json(r#"{"type":"get_project"}"#), "Guitare").id;
    assert_eq!(sig(guitar), -120.0, "pas d'envoi : rien n'arrive");
    h.command_json(r#"{"type":"stop_monitor"}"#);
}
