//! Contrat hôte ↔ plugin, vérifié avec le plugin de test maison (comportement exactement connu).

use std::sync::Arc;

use daw_clap_host::{Bundle, Plugin, PluginError, ProcessStatus};
use daw_test_plugin::{entry, LATENCY, PARAM_GAIN, PLUGIN_ID};

fn bundle() -> Arc<Bundle> {
    Arc::new(Bundle::from_static_entry(entry()).unwrap())
}

fn started(bundle: &Arc<Bundle>, max: u32) -> Plugin {
    let mut p = Plugin::instantiate(bundle, PLUGIN_ID.to_str().unwrap()).unwrap();
    p.activate(48_000.0, max).unwrap();
    p.start_processing().unwrap();
    p
}

/// Fait traiter `input` (même signal à gauche et à droite) par blocs de `block`, avec des changements de paramètre datés
/// en échantillons absolus. Retourne la sortie gauche.
fn run(p: &mut Plugin, input: &[f32], block: usize, params: &[(usize, f64)]) -> Vec<f32> {
    let mut out = vec![0.0f32; input.len()];
    let mut scratch_r = vec![0.0f32; block];
    let mut pos = 0;
    while pos < input.len() {
        let n = block.min(input.len() - pos);
        for &(at, v) in params {
            if at >= pos && at < pos + n {
                assert!(p.queue_param(PARAM_GAIN, v, (at - pos) as u32));
            }
        }
        let status = p.process(&input[pos..pos + n], &input[pos..pos + n], &mut out[pos..pos + n], &mut scratch_r[..n]).unwrap();
        assert_eq!(status, ProcessStatus::Continue);
        pos += n;
    }
    out
}

#[test]
fn lists_the_plugin_descriptor() {
    let list = bundle().plugins().unwrap();
    // Trois plugins : le gain de test, sa variante à chaîne latérale et la variante à sorties multiples.
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].id, "com.daw-perso.test-gain");
    assert_eq!(list[0].name, "Gain de test");
    assert_eq!(list[0].features, vec!["audio-effect", "utility", "stereo"]);
    assert_eq!((list[1].id.as_str(), list[1].name.as_str()), ("com.daw-perso.test-sidechain", "Ducker de test"));
    assert_eq!((list[2].id.as_str(), list[2].name.as_str()), ("com.daw-perso.test-multiout", "Multi-sorties de test"));
}

#[test]
fn unknown_plugin_id_is_refused() {
    assert!(matches!(Plugin::instantiate(&bundle(), "n.existe.pas"), Err(PluginError::UnknownPlugin)));
}

#[test]
fn reports_ports_parameters_and_latency() {
    let b = bundle();
    let mut p = Plugin::instantiate(&b, "com.daw-perso.test-gain").unwrap();
    let ports = p.audio_ports();
    assert_eq!((ports.inputs.as_slice(), ports.outputs.as_slice()), (&[2u32][..], &[2u32][..]));
    let params = p.params();
    assert_eq!(params.len(), 2);
    assert_eq!((params[0].id, params[0].name.as_str(), params[0].min, params[0].max, params[0].default), (0, "Gain", 0.0, 2.0, 1.0));
    assert_eq!((params[1].id, params[1].max), (1, 192.0));
    assert_eq!(p.param_value(0), Some(1.0));
    assert_eq!(p.param_value(7), None);
    assert_eq!(p.param_text(0, 0.5).as_deref(), Some("0.500"));
    p.activate(48_000.0, 256).unwrap();
    assert_eq!(p.latency(), LATENCY);
}

#[test]
fn lifecycle_is_enforced() {
    let b = bundle();
    let mut p = Plugin::instantiate(&b, "com.daw-perso.test-gain").unwrap();
    let (mut l, mut r) = ([0.0f32; 8], [0.0f32; 8]);
    assert_eq!(p.start_processing(), Err(PluginError::State("pas activé")));
    assert!(matches!(p.process(&[0.0; 8], &[0.0; 8], &mut l, &mut r), Err(PluginError::State(_))));
    p.activate(48_000.0, 64).unwrap();
    assert_eq!(p.activate(48_000.0, 64), Err(PluginError::State("déjà activé")));
    p.start_processing().unwrap();
    assert!(p.process(&[0.0; 8], &[0.0; 8], &mut l, &mut r).is_ok());
    // Plus grand que le bloc maximal annoncé.
    let big = vec![0.0f32; 65];
    let (mut bl, mut br) = (vec![0.0f32; 65], vec![0.0f32; 65]);
    assert_eq!(p.process(&big, &big, &mut bl, &mut br), Err(PluginError::BadBlock));
    p.deactivate();
    p.activate(44_100.0, 32).unwrap(); // réactivable
}

#[test]
fn impulse_comes_out_after_the_declared_latency() {
    let b = bundle();
    let mut p = started(&b, 128);
    let mut input = vec![0.0f32; 400];
    input[0] = 1.0;
    let out = run(&mut p, &input, 100, &[]);
    for (i, s) in out.iter().enumerate() {
        assert_eq!(*s, if i == LATENCY as usize { 1.0 } else { 0.0 }, "échantillon {i}");
    }
}

