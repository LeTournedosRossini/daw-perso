//! Automation de gain, de panoramique et de paramètres d'effets, rendue par le vrai moteur.

mod common;

use common::*;
use daw_engine::{AutoLane, AutoPoint, AutoTarget, Command, GraphDesc, GraphError, Insert, InsertFactory, MidiMsg, TrackFormat, AUTO_GRID};
use daw_offline_render::{render, RenderSpec};

fn dc_track(g: &mut GraphDesc, len: usize) -> daw_core_types::NodeId {
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, clip(&stereo(vec![1.0; len], vec![1.0; len]), 0));
    t
}

fn render_block(g: &GraphDesc, frames: u64, block: usize, engine_block: usize) -> daw_offline_render::Rendered {
    let mut spec = RenderSpec::new(SR, frames, g.clone());
    spec.block = block;
    spec.engine_block = engine_block;
    render(&spec).unwrap()
}

#[test]
fn gain_automation_follows_the_curve_sample_by_sample() {
    let mut g = GraphDesc::new();
    let t = dc_track(&mut g, 3_000);
    g.set_automation(t, AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(1_000, 0.0), AutoPoint::new(2_000, 1.0)]));
    let o = render_block(&g, 3_000, 256, 256).left;
    assert_eq!(o[0], 0.0); // avant le premier point : sa valeur
    assert_eq!(o[999], 0.0);
    assert_eq!(o[1_000], 0.0);
    assert_eq!(o[1_500], 0.5);
    assert_eq!(o[1_250], 0.25);
    assert_eq!(o[2_000], 1.0);
    assert_eq!(o[2_999], 1.0); // après le dernier point : sa valeur
    assert!(o[1_000..2_000].windows(2).all(|w| w[1] > w[0]));
}

#[test]
fn step_curve_holds_then_jumps() {
    let mut g = GraphDesc::new();
    let t = dc_track(&mut g, 1_000);
    g.set_automation(t, AutoLane::new(AutoTarget::Gain, vec![AutoPoint::step(0, 0.25), AutoPoint::step(500, 1.0), AutoPoint::new(800, 0.0)]));
    let o = render_block(&g, 1_000, 64, 256).left;
    assert!(o[..500].iter().all(|&x| x == 0.25));
    assert!(o[500..800].iter().all(|&x| x == 1.0));
    assert_eq!(o[800], 0.0);
}

#[test]
fn pan_automation_moves_the_sound_and_overrides_the_manual_value() {
    let mut g = GraphDesc::new();
    let t = dc_track(&mut g, 2_000);
    g.set_params(t, daw_engine::NodeParams { pan: 1.0, ..Default::default() }); // réglage manuel ignoré
    g.set_automation(t, AutoLane::new(AutoTarget::Pan, vec![AutoPoint::new(0, -1.0), AutoPoint::new(1_000, 1.0)]));
    let r = render_block(&g, 2_000, 128, 256);
    assert_eq!((r.left[0], r.right[0]), (1.0, 0.0)); // tout à gauche (balance : gauche pleine, droite nulle)
    assert!(r.left[900] < r.left[100] && r.right[900] > r.right[100]);
    assert_eq!((r.left[1_999], r.right[1_999]), (0.0, 1.0));
}

#[test]
fn automated_gain_keeps_mute_working() {
    let mut g = GraphDesc::new();
    let t = dc_track(&mut g, 4_000);
    g.set_automation(t, AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(0, 0.5)]));
    let mut spec = RenderSpec::new(SR, 4_000, g);
    spec.events.push((1_000, Command::SetMute { node: t.0, mute: true }));
    let o = render(&spec).unwrap().left;
    assert_eq!(o[999], 0.5);
    assert_eq!(o[3_999], 0.0);
}

#[test]
fn automation_does_not_depend_on_block_sizes() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, clip(&stereo(noise(1, 20_000), noise(2, 20_000)), 0));
    g.set_automation(t, AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(300, 0.1), AutoPoint::step(7_000, 1.7), AutoPoint::new(9_001, 0.3), AutoPoint::new(15_000, 1.0)]));
    g.set_automation(t, AutoLane::new(AutoTarget::Pan, vec![AutoPoint::new(0, -0.9), AutoPoint::new(19_999, 0.9)]));
    let reference = render_block(&g, 20_000, 4_096, 4_096).hash();
    for (block, engine_block) in [(1, 1), (7, 64), (64, 256), (333, 256), (1_024, 1_024), (50_000, 4_096)] {
        assert_eq!(render_block(&g, 20_000, block, engine_block).hash(), reference, "bloc {block}/{engine_block}");
    }
}

