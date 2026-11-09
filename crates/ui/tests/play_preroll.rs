//! Pré-roll à la lecture normale : une lecture qui part d'une tête placée par un déplacement commence quelques mesures (ou secondes) avant elle ;
//! reprendre après une pause ne bouge pas ; sans pré-roll réglé rien ne change.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;
/// À 120 bpm en 4/4 une mesure dure 2 s.
const BAR: u64 = 2 * SR as u64;

struct Rig {
    engine: Engine,
    host: Host,
}

fn rig() -> Rig {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    Rig { engine, host: Host::new(handle, session, meta).unwrap() }
}

/// Un hôte où le pré-roll s'applique à la lecture et où l'arrêt est une pause (les réglages que ces tests-là exercent).
fn rig_with_pre_roll() -> Rig {
    let mut r = rig();
    r.transport(true, false);
    r
}

impl Rig {
    fn run(&mut self, blocks: usize) {
        let (mut l, mut r) = ([0f32; 64], [0f32; 64]);
        for _ in 0..blocks {
            self.engine.process(&mut l, &mut r);
        }
    }

    /// Réglages du transport. À appeler avant `pre_roll` : l'enregistrement des préférences remet aussi le métronome à ce qu'elles contiennent.
    fn transport(&mut self, play_pre_roll: bool, return_on_stop: bool) {
        let ev = self.host.command_json(&format!(r#"{{"type":"set_prefs","prefs":{{"transport":{{"play_pre_roll":{play_pre_roll},"return_on_stop":{return_on_stop}}}}}}}"#));
        assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    }

    fn pre_roll(&mut self, unit: &str, bars: u32, secs: f64) {
        let ev = self.host.command_json(&format!(r#"{{"type":"set_metronome","enabled":false,"only_recording":false,"volume":0.8,"count_in_bars":0,"pre_roll":{secs},"post_roll":0,"pre_roll_bars":{bars},"pre_roll_unit":"{unit}"}}"#));
        assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    }

    fn seek(&mut self, pos: u64) {
        self.host.command_json(&format!(r#"{{"type":"seek","pos":{pos}}}"#));
        self.run(2);
    }

    /// Lance ou arrête (`action` : play, stop) puis laisse le moteur avancer de `blocks` blocs ; rend la position vue par l'hôte.
    fn go(&mut self, action: &str, blocks: usize) -> u64 {
        self.host.command_json(&format!(r#"{{"type":"transport","action":"{action}"}}"#));
        self.run(blocks);
        let ev = self.host.tick();
        ev.iter().rev().find_map(|e| if let Event::Position { pos, .. } = e { Some(*pos) } else { None }).expect("événement position")
    }
}

#[test]
fn playing_from_a_placed_head_starts_the_pre_roll_earlier_and_resuming_after_a_pause_does_not() {
    let mut r = rig_with_pre_roll();
    r.pre_roll("bars", 2, 0.0);
    r.seek(4 * BAR); // mesure 5
    let pos = r.go("play", 8);
    assert!((2 * BAR..2 * BAR + 2_000).contains(&pos), "départ 2 mesures avant (mesure 3), pos = {pos}");

    // Pause puis reprise : la lecture continue là où elle s'est arrêtée, sans retourner en arrière.
    let paused = r.go("stop", 1);
    let resumed = r.go("play", 8);
    assert!(resumed >= paused && resumed < paused + 2_000, "reprise sans pré-roll : {paused} → {resumed}");

    // Un nouveau déplacement ramène le pré-roll.
    r.go("stop", 1);
    r.seek(6 * BAR);
    let pos = r.go("play", 8);
    assert!((4 * BAR..4 * BAR + 2_000).contains(&pos), "mesure 7 − 2 mesures, pos = {pos}");
}

#[test]
fn the_pre_roll_in_seconds_counts_too_and_nothing_changes_without_a_pre_roll_or_at_the_start() {
    let mut r = rig_with_pre_roll();
    // Sans pré-roll (réglage par défaut) : la lecture part de la tête.
    r.seek(4 * BAR);
    let pos = r.go("play", 8);
    assert!((4 * BAR..4 * BAR + 2_000).contains(&pos), "sans pré-roll, pos = {pos}");
    r.go("stop", 1);

    r.pre_roll("seconds", 0, 1.5);
    r.seek(4 * BAR);
    let pos = r.go("play", 8);
    let want = 4 * BAR - 3 * SR as u64 / 2;
    assert!((want..want + 2_000).contains(&pos), "1,5 s avant : {want}, pos = {pos}");
    r.go("stop", 1);

    // Au début du projet il n'y a rien avant : la lecture part de 0.
    r.seek(0);
    let pos = r.go("play", 8);
    assert!(pos < 2_000, "tête à 0 : pos = {pos}");
}

#[test]
fn a_section_jump_followed_by_play_includes_the_pre_roll_like_a_tablet_does() {
    let mut r = rig_with_pre_roll();
    r.pre_roll("bars", 1, 0.0);
    r.host.command_json(r#"{"type":"set_structure","sections":[{"at":[0,1],"dur":[4,1],"name":"Intro"},{"at":[4,1],"dur":[4,1],"name":"Refrain"}]}"#);
    // Deux commandes dans le même tick, sans que le moteur ait rendu la nouvelle position (cas d'une tablette) : la lecture part quand même
    // d'une mesure avant le Refrain.
    r.host.command_json(&format!(r#"{{"type":"seek","pos":{}}}"#, 8 * SR as u64));
    r.host.command_json(r#"{"type":"transport","action":"play"}"#);
    r.run(8);
    let ev = r.host.tick();
    let pos = ev.iter().rev().find_map(|e| if let Event::Position { pos, .. } = e { Some(*pos) } else { None }).unwrap();
    assert!((3 * BAR..3 * BAR + 2_000).contains(&pos), "mesure 4 (une mesure avant le Refrain en mesure 5), pos = {pos}");
}

#[test]
fn by_default_playback_has_no_pre_roll_and_stopping_returns_the_head_to_where_it_started() {
    // Réglages par défaut : pas de pré-roll à la lecture, retour de la tête à l'arrêt.
    let mut r = rig();
    r.pre_roll("bars", 2, 0.0);
    r.seek(4 * BAR);
    let pos = r.go("play", 8);
    assert!((4 * BAR..4 * BAR + 2_000).contains(&pos), "le pré-roll réglé ne joue pas à la lecture normale : pos = {pos}");
    let pos = r.go("stop", 4);
    assert!((4 * BAR..4 * BAR + 100).contains(&pos), "la tête est revenue au départ : pos = {pos}");
    // Relancer repart du même point.
    let pos = r.go("play", 8);
    assert!((4 * BAR..4 * BAR + 2_000).contains(&pos), "pos = {pos}");

    // Un déplacement pendant la lecture devient le nouveau point de départ.
    r.seek(6 * BAR);
    let pos = r.go("stop", 4);
    assert!((6 * BAR..6 * BAR + 100).contains(&pos), "pos = {pos}");
}

#[test]
fn with_both_settings_the_return_then_play_applies_the_pre_roll_again() {
    let mut r = rig();
    r.transport(true, true);
    r.pre_roll("bars", 1, 0.0);
    r.seek(4 * BAR);
    r.go("play", 8);
    let pos = r.go("stop", 4);
    assert!((4 * BAR..4 * BAR + 100).contains(&pos), "retour au point de départ (pas au pré-roll) : pos = {pos}");
    let pos = r.go("play", 8);
    assert!((3 * BAR..3 * BAR + 2_000).contains(&pos), "la relecture repart une mesure avant : pos = {pos}");
}

#[test]
fn without_the_return_stopping_is_a_pause_and_the_setting_is_saved_in_the_preferences() {
    let mut r = rig();
    r.transport(false, false);
    r.seek(4 * BAR);
    r.go("play", 8);
    let paused = r.go("stop", 4);
    assert!(paused > 4 * BAR + 100, "pause : la tête ne revient pas ({paused})");
    let ev = r.host.command_json(r#"{"type":"get_prefs"}"#);
    let saved = ev.iter().find_map(|e| if let Event::Prefs { prefs, .. } = e { Some(prefs.transport) } else { None }).expect("prefs");
    assert!(!saved.return_on_stop && !saved.play_pre_roll);
}
