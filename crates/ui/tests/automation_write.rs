//! Modes d'écriture de l'automation : Toucher, Latch, Écriture, avec un vrai moteur qui fait avancer la tête de lecture.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{LaneInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

struct Rig {
    engine: Engine,
    host: Host,
    events: Vec<Event>,
}

fn rig() -> Rig {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    Rig { engine, host: Host::new(handle, session, meta).unwrap(), events: Vec::new() }
}

impl Rig {
    fn cmd(&mut self, json: &str) -> Vec<Event> {
        let ev = self.host.command_json(json);
        self.events.extend(ev.iter().cloned());
        ev
    }

    /// Fait tourner le moteur sur `blocks` blocs de 64 échantillons, avec un `tick` de l'hôte tous les 4 blocs.
    fn advance(&mut self, blocks: usize) {
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        for b in 0..blocks {
            self.engine.process(&mut l, &mut r);
            if b % 4 == 3 {
                let ev = self.host.tick();
                self.events.extend(ev);
            }
        }
    }

    fn project(&mut self) -> ProjectInfo {
        let ev = self.host.command_json(r#"{"type":"get_project"}"#);
        ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p.clone()) } else { None }).unwrap()
    }

    fn lane(&mut self, node: u32) -> Option<LaneInfo> {
        self.project().nodes.iter().find(|n| n.id == node).unwrap().automation.first().cloned()
    }
}

const GAIN: &str = r#"{"kind":"gain"}"#;

