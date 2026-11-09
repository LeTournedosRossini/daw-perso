//! Chaîne latérale d'un plugin CLAP : le second port d'entrée audio reçoit le signal d'une autre piste.
//! Le « Ducker de test » coupe sa sortie à tout échantillon où la chaîne latérale dépasse 0,5.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle, Plugin};
use daw_engine::{AudioData, Clip, Command, GraphDesc, NodeParams, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_test_plugin::{entry, SIDECHAIN_ID};

const SR: u32 = 48_000;

fn clip(v: Vec<f32>) -> Clip {
    Clip::whole(AudioData::new(SR, vec![v.clone(), v]).unwrap(), 0)
}

fn graph(with_sidechain: bool) -> GraphDesc {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let mut g = GraphDesc::new();
    let a = g.add_track("déclencheur", TrackFormat::Stereo);
    let b = g.add_track("ducké", TrackFormat::Stereo);
    let trash = g.add_bus("poubelle");
    g.set_params(trash, NodeParams { mute: true, ..NodeParams::default() });
    g.set_output(a, trash);
    let mut pulse = vec![0.0f32; 6_000];
    pulse[2_000..2_100].fill(1.0);
    g.add_clip(a, clip(pulse));
    g.add_clip(b, clip(vec![0.3f32; 6_000]));
    let id = g.add_insert(b, "Ducker de test", clap_insert_factory(bundle, SIDECHAIN_ID.to_str().unwrap().into())).unwrap();
    if with_sidechain {
        g.node_mut(b).unwrap().inserts.iter_mut().find(|i| i.id == id).unwrap().sidechain = Some(a);
    }
    g
}

fn render_out(g: GraphDesc, block: usize) -> Vec<f32> {
    let mut spec = RenderSpec::new(SR, 5_000, g);
    spec.block = block;
    spec.events.push((0, Command::Play));
    render(&spec).unwrap().left
}

#[test]
fn the_plugin_declares_a_side_chain_port_and_the_host_feeds_it() {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let p = Plugin::instantiate(&bundle, SIDECHAIN_ID.to_str().unwrap()).unwrap();
    assert!(p.has_sidechain());
    assert_eq!(p.audio_ports().inputs, vec![2, 2], "un port principal et un port de chaîne latérale");
    // Le gain de test ordinaire n'en a pas.
    let plain = Plugin::instantiate(&bundle, "com.daw-perso.test-gain").unwrap();
    assert!(!plain.has_sidechain());
    for block in [64usize, 100, 256] {
        let free = render_out(graph(false), block);
        assert!(free[500..4_500].iter().all(|&v| (v - 0.3).abs() < 1e-4), "sans chaîne latérale, aucune coupure (bloc {block})");
        let ducked = render_out(graph(true), block);
        let zeros: Vec<usize> = (500..4_500).filter(|&i| ducked[i].abs() < 1e-6).collect();
        assert_eq!(zeros.len(), 100, "exactement les 100 échantillons de l'impulsion sont coupés (bloc {block}) : {}", zeros.len());
        assert_eq!(zeros[99] - zeros[0], 99, "d'un seul tenant");
        assert!(ducked[500..4_500].iter().filter(|v| v.abs() > 1e-6).all(|&v| (v - 0.3).abs() < 1e-4), "le reste est intact");
    }
}

#[test]
fn the_gap_does_not_depend_on_the_block_size() {
    let a = render_out(graph(true), 64);
    for block in [100usize, 256, 333] {
        assert_eq!(render_out(graph(true), block), a, "bloc {block}");
    }
}
