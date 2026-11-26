//! Essai de charge des interfaces X11 : ajoute `N` instances du même plugin (sur les pistes de la démonstration), ouvre leur interface
//! l'une après l'autre en les laissant ouvertes, puis ferme et rouvre la première. Sert à reproduire « l'interface reste noire après avoir
//! ouvert plusieurs fois le même plugin ». À lancer sur un serveur X d'essai (`Xephyr :9 -screen 1400x900 &`) :
//! `DISPLAY=:9 cargo run --release --example x11_plugin_stress -- Convolution 6 4`   (nom, nombre d'instances, secondes entre deux ouvertures)
#[cfg(target_os = "linux")]
#[path = "../src/x11_window.rs"]
mod x11_window;

#[cfg(target_os = "linux")]
fn main() {
    use std::time::{Duration, Instant};
    let a: Vec<String> = std::env::args().skip(1).collect();
    let want = a.first().cloned().unwrap_or_else(|| "Convolution".into());
    let count: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(4);
    let gap: u64 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
    let (_engine, handle) = daw_engine::Engine::new(48_000, 256);
    let (session, meta, _) = daw_ui::demo::demo_session_with_instruments(48_000);
    let mut host = daw_ui::Host::new(handle, session, meta).expect("hôte");
    host.set_window_system(Box::new(x11_window::X11System::new()));
    let mut chosen = None;
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
    let Some((clap_id, file)) = chosen else { return println!("plugin « {want} » introuvable") };
    let nodes = [1u32, 2, 3, 4, 5];
    let pump = |host: &mut daw_ui::Host, secs: u64| {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            for e in host.pump_windows() {
                println!("  événement : {}", serde_json::to_string(&e).unwrap_or_default());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let mut ids: Vec<u64> = Vec::new();
    for i in 0..count {
        let node = nodes[i % nodes.len()];
        let cmd = serde_json::json!({ "type": "add_plugin", "node": node, "clap_id": clap_id, "file": file }).to_string();
        for e in host.command_json(&cmd) {
            if let daw_ui::Event::Error { message, .. } = &e {
                println!("ajout refusé : {message}");
            }
        }
        // L'identifiant du plugin ajouté : le plus grand de la piste.
        for e in host.command_json(r#"{"type":"get_project"}"#) {
            let v = serde_json::to_value(&e).unwrap_or_default();
            for n in v["nodes"].as_array().into_iter().flatten() {
                for ins in n["inserts"].as_array().into_iter().flatten() {
                    if let Some(id) = ins["id"].as_u64() {
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                    }
                }
            }
        }
    }
    println!("{} instances : {ids:?}", ids.len());
    for (k, id) in ids.iter().enumerate() {
        println!("OUVERTURE {k} (insert {id})");
        for e in host.command_json(&format!(r#"{{"type":"open_plugin_ui","insert":{id}}}"#)) {
            println!("  → {}", serde_json::to_string(&e).unwrap_or_default());
        }
        pump(&mut host, gap);
    }
    println!("FERMETURE ET RÉOUVERTURE de la première");
    let first = ids[0];
    host.command_json(&format!(r#"{{"type":"close_plugin_ui","insert":{first}}}"#));
    pump(&mut host, 1);
    host.command_json(&format!(r#"{{"type":"open_plugin_ui","insert":{first}}}"#));
    pump(&mut host, gap);
    println!("FIN");
}
#[cfg(not(target_os = "linux"))]
fn main() {}