fn arm(r: &mut Rig, mode: &str) -> Vec<Event> {
    r.cmd(&format!(r#"{{"type":"set_write_mode","node":1,"target":{GAIN},"mode":"{mode}"}}"#))
}

fn set_gain(r: &mut Rig, g: f64) {
    r.cmd(&format!(r#"{{"type":"set_node","node":1,"gain":{g}}}"#));
}

fn positions(l: &LaneInfo) -> Vec<f64> {
    l.points.iter().map(|p| p.pos[0] as f64 / p.pos[1] as f64).collect()
}

#[test]
fn write_mode_records_the_fader_from_the_start_of_playback_until_stop_in_one_undo_step() {
    let mut r = rig();
    let ev = arm(&mut r, "write");
    assert!(ev.iter().any(|e| matches!(e, Event::Project(p) if p.write_arms.len() == 1 && p.write_arms[0].mode == "write")));
    r.cmd(r#"{"type":"transport","action":"play"}"#);
    r.advance(200);
    set_gain(&mut r, 0.5);
    r.advance(400);
    // Aperçu en direct : avant l'arrêt, la page a déjà reçu ce qui s'écrit (échantillon, valeur), dans l'ordre et sans trou.
    let live: Vec<[f64; 2]> = r.events.iter().filter_map(|e| if let Event::WriteLive { index, points, done: false, .. } = e { Some((*index, points.clone())) } else { None }).fold((0u32, Vec::new()), |(next, mut all), (i, p)| {
        assert_eq!(i, next, "les points arrivent à la suite les uns des autres");
        let n = next + p.len() as u32;
        all.extend(p);
        (n, all)
    }).1;
    assert!(live.len() > 20, "{} points déjà reçus avant l'arrêt", live.len());
    assert!(live.windows(2).all(|w| w[0][0] <= w[1][0]), "positions croissantes");
    assert!(live.iter().any(|p| (p[1] - 0.5).abs() < 1e-6), "la valeur du fader apparaît en direct");
    assert!(!r.events.iter().any(|e| matches!(e, Event::WriteLive { done: true, .. })), "pas fini");
    set_gain(&mut r, 0.2);
    r.advance(400);
    let ev = r.cmd(r#"{"type":"transport","action":"stop"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::WriteLive { done: true, .. })), "l'aperçu s'efface à la fin");
    assert!(ev.iter().any(|e| matches!(e, Event::History { undo_label: Some(l), .. } if l == "Modifier l'automation")), "une action annulable : {ev:?}");
    let lane = r.lane(1).expect("la courbe est écrite");
    assert_eq!(lane.target, daw_ui::protocol::LaneTarget::Gain);
    let values: Vec<f64> = lane.points.iter().map(|p| p.value).collect();
    assert!(values.iter().any(|v| (v - 0.5).abs() < 0.02) && values.iter().any(|v| (v - 0.2).abs() < 0.02), "{values:?}");
    assert!(lane.points.len() < 12, "les gestes sont allégés : {} points", lane.points.len());
    // La voie reprend en lecture et les points couvrent toute la durée jouée (≈ 1 000 blocs de 64 = 1,33 s = ~0,67 ronde à 120).
    assert_eq!(lane.mode, "read");
    assert!(positions(&lane).iter().cloned().fold(0.0, f64::max) > 0.5);
    // Annuler l'écriture : plus de courbe.
    r.cmd(r#"{"type":"undo"}"#);
    assert!(r.lane(1).is_none());
    assert!(r.events.iter().any(|e| matches!(e, Event::Writing { active: true })));
    assert!(matches!(r.events.iter().rev().find(|e| matches!(e, Event::Writing { .. })), Some(Event::Writing { active: false })));
}

#[test]
fn touch_mode_writes_only_while_the_fader_is_held_and_the_old_curve_resumes_after() {
    let mut r = rig();
    // Une courbe existante à 1,0 (lecture), puis on arme Toucher.
    r.cmd(&format!(r#"{{"type":"set_lane","node":1,"target":{GAIN},"mode":"read","points":[{{"pos":[0,1],"value":1.0,"curve":"linear"}},{{"pos":[4,1],"value":1.0,"curve":"linear"}}]}}"#));
    arm(&mut r, "touch");
    r.cmd(r#"{"type":"transport","action":"play"}"#);
    r.advance(200); // rien n'est écrit tant qu'on ne touche pas
    let before = r.lane(1).unwrap();
    assert_eq!(before.points.len(), 2);
    r.cmd(&format!(r#"{{"type":"gesture","node":1,"target":{GAIN},"active":true}}"#));
    set_gain(&mut r, 0.3);
    r.advance(300);
    r.cmd(&format!(r#"{{"type":"gesture","node":1,"target":{GAIN},"active":false}}"#));
    let ev = r.events.len();
    r.advance(200);
    assert_eq!(r.events.len() - ev, r.events[ev..].len());
    r.cmd(r#"{"type":"transport","action":"stop"}"#);
    let lane = r.lane(1).unwrap();
    let values: Vec<f64> = lane.points.iter().map(|p| p.value).collect();
    // Le geste (0,3) est dans la courbe, et elle reprend à 1,0 : début à 1,0, creux à 0,3, retour à 1,0.
    assert!(values.iter().any(|v| (v - 0.3).abs() < 0.02), "{values:?}");
    assert!((values[0] - 1.0).abs() < 1e-6 && (values.last().unwrap() - 1.0).abs() < 1e-6, "{values:?}");
    assert_eq!(lane.mode, "read");
}

#[test]
fn latch_mode_starts_at_the_first_touch_and_keeps_writing_the_last_value_until_stop() {
    let mut r = rig();
    arm(&mut r, "latch");
    r.cmd(r#"{"type":"transport","action":"play"}"#);
    r.advance(200);
    assert!(r.lane(1).is_none(), "rien n'est écrit avant le premier geste");
    r.cmd(&format!(r#"{{"type":"gesture","node":1,"target":{GAIN},"active":true}}"#));
    set_gain(&mut r, 0.4);
    r.advance(100);
    r.cmd(&format!(r#"{{"type":"gesture","node":1,"target":{GAIN},"active":false}}"#));
    r.advance(500); // on a lâché : la dernière valeur continue d'être écrite
    r.cmd(r#"{"type":"transport","action":"stop"}"#);
    let lane = r.lane(1).unwrap();
    let last = lane.points.last().unwrap();
    assert!((last.value - 0.4).abs() < 0.02);
    let end = last.pos[0] as f64 / last.pos[1] as f64;
    assert!(end > 0.4, "le latch tient jusqu'à l'arrêt ({end})");
}

#[test]
fn arming_changes_the_lane_to_read_disarming_stops_writing_and_invalid_arming_is_refused() {
    let mut r = rig();
    r.cmd(&format!(r#"{{"type":"set_lane","node":1,"target":{GAIN},"mode":"trim","points":[{{"pos":[0,1],"value":1.0,"curve":"linear"}}]}}"#));
    assert_eq!(r.lane(1).unwrap().mode, "trim");
    arm(&mut r, "touch");
    assert_eq!(r.lane(1).unwrap().mode, "read", "une courbe qu'on écrit n'est pas un trim");
    arm(&mut r, "none");
    assert!(r.project().write_arms.is_empty());
    for bad in [
        r#"{"type":"set_write_mode","node":1,"target":{"kind":"gain"},"mode":"record"}"#,
        r#"{"type":"set_write_mode","node":99,"target":{"kind":"gain"},"mode":"touch"}"#,
        r#"{"type":"set_write_mode","node":1,"target":{"kind":"send","to":77},"mode":"touch"}"#,
    ] {
        let ev = r.cmd(bad);
        assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })), "{bad}");
    }
    // Sans armement ni lecture, un geste n'écrit rien.
    set_gain(&mut r, 0.7);
    assert_eq!(r.lane(1).unwrap().points.len(), 1);
}

fn arms(p: &ProjectInfo) -> Vec<(u32, String, String)> {
    let mut v: Vec<_> = p.write_arms.iter().map(|a| (a.node, format!("{:?}", a.target), a.mode.clone())).collect();
    v.sort();
    v
}

#[test]
fn write_arms_are_saved_with_the_project_and_come_back_when_it_is_opened() {
    let dir = std::env::temp_dir().join(format!("daw-arms-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut a = rig();
    a.cmd(r#"{"type":"add_track","kind":"bus","name":"Réverbe"}"#);
    let bus = a.project().nodes.iter().find(|n| n.name == "Réverbe").unwrap().id;
    a.cmd(&format!(r#"{{"type":"add_send","node":1,"to":{bus}}}"#));
    arm(&mut a, "touch");
    a.cmd(r#"{"type":"set_write_mode","node":2,"target":{"kind":"pan"},"mode":"latch"}"#);
    a.cmd(&format!(r#"{{"type":"set_write_mode","node":1,"target":{{"kind":"send","to":{bus}}},"mode":"write"}}"#));
    let before = arms(&a.project());
    assert_eq!(before.len(), 3);
    a.cmd(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    let text = std::fs::read_to_string(dir.join("P.daw/projet.json")).unwrap();
    assert!(text.contains("\"write_arms\"") && text.contains("\"latch\"") && text.contains(&format!("send:{bus}")), "{text}");

    // Un autre hôte rouvre le projet : mêmes armements, aucun enregistrement en cours.
    let mut b = rig();
    let ev = b.cmd(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    let last_project = ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p.clone()) } else { None }).unwrap();
    assert_eq!(arms(&last_project), before);
    assert!(!ev.iter().any(|e| matches!(e, Event::Writing { active: true })));
    // Ouvrir ou créer un autre projet efface les armements du précédent.
    let ev = b.cmd(r#"{"type":"new_project"}"#);
    assert!(ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p.write_arms.len()) } else { None }).unwrap() == 0);
    // Un fichier sans la clé (projet d'avant) s'ouvre sans armement.
    let text = text.replace("\"write_arms\"", "\"autre\"");
    std::fs::write(dir.join("P.daw/projet.json"), text).unwrap();
    let ev = b.cmd(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    let p = ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p.clone()) } else { None }).unwrap();
    assert!(p.write_arms.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
