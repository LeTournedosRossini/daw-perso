//! Un plugin CLAP dans le graphe du moteur : latence annoncée → compensation → alignement sur la ligne de temps.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, default_search_dirs, find_plugins, Bundle};
use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, Command, GraphDesc, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_test_plugin::{entry, LATENCY, PARAM_GAIN};

const SR: u32 = 48_000;

fn test_bundle() -> Arc<Bundle> {
    Arc::new(Bundle::from_static_entry(entry()).unwrap())
}

fn impulse_clip(len: usize) -> Clip {
    let mut x = vec![0.0f32; len];
    x[0] = 1.0;
    Clip::whole(AudioData::new(SR, vec![x.clone(), x]).unwrap(), 0)
}

#[test]
fn clap_plugin_latency_is_compensated_against_a_dry_track() {
    let mut g = GraphDesc::new();
    let wet = g.add_track("avec plugin", TrackFormat::Stereo);
    let dry = g.add_track("sec", TrackFormat::Stereo);
    g.add_clip(wet, impulse_clip(512));
    g.add_clip(dry, impulse_clip(512));
    g.add_insert(wet, "Gain de test", clap_insert_factory(test_bundle(), "com.daw-perso.test-gain".into())).unwrap();
    assert_eq!(g.compile(SR, 256).unwrap().latency(), LATENCY);

    // Rendu compensé : les deux impulsions sont à l'échantillon 0 de la ligne de temps.
    let out = render(&RenderSpec::new(SR, 512, g.clone())).unwrap();
    assert_eq!(out.left[0], 2.0);
    assert!(out.left[1..].iter().all(|&v| v == 0.0));

    // Sans compensation : la piste sèche reste en avance de 64 échantillons.
    g.pdc = false;
    let mut raw = RenderSpec::new(SR, 512, g);
    raw.compensate_latency = false;
    let out = render(&raw).unwrap();
    assert_eq!((out.left[0], out.left[LATENCY as usize]), (1.0, 1.0));
}

#[test]
fn plugin_parameters_are_reachable_through_engine_commands() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, impulse_clip(512));
    let id = g.add_insert(t, "Gain de test", clap_insert_factory(test_bundle(), "com.daw-perso.test-gain".into())).unwrap();
    let mut spec = RenderSpec::new(SR, 512, g);
    spec.events.push((0, Command::SetInsertParam { insert: id, param: PARAM_GAIN, value: 0.5 }));
    let out = render(&spec).unwrap();
    assert_eq!(out.left[0], 0.5);
}

#[test]
fn unknown_plugin_makes_the_graph_fail_cleanly() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_insert(t, "x", clap_insert_factory(test_bundle(), "n.existe.pas".into())).unwrap();
    assert!(matches!(g.compile(SR, 64), Err(daw_engine::GraphError::InsertFailed { .. })));
}

/// Plugins de Jean seulement : Digue annonce 480 échantillons de latence (anticipation du limiteur). On vérifie que
/// le signal, une fois compensé, est vraiment aligné sur le même signal non traité (corrélation maximale au retard 0).
#[test]
fn users_digue_limiter_is_really_aligned_by_its_declared_latency() {
    let Some((bundle, id)) = find_users_plugin("Digue") else {
        eprintln!("Digue introuvable sur cette machine : essai ignoré");
        return;
    };
    // Bruit faible (très en dessous d'un seuil de limiteur) : le limiteur doit être quasi transparent.
    let mut s = 99u32;
    let noise: Vec<f32> = (0..20_000)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 8) as f32 / 16_777_216.0 - 0.5) * 0.1
        })
        .collect();
    let data = AudioData::new(SR, vec![noise.clone(), noise.clone()]).unwrap();

    let build = |with_plugin: bool| {
        let mut g = GraphDesc::new();
        let t = g.add_track("t", TrackFormat::Stereo);
        g.add_clip(t, Clip::whole(data.clone(), 0));
        if with_plugin {
            g.add_insert(t, "Digue", clap_insert_factory(Arc::clone(&bundle), id.clone())).unwrap();
        }
        g
    };
    let latency = build(true).compile(SR, 256).unwrap().latency();
    let with = render(&RenderSpec::new(SR, 20_000, build(true))).unwrap();
    let without = render(&RenderSpec::new(SR, 20_000, build(false))).unwrap();
    let _ = NodeId::MASTER;

    let corr = |lag: i32| -> f64 {
        (2_000..18_000usize).map(|i| with.left[(i as i32 + lag) as usize] as f64 * without.left[i] as f64).sum()
    };
    let best = (-600..=600).max_by(|&a, &b| corr(a).partial_cmp(&corr(b)).unwrap()).unwrap();
    eprintln!("Digue : latence annoncée {latency}, décalage résiduel mesuré après compensation {best} échantillon(s)");
    assert_eq!(best, 0, "la latence annoncée ({latency}) ne correspond pas au retard réel (décalage résiduel {best})");
}

fn find_users_plugin(name: &str) -> Option<(Arc<Bundle>, String)> {
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(b) = Bundle::open(&file) else { continue };
            let Ok(list) = b.plugins() else { continue };
            if let Some(info) = list.into_iter().find(|i| i.vendor == "Le Tournedos Rossini" && i.name == name) {
                return Some((Arc::new(b), info.id));
            }
        }
    }
    None
}
