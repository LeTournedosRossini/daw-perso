//! Modes des voies d'automation : lecture (la courbe commande), trim (le réglage manuel s'ajoute), off (courbe ignorée), niveaux d'envoi.

use daw_core_types::NodeId;
use daw_engine::{AudioData, AutoLane, AutoMode, AutoPoint, AutoTarget, Clip, Command, Engine, GraphDesc, NodeParams, TrackFormat};

const SR: u32 = 48_000;

/// Une piste stéréo qui joue 1,0 constant ; fader à `gain`, courbe optionnelle.
fn level(gain: f32, lane: Option<AutoLane>, tweak: impl FnOnce(&mut daw_engine::EngineHandle, u32)) -> (f32, f32) {
    let data = AudioData::new(SR, vec![vec![1.0f32; 40_000], vec![1.0f32; 40_000]]).unwrap();
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(data, 0));
    g.set_params(t, NodeParams { gain, ..NodeParams::default() });
    if let Some(l) = lane {
        g.set_automation(t, l);
    }
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    engine.process(&mut [0.0; 8], &mut [0.0; 8]);
    handle.send(Command::Play);
    tweak(&mut handle, t.0);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    for _ in 0..40 {
        engine.process(&mut l, &mut r);
    }
    (l[63], r[63])
}

fn gain_lane(v: f64, mode: AutoMode) -> AutoLane {
    AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(0, v)]).with_mode(mode)
}

#[test]
fn read_mode_ignores_the_fader_trim_mode_multiplies_it_and_off_ignores_the_curve() {
    // Courbe à 0,5 ; fader à 0,5 ; la piste est stéréo, le panoramique centré vaut 1 (loi « balance »).
    let (read, _) = level(0.5, Some(gain_lane(0.5, AutoMode::Read)), |_, _| {});
    assert!((read - 0.5).abs() < 1e-6, "lecture : la courbe seule ({read})");
    let (trim, _) = level(0.5, Some(gain_lane(0.5, AutoMode::Trim)), |_, _| {});
    assert!((trim - 0.25).abs() < 1e-6, "trim : courbe × fader ({trim})");
    let (off, _) = level(0.5, Some(gain_lane(0.5, AutoMode::Off)), |_, _| {});
    assert!((off - 0.5).abs() < 1e-6, "off : le fader seul ({off})");
    let (off2, _) = level(0.8, Some(gain_lane(0.5, AutoMode::Off)), |_, _| {});
    assert!((off2 - 0.8).abs() < 1e-6, "off : la courbe n'est pas lue");
}

#[test]
fn in_trim_mode_moving_the_fader_changes_the_level_while_in_read_mode_it_does_nothing() {
    let (trim, _) = level(1.0, Some(gain_lane(0.5, AutoMode::Trim)), |h, t| {
        h.send(Command::SetGain { node: t, gain: 0.5 });
    });
    assert!((trim - 0.25).abs() < 1e-6, "trim : le geste du fader agit ({trim})");
    let (read, _) = level(1.0, Some(gain_lane(0.5, AutoMode::Read)), |h, t| {
        h.send(Command::SetGain { node: t, gain: 0.1 });
    });
    assert!((read - 0.5).abs() < 1e-6, "lecture : le geste est sans effet ({read})");
}

#[test]
fn trim_on_a_moving_curve_scales_it_everywhere() {
    // Courbe de 0 à 1 sur 1 000 échantillons, fader à 0,5 : à l'échantillon 500 la valeur est 0,25 en trim, 0,5 en lecture.
    let lane = |mode| AutoLane::new(AutoTarget::Gain, vec![AutoPoint::new(0, 0.0), AutoPoint::new(1_000, 1.0)]).with_mode(mode);
    let render = |mode: AutoMode| {
        let data = AudioData::new(SR, vec![vec![1.0f32; 4_000], vec![1.0f32; 4_000]]).unwrap();
        let mut g = GraphDesc::new();
        let t = g.add_track("t", TrackFormat::Stereo);
        g.add_clip(t, Clip::whole(data, 0));
        g.set_params(t, NodeParams { gain: 0.5, ..NodeParams::default() });
        g.set_automation(t, lane(mode));
        let (mut e, mut h) = Engine::new(SR, 64);
        h.load_graph(&g).unwrap();
        e.process(&mut [0.0; 8], &mut [0.0; 8]);
        h.send(Command::Play);
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        e.process(&mut l, &mut r);
        l[499]
    };
    let (read, trim) = (render(AutoMode::Read), render(AutoMode::Trim));
    assert!((read - 0.4995).abs() < 2e-3, "{read}");
    assert!((trim - read * 0.5).abs() < 1e-4, "{trim} vs {read}");
}

