//! Application de bureau : fenêtre Tauri qui affiche l'interface web (`crates/ui/web`) et la relie au vrai moteur.
//!
//! Trois fils : le fil principal (Tauri), le fil « moteur » (ouvre le pilote ASIO, héberge le contrôle : il lit les
//! commandes de la page toutes les ~10 ms et publie position et niveaux à ~30 Hz) et le fil audio du pilote (le callback
//! du moteur, qui ne touche à aucune de ces choses). La page et le moteur ne s'échangent que des messages JSON.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use daw_audio_io::playback::{ControlFn, StereoProcessor, StreamLatency};
use daw_engine::Engine;
use daw_ui::demo::demo_session_with_instruments;
use daw_ui::prefs::{self, Effective, Prefs, Runtime};
use daw_ui::{Event, Host};
#[cfg(target_os = "windows")]
mod win32_window;

use tauri::{AppHandle, Emitter, Manager, State};

const TICK: Duration = Duration::from_millis(33);
const EVENT: &str = "engine-event";

/// Journal minimal : `%TEMP%\daw-app.log` (l'application n'a pas de console sous Windows).
fn log(msg: &str) {
    let path = std::env::temp_dir().join("daw-app.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{msg}");
    }
}

struct CommandSender(Mutex<Sender<String>>);

/// La page a-t-elle pris en charge la fermeture de la fenêtre (confirmation si le projet a des modifications non
/// enregistrées) ? Tant qu'elle ne l'a pas dit, la fenêtre se ferme normalement : une page cassée ne doit pas empêcher de quitter.
struct CloseGuard(AtomicBool);

#[tauri::command]
fn close_guard(guard: State<'_, CloseGuard>, enabled: bool) {
    guard.0.store(enabled, Ordering::Relaxed);
}

/// Quitte l'application (la page l'appelle après avoir confirmé la fermeture).
#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Relance l'application (la page l'appelle après avoir confirmé, pour que les nouveaux réglages audio prennent effet).
#[tauri::command]
fn restart_app(app: AppHandle) {
    app.restart();
}

/// Récepteur des commandes, partagé entre le pilote audio et le moteur de secours.
type SharedRx = Arc<Mutex<Receiver<String>>>;

/// Commande de la page : un message JSON, transmis tel quel au fil moteur.
#[tauri::command]
fn send_command(tx: State<'_, CommandSender>, msg: serde_json::Value) {
    let _ = tx.0.lock().unwrap_or_else(|e| e.into_inner()).send(msg.to_string());
}

struct EngineProc(Engine);

impl StereoProcessor for EngineProc {
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        self.0.process_io(in_l, in_r, out_l, out_r);
    }

    /// Toutes les entrées ouvertes par le pilote : chaque piste armée écoute et enregistre l'entrée qu'on lui a choisie.
    fn process_multi(&mut self, inputs: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32]) {
        self.0.process_inputs(inputs, out_l, out_r);
    }

    /// Toutes les sorties ouvertes : la paire principale, puis les retours de monitoring des bus qui ont une sortie physique.
    fn process_outputs(&mut self, inputs: &[&[f32]], outs: &mut [&mut [f32]]) {
        self.0.process_outputs(inputs, outs);
    }
}

/// Où vont les messages MIDI des appareils : l'entrée du moteur et la liste des pistes MIDI armées (mise à jour par la pompe).
#[derive(Default)]
struct MidiRoute {
    tx: Option<daw_engine::MidiInputTx>,
    /// Pistes MIDI armées et leur filtre d'entrée (appareil, canaux).
    armed: Vec<(u32, Vec<daw_session::midi_filter::MidiRule>)>,
}

type SharedRoute = Arc<Mutex<MidiRoute>>;

/// Fait avancer l'hôte : commandes reçues, puis position et niveaux à ~30 Hz. Retourne `false` quand la page est partie.
struct Pump {
    host: Host,
    rx: SharedRx,
    app: AppHandle,
    last_tick: Instant,
    route: SharedRoute,
    last_armed: Vec<(u32, Vec<daw_session::midi_filter::MidiRule>)>,
    /// Latences du pilote : connues une fois le flux démarré ; on en déduit alors le décalage à retirer des prises.
    latency: Option<Arc<StreamLatency>>,
    latency_done: bool,
}

