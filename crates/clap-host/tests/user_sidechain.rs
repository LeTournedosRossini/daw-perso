//! Les plugins de Jean qui ont un second port d'entrée (chaîne latérale) la reçoivent sans erreur. Ignoré s'ils ne sont pas sur la machine.
//! Ce test dépend de la machine : il ne touche à aucun plugin tiers.

use std::sync::Arc;

use daw_clap_host::{default_search_dirs, find_plugins, Bundle, Plugin, ProcessStatus};

const VENDOR: &str = "Le Tournedos Rossini";

fn noise(seed: u32, n: usize, amp: f32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 8) as f32 / 16_777_216.0 - 0.5) * 2.0 * amp
        })
        .collect()
}

#[test]
fn users_plugins_with_a_side_chain_port_accept_a_signal_on_it() {
    let files: Vec<_> = default_search_dirs().iter().flat_map(|d| find_plugins(d)).collect();
    let mut tested = 0;
    for file in files {
        let Ok(bundle) = Bundle::open(&file) else { continue };
        let bundle = Arc::new(bundle);
        let Ok(list) = bundle.plugins() else { continue };
        for info in list.into_iter().filter(|i| i.vendor == VENDOR) {
            let mut p = Plugin::instantiate(&bundle, &info.id).unwrap();
            p.activate(48_000.0, 256).unwrap();
            p.start_processing().unwrap();
            if !p.has_sidechain() {
                continue;
            }
            let ports = p.audio_ports();
            // Un signal doux sur l'entrée principale, un signal fort sur la chaîne latérale ; puis sans chaîne latérale.
            let (main, loud) = (noise(1, 256, 0.05), noise(2, 256, 0.9));
            let run = |p: &mut Plugin, sc: Option<(&[f32], &[f32])>| -> Vec<f32> {
                let mut out = Vec::new();
                for _ in 0..300 {
                    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
                    let status = p.process_with_sidechain(&main, &main, sc, &mut l, &mut r).unwrap();
                    assert_ne!(status, ProcessStatus::Error, "{} : erreur de traitement avec la chaîne latérale", info.name);
                    assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{} : sortie non finie", info.name);
                    out = l;
                }
                out
            };
            let without = run(&mut p, None);
            let with = run(&mut p, Some((&loud, &loud)));
            let differs = without.iter().zip(&with).any(|(a, b)| (a - b).abs() > 1e-6);
            let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
            eprintln!("{:<12} entrées {:?} : chaîne latérale reçue sans erreur ; sortie différente de « sans » : {differs} (rms {:.4} → {:.4})", info.name, ports.inputs, rms(&without), rms(&with));
            tested += 1;
        }
    }
    eprintln!("{tested} plugin(s) de Jean avec chaîne latérale essayé(s)");
}