#[test]
fn pan_trim_adds_to_the_curve() {
    let data = AudioData::new(SR, vec![vec![1.0f32; 8_000]]).unwrap();
    let render = |mode: AutoMode, pan: f32| {
        let mut g = GraphDesc::new();
        let t = g.add_track("t", TrackFormat::Mono);
        g.add_clip(t, Clip::whole(data.clone(), 0));
        g.set_params(t, NodeParams { pan, ..NodeParams::default() });
        g.set_automation(t, AutoLane::new(AutoTarget::Pan, vec![AutoPoint::new(0, -1.0)]).with_mode(mode));
        let (mut e, mut h) = Engine::new(SR, 64);
        h.load_graph(&g).unwrap();
        e.process(&mut [0.0; 8], &mut [0.0; 8]);
        h.send(Command::Play);
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        for _ in 0..40 {
            e.process(&mut l, &mut r);
        }
        (l[63], r[63])
    };
    let (l, r) = render(AutoMode::Read, 0.7);
    assert!(l > 0.99 && r < 1e-6, "lecture : tout à gauche malgré le réglage manuel ({l}, {r})");
    let (l, r) = render(AutoMode::Trim, 1.0);
    assert!((l - r).abs() < 1e-3, "trim : −1 + 1 = centre ({l}, {r})");
}

#[test]
fn a_send_level_curve_drives_the_send_and_trim_multiplies_the_manual_level() {
    // La piste joue 1,0 par sa sortie ordinaire ; la contribution de l'envoi est ce qui s'ajoute à ce 1,0.
    let build = |mode: AutoMode, manual: f32| {
        let data = AudioData::new(SR, vec![vec![1.0f32; 8_000], vec![1.0f32; 8_000]]).unwrap();
        let mut g = GraphDesc::new();
        let t = g.add_track("t", TrackFormat::Stereo);
        g.add_clip(t, Clip::whole(data, 0));
        g.add_send(t, NodeId::MASTER, manual);
        g.set_automation(t, AutoLane::new(AutoTarget::SendGain { to: NodeId::MASTER }, vec![AutoPoint::new(0, 0.5)]).with_mode(mode));
        let (mut e, mut h) = Engine::new(SR, 64);
        h.load_graph(&g).unwrap();
        e.process(&mut [0.0; 8], &mut [0.0; 8]);
        h.send(Command::Play);
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        for _ in 0..40 {
            e.process(&mut l, &mut r);
        }
        l[63] - 1.0
    };
    assert!((build(AutoMode::Read, 1.0) - 0.5).abs() < 1e-6, "lecture : la courbe seule");
    assert!((build(AutoMode::Read, 0.2) - 0.5).abs() < 1e-6, "lecture : le niveau manuel est ignoré");
    assert!((build(AutoMode::Trim, 0.5) - 0.25).abs() < 1e-6, "trim : courbe × niveau manuel");
    assert!((build(AutoMode::Off, 0.3) - 0.3).abs() < 1e-6, "off : le niveau manuel seul");
}

#[test]
fn a_send_curve_without_its_send_is_refused() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.set_automation(t, AutoLane::new(AutoTarget::SendGain { to: NodeId::MASTER }, vec![AutoPoint::new(0, 1.0)]));
    assert!(g.compile(SR, 64).is_err(), "pas d'envoi vers cette destination");
    g.add_send(t, NodeId::MASTER, 1.0);
    assert!(g.compile(SR, 64).is_ok());
    g.node_mut(t).unwrap().automation[0].points[0].value = -1.0;
    assert!(g.compile(SR, 64).is_err(), "un niveau d'envoi négatif");
}
