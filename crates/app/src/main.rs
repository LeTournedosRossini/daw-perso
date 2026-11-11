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

#[cfg(target_os = "windows")]
use daw_audio_io::playback::ControlFn;
use daw_audio_io::playback::{StereoProcessor, StreamLatency};
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

/// Dossier des scripts de l'utilisateur (`…/DAW/scripts`).
fn scripts_dir() -> Result<std::path::PathBuf, String> {
    data_dir().map(|d| d.join("scripts")).ok_or_else(|| "dossier de données introuvable".to_string())
}

/// Taille maximale d'un script (en octets) : un script est un petit texte.
const SCRIPT_MAX_BYTES: usize = 256 * 1024;

/// Liste les scripts `*.js` du dossier de l'utilisateur (200 au plus, triés par nom).
#[tauri::command]
fn scripts_list() -> Result<serde_json::Value, String> {
    let dir = scripts_dir()?;
    let existed = dir.is_dir();
    let mut scripts = Vec::new();
    if existed {
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| format!("lecture du dossier des scripts : {e}"))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "js"))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        for file in names.into_iter().take(200) {
            if let Ok(source) = std::fs::read_to_string(dir.join(&file)) {
                if source.len() <= SCRIPT_MAX_BYTES {
                    scripts.push(serde_json::json!({ "file": file, "source": source }));
                }
            }
        }
    }
    // `existed` : le dossier existait-il déjà ? (Sinon la page y dépose les exemples.)
    Ok(serde_json::json!({ "dir": dir.to_string_lossy(), "existed": existed, "scripts": scripts }))
}

/// Écrit un script dans le dossier des scripts. Le nom est un simple nom de fichier `.js` (lettres, chiffres, espace, `_`, `-`, `.`) :
/// jamais de chemin, donc rien ne s'écrit hors du dossier. Écrasement seulement si `overwrite`.
#[tauri::command]
fn script_save(file: String, source: String, overwrite: bool) -> Result<(), String> {
    let ok_name = !file.is_empty()
        && file.len() <= 80
        && file.ends_with(".js")
        && !file.starts_with('.')
        && file.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'))
        && !file.contains("..");
    if !ok_name {
        return Err("nom de script invalide (lettres, chiffres, espace, _ - . ; extension .js)".into());
    }
    if source.len() > SCRIPT_MAX_BYTES {
        return Err("script trop long (256 Ko au plus)".into());
    }
    let dir = scripts_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("création du dossier des scripts : {e}"))?;
    let path = dir.join(&file);
    if path.exists() && !overwrite {
        return Err("ce script existe déjà".into());
    }
    std::fs::write(&path, source).map_err(|e| format!("écriture du script : {e}"))
}

// ---- fichiers des scripts (lot 4) ---------------------------------------------------------------------------------------------
//
// Ces commandes sont des primitives : la page décide quelles racines un script a le droit de toucher (dossier du projet, dossier des
// scripts, fichiers et dossiers que l'utilisateur a choisis dans un dialogue) et les passe à chaque appel ; ici on vérifie encore,
// sur les chemins réels (`canonicalize` : `..` et liens symboliques ne sortent pas des racines).

/// Taille maximale d'un fichier texte lu ou écrit par un script (1 Mo).
const SCRIPT_FS_MAX: u64 = 1024 * 1024;

/// Extensions qu'un script ne peut pas écrire (programmes, scripts, plugins).
const SCRIPT_FS_DENIED_EXT: &[&str] = &["exe", "dll", "bat", "cmd", "com", "msi", "scr", "ps1", "vbs", "lnk", "sh", "app", "so", "dylib", "clap", "js"];

/// Chemin réel de `p` : `canonicalize` s'il existe (ou si `must_exist`), sinon le chemin réel de son dossier suivi de son nom (un fichier
/// que « Enregistrer sous » vient de désigner n'existe pas encore).
fn script_fs_real(p: &std::path::Path, must_exist: bool) -> Result<std::path::PathBuf, String> {
    if must_exist || p.exists() {
        return p.canonicalize().map_err(|e| format!("chemin introuvable : {e}"));
    }
    let parent = p.parent().ok_or_else(|| "chemin invalide".to_string())?;
    let name = p.file_name().ok_or_else(|| "nom de fichier invalide".to_string())?;
    Ok(parent.canonicalize().map_err(|e| format!("dossier introuvable : {e}"))?.join(name))
}

