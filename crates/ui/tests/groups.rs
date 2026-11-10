//! Groupes de pistes (projet) et mute / solo par envoi, par le protocole et par la tablette de monitoring.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de nœud « {name} »"))
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

fn host() -> Host {
    let (_engine, handle) = Engine::new(SR, 64);
    std::mem::forget(_engine);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_audio_outputs(8);
    h
}

fn with_return() -> (Host, u32) {
    let mut h = host();
    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Marie"}"#);
    let bus = node(&ev, "Retour Marie").id;
    h.command_json(&format!(r#"{{"type":"set_return_output","node":{bus},"output":{{"first":3,"stereo":true,"mono":false}}}}"#));
    (h, bus)
}

fn state(h: &mut Host) -> Vec<Event> {
    h.command_json(r#"{"type":"get_project"}"#)
}

#[test]
fn a_group_is_created_edited_deleted_and_every_step_is_undoable() {
    let mut h = host();
    let p = state(&mut h);
    let (drums, bass, guitar) = (node(&p, "Batterie").id, node(&p, "Basse").id, node(&p, "Guitare").id);
    let ev = h.command_json(&format!(r##"{{"type":"create_group","name":"Rythmique","color":"#ff8800","nodes":[{drums},{bass}]}}"##));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let g = project(&ev).groups[0].clone();
    assert_eq!((g.name.as_str(), g.color.as_str()), ("Rythmique", "#ff8800"));
    assert_eq!((node(&ev, "Batterie").group, node(&ev, "Basse").group, node(&ev, "Guitare").group), (g.id, g.id, 0));
    // Un nœud n'est que dans un groupe : en créer un second avec la basse la fait changer de groupe.
    let ev = h.command_json(&format!(r#"{{"type":"create_group","name":"Cordes","nodes":[{bass},{guitar}]}}"#));
    let g2 = project(&ev).groups[1].clone();
    assert_eq!((node(&ev, "Basse").group, node(&ev, "Guitare").group, node(&ev, "Batterie").group), (g2.id, g2.id, g.id));
    // Modifier : nom, couleur, membres.
    let ev = h.command_json(&format!(r##"{{"type":"edit_group","group":{},"name":"Section","color":"#00aa55","nodes":[{guitar}]}}"##, g2.id));
    assert_eq!(project(&ev).groups[1].name, "Section");
    assert_eq!((node(&ev, "Basse").group, node(&ev, "Guitare").group), (0, g2.id));
    // Supprimer : les membres restent, sans groupe.
    let ev = h.command_json(&format!(r#"{{"type":"delete_group","group":{}}}"#, g.id));
    assert_eq!(project(&ev).groups.len(), 1);
    assert_eq!(node(&ev, "Batterie").group, 0);
    // Tout s'annule dans l'ordre.
    for _ in 0..4 {
        let ev = h.command_json(r#"{"type":"undo"}"#);
        assert!(errors(&ev).is_empty());
    }
    assert!(project(&state(&mut h)).groups.is_empty());
}

#[test]
fn invalid_group_commands_are_refused_without_trace() {
    let mut h = host();
    let before = project(&state(&mut h)).groups.len();
    for cmd in [
        r#"{"type":"create_group","name":"  ","nodes":[1]}"#,
        r#"{"type":"create_group","name":"g","color":"rouge","nodes":[1]}"#,
        r#"{"type":"create_group","name":"g","nodes":[1,1]}"#,
        r#"{"type":"create_group","name":"g","nodes":[0]}"#,
        r#"{"type":"create_group","name":"g","nodes":[999]}"#,
        r#"{"type":"edit_group","group":42,"name":"x"}"#,
        r#"{"type":"delete_group","group":42}"#,
        r#"{"type":"set_group","group":42,"mute":true}"#,
        r#"{"type":"set_group","group":1}"#,
    ] {
        let ev = h.command_json(cmd);
        assert!(!errors(&ev).is_empty(), "devait être refusée : {cmd}");
    }
    assert_eq!(project(&state(&mut h)).groups.len(), before);
}

#[test]
fn a_group_mutes_solos_and_shifts_the_level_of_all_its_tracks_keeping_the_gaps() {
    let mut h = host();
    let p = state(&mut h);
    let (drums, bass) = (node(&p, "Batterie").id, node(&p, "Basse").id);
    h.command_json(&format!(r#"{{"type":"set_node","node":{drums},"gain":0.5}}"#));
    let ev = h.command_json(&format!(r#"{{"type":"create_group","name":"Rythmique","nodes":[{drums},{bass}]}}"#));
    let gid = project(&ev).groups[0].id;
    let (gd, gb) = (node(&ev, "Batterie").gain, node(&ev, "Basse").gain);
    let ev = h.command_json(&format!(r#"{{"type":"set_group","group":{gid},"mute":true}}"#));
    assert!(node(&ev, "Batterie").mute && node(&ev, "Basse").mute);
    let ev = h.command_json(&format!(r#"{{"type":"set_group","group":{gid},"mute":false,"delta_db":-6.0}}"#));
    let k = 10f32.powf(-6.0 / 20.0);
    assert!(!node(&ev, "Batterie").mute);
    assert!((node(&ev, "Batterie").gain - gd * k).abs() < 1e-4 && (node(&ev, "Basse").gain - gb * k).abs() < 1e-4);
    let ev = h.command_json(&format!(r#"{{"type":"set_group","group":{gid},"solo":true}}"#));
    assert!(node(&ev, "Batterie").solo && node(&ev, "Basse").solo && !node(&ev, "Guitare").solo);
}

#[test]
fn send_mute_and_solo_apply_to_one_return_only_and_a_group_acts_on_its_sends_to_that_return() {
    let (mut h, bus) = with_return();
    let p = state(&mut h);
    let (drums, bass, guitar) = (node(&p, "Batterie").id, node(&p, "Basse").id, node(&p, "Guitare").id);
    for n in [drums, bass, guitar] {
        h.command_json(&format!(r#"{{"type":"add_send","node":{n},"to":{bus}}}"#));
    }
    let ev = h.command_json(&format!(r#"{{"type":"set_send","node":{guitar},"to":{bus},"mute":true}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let s = node(&ev, "Guitare").sends.iter().find(|s| s.to == bus).unwrap();
    assert!(s.mute && !s.solo);
    assert!(!node(&ev, "Guitare").mute, "la piste elle-même n'est pas coupée");
    // Groupe : un seul geste sur tous les envois vers ce retour, qui ne touche pas aux pistes.
    let ev = h.command_json(&format!(r#"{{"type":"create_group","name":"Rythmique","nodes":[{drums},{bass}]}}"#));
    let gid = project(&ev).groups[0].id;
    let ev = h.command_json(&format!(r#"{{"type":"set_group","group":{gid},"to":{bus},"solo":true,"delta_db":-6.0}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    for name in ["Batterie", "Basse"] {
        let s = node(&ev, name).sends.iter().find(|s| s.to == bus).unwrap();
        assert!(s.solo, "{name}");
        assert!((s.gain - 10f32.powf(-6.0 / 20.0)).abs() < 1e-4, "{name} {}", s.gain);
        assert!(!node(&ev, name).solo, "le solo de la piste n'est pas touché");
    }
    assert!(!node(&ev, "Guitare").sends.iter().find(|s| s.to == bus).unwrap().solo);
    // Refusé : pas d'envoi vers un nœud inconnu, rien d'écrit.
    let ev = h.command_json(&format!(r#"{{"type":"set_send","node":{guitar},"to":77,"mute":true}}"#));
    assert!(!errors(&ev).is_empty());
}

fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let _ = write!(s, "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    let mut raw = String::new();
    let _ = s.read_to_string(&mut raw);
    let status = raw.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

#[test]
fn a_tablet_mutes_solos_and_moves_a_group_in_its_own_return_only() {
    let (mut h, bus) = with_return();
    let p = state(&mut h);
    let (drums, bass, guitar) = (node(&p, "Batterie").id, node(&p, "Basse").id, node(&p, "Guitare").id);
    let ev = h.command_json(&format!(r##"{{"type":"create_group","name":"Rythmique","color":"#ff8800","nodes":[{drums},{bass}]}}"##));
    let gid = project(&ev).groups[0].id;
    let ev = h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#);
    let (port, url) = ev.iter().rev().find_map(|e| if let Event::Monitor { port, returns, .. } = e { Some((*port, returns[0].url.clone())) } else { None }).unwrap();
    let path = format!("/{}", url.splitn(4, '/').nth(3).unwrap());
    h.tick();
    let st = |h: &mut Host| {
        std::thread::sleep(Duration::from_millis(250));
        h.tick();
        serde_json::from_str::<serde_json::Value>(&http(port, "GET", &format!("{path}/state"), "").1).unwrap()
    };
    let s = st(&mut h);
    assert_eq!(s["groups"][0]["name"], "Rythmique");
    assert_eq!(s["groups"][0]["id"], gid);
    let src = |s: &serde_json::Value, id: u32| s["sources"].as_array().unwrap().iter().find(|x| x["id"] == id).unwrap().clone();
    assert_eq!(src(&s, drums)["group"], gid);
    assert_eq!(src(&s, guitar)["group"], 0);

    // Mute d'une source sans envoi : l'envoi est créé à niveau nul et coupé.
    assert_eq!(http(port, "POST", &format!("{path}/set"), &format!(r#"{{"node":{guitar},"mute":true}}"#)).0, 200);
    // Groupe : solo + décalage (aucun envoi encore : le solo en crée à niveau nul).
    assert_eq!(http(port, "POST", &format!("{path}/set"), &format!(r#"{{"group":{gid},"solo":true}}"#)).0, 200);
    std::thread::sleep(Duration::from_millis(100));
    h.tick();
    let p = state(&mut h);
    let send = |name: &str| node(&p, name).sends.iter().find(|s| s.to == bus).cloned();
    assert!(send("Guitare").unwrap().mute);
    assert!(send("Batterie").unwrap().solo && send("Basse").unwrap().solo);
    assert!(!node(&p, "Batterie").solo && !node(&p, "Batterie").mute, "la piste n'est pas touchée : seulement son envoi vers ce retour");
    let s = st(&mut h);
    assert_eq!(src(&s, drums)["solo"], true);
    assert_eq!(src(&s, guitar)["mute"], true);

    // Niveau d'un groupe : décalage **absolu** qui reste à sa place (+6 dB), écarts conservés ; la tablette le relit.
    http(port, "POST", &format!("{path}/set"), &format!(r#"{{"node":{drums},"db":-12.0}}"#));
    http(port, "POST", &format!("{path}/set"), &format!(r#"{{"node":{bass},"db":-20.0}}"#));
    std::thread::sleep(Duration::from_millis(100));
    h.tick();
    assert_eq!(http(port, "POST", &format!("{path}/set"), &format!(r#"{{"group":{gid},"offset_db":6.0}}"#)).0, 200);
    let s = st(&mut h);
    assert!((src(&s, drums)["db"].as_f64().unwrap() + 6.0).abs() < 0.1, "{}", src(&s, drums));
    assert!((src(&s, bass)["db"].as_f64().unwrap() + 14.0).abs() < 0.1, "écart de 8 dB conservé : {}", src(&s, bass));
    assert!((s["groups"][0]["offset_db"].as_f64().unwrap() - 6.0).abs() < 1e-9, "le curseur relit sa position : {}", s["groups"][0]);
    // Redemander la même valeur ne bouge rien ; la remettre à 0 rend les niveaux d'avant.
    http(port, "POST", &format!("{path}/set"), &format!(r#"{{"group":{gid},"offset_db":6.0}}"#));
    http(port, "POST", &format!("{path}/set"), &format!(r#"{{"group":{gid},"offset_db":0}}"#));
    let s = st(&mut h);
    assert!((src(&s, drums)["db"].as_f64().unwrap() + 12.0).abs() < 0.1 && (src(&s, bass)["db"].as_f64().unwrap() + 20.0).abs() < 0.1, "{s}");
    assert_eq!(s["groups"][0]["offset_db"].as_f64().unwrap(), 0.0);
    // Borné à ±12 dB côté tablette.
    http(port, "POST", &format!("{path}/set"), &format!(r#"{{"group":{gid},"offset_db":40}}"#));
    let s = st(&mut h);
    assert_eq!(s["groups"][0]["offset_db"].as_f64().unwrap(), 12.0);

    // Demandes invalides : refusées, rien ne change.
    for body in [r#"{"group":1}"#, r#"{"node":1}"#, r#"{"node":1,"mute":"oui"}"#, r#"{"group":"x","mute":true}"#, "{}"] {
        assert_eq!(http(port, "POST", &format!("{path}/set"), body).0, 400, "{body}");
    }
    // Une demande visant un retour ou la sortie principale est ignorée sans effet.
    http(port, "POST", &format!("{path}/set"), &format!(r#"{{"node":{bus},"mute":true}}"#));
    http(port, "POST", &format!("{path}/set"), r#"{"node":0,"mute":true}"#);
    std::thread::sleep(Duration::from_millis(100));
    h.tick();
    let p = state(&mut h);
    assert!(node(&p, "Retour Marie").sends.is_empty());
    h.command_json(r#"{"type":"stop_monitor"}"#);
}

#[test]
fn a_group_offset_stays_in_place_is_shared_and_restores_the_levels_at_zero() {
    let (mut h, bus) = with_return();
    let p = state(&mut h);
    let (drums, bass) = (node(&p, "Batterie").id, node(&p, "Basse").id);
    let ev = h.command_json(&format!(r#"{{"type":"create_group","name":"Rythmique","nodes":[{drums},{bass}]}}"#));
    let gid = project(&ev).groups[0].id;
    let (gd, gb) = (node(&ev, "Batterie").gain, node(&ev, "Basse").gain);
    // Pistes : décalage absolu de −6 dB, donné deux fois = appliqué une fois.
    let cmd = format!(r#"{{"type":"set_group_offset","group":{gid},"db":-6.0}}"#);
    let ev = h.command_json(&cmd);
    let k = 10f32.powf(-6.0 / 20.0);
    assert!((node(&ev, "Batterie").gain - gd * k).abs() < 1e-4);
    assert_eq!(project(&ev).groups[0].offsets.iter().map(|o| (o.to, o.db)).collect::<Vec<_>>(), vec![(None, -6.0)]);
    let ev = h.command_json(&cmd);
    assert!(ev.is_empty() || (node(&state(&mut h), "Batterie").gain - gd * k).abs() < 1e-4, "pas de double application");
    // Vers un retour : un décalage à part, qui ne touche pas aux pistes.
    for n in [drums, bass] {
        h.command_json(&format!(r#"{{"type":"add_send","node":{n},"to":{bus}}}"#));
    }
    let ev = h.command_json(&format!(r#"{{"type":"set_group_offset","group":{gid},"db":3.0,"to":{bus}}}"#));
    let s = node(&ev, "Batterie").sends.iter().find(|s| s.to == bus).unwrap().gain;
    assert!((s - 10f32.powf(3.0 / 20.0)).abs() < 1e-3, "{s}");
    assert!((node(&ev, "Batterie").gain - gd * k).abs() < 1e-4, "les pistes gardent leur décalage de −6 dB");
    assert_eq!(project(&ev).groups[0].offsets.len(), 2);
    // Retour à zéro : niveaux d'origine.
    h.command_json(&format!(r#"{{"type":"set_group_offset","group":{gid},"db":0}}"#));
    let ev = h.command_json(&format!(r#"{{"type":"set_group_offset","group":{gid},"db":0,"to":{bus}}}"#));
    assert!((node(&ev, "Batterie").gain - gd).abs() < 1e-4 && (node(&ev, "Basse").gain - gb).abs() < 1e-4);
    assert!(project(&ev).groups[0].offsets.is_empty());
    // Un autre projet repart de zéro ; refus des demandes invalides.
    h.command_json(&format!(r#"{{"type":"set_group_offset","group":{gid},"db":5.0}}"#));
    h.command_json(r#"{"type":"new_project"}"#);
    assert!(project(&state(&mut h)).groups.iter().all(|g| g.offsets.is_empty()));
    for cmd in [r#"{"type":"set_group_offset","group":0,"db":1}"#, r#"{"type":"set_group_offset","group":1,"db":99}"#, r#"{"type":"set_group_offset","group":77,"db":1}"#] {
        assert!(!errors(&h.command_json(cmd)).is_empty(), "{cmd}");
    }
}

#[test]
fn a_return_can_have_its_own_groups_that_replace_the_project_ones_in_its_mix_only() {
    let (mut h, bus) = with_return();
    let p = state(&mut h);
    let (drums, bass, guitar) = (node(&p, "Batterie").id, node(&p, "Basse").id, node(&p, "Guitare").id);
    for n in [drums, bass, guitar] {
        h.command_json(&format!(r#"{{"type":"add_send","node":{n},"to":{bus}}}"#));
    }
    let ev = h.command_json(&format!(r#"{{"type":"create_group","name":"Rythmique","nodes":[{drums},{bass}]}}"#));
    let pg = project(&ev).groups[0].id;
    // Sans groupes propres : refusé. Un décalage du groupe du projet vers le retour est posé avant de personnaliser.
    let ev = h.command_json(&format!(r#"{{"type":"create_group","name":"X","nodes":[{guitar}],"ret":{bus}}}"#));
    assert!(!errors(&ev).is_empty());
    h.command_json(&format!(r#"{{"type":"set_group_offset","group":{pg},"db":6.0,"to":{bus}}}"#));
    let raised = node(&state(&mut h), "Batterie").sends.iter().find(|s| s.to == bus).unwrap().gain;
    assert!((raised - 10f32.powf(6.0 / 20.0)).abs() < 1e-3);
    let ev = h.command_json(&format!(r#"{{"type":"customize_return_groups","ret":{bus}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let rg = node(&ev, "Retour Marie").return_groups.clone().expect("groupes propres");
    assert_eq!(rg.groups.len(), 1);
    assert_ne!(rg.groups[0].id, pg, "identifiant neuf : les deux listes sont indépendantes");
    assert_eq!(rg.members, vec![[drums, rg.groups[0].id], [bass, rg.groups[0].id]]);
    assert!((node(&ev, "Batterie").sends.iter().find(|s| s.to == bus).unwrap().gain - 1.0).abs() < 1e-3, "le décalage du groupe du projet est remis à 0 avant de changer");
    assert!(!errors(&h.command_json(&format!(r#"{{"type":"customize_return_groups","ret":{bus}}}"#))).is_empty(), "déjà personnalisé");
    // Modifier les groupes du retour : le projet ne bouge pas.
    let ev = h.command_json(&format!(r##"{{"type":"create_group","name":"Soliste","color":"#00aa55","nodes":[{guitar}],"ret":{bus}}}"##));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    assert_eq!(node(&ev, "Retour Marie").return_groups.as_ref().unwrap().groups.len(), 2);
    assert_eq!(project(&ev).groups.len(), 1, "le projet garde son seul groupe");
    assert_eq!(node(&ev, "Guitare").group, 0);
    let sid = node(&ev, "Retour Marie").return_groups.as_ref().unwrap().groups[1].id;
    // Une action de groupe vers ce retour vise les membres du groupe **de ce retour**.
    let ev = h.command_json(&format!(r#"{{"type":"set_group","group":{sid},"to":{bus},"solo":true}}"#));
    assert!(errors(&ev).is_empty());
    let solo = |ev: &[Event], name: &str| node(ev, name).sends.iter().find(|s| s.to == bus).unwrap().solo;
    assert!(solo(&ev, "Guitare") && !solo(&ev, "Batterie"));
    // Le groupe du projet n'existe pas dans ce mix : refusé quand on vise ce retour, accepté pour les pistes.
    assert!(!errors(&h.command_json(&format!(r#"{{"type":"set_group","group":{pg},"to":{bus},"mute":true}}"#))).is_empty());
    assert!(errors(&h.command_json(&format!(r#"{{"type":"set_group","group":{pg},"mute":true}}"#))).is_empty());
    // Retour aux groupes du projet : un pas annulable ; l'annulation rend les groupes du retour.
    let ev = h.command_json(&format!(r#"{{"type":"reset_return_groups","ret":{bus}}}"#));
    assert!(node(&ev, "Retour Marie").return_groups.is_none());
    assert!(!errors(&h.command_json(&format!(r#"{{"type":"reset_return_groups","ret":{bus}}}"#))).is_empty());
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(node(&ev, "Retour Marie").return_groups.as_ref().unwrap().groups.len(), 2);
    // Pas un retour : refusé.
    assert!(!errors(&h.command_json(&format!(r#"{{"type":"customize_return_groups","ret":{guitar}}}"#))).is_empty());
    // Le projet enregistré garde les groupes du retour.
    let dir = std::env::temp_dir().join(format!("daw-retgroups-{}.daw", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    h.command_json(&format!(r#"{{"type":"save_project","path":{}}}"#, serde_json::to_string(&dir.to_string_lossy()).unwrap()));
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{}}}"#, serde_json::to_string(&dir.to_string_lossy()).unwrap()));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let p = state(&mut h);
    assert_eq!(node(&p, "Retour Marie").return_groups.as_ref().unwrap().groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["Rythmique", "Soliste"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tablet_sees_the_groups_of_its_own_return() {
    let (mut h, bus) = with_return();
    let p = state(&mut h);
    let (drums, guitar) = (node(&p, "Batterie").id, node(&p, "Guitare").id);
    h.command_json(&format!(r#"{{"type":"create_group","name":"Projet","nodes":[{drums}]}}"#));
    h.command_json(&format!(r#"{{"type":"customize_return_groups","ret":{bus}}}"#));
    h.command_json(&format!(r#"{{"type":"create_group","name":"Perso","nodes":[{guitar}],"ret":{bus}}}"#));
    let ev = h.command_json(r#"{"type":"start_monitor","port":0,"local_only":true}"#);
    let (port, url) = ev.iter().rev().find_map(|e| if let Event::Monitor { port, returns, .. } = e { Some((*port, returns[0].url.clone())) } else { None }).unwrap();
    let path = format!("/{}", url.splitn(4, '/').nth(3).unwrap());
    h.tick();
    std::thread::sleep(Duration::from_millis(250));
    h.tick();
    let st: serde_json::Value = serde_json::from_str(&http(port, "GET", &format!("{path}/state"), "").1).unwrap();
    let names: Vec<&str> = st["groups"].as_array().unwrap().iter().map(|g| g["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Projet", "Perso"], "copie du projet puis le groupe propre");
    let src = |id: u32| st["sources"].as_array().unwrap().iter().find(|x| x["id"] == id).unwrap().clone();
    assert_eq!(src(guitar)["group"], st["groups"][1]["id"]);
    assert_eq!(src(drums)["group"], st["groups"][0]["id"]);
    h.command_json(r#"{"type":"stop_monitor"}"#);
}

#[test]
fn a_return_turned_back_into_a_plain_bus_gives_its_group_offsets_back() {
    let (mut h, bus) = with_return();
    let p = state(&mut h);
    let (drums, bass) = (node(&p, "Batterie").id, node(&p, "Basse").id);
    for n in [drums, bass] {
        h.command_json(&format!(r#"{{"type":"add_send","node":{n},"to":{bus}}}"#));
    }
    let ev = h.command_json(&format!(r#"{{"type":"create_group","name":"Rythmique","nodes":[{drums},{bass}]}}"#));
    let gid = project(&ev).groups[0].id;
    let send = |ev: &[Event], name: &str| node(ev, name).sends.iter().find(|s| s.to == bus).unwrap().clone();
    h.command_json(&format!(r#"{{"type":"set_group_offset","group":{gid},"db":6.0,"to":{bus}}}"#));
    h.command_json(&format!(r#"{{"type":"set_send","node":{drums},"to":{bus},"mute":true}}"#));
    assert!((send(&state(&mut h), "Batterie").gain - 10f32.powf(6.0 / 20.0)).abs() < 1e-3);
    // Le bus n'est plus un retour (sortie ramenée à la sortie principale) : le décalage est rendu, la coupe est conservée mais sans effet.
    let ev = h.command_json(&format!(r#"{{"type":"set_return_output","node":{bus},"output":null}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let p = state(&mut h);
    assert!(node(&p, "Retour Marie").phys.is_none());
    assert!((send(&p, "Batterie").gain - 1.0).abs() < 1e-3 && (send(&p, "Basse").gain - 1.0).abs() < 1e-3, "niveaux d'avant le décalage");
    assert!(project(&p).groups[0].offsets.is_empty());
    assert!(send(&p, "Batterie").mute, "réglage conservé");
}

#[test]
fn a_track_midi_filter_is_a_list_of_device_and_channel_rules_saved_with_the_project() {
    let mut h = host();
    h.set_midi_devices(vec!["Clavier A".into(), "Pad B".into()]);
    let p = state(&mut h);
    assert_eq!(project(&p).midi_devices, vec!["Clavier A".to_string(), "Pad B".to_string()]);
    let synth = node(&p, "Claviers").id;
    assert!(node(&p, "Claviers").midi_filter.is_empty(), "par défaut tout passe");
    let ev = h.command_json(&format!(r#"{{"type":"set_track_midi_filter","node":{synth},"rules":[{{"device":"Clavier A","channels":[1,2]}},{{"device":"Pad B"}},{{"channels":[10]}}]}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let f = &node(&ev, "Claviers").midi_filter;
    assert_eq!(f.len(), 3);
    assert_eq!((f[0].device.as_str(), f[0].channels.clone()), ("Clavier A", vec![1, 2]));
    assert!(f[1].channels.is_empty() && f[2].device.is_empty() && f[2].channels == vec![10]);
    // Routage : seules les pistes MIDI armées sont routées, avec leur filtre.
    h.command_json(&format!(r#"{{"type":"set_track_input","node":{synth},"input":{{"kind":"midi"}}}}"#));
    h.command_json(&format!(r#"{{"type":"set_node","node":{synth},"arm":true}}"#));
    let routes = h.armed_midi_routes();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].0, synth);
    use daw_session::midi_filter::accepts;
    assert!(accepts(&routes[0].1, "Clavier A", 1) && !accepts(&routes[0].1, "Clavier A", 5));
    assert!(accepts(&routes[0].1, "Pad B", 15) && accepts(&routes[0].1, "Autre", 9) && !accepts(&routes[0].1, "Autre", 8));
    // Enregistré et rouvert.
    let dir = std::env::temp_dir().join(format!("daw-midifilter-{}.daw", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let p = serde_json::to_string(&dir.to_string_lossy()).unwrap();
    h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#));
    h.command_json(r#"{"type":"new_project"}"#);
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    assert_eq!(node(&state(&mut h), "Claviers").midi_filter.len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
    // Refus : canal hors 1-16, trop de règles, pas une piste, sortie principale.
    for cmd in [
        format!(r#"{{"type":"set_track_midi_filter","node":{synth},"rules":[{{"channels":[17]}}]}}"#),
        format!(r#"{{"type":"set_track_midi_filter","node":{synth},"rules":[{{"channels":[0]}}]}}"#),
        format!(r#"{{"type":"set_track_midi_filter","node":{synth},"rules":{}}}"#, serde_json::to_string(&vec![serde_json::json!({}); 17]).unwrap()),
        r#"{"type":"set_track_midi_filter","node":0,"rules":[]}"#.to_string(),
    ] {
        assert!(!errors(&h.command_json(&cmd)).is_empty(), "{cmd}");
    }
}

#[test]
fn a_track_input_dynamics_is_set_through_the_protocol_heard_on_the_monitored_input_and_saved() {
    let (mut engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_audio_inputs(2);
    let p = state(&mut h);
    let voice = node(&p, "Voix").id;
    assert!(!node(&p, "Voix").dynamics.enabled, "désactivée par défaut");
    h.command_json(&format!(r#"{{"type":"set_track_input","node":{voice},"input":{{"kind":"audio","channels":[1]}}}}"#));
    h.command_json(&format!(r#"{{"type":"set_node","node":{voice},"arm":true,"monitor":true}}"#));
    let level = |h: &mut Host, engine: &mut Engine| -> f32 {
        let x: Vec<f32> = (0..SR as usize / 2).map(|i| 0.4 * (std::f32::consts::TAU * 440.0 * i as f32 / SR as f32).sin()).collect();
        let (mut peak, mut r) = (0.0f32, vec![0.0f32; 64]);
        for (k, c) in x.chunks(64).enumerate() {
            let mut l = vec![0.0f32; c.len()];
            engine.process_inputs(&[c], &mut l, &mut r[..c.len()]);
            h.tick();
            if k * 64 > SR as usize / 4 {
                // En régime établi : on laisse passer l'attaque du compresseur.
                peak = peak.max(l.iter().fold(0.0, |m, v| m.max(v.abs())));
            }
        }
        peak
    };
    let raw = level(&mut h, &mut engine);
    let ev = h.command_json(&format!(r#"{{"type":"set_track_dynamics","node":{voice},"dynamics":{{"enabled":true,"gate_db":-90,"threshold_db":-30,"ratio":8,"attack_ms":1,"release_ms":50,"makeup_db":0,"record":false}}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let d = node(&ev, "Voix").dynamics;
    assert!(d.enabled && d.ratio == 8.0 && !d.record);
    let squeezed = level(&mut h, &mut engine);
    assert!(squeezed < 0.5 * raw, "le compresseur réduit l'entrée écoutée : {squeezed} contre {raw}");
    // Refus : valeur hors plage, sortie principale, piste inexistante.
    for cmd in [
        format!(r#"{{"type":"set_track_dynamics","node":{voice},"dynamics":{{"enabled":true,"gate_db":-90,"threshold_db":-30,"ratio":50,"attack_ms":1,"release_ms":50,"makeup_db":0}}}}"#),
        r#"{"type":"set_track_dynamics","node":0,"dynamics":{"enabled":false,"gate_db":-90,"threshold_db":-30,"ratio":2,"attack_ms":1,"release_ms":50,"makeup_db":0}}"#.to_string(),
        r#"{"type":"set_track_dynamics","node":99,"dynamics":{"enabled":false,"gate_db":-90,"threshold_db":-30,"ratio":2,"attack_ms":1,"release_ms":50,"makeup_db":0}}"#.to_string(),
    ] {
        assert!(!errors(&h.command_json(&cmd)).is_empty(), "{cmd}");
    }
    // Enregistré et rouvert.
    let dir = std::env::temp_dir().join(format!("daw-dynamics-{}.daw", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let p = serde_json::to_string(&dir.to_string_lossy()).unwrap();
    h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#));
    h.command_json(r#"{"type":"new_project"}"#);
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let d = node(&state(&mut h), "Voix").dynamics;
    assert!(d.enabled && d.threshold_db == -30.0 && d.ratio == 8.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_track_velocity_curve_is_set_through_the_protocol_refused_when_invalid_and_saved() {
    let (_engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    let synth = node(&state(&mut h), "Claviers").id;
    let c = node(&state(&mut h), "Claviers").velocity_curve;
    assert!(!c.enabled && c.min == 1 && c.max == 127 && c.fixed == 0, "désactivée par défaut : {c:?}");
    let ev = h.command_json(&format!(r#"{{"type":"set_track_velocity_curve","node":{synth},"curve":{{"enabled":true,"bend":400,"smooth":200,"min":30,"max":110}}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let c = node(&ev, "Claviers").velocity_curve;
    assert!(c.enabled && c.bend == 400 && c.smooth == 200 && c.min == 30 && c.max == 110 && c.fixed == 0);
    for cmd in [
        format!(r#"{{"type":"set_track_velocity_curve","node":{synth},"curve":{{"enabled":true,"min":0,"max":110}}}}"#),
        format!(r#"{{"type":"set_track_velocity_curve","node":{synth},"curve":{{"enabled":true,"bend":1001,"min":1,"max":127}}}}"#),
        format!(r#"{{"type":"set_track_velocity_curve","node":{synth},"curve":{{"enabled":true,"min":1,"max":127,"fixed":200}}}}"#),
        r#"{"type":"set_track_velocity_curve","node":0,"curve":{"enabled":false,"min":1,"max":127}}"#.to_string(),
        r#"{"type":"set_track_velocity_curve","node":99,"curve":{"enabled":false,"min":1,"max":127}}"#.to_string(),
    ] {
        assert!(!errors(&h.command_json(&cmd)).is_empty(), "{cmd}");
    }
    assert_eq!(node(&state(&mut h), "Claviers").velocity_curve.min, 30, "un refus ne change rien");
    let dir = std::env::temp_dir().join(format!("daw-velcurve-{}.daw", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let p = serde_json::to_string(&dir.to_string_lossy()).unwrap();
    h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#));
    h.command_json(r#"{"type":"new_project"}"#);
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let c = node(&state(&mut h), "Claviers").velocity_curve;
    assert!(c.enabled && c.bend == 400 && c.smooth == 200 && c.min == 30 && c.max == 110, "{c:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_return_copies_the_groups_of_another_return_and_a_track_cannot_output_to_a_return() {
    let (mut h, marie) = with_return();
    let ev = h.command_json(r#"{"type":"add_track","kind":"return","name":"Retour Paul"}"#);
    let paul = node(&ev, "Retour Paul").id;
    // Marie a ses propres groupes (une copie de ceux du projet) et en crée un ; Paul en copie le contenu.
    let p = state(&mut h);
    let guitar = node(&p, "Guitare").id;
    assert!(errors(&h.command_json(&format!(r#"{{"type":"customize_return_groups","ret":{marie}}}"#))).is_empty());
    let ev = h.command_json(&format!(r##"{{"type":"create_group","name":"Cordes","color":"#ff8800","nodes":[{guitar}],"ret":{marie}}}"##));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let ev = h.command_json(&format!(r#"{{"type":"copy_return_groups","ret":{paul},"from":{marie}}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let own = |ev: &[Event], id: u32| ev.iter().rev().find_map(|e| if let Event::Project(p) = e { p.nodes.iter().find(|n| n.id == id).and_then(|n| n.return_groups.clone()) } else { None });
    let (m, pa) = (own(&ev, marie).unwrap(), own(&ev, paul).unwrap());
    assert_eq!(pa.groups.iter().map(|g| g.name.clone()).collect::<Vec<_>>(), m.groups.iter().map(|g| g.name.clone()).collect::<Vec<_>>());
    assert!(pa.groups.iter().all(|g| !m.groups.iter().any(|o| o.id == g.id)), "identifiants neufs");
    assert!(pa.members.iter().any(|[n, _]| *n == guitar), "les membres suivent");
    // Un seul pas d'annulation : Paul suit de nouveau les groupes du projet.
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert!(own(&ev, paul).is_none() && own(&ev, marie).is_some());
    // Refus : de soi-même, d'un bus ordinaire, d'un nœud inconnu.
    let ev = h.command_json(r#"{"type":"add_track","kind":"bus","name":"Sous-groupe"}"#);
    let bus = node(&ev, "Sous-groupe").id;
    for cmd in [
        format!(r#"{{"type":"copy_return_groups","ret":{marie},"from":{marie}}}"#),
        format!(r#"{{"type":"copy_return_groups","ret":{paul},"from":{bus}}}"#),
        format!(r#"{{"type":"copy_return_groups","ret":{bus},"from":{marie}}}"#),
        format!(r#"{{"type":"copy_return_groups","ret":{paul},"from":999}}"#),
    ] {
        assert_eq!(errors(&h.command_json(&cmd)).len(), 1, "{cmd}");
    }
    // Un retour ne reçoit que des envois et n'a pas de destination : sortie de piste vers un retour refusée, sortie d'un retour refusée.
    for cmd in [format!(r#"{{"type":"set_track_output","node":{guitar},"output":{marie}}}"#), format!(r#"{{"type":"set_track_output","node":{marie},"output":{bus}}}"#)] {
        assert_eq!(errors(&h.command_json(&cmd)).len(), 1, "{cmd}");
    }
    assert!(errors(&h.command_json(&format!(r#"{{"type":"set_track_output","node":{guitar},"output":{bus}}}"#))).is_empty(), "vers un bus ordinaire : accepté");
}
