//! Poignée « fil principal » : état d'un plugin en cours de traitement, redémarrage demandé par le plugin (changement
//! de latence), durée de vie du plugin.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle, PluginHandle};
use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, InsertId, TrackFormat};
use daw_test_plugin::{entry, LATENCY, MAX_EXTRA_LATENCY, PARAM_EXTRA_LATENCY, PARAM_GAIN};

const SR: u32 = 48_000;

fn bundle() -> Arc<Bundle> {
    Arc::new(Bundle::from_static_entry(entry()).unwrap())
}

fn impulse_clip(len: usize) -> Clip {
    let mut x = vec![0.0f32; len];
    x[0] = 1.0;
    Clip::whole(AudioData::new(SR, vec![x.clone(), x]).unwrap(), 0)
}

/// Piste avec plugin + piste sèche, une impulsion chacune. Retourne le graphe et l'identifiant du plugin.
fn wet_and_dry() -> (GraphDesc, InsertId) {
    let mut g = GraphDesc::new();
    let wet = g.add_track("avec plugin", TrackFormat::Stereo);
    let dry = g.add_track("sec", TrackFormat::Stereo);
    g.add_clip(wet, impulse_clip(1_000));
    g.add_clip(dry, impulse_clip(1_000));
    let id = g.add_insert(wet, "Gain de test", clap_insert_factory(bundle(), "com.daw-perso.test-gain".into())).unwrap();
    (g, id)
}

fn plugin_handle(h: &daw_engine::EngineHandle, id: InsertId) -> PluginHandle {
    let c = h.insert_control(id).expect("poignée de contrôle");
    c.as_any().downcast_ref::<PluginHandle>().expect("c'est un plugin CLAP").clone()
}

fn run(engine: &mut Engine, frames: usize) -> Vec<f32> {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    engine.process(&mut l, &mut r);
    l
}

#[test]
fn state_of_a_running_plugin_is_saved_and_restored_through_the_engine() {
    let (g, id) = wet_and_dry();
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    run(&mut engine, 64); // le graphe est pris en compte
    handle.send(Command::SetInsertParam { insert: id, param: PARAM_GAIN, value: 0.25 });
    run(&mut engine, 64); // le plugin reçoit l'événement dans le callback

    let control = handle.insert_control(id).unwrap();
    let state = control.save_state().expect("le plugin sauvegarde son état");
    assert_eq!(state.len(), daw_test_plugin::STATE_LEN);
    assert_eq!(&state[..4], b"DTP2");
    assert_eq!(f64::from_le_bytes(state[4..12].try_into().unwrap()), 0.25, "l'état reflète ce que le callback a reçu");

    let ph = plugin_handle(&handle, id);
    assert_eq!(ph.param_value(PARAM_GAIN), Some(0.25));
    assert!(!control.load_state(b"n'importe quoi"), "un état invalide est refusé");
    assert_eq!(ph.param_value(PARAM_GAIN), Some(0.25), "et ne change rien");
}

#[test]
fn a_restart_requested_by_the_plugin_recomputes_latency_and_keeps_its_state() {
    let (g, id) = wet_and_dry();
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    run(&mut engine, 64);
    assert_eq!(handle.latency_samples(), LATENCY);
    assert_eq!(handle.poll_restarts(&g), Ok(0), "rien à relancer au départ");

    // Le plugin reçoit un nouveau réglage de latence : il la change à sa prochaine activation et demande un redémarrage.
    handle.send(Command::SetInsertParam { insert: id, param: PARAM_GAIN, value: 0.5 });
    handle.send(Command::SetInsertParam { insert: id, param: PARAM_EXTRA_LATENCY, value: 100.0 });
    run(&mut engine, 64);
    let ph = plugin_handle(&handle, id);
    assert!(ph.restart_requested(), "le plugin a appelé request_restart");
    assert_eq!(ph.latency(), LATENCY, "tant qu'il n'est pas relancé, sa latence active ne change pas");

    assert_eq!(handle.poll_restarts(&g), Ok(1));
    assert_eq!(handle.latency_samples(), LATENCY + 100, "la compensation est recalculée");
    run(&mut engine, 64); // le nouveau graphe, avec la nouvelle instance, est échangé
    assert_eq!(handle.poll_restarts(&g), Ok(0), "la nouvelle instance n'a rien demandé");

    // La nouvelle instance a repris le gain (état restauré) et sa nouvelle latence.
    let ph2 = plugin_handle(&handle, id);
    assert_eq!(ph2.param_value(PARAM_GAIN), Some(0.5));
    assert_eq!(ph2.latency(), LATENCY + 100);

    // Et le son est aligné : on rejoue depuis le début, les deux impulsions tombent ensemble à la latence totale.
    handle.send(Command::Seek(0));
    let out = run(&mut engine, 400);
    let hits: Vec<(usize, f32)> = out.iter().copied().enumerate().filter(|(_, v)| v.abs() > 1e-6).collect();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].0, (LATENCY + 100) as usize);
    assert!((hits[0].1 - 1.5).abs() < 1e-6, "piste sèche (1,0) + piste du plugin au gain restauré (0,5) : {}", hits[0].1);
}

#[test]
fn the_plugin_outlives_its_graph_while_a_handle_exists() {
    let (g, id) = wet_and_dry();
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    run(&mut engine, 64);
    let ph = plugin_handle(&handle, id);

    // Un graphe sans le plugin remplace l'ancien ; l'ancien (avec l'instance côté audio) revient au contrôle et est libéré.
    handle.load_graph(&GraphDesc::new()).unwrap();
    run(&mut engine, 64);
    assert_eq!(handle.collect_garbage(), 2);
    assert!(handle.insert_control(id).is_none());

    // La poignée garde le plugin en vie : on peut encore l'interroger sans accès invalide.
    assert_eq!(ph.params().len(), 2);
    assert_eq!(ph.save_state().unwrap().len(), daw_test_plugin::STATE_LEN);
    drop(ph); // dernier propriétaire : destruction propre du plugin
}

#[test]
fn extra_latency_is_clamped_and_reported_in_parameters() {
    let (g, id) = wet_and_dry();
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    run(&mut engine, 64);
    handle.send(Command::SetInsertParam { insert: id, param: PARAM_EXTRA_LATENCY, value: 10_000.0 });
    run(&mut engine, 64);
    assert_eq!(plugin_handle(&handle, id).param_value(PARAM_EXTRA_LATENCY), Some(MAX_EXTRA_LATENCY as f64));
}
