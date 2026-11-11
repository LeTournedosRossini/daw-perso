//! `daw-headless` : le moteur et l'hôte du DAW **sans fenêtre ni carte son**, pilotés par des lignes JSON.
//!
//! Entrée standard : une commande du protocole de l'interface par ligne (voir `docs/protocole-ui.md`), avec un champ facultatif `"id"` renvoyé
//! dans l'accusé de réception. Quelques commandes sont propres à cet outil :
//! - `{"type":"advance","frames":N}` : fait tourner le moteur de N échantillons (hors temps réel, par blocs), l'hôte étant relevé tous les ~10 ms ;
//!   envoie ensuite la position et les crêtes (`meters`) du dernier relevé ;
//! - `{"type":"wait_for","event":"export","timeout_ms":60000}` : relève l'hôte jusqu'à l'événement demandé (pour un événement qui porte
//!   `active`, jusqu'à ce qu'il vaille faux) — import, export, étirement, calage… tournent sur des fils ;
//! - `{"type":"quit"}`.
//! Sortie standard : un événement JSON par ligne, puis `{"type":"ack","id":…,"error":null}` quand la ligne est traitée.
//! Les événements de position et de niveaux ne sont envoyés que par `advance` (ou tous, avec `--all`).
//!
//! Options : `--rate 48000`, `--block 64`, `--demo` (part du projet de démonstration au lieu d'un projet vide), `--all`.

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

struct Opts {
    rate: u32,
    block: usize,
    demo: bool,
    all: bool,
}

fn parse_args() -> Result<Opts, String> {
    let mut o = Opts { rate: 48_000, block: 64, demo: false, all: false };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rate" => o.rate = args.next().and_then(|v| v.parse().ok()).filter(|r| (8_000..=192_000).contains(r)).ok_or("--rate : 8000 à 192000")?,
            "--block" => o.block = args.next().and_then(|v| v.parse().ok()).filter(|b| (1..=4096).contains(b)).ok_or("--block : 1 à 4096")?,
            "--demo" => o.demo = true,
            "--all" => o.all = true,
            "-h" | "--help" => return Err("usage : daw-headless [--rate 48000] [--block 64] [--demo] [--all]".into()),
            other => return Err(format!("option inconnue : {other}")),
        }
    }
    Ok(o)
}

struct Out<W: Write> {
    w: W,
    all: bool,
}

impl<W: Write> Out<W> {
    fn event(&mut self, e: &Event, force: bool) {
        if !force && !self.all && matches!(e, Event::Position { .. } | Event::Meters { .. }) {
            return;
        }
        if let Ok(line) = serde_json::to_string(e) {
            let _ = writeln!(self.w, "{line}");
        }
    }

    fn events(&mut self, es: &[Event]) {
        for e in es {
            self.event(e, false);
        }
    }

    fn ack(&mut self, id: &serde_json::Value, error: Option<String>) {
        let line = serde_json::json!({ "type": "ack", "id": id, "error": error });
        let _ = writeln!(self.w, "{line}");
        let _ = self.w.flush();
    }
}

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(m) => {
            eprintln!("{m}");
            std::process::exit(2);
        }
    };
    let (mut engine, handle) = Engine::new(opts.rate, opts.block);
    let (session, meta) = demo_session(opts.rate);
    let mut host = match Host::new(handle, session, meta) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("erreur : {e:?}");
            std::process::exit(1);
        }
    };
    let mut out = Out { w: std::io::BufWriter::new(std::io::stdout().lock()), all: opts.all };
    if !opts.demo {
        out.events(&host.command_json(r#"{"type":"new_project"}"#));
    }
    out.ack(&serde_json::Value::Null, None); // prêt
    let (mut l, mut r) = (vec![0f32; opts.block], vec![0f32; opts.block]);

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                out.ack(&serde_json::Value::Null, Some(format!("JSON invalide : {e}")));
                continue;
            }
        };
        let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
        match value.get("type").and_then(|t| t.as_str()) {
            Some("quit") => {
                out.ack(&id, None);
                break;
            }
            Some("advance") => {
                let Some(frames) = value.get("frames").and_then(|f| f.as_u64()).filter(|&f| f <= 3_600 * u64::from(opts.rate)) else {
                    out.ack(&id, Some("advance : frames (entier, une heure au plus)".into()));
                    continue;
                };
                let tick_every = (u64::from(opts.rate) / 100).max(1);
                let (mut done, mut since_tick) = (0u64, 0u64);
                let mut last: Vec<Event> = Vec::new();
                while done < frames {
                    let n = (frames - done).min(opts.block as u64) as usize;
                    engine.process(&mut l[..n], &mut r[..n]);
                    done += n as u64;
                    since_tick += n as u64;
                    if since_tick >= tick_every {
                        since_tick = 0;
                        last = host.tick();
                        out.events(&last);
                    }
                }
                last.extend(host.tick());
                for e in last.iter().filter(|e| matches!(e, Event::Position { .. } | Event::Meters { .. })) {
                    out.event(e, true);
                }
                out.ack(&id, None);
            }
            Some("wait_for") => {
                let (Some(want), timeout) = (value.get("event").and_then(|e| e.as_str()), value.get("timeout_ms").and_then(|t| t.as_u64()).unwrap_or(60_000)) else {
                    out.ack(&id, Some("wait_for : event (nom de l'événement)".into()));
                    continue;
                };
                let start = Instant::now();
                let mut found = false;
                while !found && start.elapsed() < Duration::from_millis(timeout) {
                    let es = host.tick();
                    for e in &es {
                        if let Ok(v) = serde_json::to_value(e) {
                            let over = v.get("type").and_then(|t| t.as_str()) == Some(want) && v.get("active").and_then(|a| a.as_bool()) != Some(true);
                            found |= over;
                        }
                    }
                    out.events(&es);
                    if !found {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                out.ack(&id, if found { None } else { Some(format!("wait_for : « {want} » n'est pas arrivé en {timeout} ms")) });
            }
            _ => {
                let es = host.command_json(&line);
                out.events(&es);
                // Une commande refusée est dite dans l'accusé comme dans un événement `error`.
                let err = es.iter().find_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None });
                out.ack(&id, err);
            }
        }
    }
    let _ = out.w.flush();
}
