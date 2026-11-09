//! Liste, pour chaque plugin CLAP installé, les interfaces graphiques qu'il annonce (extension `gui`) : `cargo run --example gui_probe`.
//! Sert à décider comment héberger les fenêtres de plugins ; n'active ni ne traite rien.

use std::sync::Arc;

use daw_clap_host::{default_search_dirs, find_plugins, Bundle, Plugin};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let files: Vec<_> = if args.is_empty() { default_search_dirs().iter().flat_map(|d| find_plugins(d)).collect() } else { args.iter().map(Into::into).collect() };
    for f in files {
        let Ok(bundle) = Bundle::open(&f) else { println!("{} : ne se charge pas", f.display()); continue };
        let bundle = Arc::new(bundle);
        let Ok(list) = bundle.plugins() else { continue };
        for info in list {
            match Plugin::instantiate(&bundle, &info.id) {
                Ok(p) => {
                    let g = p.gui_support();
                    let apis: Vec<String> = g.apis.iter().map(|(a, fl)| format!("{a}{}", if *fl { " flottante" } else { " intégrée" })).collect();
                    println!("{:<28} {:<22} gui={:<5} préféré={:?}  api: {}", info.name, f.file_name().unwrap_or_default().to_string_lossy(), g.has_gui, g.preferred, if apis.is_empty() { "—".into() } else { apis.join(", ") });
                }
                Err(e) => println!("{} : instanciation impossible ({e:?})", info.name),
            }
        }
    }
}
