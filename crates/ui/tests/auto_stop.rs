//! Arrêt automatique : désactivé par défaut ; une fois activé, la lecture s'arrête passé le délai après la fin du projet, et une prise après
//! le délai plus long « rien derrière » (elle s'arrête alors comme à un arrêt normal : le clip naît). Jamais en boucle.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn project_end() -> u64 {
    daw_offline_render::export::project_end(demo_session(SR).0.graph())
}

struct Rig {
    engine: Engine,
    host: Host,
}

fn rig() -> Rig {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut host = Host::new(handle, session, meta).unwrap();
    host.set_audio_inputs(4);
    host.set_record_compensation(0);
    Rig { engine, host }
}

impl Rig {
    fn run(&mut self, frames: usize) {
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        let silence = [vec![0.0f32; 64], vec![0.0f32; 64], vec![0.0f32; 64], vec![0.0f32; 64]];
        let refs: Vec<&[f32]> = silence.iter().map(Vec::as_slice).collect();
        for _ in 0..frames / 64 {
            self.engine.process_inputs(&refs, &mut l, &mut r);
        }
    }

    fn seek_play(&mut self, pos: u64) {
        self.host.command_json(&format!(r#"{{"type":"seek","pos":{pos}}}"#));
        self.host.command_json(r#"{"type":"transport","action":"play"}"#);
        self.run(64 * 4); // le moteur prend la commande
    }
}

fn enable(host: &mut Host, after: u32, recording: u32) {
    let ev = host.command_json(&format!(r#"{{"type":"set_prefs","prefs":{{"auto_stop":{{"enabled":true,"after_end_secs":{after},"recording_secs":{recording}}}}}}}"#));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
}

fn stopped(ev: &[Event]) -> bool {
    ev.iter().any(|e| matches!(e, Event::Error { command, .. } if command == "auto_stop"))
}

#[test]
fn nothing_stops_by_itself_unless_the_option_is_on() {
    let mut r = rig();
    let end = project_end();
    r.seek_play(end + 3_600 * SR as u64);
    assert!(!stopped(&r.host.tick()), "désactivé par défaut");
    // Valeurs refusées : hors de 10 à 3 600 s.
    let ev = r.host.command_json(r#"{"type":"set_prefs","prefs":{"auto_stop":{"enabled":true,"after_end_secs":5,"recording_secs":300}}}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })));
}

#[test]
fn playback_stops_after_the_delay_past_the_end_of_the_project() {
    let mut r = rig();
    enable(&mut r.host, 10, 300);
    let end = project_end();
    r.seek_play(end + 5 * SR as u64);
    assert!(!stopped(&r.host.tick()), "5 s après la fin : pas encore (délai de 10 s)");
    r.run(6 * SR as usize);
    let ev = r.host.tick();
    assert!(stopped(&ev), "11 s après la fin : arrêt");
    assert!(ev.iter().any(|e| matches!(e, Event::Position { playing: false, .. })));
    assert!(!stopped(&r.host.tick()), "un seul arrêt");
}

#[test]
fn a_loop_is_never_stopped() {
    let mut r = rig();
    enable(&mut r.host, 10, 300);
    let end = project_end();
    r.host.command_json(&format!(r#"{{"type":"loop","start":{},"end":{}}}"#, end + 20 * SR as u64, end + 30 * SR as u64));
    r.seek_play(end + 25 * SR as u64);
    assert!(!stopped(&r.host.tick()));
}

#[test]
fn a_take_stops_like_a_normal_stop_after_the_longer_delay_with_nothing_behind() {
    let mut r = rig();
    let dir = std::env::temp_dir().join(format!("daw-autostop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    r.host.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    enable(&mut r.host, 600, 10); // arrêt de lecture très tardif ; prise : 10 s sans rien derrière
    let ev = r.host.command_json(r#"{"type":"get_project"}"#);
    let guitar = ev.iter().rev().find_map(|e| if let Event::Project(p) = e { p.nodes.iter().find(|n| n.name == "Guitare").map(|n| n.id) } else { None }).unwrap();
    r.host.command_json(&format!(r#"{{"type":"set_track_input","node":{guitar},"input":{{"kind":"audio","channels":[0]}}}}"#));
    r.host.command_json(&format!(r#"{{"type":"set_node","node":{guitar},"arm":true}}"#));
    let end = project_end();
    // La prise démarre bien après la fin du projet : elle s'arrête 10 s après son propre début.
    r.host.command_json(&format!(r#"{{"type":"seek","pos":{}}}"#, end + 30 * SR as u64));
    r.run(64 * 4); // le moteur prend le déplacement : l'enregistrement part de la position de la tête
    let ev = r.host.command_json(r#"{"type":"record"}"#);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    r.run(64 * 4);
    assert!(!stopped(&r.host.tick()));
    r.run(5 * SR as usize);
    assert!(!stopped(&r.host.tick()), "5 s : pas encore");
    r.run(6 * SR as usize);
    let mut all = r.host.tick();
    assert!(stopped(&all), "11 s sans rien derrière : arrêt");
    // La prise se termine proprement : le moteur la ferme, l'hôte pose le clip.
    for _ in 0..400 {
        r.run(64);
        all.extend(r.host.tick());
        if all.iter().any(|e| matches!(e, Event::Recording { active: false, .. })) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(all.iter().any(|e| matches!(e, Event::Recording { active: false, .. })), "prise terminée");
    let label = all.iter().rev().find_map(|e| if let Event::History { undo_label, .. } = e { undo_label.clone() } else { None });
    assert_eq!(label.as_deref(), Some("Enregistrer"), "le clip de la prise est posé en une action annulable");
    let _ = std::fs::remove_dir_all(&dir);
}
