use daw_core_types::time::Frac;
use daw_core_types::NodeId;
use daw_engine::{AutoCurve, AutoTarget, Clip, AudioData, GraphDesc, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_session::automation_rec::thin;
use daw_session::{MLane, MPoint, Session};

const SR: u32 = 48_000;

fn f(n: u64, d: u64) -> Frac {
    Frac::new(n, d).unwrap()
}

fn session() -> (Session, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![vec![1.0; 400_000], vec![1.0; 400_000]]).unwrap(), 0));
    (Session::new(g), t)
}

#[test]
fn thinning_keeps_corners_and_respects_the_tolerance() {
    // Une droite devient deux points.
    let line: Vec<(u64, f64)> = (0..100).map(|i| (i * 10, i as f64 * 0.01)).collect();
    assert_eq!(thin(&line, 1e-9), vec![(0, 0.0), (990, 0.99)]);
    // Un coin est conservé.
    let corner: Vec<(u64, f64)> = (0..=100).map(|i| (i * 10, if i <= 50 { i as f64 } else { 100.0 - i as f64 })).collect();
    let t = thin(&corner, 0.01);
    assert_eq!(t, vec![(0, 0.0), (500, 50.0), (1000, 0.0)]);
    // Courbe lisse : l'écart avec l'original ne dépasse jamais la tolérance, et on gagne beaucoup de points.
    let sine: Vec<(u64, f64)> = (0..2_000).map(|i| (i * 5, (i as f64 * 0.01).sin())).collect();
    let tol = 0.01;
    let t = thin(&sine, tol);
    assert!(t.len() < sine.len() / 5, "{} points gardés", t.len());
    for &(x, v) in &sine {
        let i = t.partition_point(|p| p.0 <= x);
        let (a, b) = (t[i.saturating_sub(1)], t[i.min(t.len() - 1)]);
        let line = if b.0 == a.0 { a.1 } else { a.1 + (b.1 - a.1) * ((x - a.0) as f64 / (b.0 - a.0) as f64) };
        assert!((line - v).abs() <= tol + 1e-12, "écart {} à {x}", (line - v).abs());
    }
    // Cas limites : vide, un point, deux points, deux gestes au même instant (le dernier gagne).
    assert!(thin(&[], 0.1).is_empty());
    assert_eq!(thin(&[(5, 1.0)], 0.1), vec![(5, 1.0)]);
    assert_eq!(thin(&[(1, 1.0), (1, 2.0), (3, 0.0)], 0.1), vec![(1, 2.0), (3, 0.0)]);
}

#[test]
fn gestures_become_a_lane_on_the_tick_grid_and_drive_the_engine() {
    let (mut s, t) = session();
    // Fader tiré de 0 à 1 en une seconde (48 000 échantillons) par paliers de 480 ; 1 tick = 25 échantillons.
    let rec: Vec<(u64, f64)> = (0..=100).map(|i| (i * 480, i as f64 / 100.0)).collect();
    s.write_automation(t, AutoTarget::Gain, &rec, 0, 48_000, 0.001).unwrap();
    let lane = s.lane(t, AutoTarget::Gain).unwrap();
    assert_eq!(lane.points.len(), 2, "une droite : deux points");
    assert_eq!((lane.points[0].pos, lane.points[1].pos), (Frac::ZERO, f(1, 2)));
    assert_eq!((lane.points[0].value, lane.points[1].value), (0.0, 1.0));
    let out = render(&RenderSpec::new(SR, 50_000, s.graph().clone())).unwrap().left;
    assert_eq!(out[24_000], 0.5);
    assert_eq!(out[49_000], 1.0);
    // Une seule action annulable.
    assert_eq!(s.undo_label(), Some("Modifier l'automation"));
    s.undo();
    assert!(s.lane(t, AutoTarget::Gain).is_none());
}

#[test]
fn punch_in_replaces_only_the_recorded_zone_and_blends_back_into_the_old_curve() {
    let (mut s, t) = session();
    let old = MLane {
        target: AutoTarget::Gain,
        points: vec![
            MPoint { pos: Frac::ZERO, value: 1.0, curve: AutoCurve::Linear },
            MPoint { pos: f(1, 1), value: 1.0, curve: AutoCurve::Linear },
            MPoint { pos: f(2, 1), value: 0.0, curve: AutoCurve::Linear },
        ], mode: daw_engine::AutoMode::Read,
    };
    s.set_lane(t, old.clone()).unwrap();
    // 120 bpm : une ronde = 96 000 échantillons. On enregistre de 1/4 de ronde (24 000) à 3/4 (72 000), mais le geste
    // s'arrête à 1/2 (48 000).
    let rec = [(24_000, 0.2), (30_000, 0.4), (48_000, 0.8)];
    s.write_automation(t, AutoTarget::Gain, &rec, 24_000, 72_000, 1e-9).unwrap();
    let l = s.lane(t, AutoTarget::Gain).unwrap();
    let pts: Vec<(Frac, f64)> = l.points.iter().map(|p| (p.pos, p.value)).collect();
    // Avant la zone : le point à 0 est conservé. Dans la zone : les gestes. À la sortie : retour à la valeur d'origine
    // (1,0 à 3/4 de ronde). Après la zone : les points d'origine à 1 et 2 rondes.
    assert_eq!(pts.first(), Some(&(Frac::ZERO, 1.0)));
    assert!(pts.contains(&(f(1, 4), 0.2)) && pts.contains(&(f(1, 2), 0.8)), "{pts:?}");
    assert!(pts.contains(&(f(3, 4), 1.0)), "valeur d'origine à la sortie : {pts:?}");
    assert!(pts.contains(&(f(1, 1), 1.0)));
    assert_eq!(pts.last(), Some(&(f(2, 1), 0.0)));
    assert!(pts.windows(2).all(|w| w[0].0 < w[1].0), "positions croissantes : {pts:?}");
    // Annulation exacte.
    s.undo();
    assert_eq!(s.lane(t, AutoTarget::Gain), Some(&old));
}

#[test]
fn gestures_outside_the_zone_or_none_change_nothing() {
    let (mut s, t) = session();
    s.write_automation(t, AutoTarget::Pan, &[], 0, 1_000, 0.01).unwrap();
    s.write_automation(t, AutoTarget::Pan, &[(5_000, 0.5)], 0, 1_000, 0.01).unwrap(); // hors zone
    s.write_automation(t, AutoTarget::Pan, &[(10, f64::NAN)], 0, 1_000, 0.01).unwrap(); // non fini
    s.write_automation(t, AutoTarget::Pan, &[(10, 0.5)], 1_000, 1_000, 0.01).unwrap(); // zone vide
    assert!(s.lane(t, AutoTarget::Pan).is_none() && !s.can_undo());
    assert!(s.write_automation(NodeId(9), AutoTarget::Pan, &[(10, 0.5)], 0, 1_000, 0.01).is_err());
}

#[test]
fn recorded_automation_follows_the_tempo_afterwards() {
    let (mut s, t) = session();
    let rec = [(0, 0.0), (48_000, 1.0)];
    s.write_automation(t, AutoTarget::Gain, &rec, 0, 48_000, 0.001).unwrap();
    s.set_tempo(Frac::ZERO, daw_core_types::time::Tempo::bpm(240).unwrap()).unwrap();
    let pts = |s: &Session| s.graph().nodes[1].automation[0].points.iter().map(|p| p.time).collect::<Vec<_>>();
    assert_eq!(pts(&s), vec![0, 24_000]);
}