/// Chemin réel de `path` s'il est dans l'une des `roots` (elles-mêmes prises au réel, même si le fichier désigné n'existe pas encore).
/// `must_exist` faux : le fichier peut ne pas exister encore, c'est alors son dossier qui est vérifié.
fn script_fs_resolve(path: &str, roots: &[String], must_exist: bool) -> Result<std::path::PathBuf, String> {
    if roots.is_empty() {
        return Err("aucun dossier autorisé pour ce script".into());
    }
    let real = script_fs_real(std::path::Path::new(path), must_exist)?;
    let allowed = roots.iter().filter_map(|r| script_fs_real(std::path::Path::new(r), false).ok()).any(|r| real.starts_with(&r));
    if allowed {
        Ok(real)
    } else {
        Err("ce script n'a pas le droit d'accéder à ce chemin (choisis-le avec un dialogue, ou utilise le dossier du projet)".into())
    }
}

/// Lit un fichier texte (UTF-8, 1 Mo au plus).
#[tauri::command]
fn script_fs_read(path: String, roots: Vec<String>) -> Result<String, String> {
    let real = script_fs_resolve(&path, &roots, true)?;
    let meta = std::fs::metadata(&real).map_err(|e| format!("lecture : {e}"))?;
    if !meta.is_file() {
        return Err("ce n'est pas un fichier".into());
    }
    if meta.len() > SCRIPT_FS_MAX {
        return Err("fichier trop gros (1 Mo au plus)".into());
    }
    let bytes = std::fs::read(&real).map_err(|e| format!("lecture : {e}"))?;
    String::from_utf8(bytes).map_err(|_| "ce fichier n'est pas du texte UTF-8".to_string())
}

/// Écrit un fichier texte. Sans `overwrite`, un fichier qui existe déjà est refusé.
#[tauri::command]
fn script_fs_write(path: String, text: String, roots: Vec<String>, overwrite: bool) -> Result<(), String> {
    let real = script_fs_resolve(&path, &roots, false)?;
    if text.len() as u64 > SCRIPT_FS_MAX {
        return Err("texte trop long (1 Mo au plus)".into());
    }
    let ext = real.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if SCRIPT_FS_DENIED_EXT.contains(&ext.as_str()) {
        return Err(format!("un script ne peut pas écrire de fichier « .{ext} »"));
    }
    if real.is_dir() {
        return Err("c'est un dossier".into());
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true);
    if overwrite {
        opts.create(true).truncate(true);
    } else {
        opts.create_new(true);
    }
    use std::io::Write as _;
    let mut f = opts.open(&real).map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { "ce fichier existe déjà".to_string() } else { format!("écriture : {e}") })?;
    f.write_all(text.as_bytes()).map_err(|e| format!("écriture : {e}"))
}

/// Liste un dossier (500 entrées au plus, triées, sans descendre dans les sous-dossiers).
#[tauri::command]
fn script_fs_list(dir: String, roots: Vec<String>) -> Result<serde_json::Value, String> {
    let real = script_fs_resolve(&dir, &roots, true)?;
    if !real.is_dir() {
        return Err("ce n'est pas un dossier".into());
    }
    let mut entries: Vec<(String, bool, u64)> = std::fs::read_dir(&real)
        .map_err(|e| format!("lecture du dossier : {e}"))?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((e.file_name().into_string().ok()?, meta.is_dir(), meta.len()))
        })
        .collect();
    entries.sort();
    entries.truncate(500);
    Ok(serde_json::Value::Array(entries.into_iter().map(|(name, is_dir, size)| serde_json::json!({ "name": name, "is_dir": is_dir, "size": size })).collect()))
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
                self.host.set_driver_latency(input, output);
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

