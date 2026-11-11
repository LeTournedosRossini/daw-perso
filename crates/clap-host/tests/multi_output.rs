//! Sorties multiples d'un plugin CLAP : chaque port de sortie auxiliaire est lu séparément et routé vers un bus.
//! Le « Multi-sorties de test » écrit le signal (retardé de 64 échantillons) sur trois ports, à 1, 1/2 et 1/4.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle, Plugin};
use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, Command, GraphDesc, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_test_plugin::{entry, MULTIOUT_ID};

const SR: u32 = 48_000;
const LAT: usize = 64;

/// `aux[k]` : `Some(0)` = bus « sortie 2 », `Some(1)` = bus « sortie 3 », `None` = port non branché.
fn graph(aux: &[Option<usize>]) -> (GraphDesc, NodeId) {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let mut g = GraphDesc::new();
    let t = g.add_track("piste", TrackFormat::Stereo);
    let (b1, b2) = (g.add_bus("sortie 2"), g.add_bus("sortie 3"));
    // Une impulsion à l'échantillon 1 000 et un échelon de 0,2 dès 3 000.
    let mut x = vec![0.0f32; 6_000];
    x[1_000] = 1.0;
    x[3_000..].fill(0.2);
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![x.clone(), x]).unwrap(), 0));
    let id = g.add_insert(t, "Multi-sorties de test", clap_insert_factory(bundle, MULTIOUT_ID.to_str().unwrap().into())).unwrap();
    g.node_mut(t).unwrap().inserts.iter_mut().find(|i| i.id == id).unwrap().aux = aux.iter().map(|a| a.map(|k| [b1, b2][k])).collect();
    (g, t)
}

fn out(g: GraphDesc, block: usize) -> Vec<f32> {
    let mut spec = RenderSpec::new(SR, 5_000, g);
    spec.block = block;
    spec.compensate_latency = false;
    spec.events.push((0, Command::Play));
    render(&spec).unwrap().left
}

#[test]
fn the_plugin_declares_three_stereo_output_ports_and_the_host_activates_them() {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let p = Plugin::instantiate(&bundle, MULTIOUT_ID.to_str().unwrap()).unwrap();
    let ports = p.audio_ports();
    assert_eq!(ports.outputs, vec![2, 2, 2]);
    assert_eq!(ports.output_names, vec!["Principal", "Sortie 2", "Sortie 3"]);
}

#[test]
fn unrouted_extra_ports_add_nothing_and_routed_ones_arrive_aligned_at_their_levels() {
    // Sans route : seul le premier port est entendu (impulsion de 1 à 1 000 + 64, échelon de 0,2 dès 3 064).
    let (g, _) = graph(&[]);
    let plain = out(g, 64);
    assert!((plain[1_000 + LAT] - 1.0).abs() < 1e-5 && (plain[4_000] - 0.2).abs() < 1e-5);
    // Port 2 → bus « sortie 2 » (×0,5), port 3 → bus « sortie 3 » (×0,25), les deux vers la sortie principale : 1 + 0,5 + 0,25.
    for block in [64usize, 100, 256, 333] {
        let (g, _) = graph(&[Some(0), Some(1)]);
        let mix = out(g, block);
        assert!((mix[1_000 + LAT] - 1.75).abs() < 1e-5, "impulsion : {} (bloc {block})", mix[1_000 + LAT]);
        assert!((mix[4_000] - 0.35).abs() < 1e-5, "échelon : {} (bloc {block})", mix[4_000]);
        assert!(mix[1_000 + LAT - 1].abs() < 1e-6 && mix[1_000 + LAT + 1].abs() < 1e-6, "les trois ports arrivent au même échantillon");
    }
    // Un seul port routé (le troisième) : 1 + 0,25.
    let (g, _) = graph(&[None, Some(1)]);
    let one = out(g, 64);
    assert!((one[1_000 + LAT] - 1.25).abs() < 1e-5, "{}", one[1_000 + LAT]);
}

#[test]
fn the_result_does_not_depend_on_the_block_size() {
    let (g, _) = graph(&[Some(0), Some(1)]);
    let a = out(g, 64);
    for block in [100usize, 256, 333] {
        let (g, _) = graph(&[Some(0), Some(1)]);
        assert_eq!(out(g, block), a, "bloc {block}");
    }
}

#[test]
fn an_aux_route_must_target_a_bus_and_not_create_a_loop() {
    let (mut g, track) = graph(&[Some(0)]);
    assert!(g.validate().is_ok());
    // Vers la piste elle-même : refusé.
    g.node_mut(track).unwrap().inserts[0].aux = vec![Some(track)];
    assert!(g.validate().is_err(), "vers elle-même");
}

#[test]
fn a_call_longer_than_the_promised_block_does_not_panic_with_aux_ports() {
    // Jumelle du calcul en avance : blocs de 256 pour un plugin activé avec 64 au plus (régression : panic « range end index 128 »).
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let factory = clap_insert_factory(bundle, MULTIOUT_ID.to_str().unwrap().into());
    let mut fx = factory(SR, 64, None).unwrap();
    let input = vec![0.5f32; 256];
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    for _ in 0..4 {
        fx.process(&input, &input, &mut l, &mut r);
    }
    assert!(l.iter().all(|v| v.is_finite()));
    assert!(l[128..].iter().any(|v| *v != 0.0), "la sortie principale est traitée sur tout le bloc");
}
