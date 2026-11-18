//! Essai des fenêtres X11 des interfaces de plugins, sans l'application : ajoute le premier plugin de Jean sur « Claviers » et ouvre son
//! interface pendant N secondes (pompe des fenêtres toutes les 10 ms). À lancer sur un serveur X d'essai :
//! `Xephyr :9 -screen 1000x700 &` puis `DISPLAY=:9 cargo run --example x11_plugin_ui -- [identifiant-du-plugin|nom] [secondes]`.
#[cfg(target_os = "linux")]
#[path = "../src/x11_window.rs"]
mod x11_window;

#[cfg(target_os = "linux")]
fn main() {
    use std::time::{Duration, Instant};
    let a: Vec<String> = std::env::args().skip(1).collect();
    let seconds: u64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(6);
    let rate = 48_000;
    let (_engine, handle) = daw_engine::Engine::new(rate, 256);
    let (session, meta, note) = daw_ui::demo::demo_session_with_instruments(rate);
    println!("{note}");
    let mut host = daw_ui::Host::new(handle, session, meta).expect("hôte");
    host.set_window_system(Box::new(x11_window::X11System::new()));
    // Un nom de plugin donné en argument : il est ajouté sur la piste « Basse » (nœud 2) avant d'ouvrir son interface.
    if let Some(want) = a.first().filter(|w| !w.is_empty() && w.as_str() != "-") {
        let mut chosen: Option<(String, String)> = None;
        for dir in daw_clap_host::default_search_dirs() {
            for file in daw_clap_host::find_plugins(&dir) {
                let Ok(b) = daw_clap_host::Bundle::open(&file) else { continue };
                for info in b.plugins().unwrap_or_default() {
                    if chosen.is_none() && info.name.to_lowercase().contains(&want.to_lowercase()) {
                        chosen = Some((info.id.clone(), file.to_string_lossy().into_owned()));
                    }
                }
            }
        }
        let Some((clap_id, file)) = chosen else {
            println!("plugin « {want} » introuvable");
            return;
        };
        let cmd = serde_json::json!({ "type": "add_plugin", "node": 2, "clap_id": clap_id, "file": file }).to_string();
        for e in host.command_json(&cmd) {
            if let daw_ui::Event::Error { message, .. } = &e {
                println!("ajout refusé : {message}");
            }
        }
    }
    let find = |host: &mut daw_ui::Host| -> Option<(u64, String)> {
        let evs = host.command_json(r#"{"type":"get_project"}"#);
        for e in evs {
            let v = serde_json::to_value(&e).unwrap_or_default();
            for n in v["nodes"].as_array().into_iter().flatten() {
                for i in n["inserts"].as_array().into_iter().flatten().rev() {
                    if i["gui"].as_bool() == Some(true) && (a.first().is_none_or(|w| w == "-" || w.is_empty()) || n["id"].as_u64() == Some(2)) {
                        return Some((i["id"].as_u64()?, i["name"].as_str().unwrap_or("").to_string()));
                    }
                }
            }
        }
        None
    };
    let Some((id, name)) = find(&mut host) else {
        println!("aucun plugin avec interface sur les pistes de la démonstration");
        return;
    };
    println!("ouverture de l'interface de « {name} » (insert {id})");
    let evs = host.command_json(&format!(r#"{{"type":"open_plugin_ui","insert":{id}}}"#));
    for e in &evs {
        println!("  → {}", serde_json::to_string(e).unwrap_or_default());
    }
    let end = Instant::now() + Duration::from_secs(seconds);
    let mut last_print = Instant::now();
    while Instant::now() < end {
        for e in host.pump_windows() {
            println!("  événement : {}", serde_json::to_string(&e).unwrap_or_default());
        }
        if last_print.elapsed() > Duration::from_secs(2) {
            println!("  … toujours ouverte");
            last_print = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let evs = host.command_json(&format!(r#"{{"type":"close_plugin_ui","insert":{id}}}"#));
    for e in &evs {
        println!("  fermeture → {}", serde_json::to_string(e).unwrap_or_default());
    }
    for e in host.pump_windows() {
        println!("  événement : {}", serde_json::to_string(&e).unwrap_or_default());
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {}