impl Pump {
    fn step(&mut self) -> bool {
        // Messages des fenêtres de plugins, minuteurs de leurs interfaces : à chaque tour (~10 ms), pas seulement au `tick` de 30 Hz.
        for e in self.host.pump_windows() {
            self.publish(&e);
        }
        loop {
            let received = self.rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv();
            match received {
                Ok(json) => {
                    for e in self.host.command_json(&json) {
                        self.publish(&e);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        // Les messages MIDI des appareils vont aux pistes MIDI armées.
        let armed = self.host.armed_midi_routes();
        if armed != self.last_armed {
            self.route.lock().unwrap_or_else(|e| e.into_inner()).armed = armed.clone();
            self.last_armed = armed;
        }
        if !self.latency_done {
            if let Some((input, output)) = self.latency.as_ref().and_then(|l| l.get()) {
                let comp = daw_audio_io::playback::record_compensation(input, output);
                self.host.set_record_compensation(comp);
                log(&format!("latence du pilote : {input} en entrée + {output} en sortie ; compensation des prises : {comp} échantillons"));
                self.latency_done = true;
            }
        }
        if self.last_tick.elapsed() >= TICK {
            self.last_tick = Instant::now();
            for e in self.host.tick() {
                self.publish(&e);
            }
        }
        true
    }

    fn publish(&self, e: &Event) {
        let _ = self.app.emit(EVENT, e);
    }
}

/// Données locales de l'utilisateur de l'application (`…/DAW`) : sauvegarde automatique, préférences.
fn data_dir() -> Option<std::path::PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
    }?;
    Some(base.join("DAW"))
}

/// Dossier où vit la sauvegarde automatique d'un projet sans titre (`…/DAW/autosave`).
fn autosave_root() -> Option<std::path::PathBuf> {
    data_dir().map(|d| d.join("autosave"))
}

/// Fichier des préférences (`…/DAW/preferences.json`).
fn prefs_path() -> Option<std::path::PathBuf> {
    data_dir().map(|d| d.join("preferences.json"))
}

/// Préférences lues une seule fois au démarrage (le moteur audio en dépend), avec l'éventuel message d'un fichier abîmé.
fn startup_prefs() -> &'static (Prefs, Option<String>) {
    static PREFS: std::sync::OnceLock<(Prefs, Option<String>)> = std::sync::OnceLock::new();
    PREFS.get_or_init(|| {
        // Premier démarrage (pas de fichier) : réglages recommandés selon le nombre de cœurs (avance de 50 ms à partir de 4 cœurs).
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
        let defaults = Prefs::recommended(cores);
        match prefs_path() {
            Some(path) => prefs::load(&path, &defaults),
            None => (defaults, None),
        }
    })
}

/// Réglages du moteur audio effectivement utilisés : variables d'environnement, sinon préférences, sinon valeurs par défaut.
fn effective() -> Effective {
    prefs::resolve(&startup_prefs().0, &|k| std::env::var(k).ok())
}

fn build_host(rate: u32, buffer: usize, inputs: u32, route: &SharedRoute, eff: &Effective, audio: bool) -> Result<(Engine, Host), String> {
    let (mut engine, mut handle) = Engine::new(rate, buffer.max(64));
    // Calcul des pistes en parallèle : réglage « fils de calcul » (ou `DAW_THREADS=N`), fil audio compris ; 1 = un seul cœur. Les
    // fils de travail tournent en attente active pendant la lecture et passent en priorité temps réel quand le système le permet.
    let threads = eff.threads as usize;
    if threads >= 2 {
        engine.set_pool(daw_engine::Pool::new(threads, Some(Arc::new(|_| {
            let _ = daw_audio_io::rt_priority::promote_current_thread();
        }))));
        log(&format!("calcul parallèle : {threads} fils (le fil audio + {} fils de travail)", threads - 1));
    }
    // Calcul en avance des pistes de lecture : réglage « avance » (ou `DAW_AHEAD=50` / `100` ms) ; 0 = désactivé. Les pistes armées et
    // les bus restent calculés dans le callback au plus petit tampon ; `ahead_threads` = fils de rendu.
    if eff.ahead_ms > 0 {
        let (ms, n) = (eff.ahead_ms, eff.ahead_threads as usize);
        engine.enable_ahead(&mut handle, &daw_engine::AheadConfig { lookahead_ms: ms.min(500), block: 256, threads: n.max(1), on_start: Some(Arc::new(|_| {
            let _ = daw_audio_io::rt_priority::promote_current_thread();
        })) });
        log(&format!("calcul en avance : {ms} ms, {} fil(s) de rendu", n.max(1)));
    }
    let (session, meta, note) = demo_session_with_instruments(rate);
    log(&note);
    let mut host = Host::new(handle, session, meta).map_err(|e| format!("graphe refusé : {e:?}"))?;
    #[cfg(target_os = "windows")]
    host.set_midi_devices(daw_audio_io::winmm_midi::list_inputs());
    host.set_name("Démonstration");
    host.set_autosave(autosave_root(), Duration::from_secs(60));
    host.set_audio_inputs(inputs);
    // Préférences (le métronome en reprend ses réglages) et description de ce qui tourne, pour la fenêtre des réglages.
    let (prefs, warning) = startup_prefs();
    if let Some(w) = warning {
        log(w);
    }
    host.set_preferences(prefs.clone(), prefs_path());
    host.set_runtime(Runtime {
        effective: eff.clone(),
        sample_rate: rate,
        inputs,
        drivers: daw_audio_io::asio_backend::list_drivers().into_iter().map(|d| d.name).collect(),
        cores: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
        audio,
    });
    // Interfaces natives des plugins : fenêtres Win32 pompées par ce fil (celui des plugins). Pas encore écrit pour macOS et Linux.
    #[cfg(target_os = "windows")]
    host.set_window_system(Box::new(win32_window::Win32System::new()));
    // L'entrée MIDI du moteur est confiée au routeur : les fils des appareils y déposent leurs messages.
    route.lock().unwrap_or_else(|e| e.into_inner()).tx = host.take_midi_input();
    Ok((engine, host))
}

/// Entrées MIDI de tous les appareils (Windows : WinMM). Chaque message va aux pistes MIDI armées, au début du prochain bloc.
#[cfg(target_os = "windows")]
fn open_midi_inputs(route: &SharedRoute) -> Vec<daw_audio_io::winmm_midi::MidiInput> {
    use daw_audio_io::winmm_midi::{list_inputs, MidiInput};
    let mut open = Vec::new();
    for (i, name) in list_inputs().iter().enumerate() {
        let r = Arc::clone(route);
        let device = name.clone();
        match MidiInput::open(i as u32, move |bytes: &[u8]| deliver_midi(&r, &device, bytes)) {
            Ok(input) => {
                log(&format!("entrée MIDI ouverte : {name}"));
                open.push(input);
            }
            Err(e) => log(&format!("entrée MIDI « {name} » indisponible : {e}")),
        }
    }
    open
}

#[cfg(target_os = "windows")]
fn deliver_midi(route: &SharedRoute, device: &str, bytes: &[u8]) {
    let Some(msg) = daw_engine::parse_midi(bytes) else { return };
    // Canal du message (0 à 15) : l'octet d'état d'un message de canal (0x80 à 0xEF).
    let channel = bytes.first().map_or(0, |b| b & 0x0F);
    // Fil WinMM (pas le fil audio) : un verrou court et une petite copie sont sans conséquence.
    let mut r = route.lock().unwrap_or_else(|e| e.into_inner());
    let armed = r.armed.clone();
    if let Some(tx) = r.tx.as_mut() {
        for (node, rules) in armed {
            // Le filtre d'entrée de la piste : une liste vide laisse tout passer.
            if daw_session::midi_filter::accepts(&rules, device, channel) {
                tx.push(node, msg, 0);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn run_with_driver(app: &AppHandle, rx: &SharedRx, route: &SharedRoute) -> Result<(), String> {
    use daw_audio_io::asio_backend::{self, PlayOptions};
    use daw_audio_io::playback::PlaybackReport;
    let eff = effective();
    let (driver, buffer) = (eff.driver.clone(), eff.buffer);
    // Une tentative : avec toutes les entrées du pilote (pour enregistrer), ou sans aucune entrée (repli).
    let attempt = |with_inputs: bool| -> Result<PlaybackReport, String> {
        let opts = PlayOptions { seconds: 24 * 3600, driver: driver.clone(), buffer: Some(buffer), sample_rate: None, output_channel: 1, input_channel: u32::from(with_inputs), input_count: if with_inputs { u32::MAX } else { 0 }, output_count: u32::MAX };
        log(&format!("ouverture du pilote ASIO « {driver} », tampon {buffer}, {}", if with_inputs { "toutes les entrées" } else { "sans entrée" }));
        let (app2, rx2, route2, eff2) = (app.clone(), Arc::clone(rx), Arc::clone(route), eff.clone());
        asio_backend::run_playback_ex(&opts, move |info| {
            log(&format!("pilote ouvert : {} Hz, tampon {}, {} entrée(s) audio, {} sortie(s) audio", info.sample_rate, info.buffer_size, info.inputs, info.outputs));
            let (engine, mut host) = build_host(info.sample_rate, buffer as usize, info.inputs, &route2, &eff2, true).expect("projet de démonstration valide");
            host.set_audio_outputs(info.outputs);
            let mut pump = Pump { host, rx: rx2, app: app2, last_tick: Instant::now(), route: route2, last_armed: Vec::new(), latency: Some(Arc::clone(&info.latency)), latency_done: false };
            let control: ControlFn = Box::new(move |_| pump.step());
            (EngineProc(engine), control)
        })
    };
    let report = match attempt(true) {
        Ok(r) => r,
        Err(e) => {
            log(&format!("ouverture avec les entrées impossible ({e}) : nouvel essai sans entrée"));
            attempt(false)?
        }
    };
    log(&format!("pilote arrêté : {} xruns signalés, allocations dans le callback : {}", report.xruns, report.callback_allocations));
    Ok(())
}

/// Sans pilote audio (autre plateforme, ou pilote indisponible) : le moteur tourne quand même, au rythme de l'horloge,
/// sans son. L'interface reste utilisable (position, niveaux, commandes).
fn run_silent(app: &AppHandle, rx: SharedRx, route: &SharedRoute) {
    let rate = 48_000;
    let block = 256usize;
    let eff = effective();
    let Ok((mut engine, host)) = build_host(rate, block, 0, route, &eff, false) else { return };
    let mut pump = Pump { host, rx, app: app.clone(), last_tick: Instant::now(), route: Arc::clone(route), last_armed: Vec::new(), latency: None, latency_done: true };
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let period = Duration::from_secs_f64(block as f64 / rate as f64);
    let mut next = Instant::now();
    loop {
        if !pump.step() {
            return;
        }
        engine.process(&mut l, &mut r);
        next += period;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        } else {
            next = Instant::now();
        }
    }
}

fn engine_thread(app: AppHandle, rx: SharedRx) {
    let route: SharedRoute = Arc::default();
    // Les entrées MIDI restent ouvertes tant que ce fil vit.
    #[cfg(target_os = "windows")]
    let _midi = open_midi_inputs(&route);
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = run_with_driver(&app, &rx, &route) {
            log(&format!("pilote ASIO indisponible : {e} — moteur sans son"));
            let _ = app.emit(EVENT, &Event::Error { command: "audio".into(), message: format!("pilote ASIO indisponible ({e}) : moteur sans son") });
            run_silent(&app, rx, &route);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        log("plateforme sans pilote branché : moteur sans son");
        run_silent(&app, rx, &route);
    }
}

fn main() {
    log("démarrage de l'application");
    let (tx, rx) = mpsc::channel::<String>();
    let rx: SharedRx = Arc::new(Mutex::new(rx));
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(CommandSender(Mutex::new(tx)))
        .manage(CloseGuard(AtomicBool::new(false)))
        .invoke_handler(tauri::generate_handler![send_command, close_guard, quit, restart_app])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.state::<CloseGuard>().0.load(Ordering::Relaxed) {
                    api.prevent_close();
                    let _ = window.emit("close-requested", ());
                }
            }
        })
        .setup(move |app| {
            log("fenêtre prête : démarrage du fil moteur");
            let handle = app.handle().clone();
            std::thread::Builder::new().name("moteur".into()).spawn(move || engine_thread(handle, rx)).expect("fil moteur");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'application");
}
