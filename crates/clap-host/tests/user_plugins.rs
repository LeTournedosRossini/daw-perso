//! Essai de fumée sur les plugins de Jean (éditeur « Le Tournedos Rossini ») trouvés sur la machine.
//! Ignoré silencieusement s'il n'y en a aucun : ce test dépend de la machine, pas du dépôt.
//! On ne touche volontairement à aucun plugin tiers.

use std::sync::Arc;

use daw_clap_host::{default_search_dirs, find_plugins, Bundle, Plugin, ProcessStatus};

const VENDOR: &str = "Le Tournedos Rossini";

#[test]
fn users_own_plugins_instantiate_and_process_cleanly() {
    let files: Vec<_> = default_search_dirs().iter().flat_map(|d| find_plugins(d)).collect();
    let mut tested = 0;
    for file in files {
        let Ok(bundle) = Bundle::open(&file) else { continue };
        let bundle = Arc::new(bundle);
        let Ok(list) = bundle.plugins() else { continue };
        for info in list.into_iter().filter(|i| i.vendor == VENDOR) {
            let mut p = Plugin::instantiate(&bundle, &info.id).unwrap_or_else(|e| panic!("{} : instanciation : {e}", info.name));
            p.activate(48_000.0, 256).unwrap_or_else(|e| panic!("{} : activation : {e}", info.name));
            p.start_processing().unwrap_or_else(|e| panic!("{} : démarrage : {e}", info.name));
            let ports = p.audio_ports();
            let params = p.params();
            eprintln!(
                "{:<12} entrées {:?} sorties {:?}  latence {:>5}  {:>3} paramètres  {}",
                info.name,
                ports.inputs,
                ports.outputs,
                p.latency(),
                params.len(),
                params.iter().take(4).map(|q| q.name.as_str()).collect::<Vec<_>>().join(", ")
            );
            // Bruit reproductible, quelques changements de paramètres au milieu.
            let mut s = 12345u32;
            let mut noise = || {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / 16_777_216.0 - 0.5
            };
            let (mut peak, mut errors) = (0.0f32, 0);
            for block in 0..200 {
                let input: Vec<f32> = (0..256).map(|_| noise()).collect();
                let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
                if block % 40 == 20 {
                    for q in params.iter().filter(|q| q.flags & 0x20 != 0).take(8) {
                        p.queue_param(q.id, q.min + (q.max - q.min) * 0.3, 17);
                    }
                }
                match p.process(&input, &input, &mut l, &mut r).unwrap() {
                    ProcessStatus::Error => errors += 1,
                    _ => {}
                }
                assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{} : sortie non finie au bloc {block}", info.name);
                peak = l.iter().chain(&r).fold(peak, |m, v| m.max(v.abs()));
            }
            assert_eq!(errors, 0, "{} : le traitement a signalé une erreur", info.name);
            // Sauvegarde / restauration de l'état si le plugin le propose.
            if let Ok(state) = p.save_state() {
                let mut q = Plugin::instantiate(&bundle, &info.id).unwrap();
                q.load_state(&state).unwrap_or_else(|e| panic!("{} : rechargement de son propre état : {e}", info.name));
            }
            eprintln!("             crête de sortie {peak:.3}");
            tested += 1;
        }
    }
    eprintln!("{tested} plugin(s) de Jean testé(s)");
}