#[test]
fn effect_parameter_is_sent_on_the_absolute_grid() {
    let mut g = GraphDesc::new();
    let t = dc_track(&mut g, 4_000);
    let ins = g.add_insert(t, "gain", delay_factory(0)).unwrap();
    let lane = AutoLane::new(AutoTarget::InsertParam { insert: ins, param: 0 }, vec![AutoPoint::new(0, 0.0), AutoPoint::new(2_000, 1.0)]);
    g.set_automation(t, lane.clone());
    let o = render_block(&g, 4_000, 100, 256).left; // blocs qui ne tombent pas sur la grille
    for (i, &x) in o.iter().enumerate() {
        let grid_start = i as u64 / AUTO_GRID * AUTO_GRID;
        assert_eq!(x, lane.value_at(grid_start) as f32, "échantillon {i}");
    }
    // Indépendant des blocs.
    let h = render_block(&g, 4_000, 4_096, 4_096).hash();
    for (b, e) in [(1, 1), (7, 64), (333, 256), (50_000, 4_096)] {
        assert_eq!(render_block(&g, 4_000, b, e).hash(), h, "{b}/{e}");
    }
}

/// Effet de test : pose une impulsion de 1,0 sur la sortie gauche à chaque NoteOn.
struct Clicker;
impl Insert for Clicker {
    fn latency(&self) -> u32 {
        0
    }
    fn process(&mut self, _: &[f32], _: &[f32], l: &mut [f32], r: &mut [f32]) {
        l.fill(0.0);
        r.fill(0.0);
    }
    fn accepts_notes(&self) -> bool {
        true
    }
    fn process_with_notes(&mut self, _: &[f32], _: &[f32], l: &mut [f32], r: &mut [f32], notes: &[daw_engine::MidiEvent]) {
        l.fill(0.0);
        r.fill(0.0);
        for e in notes {
            if let MidiMsg::NoteOn { .. } = e.msg {
                l[e.time as usize] = 1.0;
            }
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

#[test]
fn notes_keep_their_exact_instant_when_the_block_is_split_for_automation() {
    let mut g = GraphDesc::new();
    let t = g.add_track("synthé", TrackFormat::Stereo);
    let factory: InsertFactory = std::sync::Arc::new(|_, _, _| Ok(Box::new(Clicker) as Box<dyn Insert>));
    let ins = g.add_insert(t, "clicker", factory).unwrap();
    let starts = [5u64, 31, 32, 33, 100, 777, 1_000, 1_023];
    let notes: Vec<_> = starts.iter().map(|&s| daw_engine::Note { start: s, len: 3, key: 60, velocity: 1.0, channel: 0 }).collect();
    g.add_midi_clip(t, daw_engine::MidiClip::new(0, 2_000, std::sync::Arc::new(notes)));
    g.set_automation(t, AutoLane::new(AutoTarget::InsertParam { insert: ins, param: 0 }, vec![AutoPoint::new(0, 0.0), AutoPoint::new(1_000, 1.0)]));
    for (block, eb) in [(1, 1), (7, 64), (100, 256), (333, 256), (2_048, 2_048)] {
        let o = render_block(&g, 1_100, block, eb).left;
        let found: Vec<u64> = (0..o.len()).filter(|&i| o[i] != 0.0).map(|i| i as u64).collect();
        assert_eq!(found, starts, "bloc {block}/{eb}");
    }
}

#[test]
fn invalid_lanes_are_refused() {
    let mut g = GraphDesc::new();
    let t = dc_track(&mut g, 100);
    let compile = |g: &GraphDesc| g.compile(SR, 64).err();
    let with = |lane: AutoLane| {
        let mut h = g.clone();
        h.set_automation(t, lane);
        compile(&h)
    };
    assert_eq!(with(AutoLane::new(AutoTarget::Gain, vec![])), Some(GraphError::BadAutomation(t)));
    assert_eq!(with(AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(5, 1.0), AutoPoint::new(5, 0.5)])), Some(GraphError::BadAutomation(t)));
    assert_eq!(with(AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(9, 1.0), AutoPoint::new(5, 0.5)])), Some(GraphError::BadAutomation(t)));
    assert_eq!(with(AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(0, f64::NAN)])), Some(GraphError::BadAutomation(t)));
    assert_eq!(with(AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(0, -1.0)])), Some(GraphError::BadAutomation(t)));
    assert_eq!(with(AutoLane::new(AutoTarget::InsertParam { insert: 42, param: 0 }, vec![AutoPoint::new(0, 1.0)])), Some(GraphError::BadAutomation(t)));
    assert_eq!(with(AutoLane::new(AutoTarget::Pan, vec![AutoPoint::new(0, 0.5)])), None);
    // Remplacer la courbe d'une même cible ne crée pas de doublon.
    g.set_automation(t, AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(0, 1.0)]));
    g.set_automation(t, AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(0, 0.5)]));
    assert_eq!(g.nodes[t.0 as usize].automation.len(), 1);
    assert_eq!(compile(&g), None);
}