/// Banque personnelle de pages de contrôle des plugins (`…/DAW/control-pages.json`), valable pour tous les projets.
fn control_pages_path() -> Option<std::path::PathBuf> {
    data_dir().map(|d| d.join("control-pages.json"))
}

/// Cache du balayage des plugins installés (`…/DAW/plugin-cache.json`).
fn plugin_cache_path() -> Option<std::path::PathBuf> {
    data_dir().map(|d| d.join("plugin-cache.json"))
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
    // Balayage des plugins installés dès le démarrage (fil à part, avec cache disque) : le sélecteur est prêt quand on l'ouvre.
    host.set_plugin_cache(plugin_cache_path());
    host.start_plugin_scan();
    host.set_control_pages_path(control_pages_path());
    host.set_audio_inputs(inputs);
    // Préférences (le métronome en reprend ses réglages) et description de ce qui tourne, pour la fenêtre des réglages.
    let (prefs, warning) = startup_prefs();
    if let Some(w) = warning {
        log(w);
    }
    host.set_preferences(prefs.clone(), prefs_path());
    // Pilotes ASIO installés (Windows seulement : les autres plates-formes n'ont pas de pilote branché pour l'instant).
    #[cfg(target_os = "windows")]
    let drivers: Vec<String> = daw_audio_io::asio_backend::list_drivers().into_iter().map(|d| d.name).collect();
    #[cfg(not(target_os = "windows"))]
    let drivers: Vec<String> = Vec::new();
    host.set_runtime(Runtime {
        effective: eff.clone(),
        sample_rate: rate,
        inputs,
        drivers,
        cores: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
        audio,
    });
    // Interfaces natives des plugins : fenêtres Win32 pompées par ce fil (celui des plugins). Pas encore écrit pour macOS et Linux.
    #[cfg(target_os = "windows")]
    host.set_window_system(Box::new(win32_window::DockedSystem::new()));
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
            host.set_audio_status(driver.clone(), info.sample_rate, info.buffer_size as u32);
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

/// Linux : le moteur joue par JACK (sous PipeWire : lancer l'application avec `pw-jack`, voir `tools/lancer-linux.sh`). Deux sorties reliées aux
/// premières sorties physiques et deux entrées reliées aux premières entrées physiques ; la taille du tampon est celle du serveur. Pas d'objectif de
/// basse latence sous Linux (décision du 4 novembre 2026). Pas encore : entrées MIDI, compensation de latence d'entrée, plus de deux sorties.
#[cfg(target_os = "linux")]
fn run_with_jack(app: &AppHandle, rx: &SharedRx, route: &SharedRoute) -> Result<(), String> {
    use daw_audio_io::jack_backend::{run_playback, PlayOptions};
    let eff = effective();
    log("ouverture de JACK (sous PipeWire : lancer avec pw-jack)");
    let (app2, rx2, route2) = (app.clone(), Arc::clone(rx), Arc::clone(route));
    let opts = PlayOptions { seconds: 24 * 3600, ..PlayOptions::default() };
    let report = run_playback(&opts, move |rate| {
        log(&format!("JACK ouvert : {rate} Hz, 2 entrées, 2 sorties"));
        // Taille maximale d'un bloc du moteur : un appel plus grand est découpé en blocs internes, sans allocation.
        let (engine, mut host) = build_host(rate, 1024, 2, &route2, &eff, true).expect("projet de démonstration valide");
        host.set_audio_outputs(2);
        host.set_audio_status("JACK".into(), rate, 0);
        let mut pump = Pump { host, rx: rx2, app: app2, last_tick: Instant::now(), route: route2, last_armed: Vec::new(), latency: None, latency_done: true };
        let control: daw_audio_io::playback::ControlFn = Box::new(move |_| pump.step());
        (EngineProc(engine), control)
    })?;
    log(&format!("JACK arrêté : {} xruns signalés, allocations dans le callback : {}", report.xruns, report.callback_allocations));
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
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = run_with_jack(&app, &rx, &route) {
            log(&format!("JACK indisponible : {e} — moteur sans son"));
            let _ = app.emit(EVENT, &Event::Error { command: "audio".into(), message: format!("JACK indisponible ({e}) : moteur sans son. Sous PipeWire, lance l'application avec « pw-jack ». ") });
            run_silent(&app, rx, &route);
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        log("plateforme sans pilote branché : moteur sans son");
        run_silent(&app, rx, &route);
    }
}

fn main() {
    log("démarrage de l'application");
    // Un panic dans un fil (callback audio compris, où il est contenu et rend du silence) doit laisser une trace.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        log(&format!("PANIC (fil « {} ») : {info}", thread.name().unwrap_or("sans nom")));
        previous(info);
    }));
    let (tx, rx) = mpsc::channel::<String>();
    let rx: SharedRx = Arc::new(Mutex::new(rx));
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(CommandSender(Mutex::new(tx)))
        .manage(CloseGuard(AtomicBool::new(false)))
        .invoke_handler(tauri::generate_handler![send_command, close_guard, quit, restart_app, scripts_list, script_save, script_fs_read, script_fs_write, script_fs_list])
        .on_window_event(|window, event| {
            // Seule la fenêtre principale demande confirmation : une fenêtre de plugin se ferme par l'hôte (voir `win32_window`).
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.state::<CloseGuard>().0.load(Ordering::Relaxed) {
                    api.prevent_close();
                    let _ = window.emit("close-requested", ());
                }
            }
        })
        .setup(move |app| {
            log("fenêtre prête : démarrage du fil moteur");
            #[cfg(target_os = "windows")]
            win32_window::set_app(app.handle().clone());
            let handle = app.handle().clone();
            std::thread::Builder::new().name("moteur".into()).spawn(move || engine_thread(handle, rx)).expect("fil moteur");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'application");
}

#[cfg(test)]
mod script_fs_tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("daw-script-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn s(p: &std::path::Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn a_script_only_reaches_its_roots() {
        let (root, other) = (temp("root"), temp("other"));
        std::fs::write(root.join("a.txt"), "bonjour").unwrap();
        std::fs::write(other.join("secret.txt"), "non").unwrap();
        let roots = vec![s(&root)];
        assert_eq!(script_fs_read(s(&root.join("a.txt")), roots.clone()).unwrap(), "bonjour");
        assert!(script_fs_read(s(&other.join("secret.txt")), roots.clone()).is_err(), "hors des racines");
        // `..` ne permet pas de sortir.
        let sneaky = format!("{}/../{}/secret.txt", s(&root), other.file_name().unwrap().to_string_lossy());
        assert!(script_fs_read(sneaky, roots.clone()).is_err());
        // Aucune racine : rien n'est permis.
        assert!(script_fs_read(s(&root.join("a.txt")), vec![]).is_err());
        // Un fichier choisi seul est une racine (il ne donne pas accès à ses voisins).
        let only = vec![s(&root.join("a.txt"))];
        assert!(script_fs_read(s(&root.join("a.txt")), only.clone()).is_ok());
        std::fs::write(root.join("b.txt"), "x").unwrap();
        assert!(script_fs_read(s(&root.join("b.txt")), only).is_err());
        // Un dossier voisin au préfixe commun n'est pas « dedans » (« root » et « root-bis »).
        let bis = std::path::PathBuf::from(format!("{}-bis", s(&root)));
        std::fs::create_dir_all(&bis).unwrap();
        std::fs::write(bis.join("c.txt"), "x").unwrap();
        assert!(script_fs_read(s(&bis.join("c.txt")), roots).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_cannot_leave_the_roots() {
        let (root, other) = (temp("link-root"), temp("link-other"));
        std::fs::write(other.join("secret.txt"), "non").unwrap();
        std::os::unix::fs::symlink(&other, root.join("lien")).unwrap();
        assert!(script_fs_read(s(&root.join("lien/secret.txt")), vec![s(&root)]).is_err());
    }

    #[test]
    fn writing_never_overwrites_by_surprise_and_refuses_programs() {
        let root = temp("write");
        let roots = vec![s(&root)];
        let f = s(&root.join("sortie.txt"));
        script_fs_write(f.clone(), "un".into(), roots.clone(), false).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "un");
        assert_eq!(script_fs_write(f.clone(), "deux".into(), roots.clone(), false).unwrap_err(), "ce fichier existe déjà");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "un", "rien n'a changé");
        script_fs_write(f.clone(), "deux".into(), roots.clone(), true).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "deux");
        for bad in ["a.exe", "a.BAT", "a.ps1", "a.js", "a.clap", "a.sh"] {
            assert!(script_fs_write(s(&root.join(bad)), "x".into(), roots.clone(), false).unwrap_err().contains("ne peut pas écrire"), "{bad}");
        }
        // Hors des racines, ou dans un dossier qui n'existe pas : refusé.
        assert!(script_fs_write(s(&std::env::temp_dir().join("daw-hors-racine.txt")), "x".into(), roots.clone(), true).is_err());
        assert!(script_fs_write(s(&root.join("nouveau/x.txt")), "x".into(), roots.clone(), false).is_err());
        assert!(script_fs_write(s(&root), "x".into(), roots.clone(), true).is_err(), "un dossier n'est pas un fichier");
        // Trop long.
        let big = "x".repeat(SCRIPT_FS_MAX as usize + 1);
        assert!(script_fs_write(s(&root.join("gros.txt")), big, roots, false).unwrap_err().contains("trop long"));
    }

    #[test]
    fn reading_is_text_only_and_bounded_and_listing_is_flat() {
        let root = temp("read");
        let roots = vec![s(&root)];
        std::fs::write(root.join("binaire.dat"), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
        assert!(script_fs_read(s(&root.join("binaire.dat")), roots.clone()).unwrap_err().contains("UTF-8"));
        std::fs::write(root.join("gros.txt"), vec![b'a'; SCRIPT_FS_MAX as usize + 1]).unwrap();
        assert!(script_fs_read(s(&root.join("gros.txt")), roots.clone()).unwrap_err().contains("trop gros"));
        assert!(script_fs_read(s(&root.join("absent.txt")), roots.clone()).is_err());
        std::fs::create_dir_all(root.join("sous")).unwrap();
        std::fs::write(root.join("sous/profond.txt"), "x").unwrap();
        std::fs::write(root.join("a.txt"), "12345").unwrap();
        let list = script_fs_list(s(&root), roots.clone()).unwrap();
        let names: Vec<&str> = list.as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["a.txt", "binaire.dat", "gros.txt", "sous"], "trié, sans descendre dans les sous-dossiers");
        let a = list.as_array().unwrap().iter().find(|e| e["name"] == "a.txt").unwrap();
        assert_eq!((a["is_dir"].as_bool(), a["size"].as_u64()), (Some(false), Some(5)));
        assert!(script_fs_list(s(&root.join("a.txt")), roots.clone()).is_err(), "pas un dossier");
        assert!(script_fs_list(s(&std::env::temp_dir()), roots).is_err(), "hors des racines");
    }
}

#[cfg(test)]
mod script_fs_new_file_tests {
    use super::*;

    #[test]
    fn a_file_chosen_in_a_save_dialog_is_writable_before_it_exists() {
        let dir = std::env::temp_dir().join(format!("daw-script-fs-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let chosen = dir.join("notes.csv"); // « Enregistrer sous » rend ce chemin : le fichier n'existe pas encore
        let roots = vec![chosen.to_string_lossy().into_owned()];
        script_fs_write(chosen.to_string_lossy().into_owned(), "a;b\n".into(), roots.clone(), false).unwrap();
        assert_eq!(std::fs::read_to_string(&chosen).unwrap(), "a;b\n");
        // Le voisin n'est pas accessible pour autant.
        assert!(script_fs_write(dir.join("autre.csv").to_string_lossy().into_owned(), "x".into(), roots, false).is_err());
    }
}