#[test]
fn latency_is_independent_of_block_size() {
    let b = bundle();
    let input: Vec<f32> = (0..1000).map(|i| (i % 17) as f32 - 8.0).collect();
    let reference = run(&mut started(&b, 1024), &input, 1000, &[]);
    for block in [1usize, 7, 64, 333] {
        assert_eq!(run(&mut started(&b, 1024), &input, block, &[]), reference, "bloc {block}");
    }
    assert_eq!(&reference[LATENCY as usize..], &input[..input.len() - LATENCY as usize]);
}

#[test]
fn parameter_events_apply_at_the_exact_sample() {
    let b = bundle();
    let mut p = started(&b, 128);
    let input = vec![1.0f32; 300];
    // Gain 0,5 à l'échantillon 100 (bloc de 128 : dans le premier bloc), gain 0 à 200 (dans le deuxième).
    let out = run(&mut p, &input, 128, &[(100, 0.5), (200, 0.0)]);
    for (i, s) in out.iter().enumerate() {
        let expected = if i < LATENCY as usize || i >= 200 { 0.0 } else if i < 100 { 1.0 } else { 0.5 };
        assert_eq!(*s, expected, "échantillon {i}");
    }
    assert_eq!(p.param_value(0), Some(0.0));
}

#[test]
fn out_of_range_and_nan_parameter_values_are_contained() {
    let b = bundle();
    let mut p = started(&b, 16);
    let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
    p.queue_param(PARAM_GAIN, 99.0, 0);
    p.process(&[1.0; 16], &[1.0; 16], &mut l, &mut r).unwrap();
    assert_eq!(p.param_value(0), Some(2.0));
    p.queue_param(PARAM_GAIN, f64::NAN, 0);
    p.queue_param(12345, 1.0, 0); // paramètre inconnu : ignoré
    p.process(&[1.0; 16], &[1.0; 16], &mut l, &mut r).unwrap();
    assert_eq!(p.param_value(0), Some(2.0));
}

#[test]
fn event_queue_overflow_is_counted_not_fatal() {
    let b = bundle();
    let mut p = started(&b, 16);
    let accepted = (0..600).filter(|_| p.queue_param(PARAM_GAIN, 1.0, 0)).count();
    assert_eq!(accepted, 512);
    assert_eq!(p.dropped_events(), 88);
    let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
    p.process(&[1.0; 16], &[1.0; 16], &mut l, &mut r).unwrap();
    assert!(p.queue_param(PARAM_GAIN, 1.0, 0), "la file est vidée après chaque bloc");
}

#[test]
fn state_round_trips_between_instances() {
    let b = bundle();
    let mut a = started(&b, 16);
    let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
    a.queue_param(PARAM_GAIN, 0.25, 0);
    a.process(&[1.0; 16], &[1.0; 16], &mut l, &mut r).unwrap();
    let bytes = a.save_state().unwrap();
    assert_eq!(bytes.len(), daw_test_plugin::STATE_LEN);

    let mut c = Plugin::instantiate(&b, "com.daw-perso.test-gain").unwrap();
    assert_eq!(c.param_value(0), Some(1.0));
    c.load_state(&bytes).unwrap();
    assert_eq!(c.param_value(0), Some(0.25));
}

#[test]
fn bad_state_is_rejected_without_changing_the_plugin() {
    let b = bundle();
    let mut p = Plugin::instantiate(&b, "com.daw-perso.test-gain").unwrap();
    for bad in [&b""[..], &b"DTP1"[..], &b"DTP2\0\0\0\0\0\0\xf0\x3f"[..], &b"XXXX\0\0\0\0\0\0\xf0\x3f\0\0\0\0"[..]] {
        assert!(p.load_state(bad).is_err(), "{bad:?}");
    }
    let mut nan = b"DTP2".to_vec();
    nan.extend_from_slice(&f64::NAN.to_le_bytes());
    nan.extend_from_slice(&0u32.to_le_bytes());
    assert!(p.load_state(&nan).is_err());
    assert_eq!(p.param_value(0), Some(1.0));
}

#[test]
fn instances_are_independent() {
    let b = bundle();
    let (mut x, mut y) = (started(&b, 16), started(&b, 16));
    let (mut l, mut r) = ([0.0f32; 16], [0.0f32; 16]);
    x.queue_param(PARAM_GAIN, 0.1, 0);
    x.process(&[1.0; 16], &[1.0; 16], &mut l, &mut r).unwrap();
    assert_eq!(x.param_value(0), Some(0.1));
    assert_eq!(y.param_value(0), Some(1.0));
    drop(x); // détruire l'une ne touche pas l'autre
    y.process(&[1.0; 16], &[1.0; 16], &mut l, &mut r).unwrap();
}
