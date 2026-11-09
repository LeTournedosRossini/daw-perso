//! Hôte : état de l'interface côté Rust. Il possède la session (le projet) et la poignée du moteur.
//!
//! Les identifiants de clips du protocole sont uniques : un clip audio garde son `ClipId` de la session, un clip de notes
//! reçoit `MIDI_CLIP_BASE + MidiClipId`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daw_core_types::time::{Frac, Meter, Tempo, TempoMap};
use daw_core_types::NodeId;
use daw_engine::{AutoCurve, AutoMode, AutoTarget, MidiMsg, AudioData, Clip, Command as EngineCommand, EditError, EngineHandle, Fade, FadeShape, GraphDesc, InsertFactory, LoadError, MeterBank, MidiInputTx, MidiRecordReceiver, NodeKind, NodeParams, RecordSource, RecordedMidi, TrackFormat, MAX_METERED};
use daw_recorder::Recording;
use daw_session::peaks::Peaks;
use daw_session::nodes::{NewNode, NodeInput};
use daw_session::xfade::Anchor;
use daw_session::project::shape_from_name;
use daw_session::{ClipId, MEvent, MKind, MLane, MNote, MPoint, MidiClipId, MidiTakeData, NoteEdit, Session, SessionError, RECORD_PPQ};

use crate::prefs::{self, Prefs, Runtime};
use crate::project_io::{self, PluginEntry, UiInput, UiMeta};
use crate::protocol::{DynamicsSpec, VelocityCurveSpec, MidiRuleSpec, ReturnGroupsInfo, GroupInfo, SectionDto, LyricDto, ReturnOutput, ScaleInfo, LaneInfo, LanePoint, LaneTarget, SendInfo, NoteDetail, EventDetail, ClipInfo, Command, CurveSpec, Event, InputSpec, InsertInfo, LoopRange, MeterChange, NodeInfo, NoteInfo, ParamEntry, PluginListing, ProjectInfo, TempoChange, TransportAction};

/// Dossier (sous la racine de la sauvegarde automatique) du projet qui n'a pas encore de dossier à lui.
const UNTITLED_DIR: &str = "sans-titre.daw";

/// Décalage des identifiants de clips de notes dans le protocole (les clips audio gardent leur identifiant).
pub const MIDI_CLIP_BASE: u64 = 1_000_000;

enum ClipRef {
    Audio(ClipId),
    Midi(MidiClipId),
}

/// Fondus enchaînés automatiques sur une piste : pour chaque paire de clips « a puis b » qui se chevauchent (a commence avant
/// b, finit après le début de b et avant la fin de b), la zone commune devient le fondu de sortie de a et le fondu d'entrée de b
/// (puissance constante). Les paires dont les fondus correspondent déjà ne sont pas touchées.
fn auto_crossfades(s: &mut Session, node: NodeId) -> Result<(), SessionError> {
    let mut clips: Vec<(u64, ClipId)> = s.clip_ids(node).iter().filter_map(|&id| s.clip(id).map(|c| (c.start, id))).collect();
    clips.sort();
    for w in clips.windows(2) {
        let (a, b) = (w[0].1, w[1].1);
        let (mut ca, mut cb) = (s.clip(a).expect("clip de la liste").clone(), s.clip(b).expect("clip de la liste").clone());
        if ca.start < cb.start && cb.start < ca.end() && ca.end() <= cb.end() {
            let overlap = ca.end() - cb.start;
            if ca.fade_out.len != overlap || cb.fade_in.len != overlap {
                // La forme déjà choisie par l'utilisateur est conservée ; sans fondu préalable : puissance constante.
                ca.fade_out = if ca.fade_out.len > 0 { ca.fade_out.with_len(overlap) } else { Fade::new(overlap, FadeShape::EqualPower) };
                cb.fade_in = if cb.fade_in.len > 0 { cb.fade_in.with_len(overlap) } else { Fade::new(overlap, FadeShape::EqualPower) };
                s.set_clip(a, ca)?;
                s.set_clip(b, cb)?;
            }
        }
    }
    Ok(())
}

fn explain(e: &SessionError) -> String {
    match e {
        SessionError::Edit(EditError::OutOfRange) => "position hors du clip, ou clip qui serait vide, ou données audio insuffisantes".into(),
        SessionError::Edit(EditError::NoOverlap) => "les clips ne se chevauchent pas".into(),
        SessionError::UnknownNode(n) => format!("nœud inconnu : {}", n.0),
        SessionError::UnknownClip(c) => format!("clip inconnu : {}", c.0),
        SessionError::UnknownMidiClip(c) => format!("clip de notes inconnu : {}", c.0),
        SessionError::NotATrack(n) => format!("le nœud {} n'est pas une piste : un clip ne peut être posé que sur une piste", n.0),
        SessionError::BadMidi => "notes ou clip de notes invalide".into(),
        SessionError::BadLane => "courbe d'automation invalide".into(),
        SessionError::BadNoteIndex(i) => format!("note inexistante : {i}"),
        SessionError::TimeOverflow => "position trop grande pour être convertie".into(),
        SessionError::Routing(m) => format!("sortie refusée : {m}"),
        SessionError::BadName => "nom de 1 à 60 caractères".into(),
        SessionError::UnknownInsert(i) => format!("plugin inconnu : {i}"),
        SessionError::BadChordTrack(m) | SessionError::BadStructure(m) | SessionError::BadLyrics(m) | SessionError::BadGroups(m) | SessionError::BadMidiFilter(m) => m.clone(),
        SessionError::BadVelocityCurve => "courbe de vélocité : courbure −1000 à 1000, adoucissement 0 à 1000, vélocités de 1 à 127".into(),
        SessionError::BadInputDynamics => "dynamique d'entrée : porte −90 à −20 dB, seuil −60 à 0 dB, rapport 1 à 20, attaque 0,1 à 100 ms, relâchement 10 à 2 000 ms, compensation 0 à 24 dB".into(),
        SessionError::BadPhysOut(n) => format!("sortie refusée pour le nœud {} : ce doit être un bus, sur des sorties libres de la carte (3 et suivantes)", n.0),
        SessionError::MasterNode => "la sortie principale ne peut être ni supprimée, ni renommée, ni déplacée, ni redirigée".into(),
    }
}

/// Ce que l'interface affiche et que le moteur ne connaît pas : couleurs de pistes, noms de clips.
#[derive(Debug, Default, Clone)]
pub struct ProjectMeta {
    pub node_colors: HashMap<u32, String>,
    pub clip_names: HashMap<u64, String>,
    /// D'où vient chaque effet inséré (par identifiant d'insert) : sans cela, on ne sait pas l'enregistrer dans le projet.
    pub plugins: HashMap<u32, PluginRef>,
}

/// Identité d'un plugin CLAP inséré, telle qu'elle est enregistrée dans le projet.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginRef {
    pub clap_id: String,
    pub vendor: String,
    pub name: String,
    /// Nom du fichier du plugin (pour le retrouver vite à l'ouverture).
    pub file: String,
}

/// Couleurs proposées aux nouvelles pistes, à tour de rôle.
const TRACK_COLORS: [&str; 9] = ["#4aa3ff", "#e5484d", "#f5a524", "#30a46c", "#a78bfa", "#f76b15", "#12a594", "#d6409f", "#9aa3b2"];

/// Retrouve un plugin à l'ouverture d'un projet : l'usine qui le crée et son identité, ou `None` s'il n'est pas installé.
pub type PluginResolver = Box<dyn FnMut(&PluginEntry) -> Option<(InsertFactory, PluginRef)>>;

/// Cherche dans les dossiers CLAP du système : d'abord le fichier enregistré dans le projet, puis tous les autres.
pub fn default_resolver() -> PluginResolver {
    use daw_clap_host::{clap_insert_factory, default_search_dirs, find_plugins, Bundle, PluginInfo};
    let mut opened: HashMap<std::path::PathBuf, Option<(Arc<Bundle>, Vec<PluginInfo>)>> = HashMap::new();
    Box::new(move |e: &PluginEntry| {
        let files: Vec<std::path::PathBuf> = default_search_dirs().iter().flat_map(|d| find_plugins(d)).collect();
        let named = |f: &std::path::PathBuf| !e.file.is_empty() && f.file_name().is_some_and(|n| n.to_string_lossy() == e.file);
        let (first, rest): (Vec<_>, Vec<_>) = files.into_iter().partition(named);
        for f in first.into_iter().chain(rest) {
            let slot = opened.entry(f.clone()).or_insert_with(|| {
                let bundle = Bundle::open(&f).ok()?;
                let list = bundle.plugins().ok()?;
                Some((Arc::new(bundle), list))
            });
            if let Some((bundle, list)) = slot {
                if let Some(info) = list.iter().find(|i| i.id == e.clap_id) {
                    let file = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let pref = PluginRef { clap_id: info.id.clone(), vendor: info.vendor.clone(), name: info.name.clone(), file };
                    return Some((clap_insert_factory(Arc::clone(bundle), info.id.clone()), pref));
                }
            }
        }
        None
    })
}

/// Balayage des plugins installés (exécuté dans un fil à part, une seule fois).
pub type CatalogScan = Box<dyn FnOnce() -> Vec<PluginListing> + Send>;

/// Balaie les dossiers CLAP du système : ne fait que lire le descriptif de chaque fichier, rien n'est instancié.
pub fn default_scan() -> CatalogScan {
    Box::new(|| {
        use daw_clap_host::{default_search_dirs, find_plugins, Bundle};
        let mut out = Vec::new();
        for dir in default_search_dirs() {
            for file in find_plugins(&dir) {
                let Ok(bundle) = Bundle::open(&file) else { continue };
                let Ok(list) = bundle.plugins() else { continue };
                let file_name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                for i in list {
                    out.push(PluginListing { clap_id: i.id, name: i.name, vendor: i.vendor, file: file_name.clone(), description: i.description, features: i.features });
                }
            }
        }
        out.sort_by_key(|p| (p.vendor.to_lowercase(), p.name.to_lowercase()));
        out
    })
}

/// L'usine `inner` qui crée l'instance avec `state` quand l'appelant n'en fournit pas (première création après l'ouverture).
fn with_state(inner: InsertFactory, state: Option<Vec<u8>>) -> InsertFactory {
    Arc::new(move |sample_rate, max_block, given| inner(sample_rate, max_block, given.or(state.as_deref())))
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct NodeState {
    gain: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    invert: bool,
    arm: bool,
}

/// Une prise en cours : la piste, son nom (pour nommer le clip) et ce qu'elle écrit.
struct TakeJob {
    node: u32,
    track_name: String,
    number: u32,
    kind: TakeKind,
}

enum TakeKind {
    /// Prise audio : le fil d'écriture du disque et le chemin relatif au dossier du projet.
    Audio { rec: Recording, rel: String },
    /// Prise MIDI : la file du moteur (vidée à chaque `tick`) et ce qu'elle a déjà donné.
    /// `passes` : pour un enregistrement en boucle, le numéro de passage de chaque événement (même longueur que `events`).
    Midi { rx: MidiRecordReceiver, events: Vec<RecordedMidi>, passes: Vec<u32> },
}

/// Enregistrement en cours.
struct RecordingState {
    jobs: Vec<TakeJob>,
    /// L'ordre d'arrêt est donné : on attend que le moteur ait retiré les prises et que le disque ait fini.
    stopping: bool,
    last_overruns: u32,
    /// Position de la ligne de temps où les prises commencent (après pré-compte et pré-roll).
    start: u64,
    /// Latence ajoutée par les plugins au départ de la prise (échantillons) : la compensation du moteur retarde toute la lecture de
    /// cette durée, donc ce que le musicien entend et rejoue, et ce que la boucle ramène. Ajoutée à la latence de la carte pour ranger
    /// la prise.
    graph_latency: u64,
    /// La fin des prises est fixée (punch-out) : elles se ferment toutes seules et le clip naît sans ordre d'arrêt.
    auto_end: bool,
    /// La boucle du moteur a été suspendue pour la prise (punch dans la zone) : à rétablir à la fin.
    loop_suspended: bool,
    /// Enregistrement en boucle : la zone (début, fin) ; chaque passage de la boucle donne une prise du groupe.
    loop_zone: Option<(u64, u64)>,
    /// Enregistrement en boucle : numéro du passage en cours et dernière position vue dans la zone (pour repérer le retour de boucle).
    loop_pass: u32,
    loop_last_pos: u64,
}

/// Découpe d'une prise faite pendant que la boucle `zone` tournait : le flux enregistré est la suite des passages. `start_pos` :
/// position où il commence, `comp` : latence à retirer, `frames` : sa longueur. Rend les (début, fin) en trames du flux : le premier
/// passage va jusqu'à la fin de la zone, les suivants font la longueur de la zone (les coupures sont décalées de `comp`, pour que
/// chaque passage commence au début musical de la zone) ; un dernier passage de moins de 512 trames (arrêt juste après le retour de
/// la boucle) est abandonné, sauf s'il est le seul.
fn loop_passes(zone: (u64, u64), start_pos: u64, comp: u64, frames: u64) -> Vec<(u64, u64)> {
    let len = zone.1.saturating_sub(zone.0).max(1);
    let off = start_pos.saturating_sub(zone.0);
    let mut cuts = vec![0u64];
    let mut k = 1u64;
    loop {
        let f = (k * len + comp).saturating_sub(off);
        if f >= frames {
            break;
        }
        if f > 0 && f > *cuts.last().unwrap_or(&0) {
            cuts.push(f);
        }
        k += 1;
    }
    cuts.push(frames);
    let mut out: Vec<(u64, u64)> = cuts.windows(2).map(|w| (w[0], w[1])).collect();
    if out.len() > 1 && out.last().is_some_and(|l| l.1 - l.0 < 512) {
        out.pop();
    }
    out
}

/// Ce que l'aperçu sonore d'un fondu écoute.
#[derive(Debug, Clone, Copy)]
enum PreviewTarget {
    Crossfade { out: ClipId, inn: ClipId },
    Single { clip: ClipId, side_in: bool },
}

/// Aperçu sonore en cours : la cible et les réglages, plus ce qu'il faudra rendre à l'utilisateur.
struct Preview {
    target: PreviewTarget,
    margin_ms: u64,
    solo: bool,
    saved_pos: u64,
    saved_playing: bool,
    saved_loop: Option<LoopRange>,
    saved_solos: Vec<bool>,
}

/// Réglages du métronome et de l'enregistrement (pré-compte, pré-roll, post-roll).
#[derive(Clone, Copy)]
struct Metro {
    enabled: bool,
    only_recording: bool,
    volume: f64,
    count_in_bars: u32,
    pre_roll: f64,
    post_roll: f64,
    pre_roll_bars: u32,
    pre_roll_unit: prefs::PreRollUnit,
}

/// Horizon du clic fabriqué d'avance : au-delà, le métronome se tait (le clic est regénéré à chaque modification du tempo).
const CLICK_HORIZON_SECONDS: u64 = 30 * 60;

mod align;
mod clips;
mod convert;
mod export;
mod groups;
mod import;
mod monitoring;
mod plugin_out;
mod plugin_ui;
mod remote;
mod write;

pub struct Host {
    handle: EngineHandle,
    session: Session,
    meta: ProjectMeta,
    meters: Arc<MeterBank>,
    nodes: Vec<NodeState>,
    playing: bool,
    loop_range: Option<LoopRange>,
    /// Punch allumé : `record` enregistre exactement dans la zone (la bande bleue de la règle, `loop_range`) au lieu de refuser quand
    /// une boucle existe ; la boucle est suspendue pendant la prise puis rétablie.
    punch_on: bool,
    /// Dernière révision de la piste d'accords envoyée à la page.
    chord_rev_sent: u32,
    /// Serveur web local des retours de monitoring (démarré à la demande de l'utilisateur).
    monitor: Option<crate::monitor::Monitor>,
    monitor_last: Instant,
    monitor_generation: u64,
    /// Décalage de niveau (dB) appliqué à chaque groupe, par destination (`None` : les pistes elles-mêmes, `Some(retour)` : leurs envois
    /// vers ce retour). Non enregistré dans le projet : un autre projet, ou sa réouverture, repart de 0.
    group_offsets: HashMap<(u32, Option<u32>), f64>,
    group_offsets_generation: u64,
    /// Plus forte crête récente de chaque nœud (linéaire) et l'instant où elle a été vue : retenue de 0,6 s pour le témoin de signal des
    /// tablettes (elles relisent l'état toutes les 0,4 s).
    peak_hold: HashMap<u32, (f32, Instant)>,
    /// Calage de prises en cours (fil de travail) et fiches des prises calées.
    align: Option<align::AlignJob>,
    /// Écoute de calage en cours (voir `listen_align`).
    listen: Option<align::Listen>,
    aligns: Vec<align::AlignRecord>,
    /// Témoin d'entrée MIDI : types reçus par piste et instant de la dernière réception ; dernier ensemble envoyé à la page.
    midi_act: HashMap<u32, (u8, Instant)>,
    midi_act_sent: BTreeMap<String, u8>,
    /// Appareils d'entrée MIDI que le système voit (donnés par l'application).
    midi_devices: Vec<String>,
    /// Enregistrement en boucle (exclusif avec Punch) : voir `start_recording`.
    loop_rec_on: bool,
    peaks: HashMap<usize, Arc<Peaks>>,
    auto_crossfade: bool,
    preview: Option<Preview>,
    /// Dossier du projet ouvert (`None` : pas encore enregistré), nom affiché, modifications non enregistrées, numéro de chargement.
    project_dir: Option<std::path::PathBuf>,
    name: String,
    dirty: bool,
    generation: u64,
    /// Plugins du fichier qu'on n'a pas retrouvés : gardés tels quels (état compris) pour ne pas les perdre à l'enregistrement.
    kept_plugins: Vec<PluginEntry>,
    resolver: PluginResolver,
    /// Dernière `Session::structure_version` vue : si elle change, des nœuds ont été ajoutés ou retirés (numéros décalés).
    last_structure: u64,
    /// Inventaire des plugins installés (balayage en arrière-plan, fait une seule fois).
    catalog: Option<Vec<PluginListing>>,
    scan: Option<CatalogScan>,
    scan_rx: Option<std::sync::mpsc::Receiver<Vec<PluginListing>>>,
    /// Canaux d'entrée audio ouverts par le pilote ; décalage (échantillons) à retirer des prises (latence aller-retour).
    audio_inputs: u32,
    /// Canaux de sortie ouverts par le pilote (0 : inconnu, ce qui permet jusqu'à `MAX_OUT_CHANNELS` ; 2 : la sortie principale seule).
    audio_outputs: u32,
    record_compensation: u64,
    recording: Option<RecordingState>,
    midi_in: Option<MidiInputTx>,
    metro: Metro,
    prefs: Prefs,
    prefs_path: Option<std::path::PathBuf>,
    prefs_dirty: bool,
    last_prefs_save: Instant,
    runtime: Option<Runtime>,
    /// Paramètres armés pour l'écriture de l'automation, et dernières valeurs envoyées aux paramètres de plugins.
    write_arms: Vec<write::WriteArm>,
    plugin_values: HashMap<(u32, u32), f64>,
    /// Dernière position connue de la fenêtre de chaque plugin (par nom), enregistrée dans le projet.
    window_places: BTreeMap<String, (i32, i32)>,
    /// Clips copiés (copier / couper / coller), hors historique.
    clipboard: Vec<clips::ClipSnap>,
    /// Sauvegarde automatique : racine des projets sans dossier, période, dernier passage, reprise à proposer au démarrage.
    autosave_root: Option<std::path::PathBuf>,
    autosave_every: Duration,
    last_autosave: Instant,
    recovery_offer: Option<Event>,
    /// Dernier refus d'une commande de télécommande (montré sur la tablette quelques secondes).
    remote_msg: Option<(Instant, String)>,
    /// Le transport vient d'être mis en pause (arrêt pendant la lecture) : reprendre la lecture reprend là, sans pré-roll. Un déplacement de la tête l'annule.
    paused: bool,
    /// Où la tête a été placée par le dernier déplacement (le moteur ne la rend qu'au prochain bloc : la lecture qui suit part de là).
    head_hint: Option<u64>,
    /// Point de départ de la lecture en cours (la tête y revient à l'arrêt si `transport.return_on_stop`).
    play_origin: Option<u64>,
    /// Banque personnelle de pages de contrôle des plugins (fichier de l'utilisateur, voir `control_pages`).
    bank: crate::control_pages::Bank,
    bank_path: Option<std::path::PathBuf>,
    bank_error: Option<String>,
    /// Retour → (droits, instant où ils ont été donnés ou vus pour la première fois) : sert à l'expiration des droits de la télécommande.
    monitor_grants: HashMap<u32, (u8, Instant)>,
    /// Empreinte des cartes de tempo et de chiffrage dont le clic a été fabriqué.
    click_key: String,
    /// Post-roll en cours : position à laquelle l'enregistrement s'arrêtera.
    stop_at: Option<u64>,
    /// Export en cours (fil de travail).
    export: Option<export::ExportJob>,
    /// Import en cours (fil de travail).
    import: Option<import::ImportJob>,
    /// Conversion d'un projet à une autre fréquence en cours (fil de travail).
    convert: Option<convert::ConvertJob>,
    /// Système de fenêtres de la plateforme (interfaces natives des plugins), fenêtres ouvertes, et cache « ce plugin a une interface pour cette plateforme ».
    window_system: Option<Box<dyn crate::plugin_window::WindowSystem>>,
    plugin_windows: Vec<plugin_ui::OpenWindow>,
    gui_cache: std::cell::RefCell<HashMap<u32, bool>>,
}

impl Host {
    /// Charge le graphe de la session dans le moteur.
    pub fn new(mut handle: EngineHandle, mut session: Session, meta: ProjectMeta) -> Result<Host, LoadError> {
        // Les couleurs des pistes vivent dans la session (annulables, enregistrées avec le projet) : on y verse celles de `meta`.
        for (id, color) in &meta.node_colors {
            session.set_node_color_untracked(NodeId(*id), color);
        }
        handle.load_graph(session.graph())?;
        let nodes = session.graph().nodes.iter().map(|n| NodeState { gain: n.params.gain, pan: n.params.pan, mute: n.params.mute, solo: n.params.solo, invert: n.params.invert, arm: false }).collect();
        let meters = handle.meters();
        let mut host = Host { handle, session, meta, meters, nodes, playing: false, loop_range: None, punch_on: false, chord_rev_sent: u32::MAX, monitor: None, monitor_last: Instant::now(), monitor_generation: 0, group_offsets: HashMap::new(), group_offsets_generation: 0, peak_hold: HashMap::new(), listen: None, align: None, aligns: Vec::new(), midi_act: HashMap::new(), midi_act_sent: BTreeMap::new(), midi_devices: Vec::new(), loop_rec_on: false, peaks: HashMap::new(), auto_crossfade: true, preview: None, project_dir: None, name: "Sans titre".into(), dirty: false, generation: 0, kept_plugins: Vec::new(), resolver: default_resolver(), last_structure: 0, catalog: None, scan: None, scan_rx: None, audio_inputs: 0, audio_outputs: 0, record_compensation: 0, recording: None, midi_in: None, metro: Metro { enabled: false, only_recording: false, volume: 0.8, count_in_bars: 0, pre_roll: 0.0, post_roll: 0.0, pre_roll_bars: 0, pre_roll_unit: prefs::PreRollUnit::Seconds }, prefs: Prefs::default(), prefs_path: None, prefs_dirty: false, last_prefs_save: Instant::now(), runtime: None, click_key: String::new(), stop_at: None, export: None, import: None, convert: None, window_system: None, plugin_windows: Vec::new(), gui_cache: Default::default(), write_arms: Vec::new(), plugin_values: HashMap::new(), window_places: BTreeMap::new(), clipboard: Vec::new(), autosave_root: None, autosave_every: Duration::from_secs(60), last_autosave: Instant::now(), recovery_offer: None, remote_msg: None, paused: false, head_hint: None, play_origin: None, bank: crate::control_pages::Bank::new(), bank_path: None, bank_error: None, monitor_grants: HashMap::new() };
        host.refresh_click();
        host.apply_metronome();
        Ok(host)
    }

    /// Analyse une commande JSON et l'applique. Toute erreur (JSON invalide, commande inconnue, valeur hors plage,
    /// nœud ou clip inconnu) devient un événement `error` : jamais d'exception ni de silence.
    pub fn command_json(&mut self, json: &str) -> Vec<Event> {
        let value: serde_json::Value = match serde_json::from_str(json) {
            Ok(v) => v,
            Err(e) => return vec![Event::Error { command: "?".into(), message: format!("JSON invalide : {e}") }],
        };
        let name = value.get("type").and_then(|t| t.as_str()).unwrap_or("?").to_owned();
        match serde_json::from_value::<Command>(value) {
            Ok(cmd) => self.command(cmd),
            Err(e) => vec![Event::Error { command: name, message: format!("commande invalide : {e}") }],
        }
    }

    pub fn command(&mut self, cmd: Command) -> Vec<Event> {
        let before = self.dirty;
        // Une écoute de calage a provisoirement coupé ou rétabli des clips : la moindre commande qui pourrait enregistrer ou éditer ces
        // clips la termine d'abord (sinon l'état provisoire entrerait dans l'historique ou dans le fichier).
        let mut lead = Vec::new();
        if self.listen.is_some() && !matches!(cmd, Command::GetProject | Command::GetPeaks { .. } | Command::GetAlign { .. } | Command::MergeAlignSettings { .. } | Command::ListenAlign { .. } | Command::StopListen | Command::GetPrefs | Command::GetNotes { .. } | Command::GetChords | Command::GetChordCatalog | Command::ProbeAudio { .. } | Command::CancelAlign) {
            lead = self.end_listen(true, false);
        }
        let mut events = self.command_inner(cmd);
        if !lead.is_empty() {
            lead.append(&mut events);
            events = lead;
        }
        self.refresh_click();
        // Les modifications non enregistrées apparaissent ou disparaissent : la page doit le savoir (titre, confirmation).
        if self.dirty != before && !events.iter().any(|e| matches!(e, Event::File { .. })) {
            events.push(self.file_event(Vec::new()));
        }
        events
    }

    fn command_inner(&mut self, cmd: Command) -> Vec<Event> {
        let name = cmd.name();
        if let Err(message) = cmd.validate() {
            return vec![Event::Error { command: name.into(), message }];
        }
        let error = |message: String| vec![Event::Error { command: name.into(), message }];
        match cmd {
            Command::GetProject => {
                let mut events = vec![self.project_event(), self.history_event(), self.file_event(Vec::new()), self.metronome_event(), self.prefs_event(), self.control_pages_event(), self.punch_event(), self.loop_record_event(), self.chords_event(Vec::new()), Self::chord_catalog_event()];
                events.extend(self.recovery_offer.take());
                events
            }
            Command::SetMetronome { enabled, only_recording, volume, count_in_bars, pre_roll, post_roll, pre_roll_bars, pre_roll_unit } => {
                self.metro = Metro { enabled, only_recording, volume, count_in_bars, pre_roll, post_roll, pre_roll_bars, pre_roll_unit };
                self.prefs.metronome = prefs::MetroPrefs { enabled, only_recording, volume, count_in_bars, pre_roll, post_roll, pre_roll_bars, pre_roll_unit };
                self.prefs_dirty = true;
                self.apply_metronome();
                vec![self.metronome_event()]
            }
            Command::SetPunch { enabled } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                self.punch_on = enabled;
                if enabled {
                    self.loop_rec_on = false;
                }
                self.dirty = true;
                vec![self.punch_event(), self.loop_record_event(), self.file_event(Vec::new())]
            }
            Command::SetLoopRecord { enabled } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                self.loop_rec_on = enabled;
                if enabled {
                    self.punch_on = false;
                }
                vec![self.punch_event(), self.loop_record_event()]
            }
            Command::GetPrefs => vec![self.prefs_event()],
            Command::SetPrefs { prefs } => {
                let m = prefs.metronome;
                self.prefs = prefs;
                self.metro = Metro { enabled: m.enabled, only_recording: m.only_recording, volume: m.volume, count_in_bars: m.count_in_bars, pre_roll: m.pre_roll, post_roll: m.post_roll, pre_roll_bars: m.pre_roll_bars, pre_roll_unit: m.pre_roll_unit };
                self.apply_metronome();
                let mut events = Vec::new();
                if let Err(message) = self.save_prefs() {
                    events.push(Event::Error { command: name.into(), message });
                }
                events.push(self.prefs_event());
                events.push(self.metronome_event());
                events
            }
            Command::Transport { action } => {
                let mut events = self.end_preview(true, false);
                // Arrêter la lecture pendant un enregistrement l'arrête aussi (les prises deviennent des clips).
                if matches!(action, TransportAction::Stop) || (matches!(action, TransportAction::Toggle) && self.playing) {
                    events.extend(self.begin_stop_recording());
                }
                let was_playing = self.playing;
                self.playing = match action {
                    TransportAction::Play => true,
                    TransportAction::Stop => false,
                    TransportAction::Toggle => !self.playing,
                };
                let before = self.handle.position();
                if self.playing && !was_playing {
                    // Pré-roll à la lecture : une lecture qui démarre d'une tête placée par un déplacement (règle, section, retour au début) part
                    // du pré-roll avant elle ; reprendre après une pause ne bouge pas. Sans pré-roll réglé, rien ne change.
                    let head = self.head_hint.take().unwrap_or(before);
                    let pre = if self.paused || !self.prefs.transport.play_pre_roll { 0 } else { self.pre_roll_samples(head) };
                    if pre > 0 && head > 0 {
                        self.handle.send(EngineCommand::Seek(head.saturating_sub(pre)));
                    }
                    self.paused = false;
                    self.play_origin = Some(head);
                } else if was_playing && !self.playing {
                    self.head_hint = None;
                    self.settle_after_stop();
                }
                self.handle.send(if self.playing { EngineCommand::Play } else { EngineCommand::Stop });
                self.send_return_seek();
                events.extend(if self.playing { self.write_begin() } else { self.write_end(before) });
                events.push(self.position_event());
                events
            }
            Command::Seek { pos } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let mut events = self.end_preview(true, false);
                events.extend(self.write_end(self.handle.position()));
                self.handle.send(EngineCommand::Seek(pos));
                self.paused = false;
                self.head_hint = Some(pos);
                if self.playing {
                    self.play_origin = Some(pos);
                }
                events.push(Event::Position { pos, playing: self.playing, loop_range: self.loop_range });
                events
            }
            Command::Loop { start, end } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let mut events = self.end_preview(false, false);
                self.loop_range = start.zip(end).map(|(start, end)| LoopRange { start, end });
                self.handle.send(EngineCommand::SetLoop(self.loop_range.map(|l| (l.start, l.end))));
                events.push(self.position_event());
                events
            }
            Command::SetNode { node, gain, pan, mute, solo, arm, monitor, invert } => {
                if arm.is_some() || monitor.is_some() {
                    if let Some(refused) = self.refuse_while_recording(name) {
                        return refused;
                    }
                }
                let Some(state) = self.nodes.get_mut(node as usize) else { return error(format!("nœud inconnu : {node}")) };
                if let Some(g) = gain {
                    state.gain = g;
                    self.handle.send(EngineCommand::SetGain { node, gain: g });
                }
                if let Some(p) = pan {
                    state.pan = p;
                    self.handle.send(EngineCommand::SetPan { node, pan: p });
                }
                if let Some(m) = mute {
                    state.mute = m;
                    self.handle.send(EngineCommand::SetMute { node, mute: m });
                }
                if let Some(s) = solo {
                    state.solo = s;
                    self.handle.send(EngineCommand::SetSolo { node, solo: s });
                }
                if let Some(i) = invert {
                    state.invert = i;
                    self.handle.send(EngineCommand::SetInvert { node, invert: i });
                }
                if let Some(a) = arm {
                    state.arm = a;
                }
                let params = NodeParams { gain: state.gain, pan: state.pan, mute: state.mute, solo: state.solo, invert: state.invert };
                let _ = self.session.set_params_untracked(NodeId(node), params);
                // Armer une piste n'est pas une modification du projet ; le reste (volume, panoramique, muet, solo) en est une.
                if gain.is_some() || pan.is_some() || mute.is_some() || solo.is_some() || invert.is_some() {
                    self.dirty = true;
                }
                if let Some(m) = monitor {
                    let mut inp = self.session.node_input(NodeId(node));
                    inp.monitor = m;
                    self.session.set_node_input_untracked(NodeId(node), inp);
                    self.dirty = true;
                }
                if arm.is_some() || monitor.is_some() {
                    self.apply_input(node);
                }
                // Réglages armés pour l'écriture de l'automation : le geste s'enregistre.
                let mut events = Vec::new();
                if let Some(g) = gain {
                    events.extend(self.note_value(node, AutoTarget::Gain, g as f64));
                }
                if let Some(p) = pan {
                    events.extend(self.note_value(node, AutoTarget::Pan, p as f64));
                }
                events.push(self.project_event());
                events
            }
            Command::SetAutoCrossfade { enabled } => {
                self.auto_crossfade = enabled;
                self.session.set_seam_ms(if enabled { 10 } else { 0 });
                self.dirty = true;
                vec![self.project_event()]
            }
            Command::MoveClip { clip, start, node } => {
                let auto = self.auto_crossfade;
                let target = node.map(NodeId);
                let r = match self.find_clip(clip) {
                    Some(ClipRef::Audio(id)) => {
                        let Some(owner) = self.owner_of_audio(id) else { return error(format!("clip inconnu : {clip}")) };
                        let dest = target.unwrap_or(owner);
                        self.session.group("Déplacer un clip", |s| {
                            s.move_clip_to(id, dest, start)?;
                            if auto {
                                auto_crossfades(s, dest)?;
                            }
                            Ok(())
                        })
                    }
                    Some(ClipRef::Midi(id)) => {
                        let Some(owner) = self.owner_of_midi(id) else { return error(format!("clip inconnu : {clip}")) };
                        self.musical(start).and_then(|f| self.session.move_midi_clip_to(id, target.unwrap_or(owner), f))
                    }
                    None => return error(format!("clip inconnu : {clip}")),
                };
                self.finish_edit(name, r)
            }
            Command::TrimClip { clip, start, end } => match self.find_clip(clip) {
                Some(ClipRef::Audio(id)) => {
                    let (auto, owner) = (self.auto_crossfade, self.owner_of_audio(id));
                    let r = self.session.group("Rogner un clip", |s| {
                        if let Some(st) = start {
                            s.trim_start(id, st)?;
                        }
                        if let Some(en) = end {
                            s.trim_end(id, en)?;
                        }
                        if let (true, Some(node)) = (auto, owner) {
                            auto_crossfades(s, node)?;
                        }
                        Ok(())
                    });
                    self.finish_edit(name, r)
                }
                Some(ClipRef::Midi(id)) => {
                    let r = self.trim_midi(id, start, end);
                    self.finish_edit(name, r)
                }
                None => error(format!("clip inconnu : {clip}")),
            },
            Command::SplitClip { clip, at } => match self.find_clip(clip) {
                Some(ClipRef::Audio(id)) => match self.session.split(id, at) {
                    Ok((_, right)) => {
                        if let Some(n) = self.meta.clip_names.get(&clip).cloned() {
                            self.meta.clip_names.insert(right.0, n);
                        }
                        self.finish_edit(name, Ok(()))
                    }
                    Err(e) => self.finish_edit(name, Err(e)),
                },
                Some(ClipRef::Midi(id)) => match self.musical(at).and_then(|f| self.session.split_midi_clip(id, f)) {
                    Ok((_, right)) => {
                        if let Some(n) = self.meta.clip_names.get(&clip).cloned() {
                            self.meta.clip_names.insert(MIDI_CLIP_BASE + right.0, n);
                        }
                        self.finish_edit(name, Ok(()))
                    }
                    Err(e) => self.finish_edit(name, Err(e)),
                },
                None => error(format!("clip inconnu : {clip}")),
            },
            Command::MoveClips { clips, delta, node_delta } => self.move_clips(name, &clips, delta, node_delta.unwrap_or(0)),
            Command::SplitClips { clips, at } => self.split_clips(name, &clips, at),
            Command::DeleteClips { clips } => self.delete_clips(name, &clips),
            Command::CopyClips { clips } => self.copy_clips(name, &clips),
            Command::PasteClips { at, node } => self.paste_clips(name, at, node),
            Command::DuplicateClips { clips, delta, node_delta } => self.duplicate_clips(name, &clips, delta, node_delta.unwrap_or(0)),
            Command::DeleteClip { clip } => {
                let r = match self.find_clip(clip) {
                    Some(ClipRef::Audio(id)) => self.session.remove_clip(id),
                    Some(ClipRef::Midi(id)) => self.session.remove_midi_clip(id),
                    None => return error(format!("clip inconnu : {clip}")),
                };
                self.finish_edit(name, r)
            }
            Command::SetActiveTake { clip } => {
                let r = match self.find_clip(clip) {
                    Some(ClipRef::Audio(id)) => self.session.set_active_take(id),
                    Some(ClipRef::Midi(id)) => self.session.set_active_midi_take(id),
                    None => return error(format!("clip inconnu : {clip}")),
                };
                self.finish_edit(name, r)
            }
            Command::ActivateTakeRange { clip, start, end } => {
                let r = match self.find_clip(clip) {
                    Some(ClipRef::Audio(id)) => self.session.activate_take_range(id, start, end).map(|_| ()),
                    Some(ClipRef::Midi(_)) => return error("composer par plage : seulement pour l'audio (un conteneur de notes s'active en entier)".into()),
                    None => return error(format!("clip inconnu : {clip}")),
                };
                self.finish_edit(name, r)
            }
            Command::SetClipMuted { clip, muted } => {
                let r = match self.find_clip(clip) {
                    Some(ClipRef::Audio(id)) => self.session.set_clip_muted(id, muted),
                    Some(ClipRef::Midi(id)) => self.session.set_midi_muted(id, muted),
                    None => return error(format!("clip inconnu : {clip}")),
                };
                self.finish_edit(name, r)
            }
            Command::SetClipFades { clip, fade_in, fade_out, curve_in, curve_out } => match self.find_clip(clip) {
                Some(ClipRef::Audio(id)) => {
                    let Some(mut c) = self.session.clip(id).cloned() else { return error(format!("clip inconnu : {clip}")) };
                    let clip_len = c.len;
                    let build = |old: Fade, len: Option<u64>, curve: &Option<CurveSpec>| -> Result<Fade, String> {
                        let len = len.map_or(old.len, |l| l.min(clip_len));
                        match curve {
                            Some(spec) => Ok(Fade::curved(len, shape_from_name(&spec.shape).ok_or(format!("forme de fondu inconnue : {}", spec.shape))?, spec.bend, spec.smooth)),
                            None => Ok(old.with_len(len)),
                        }
                    };
                    match (build(c.fade_in, fade_in, &curve_in), build(c.fade_out, fade_out, &curve_out)) {
                        (Ok(fi), Ok(fo)) => {
                            c.fade_in = fi;
                            c.fade_out = fo;
                        }
                        (Err(m), _) | (_, Err(m)) => return error(m),
                    }
                    let r = self.session.set_clip(id, c);
                    self.finish_edit(name, r)
                }
                Some(ClipRef::Midi(id)) => {
                    if curve_in.is_some() || curve_out.is_some() {
                        return error("les fondus d'un clip de notes sont linéaires : pas de courbe à choisir".into());
                    }
                    let Some(start) = self.session.midi_clip(id).map(|c| c.start) else { return error(format!("clip inconnu : {clip}")) };
                    let r = (|| {
                        let base = self.session.tempo_map().to_samples(start, self.session.sample_rate()).ok_or(SessionError::TimeOverflow)?;
                        let dur = |n: u64| -> Result<Frac, SessionError> {
                            let f = self.musical(base + n)?;
                            f.checked_sub(self.musical(base)?).ok_or(SessionError::TimeOverflow)
                        };
                        let fi = fade_in.map(dur).transpose()?;
                        let fo = fade_out.map(dur).transpose()?;
                        self.session.set_midi_fades(id, fi, fo)
                    })();
                    self.finish_edit(name, r)
                }
                None => error(format!("clip inconnu : {clip}")),
            },
            Command::SetCrossfade { clip_out, clip_in, curve_out, curve_in } => {
                let (Some(ClipRef::Audio(a)), Some(ClipRef::Audio(b))) = (self.find_clip(clip_out), self.find_clip(clip_in)) else {
                    return error("set_crossfade : deux clips audio existants".into());
                };
                let to_fade = |old: Fade, spec: &Option<CurveSpec>| -> Result<Fade, String> {
                    match spec {
                        Some(sp) => Ok(Fade::curved(old.len, shape_from_name(&sp.shape).ok_or(format!("forme de fondu inconnue : {}", sp.shape))?, sp.bend, sp.smooth)),
                        None => Ok(old),
                    }
                };
                let r = self.session.group("Courbe du fondu enchaîné", |s| {
                    let mut ca = s.clip(a).ok_or(SessionError::UnknownClip(a))?.clone();
                    let mut cb = s.clip(b).ok_or(SessionError::UnknownClip(b))?.clone();
                    // Les erreurs de forme ont été exclues par `validate` ; une forme inconnue laisserait le fondu tel quel.
                    ca.fade_out = to_fade(ca.fade_out, &curve_out).unwrap_or(ca.fade_out);
                    cb.fade_in = to_fade(cb.fade_in, &curve_in).unwrap_or(cb.fade_in);
                    s.set_clip(a, ca)?;
                    s.set_clip(b, cb)
                });
                self.finish_edit(name, r)
            }
            Command::SetCrossfadeLength { clip_out, clip_in, length, anchor } => {
                let (Some(ClipRef::Audio(a)), Some(ClipRef::Audio(b))) = (self.find_clip(clip_out), self.find_clip(clip_in)) else {
                    return error("set_crossfade_length : deux clips audio existants".into());
                };
                let anchor = match anchor.as_str() {
                    "start" => Anchor::Start,
                    "end" => Anchor::End,
                    _ => Anchor::Center,
                };
                let r = self.session.set_crossfade_length(a, b, length, anchor);
                self.finish_edit(name, r)
            }
            Command::PreviewFade { clip_out, clip_in, clip, side, margin_ms, solo } => {
                let target = match (clip_out, clip_in, clip) {
                    (Some(o), Some(i), _) => match (self.find_clip(o), self.find_clip(i)) {
                        (Some(ClipRef::Audio(out)), Some(ClipRef::Audio(inn))) => PreviewTarget::Crossfade { out, inn },
                        _ => return error("preview_fade : deux clips audio existants".into()),
                    },
                    (_, _, Some(c)) => match self.find_clip(c) {
                        Some(ClipRef::Audio(clip)) => PreviewTarget::Single { clip, side_in: side.as_deref() == Some("in") },
                        _ => return error("preview_fade : clip audio inexistant".into()),
                    },
                    _ => return error("preview_fade : cible manquante".into()),
                };
                let keep = self.preview.take();
                let (saved_pos, saved_playing, saved_loop, saved_solos) = match keep {
                    Some(p) => (p.saved_pos, p.saved_playing, p.saved_loop, p.saved_solos),
                    None => (self.handle.position(), self.playing, self.loop_range, self.nodes.iter().map(|n| n.solo).collect()),
                };
                self.preview = Some(Preview { target, margin_ms, solo, saved_pos, saved_playing, saved_loop, saved_solos });
                let Some((from, _)) = self.apply_preview() else {
                    // Rien à écouter (fondu absent) : on rend tout et on le dit.
                    let mut events = self.end_preview(true, true);
                    events.push(Event::Error { command: name.into(), message: "pas de fondu à écouter (longueur nulle ou clips qui ne se chevauchent pas)".into() });
                    return events;
                };
                self.handle.send(EngineCommand::Seek(from));
                self.handle.send(EngineCommand::Play);
                self.playing = true;
                vec![Event::Preview { active: true }, self.project_event(), self.position_event()]
            }
            Command::ListPlugins => {
                if let Some(list) = &self.catalog {
                    return vec![Event::Plugins { plugins: list.clone(), scanning: false }];
                }
                if self.scan_rx.is_none() {
                    let scan = self.scan.take().unwrap_or_else(default_scan);
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::Builder::new().name("balayage-plugins".into()).spawn(move || drop(tx.send(scan()))).expect("fil de balayage");
                    self.scan_rx = Some(rx);
                }
                vec![Event::Plugins { plugins: Vec::new(), scanning: true }]
            }
            Command::AddPlugin { node, clap_id, file } => {
                if node as usize >= self.session.graph().nodes.len() {
                    return error(format!("nœud inconnu : {node}"));
                }
                let entry = PluginEntry { node, name: String::new(), clap_id: clap_id.clone(), vendor: String::new(), file: file.unwrap_or_default(), state: None, bypass: false, sidechain: None };
                let Some((factory, pref)) = (self.resolver)(&entry) else { return error(format!("plugin introuvable : {clap_id}")) };
                match self.session.add_effect(NodeId(node), &pref.name, factory) {
                    Ok(id) => {
                        self.meta.plugins.insert(id, pref);
                        self.finish_edit(name, Ok(()))
                    }
                    Err(e) => self.finish_edit(name, Err(e)),
                }
            }
            Command::RemovePlugin { insert } => {
                let handle = &self.handle;
                let r = self.session.remove_effect(insert, &mut |ins| with_state(Arc::clone(&ins.factory), handle.insert_control(ins.id).and_then(|c| c.save_state())));
                self.finish_edit(name, r)
            }
            Command::MovePlugin { insert, before } => {
                let r = self.session.move_effect(insert, before);
                self.finish_edit(name, r)
            }
            Command::SetTempo { bpm, at_bar } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let Some(at) = self.session.meter_map().bar_start(at_bar - 1) else { return error(format!("mesure {at_bar} hors des limites")) };
                // Trois décimales exactes : le tempo est un rationnel (par exemple 133,333 = 133333/1000).
                let Some(tempo) = Tempo::new((bpm * 1000.0).round() as u32, 1000) else { return error("tempo invalide".into()) };
                let r = self.session.set_tempo(at, tempo);
                self.finish_edit(name, r)
            }
            Command::GetChords => vec![self.chords_event(Vec::new())],
            Command::SetReturnOutput { node, output } => {
                if node as usize >= self.session.graph().nodes.len() {
                    return error(format!("nœud inconnu : {node}"));
                }
                let phys = match output {
                    None => None,
                    Some(o) => {
                        let last = u32::from(o.first) + if o.stereo { 2 } else { 1 } - 1;
                        if o.first < 3 {
                            return error("les sorties 1 et 2 sont celles de la sortie principale : un retour prend les suivantes".into());
                        }
                        if self.audio_outputs > 0 && last > self.audio_outputs {
                            return error(format!("sortie {last} inexistante : le pilote en a ouvert {}", self.audio_outputs));
                        }
                        Some(daw_engine::PhysOut { first: o.first - 1, stereo: o.stereo, mono: o.mono })
                    }
                };
                // Un retour qui redevient un bus ordinaire : les décalages de niveau de ses groupes sont rendus (les niveaux d'envoi
                // reviennent à ce qu'ils étaient), sinon ils continueraient d'agir sur la sortie principale. La coupe et le solo des envois
                // n'ont plus d'effet vers un bus ordinaire (le moteur les ignore) ; les groupes propres du retour sont conservés mais ignorés.
                let mut events = if phys.is_none() && self.session.graph().nodes[node as usize].phys.is_some() { self.clear_return_offsets(node) } else { Vec::new() };
                let r = self.session.set_node_phys(NodeId(node), phys);
                events.extend(self.finish_edit(name, r));
                events
            }
            Command::GetChordCatalog => vec![Self::chord_catalog_event()],
            Command::SetChords { track, label } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let (new, report) = match daw_session::chords::ChordTrack::from_carte_json(&daw_session::chords::ChordTrack::default(), &track.to_string()) {
                    Ok(t) => t,
                    Err(e) => return error(e.to_string()),
                };
                // Une modification de la page est refusée en entier si une partie n'est pas comprise (jamais de perte silencieuse).
                if let Some(first) = report.skipped.first() {
                    return error(format!("{first}{}", if report.skipped.len() > 1 { format!(" (et {} autre(s))", report.skipped.len() - 1) } else { String::new() }));
                }
                let r = self.session.set_chord_track(label.as_deref().unwrap_or("Modifier les accords"), new);
                self.finish_edit(name, r)
            }
            Command::SetStructure { sections, label } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let mut list = Vec::new();
                for s in sections {
                    let (Some(at), Some(dur)) = (daw_core_types::time::Frac::new(s.at[0], s.at[1]), daw_core_types::time::Frac::new(s.dur[0], s.dur[1])) else {
                        return error("set_structure : fraction à dénominateur nul".into());
                    };
                    list.push(daw_session::structure::Section { at, dur, name: s.name, color: s.color, category: s.category });
                }
                let r = self.session.set_structure(label.as_deref().unwrap_or("Modifier la structure"), list);
                self.finish_edit(name, r)
            }
            Command::SetLyrics { syllables, label } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let mut list = Vec::new();
                for s in syllables {
                    let Some(at) = daw_core_types::time::Frac::new(s.at[0], s.at[1]) else {
                        return error("set_lyrics : fraction à dénominateur nul".into());
                    };
                    let end = match s.end {
                        Some(e) => match daw_core_types::time::Frac::new(e[0], e[1]) {
                            Some(f) => Some(f),
                            None => return error("set_lyrics : fraction de fin à dénominateur nul".into()),
                        },
                        None => None,
                    };
                    list.push(daw_session::lyrics::Syllable { at, text: s.text, hyphen: s.hyphen, verse: s.verse, end });
                }
                let r = self.session.set_lyrics(label.as_deref().unwrap_or("Modifier les paroles"), list);
                self.finish_edit(name, r)
            }
            Command::StartMonitor { port, local_only } => self.start_monitor(port, local_only.unwrap_or(false)),
            Command::StopMonitor => self.stop_monitor(),
            Command::RenewMonitorAddress { node } => self.renew_monitor_address(node),
            Command::GetMonitorLog => vec![Event::MonitorLog { entries: self.monitor.as_ref().map(|m| m.journal()).unwrap_or_default() }],
            Command::SetMonitorRole { node, role } => self.set_monitor_role(node, role),
            Command::ImportChords { path } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let text = match std::fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => return error(format!("lecture de « {path} » : {e}")),
                };
                match self.session.import_carte(&text) {
                    Ok(report) => {
                        let mut events = self.finish_edit(name, Ok(()));
                        // Le fichier a pu ne rien changer (aucun événement `chords` dans `finish_edit`) : on dit quand même ce qui a été ignoré.
                        events.retain(|e| !matches!(e, Event::Chords { .. }));
                        events.push(self.chords_event(report.skipped));
                        events
                    }
                    Err(e) => error(explain(&e)),
                }
            }
            Command::ExportChords { path } => {
                let text = self.session.chord_track().to_carte_json(&self.name);
                match std::fs::write(&path, text) {
                    Ok(()) => Vec::new(),
                    Err(e) => error(format!("écriture de « {path} » : {e}")),
                }
            }
            Command::GetNotes { clip } => match self.find_clip(clip) {
                Some(ClipRef::Midi(mid)) => self.notes_event(clip, mid, Vec::new()).map_or_else(|| error("clip de notes introuvable".into()), |e| vec![e]),
                _ => error(format!("clip de notes inconnu : {clip}")),
            },
            Command::AddNotes { clip, notes } => {
                let Some(ClipRef::Midi(mid)) = self.find_clip(clip) else { return error(format!("clip de notes inconnu : {clip}")) };
                let mut list = Vec::new();
                for n in &notes {
                    let (Some(start), Some(len)) = (Frac::new(n.start[0], n.start[1]), Frac::new(n.len[0], n.len[1])) else { return error("fraction invalide".into()) };
                    list.push(MNote { start, len, key: n.key, velocity: n.velocity as f32, channel: 0, voice: n.voice.unwrap_or(1) });
                }
                let r = self.session.add_notes(mid, &list);
                self.finish_notes_edit(name, clip, mid, r)
            }
            Command::SetEvents { clip, kind, controller, events, label } => {
                let Some(ClipRef::Midi(mid)) = self.find_clip(clip) else { return error(format!("clip de notes inconnu : {clip}")) };
                let mut list = Vec::new();
                for e in &events {
                    let Some(start) = Frac::new(e.start[0], e.start[1]) else { return error("fraction invalide".into()) };
                    let kind = match kind.as_str() {
                        "cc" => MKind::ControlChange { controller: controller.unwrap_or(0), value: e.value as u8 },
                        "bend" => MKind::PitchBend { value: e.value as i16 },
                        _ => MKind::ChannelPressure { value: e.value as u8 },
                    };
                    list.push(MEvent { start, channel: e.channel, kind });
                }
                let lane_kind = kind.clone();
                let lane_cc = controller;
                let is_lane = move |ev: &MEvent| match (&ev.kind, lane_kind.as_str()) {
                    (MKind::ControlChange { controller: c, .. }, "cc") => Some(*c) == lane_cc,
                    (MKind::PitchBend { .. }, "bend") | (MKind::ChannelPressure { .. }, "pressure") => true,
                    _ => false,
                };
                let label = label.unwrap_or_else(|| "Modifier une courbe de contrôle".into());
                let r = self.session.replace_events(&label, mid, is_lane, &list).map(|()| Vec::new());
                self.finish_notes_edit(name, clip, mid, r)
            }
            Command::EditNotes { clip, edits, label } => {
                let Some(ClipRef::Midi(mid)) = self.find_clip(clip) else { return error(format!("clip de notes inconnu : {clip}")) };
                let mut list = Vec::new();
                for e in &edits {
                    let frac = |f: Option<[u64; 2]>| f.map(|f| Frac::new(f[0], f[1]));
                    let (Some(start), Some(len)) = (frac(e.start).transpose_ok(), frac(e.len).transpose_ok()) else { return error("fraction invalide".into()) };
                    list.push((e.index, NoteEdit { start, len, key: e.key, velocity: e.velocity.map(|v| v as f32), channel: e.channel, voice: e.voice }));
                }
                let label = label.unwrap_or_else(|| "Modifier des notes".into());
                let r = self.session.edit_notes(mid, &label, &list);
                self.finish_notes_edit(name, clip, mid, r)
            }
            Command::DeleteNotes { clip, indices } => {
                let Some(ClipRef::Midi(mid)) = self.find_clip(clip) else { return error(format!("clip de notes inconnu : {clip}")) };
                let r = self.session.remove_notes(mid, &indices).map(|()| Vec::new());
                self.finish_notes_edit(name, clip, mid, r)
            }
            Command::QuantizeNotes { clip, grid } => {
                let Some(ClipRef::Midi(mid)) = self.find_clip(clip) else { return error(format!("clip de notes inconnu : {clip}")) };
                let Some(g) = Frac::new(grid[0], grid[1]) else { return error("grille invalide".into()) };
                let r = self.session.quantize_notes(mid, g).map(|()| Vec::new());
                self.finish_notes_edit(name, clip, mid, r)
            }
            Command::PreviewNote { node, key, velocity, on } => {
                if node as usize >= self.nodes.len() {
                    return error(format!("piste inconnue : {node}"));
                }
                let msg = if on { MidiMsg::NoteOn { channel: 0, key, velocity: velocity as f32 } } else { MidiMsg::NoteOff { channel: 0, key, velocity: 0.0 } };
                self.handle.send(EngineCommand::Midi { node, msg });
                Vec::new()
            }
            Command::RemoveTempo { pos } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let Some(at) = Frac::new(pos[0], pos[1]) else { return error("position invalide".into()) };
                let r = self.session.remove_tempo(at);
                self.finish_edit(name, r)
            }
            Command::RemoveMeter { at_bar } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let r = self.session.remove_meter(at_bar - 1);
                self.finish_edit(name, r)
            }
            Command::SetMeter { num, den, at_bar } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let Some(meter) = Meter::new(num, den) else { return error("chiffrage invalide".into()) };
                let r = self.session.set_meter(at_bar - 1, meter);
                self.finish_edit(name, r)
            }
            Command::SetTrackDynamics { node, dynamics } => {
                let old = self.session.input_dynamics(NodeId(node));
                if let Err(e) = self.session.set_input_dynamics_untracked(NodeId(node), dynamics.to_engine()) {
                    return error(explain(&e));
                }
                if let Err(e) = self.handle.load_graph(self.session.graph()) {
                    let _ = self.session.set_input_dynamics_untracked(NodeId(node), old);
                    return error(format!("le moteur a refusé la modification : {e:?}"));
                }
                self.dirty = true;
                vec![self.project_event()]
            }
            Command::SetTrackVelocityCurve { node, curve } => {
                let old = self.session.velocity_curve(NodeId(node));
                if let Err(e) = self.session.set_velocity_curve_untracked(NodeId(node), curve.to_engine()) {
                    return error(explain(&e));
                }
                if let Err(e) = self.handle.load_graph(self.session.graph()) {
                    let _ = self.session.set_velocity_curve_untracked(NodeId(node), old);
                    return error(format!("le moteur a refusé la modification : {e:?}"));
                }
                self.dirty = true;
                vec![self.project_event()]
            }
            Command::SetTrackMidiFilter { node, rules } => {
                let rules: Vec<daw_session::midi_filter::MidiRule> = rules
                    .into_iter()
                    .map(|r| daw_session::midi_filter::MidiRule { device: r.device, channels: if r.channels.is_empty() { daw_session::midi_filter::ALL_CHANNELS } else { r.channels.iter().fold(0u16, |m, &c| m | (1u16 << (c - 1))) } })
                    .collect();
                match self.session.set_midi_filter_untracked(NodeId(node), rules) {
                    Ok(()) => {
                        self.dirty = true;
                        vec![self.project_event()]
                    }
                    Err(e) => error(explain(&e)),
                }
            }
            Command::SetTrackInput { node, input } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let is_track = self.session.graph().nodes.get(node as usize).is_some_and(|n| matches!(n.kind, NodeKind::Track(_)));
                if !is_track {
                    return error(format!("set_track_input : {node} n'est pas une piste"));
                }
                let mut inp = self.session.node_input(NodeId(node));
                if input.kind == "midi" {
                    inp.midi = true;
                } else {
                    if let Some(bad) = input.channels.iter().find(|&&c| c > self.audio_inputs) {
                        return error(format!("entrée {bad} inexistante : le pilote en a ouvert {}", self.audio_inputs));
                    }
                    inp.midi = false;
                    inp.stereo = input.channels.len() == 2;
                    inp.channels = [(input.channels[0] - 1) as u8, (*input.channels.get(1).unwrap_or(&input.channels[0]) - 1) as u8];
                }
                self.session.set_node_input_untracked(NodeId(node), inp);
                self.dirty = true;
                self.apply_input(node);
                vec![self.project_event()]
            }
            Command::Record => self.start_recording(),
            Command::StopRecord => {
                if self.recording.is_none() {
                    return vec![Event::Recording { active: false, stopping: false, overruns: 0, warnings: Vec::new(), start: 0, nodes: Vec::new() }];
                }
                let rate = self.session.sample_rate() as f64;
                if self.metro.post_roll > 0.0 && self.stop_at.is_none() && self.playing {
                    // Post-roll : on continue d'enregistrer ; un second arrêt (ou l'arrêt du transport) coupe tout de suite.
                    self.stop_at = Some(self.handle.position() + (self.metro.post_roll * rate) as u64);
                    return vec![self.recording_event(true, false, self.recording.as_ref().map_or(0, |r| r.last_overruns), Vec::new())];
                }
                self.stop_at = None;
                let mut events = self.begin_stop_recording();
                self.playing = false;
                self.handle.send(EngineCommand::Stop);
                self.settle_after_stop();
                self.send_return_seek();
                events.push(self.position_event());
                events
            }
            Command::SetPluginBypass { insert, bypass } => {
                if !self.session.set_effect_bypass_untracked(insert, bypass) {
                    return error(format!("plugin inconnu : {insert}"));
                }
                self.handle.send(EngineCommand::SetInsertBypass { insert, bypass });
                self.dirty = true;
                vec![self.project_event()]
            }
            Command::SetInsertSidechain { insert, source } => {
                let old = self.session.effect_sidechain(insert);
                if let Err(e) = self.session.set_effect_sidechain_untracked(insert, source.map(NodeId)) {
                    return error(explain(&e));
                }
                if let Err(e) = self.handle.load_graph(self.session.graph()) {
                    let _ = self.session.set_effect_sidechain_untracked(insert, old);
                    return error(format!("le moteur a refusé la modification : {e:?}"));
                }
                self.dirty = true;
                vec![self.project_event()]
            }
            Command::GetPluginParams { insert } => {
                let Some(ph) = self.plugin_handle(insert) else { return error(format!("plugin inconnu : {insert}")) };
                let params = ph
                    .params()
                    .into_iter()
                    .map(|p| {
                        let value = ph.param_value(p.id).unwrap_or(p.default);
                        ParamEntry { id: p.id, name: p.name, module: p.module, min: p.min, max: p.max, default: p.default, value, text: ph.param_text(p.id, value).unwrap_or_default() }
                    })
                    .collect();
                vec![Event::PluginParams { insert, params }]
            }
            Command::GetControlPages => vec![self.control_pages_event()],
            Command::SetControlPage { plugin_id, name, params } => {
                if let Err(message) = self.bank.set(crate::control_pages::PageEntry { plugin_id, name, params }) {
                    return error(message);
                }
                self.bank_error = None;
                if let Some(path) = &self.bank_path {
                    if let Err(e) = crate::control_pages::save(path, &self.bank) {
                        self.bank_error = Some(e);
                    }
                }
                vec![self.control_pages_event()]
            }
            Command::GetInsertPage { insert } => self.insert_page_event(insert),
            Command::GetPluginParamText { insert, param, value } => {
                let Some(ph) = self.plugin_handle(insert) else { return error(format!("plugin inconnu : {insert}")) };
                vec![Event::PluginParamText { insert, param, value, text: ph.param_text(param, value).unwrap_or_default() }]
            }
            Command::SetPluginParam { insert, param, value } => {
                let Some(ph) = self.plugin_handle(insert) else { return error(format!("plugin inconnu : {insert}")) };
                self.handle.send(EngineCommand::SetInsertParam { insert, param, value });
                self.dirty = true;
                self.plugin_values.insert((insert, param), value);
                let text = ph.param_text(param, value).unwrap_or_default();
                let owner = self.session.graph().nodes.iter().position(|n| n.inserts.iter().any(|i| i.id == insert));
                let mut events = match owner {
                    Some(n) => self.note_value(n as u32, AutoTarget::InsertParam { insert, param }, value),
                    None => Vec::new(),
                };
                events.push(Event::PluginParam { insert, param, value, text });
                events
            }
            Command::AddTrack { kind, name: given } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let new = match kind.as_str() {
                    "mono" => NewNode::Mono,
                    "stereo" => NewNode::Stereo,
                    _ => NewNode::Bus,
                };
                let label = if new == NewNode::Bus { "Bus" } else { "Piste" };
                let track_name = given.unwrap_or_else(|| self.free_name(label));
                let color = TRACK_COLORS[(self.session.graph().nodes.len().saturating_sub(1)) % TRACK_COLORS.len()];
                let mut events = self.end_preview(true, false);
                let r = self.session.add_node(new, &track_name, color).map(|_| ());
                events.extend(self.finish_edit(name, r));
                events
            }
            Command::DeleteTrack { node } => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                let mut events = self.end_preview(true, false);
                let handle = &self.handle;
                let r = self.session.remove_node(NodeId(node), &mut |ins| {
                    let state = handle.insert_control(ins.id).and_then(|c| c.save_state());
                    with_state(Arc::clone(&ins.factory), state)
                });
                if r.is_ok() {
                    // Les plugins gardés tels quels (introuvables) suivent la renumérotation ; ceux de la piste disparaissent.
                    self.kept_plugins.retain(|p| p.node != node);
                    for p in &mut self.kept_plugins {
                        if p.node > node {
                            p.node -= 1;
                        }
                    }
                }
                events.extend(self.finish_edit(name, r));
                events
            }
            Command::RenameTrack { node, name: new_name } => {
                let r = self.session.rename_node(NodeId(node), &new_name);
                self.finish_edit(name, r)
            }
            Command::SetTrackColor { node, color } => {
                let r = self.session.set_node_color(NodeId(node), &color.to_ascii_lowercase());
                self.finish_edit(name, r)
            }
            Command::SetLane { node, target, mode, points, label } => {
                let n = NodeId(node);
                if node as usize >= self.session.graph().nodes.len() {
                    return error(format!("nœud inconnu : {node}"));
                }
                let t = lane_target(target);
                if points.is_empty() {
                    let r = self.session.remove_lane(n, t);
                    return self.finish_edit(name, r);
                }
                let mode = match mode.as_deref() {
                    Some(m) => lane_mode(m),
                    None => self.session.lane(n, t).map_or(AutoMode::Trim, |l| l.mode), // une courbe neuve est en « trim » : le fader reste utilisable
                };
                let mut list = Vec::new();
                for p in &points {
                    let Some(pos) = Frac::new(p.pos[0], p.pos[1]) else { return error("fraction invalide".into()) };
                    list.push(MPoint { pos, value: p.value, curve: if p.curve == "step" { AutoCurve::Step } else { AutoCurve::Linear } });
                }
                let r = self.session.set_lane_as(label.as_deref().unwrap_or("Modifier l'automation"), n, MLane { target: t, points: list, mode });
                self.finish_edit(name, r)
            }
            Command::SetWriteMode { node, target, mode } => self.set_write_mode(node, target, &mode),
            Command::Gesture { node, target, active } => self.gesture(node, target, active),
            Command::SetLaneMode { node, target, mode } => {
                let n = NodeId(node);
                let t = lane_target(target);
                let Some(lane) = self.session.lane(n, t).cloned() else { return error("pas de courbe pour cette cible".into()) };
                let r = self.session.set_lane_as("Mode d'automation", n, MLane { mode: lane_mode(&mode), ..lane });
                self.finish_edit(name, r)
            }
            Command::AddSend { node, to } => {
                let r = self.session.add_send(NodeId(node), NodeId(to));
                self.finish_edit(name, r)
            }
            Command::RemoveSend { node, to } => {
                let r = self.session.remove_send(NodeId(node), NodeId(to));
                self.finish_edit(name, r)
            }
            Command::CreateGroup { name, color, nodes, ret } => self.create_group(name, color, nodes, ret),
            Command::EditGroup { group, name, color, nodes, ret } => self.edit_group(group, name, color, nodes, ret),
            Command::DeleteGroup { group, ret } => self.delete_group(group, ret),
            Command::CustomizeReturnGroups { ret } => self.customize_return_groups(ret),
            Command::ResetReturnGroups { ret } => self.reset_return_groups(ret),
            Command::SetGroup { group, mute, solo, delta_db, to } => self.set_group(group, mute, solo, delta_db, to),
            Command::SetGroupOffset { group, db, to } => self.set_group_offset(group, db, to),
            Command::SetSend { node, to, gain, pre, mute, solo } => {
                if node as usize >= self.session.graph().nodes.len() {
                    return error(format!("nœud inconnu : {node}"));
                }
                let Some(index) = self.session.graph().nodes[node as usize].sends.iter().position(|s| s.to == NodeId(to)) else { return error(format!("pas d'envoi de {node} vers {to}")) };
                let mut events = Vec::new();
                if let Some(p) = pre {
                    let r = self.session.set_send_pre(NodeId(node), NodeId(to), p);
                    events = self.finish_edit(name, r);
                    if events.iter().any(|e| matches!(e, Event::Error { .. })) {
                        return events;
                    }
                }
                if mute.is_some() || solo.is_some() {
                    self.session.set_send_flags_untracked(NodeId(node), NodeId(to), mute, solo);
                    if let Some(m) = mute {
                        self.handle.send(EngineCommand::SetSendMute { node, send: index as u32, mute: m });
                    }
                    if let Some(s) = solo {
                        self.handle.send(EngineCommand::SetSendSolo { node, send: index as u32, solo: s });
                    }
                    self.dirty = true;
                    if gain.is_none() && pre.is_none() {
                        events.push(self.project_event());
                    }
                }
                if let Some(g) = gain {
                    self.session.set_send_gain_untracked(NodeId(node), NodeId(to), g as f32);
                    self.handle.send(EngineCommand::SetSendGain { node, send: index as u32, gain: g as f32 });
                    self.dirty = true;
                    events.extend(self.note_value(node, AutoTarget::SendGain { to: NodeId(to) }, g));
                    if pre.is_none() {
                        events.push(self.project_event());
                    }
                }
                events
            }
            Command::SetTrackOutput { node, output } => {
                let r = self.session.set_node_output(NodeId(node), NodeId(output));
                self.finish_edit(name, r)
            }
            Command::MoveTrack { node, before } => {
                let r = self.session.move_node(NodeId(node), before.map(NodeId));
                self.finish_edit(name, r)
            }
            Command::OpenPluginUi { insert } => self.open_plugin_ui(insert),
            Command::ClosePluginUi { insert } => self.close_plugin_ui(insert),
            Command::ConvertProject { path } => self.start_convert(&path),
            Command::CancelConvert => self.cancel_convert(),
            Command::ProbeAudio { files } => self.probe_audio(&files),
            Command::ImportAudio { .. } => self.start_import(cmd),
            Command::CancelImport => self.cancel_import(),
            Command::AlignClips { .. } | Command::AlignRedo { .. } => self.start_align(cmd),
            Command::CancelAlign => self.cancel_align(),
            Command::ListenAlign { guide, guide_more, node, aligned, what } => self.listen_align(guide, guide_more, node, aligned, what),
            Command::StopListen => self.end_listen(true, true),
            Command::GetAlign { clip, settings, threshold_ms } => self.get_align(clip, settings, threshold_ms),
            Command::MergeAlignSettings { dst, src } => self.merge_align_settings(&dst, &src),
            Command::Export { .. } => self.start_export(cmd),
            Command::CancelExport => self.cancel_export(),
            Command::NewProject => match self.refuse_while_recording(name).or_else(|| self.refuse_while_exporting(name)).or_else(|| self.refuse_while_importing(name)).or_else(|| self.refuse_while_aligning(name)).or_else(|| self.refuse_while_converting(name)) {
                Some(refused) => refused,
                None => self.new_project(),
            },
            Command::OpenProject { path, recover } => match self.refuse_while_recording(name).or_else(|| self.refuse_while_exporting(name)).or_else(|| self.refuse_while_importing(name)).or_else(|| self.refuse_while_aligning(name)).or_else(|| self.refuse_while_converting(name)) {
                Some(refused) => refused,
                None => self.open_project(&path, recover),
            },
            Command::DiscardAutosave { path } => {
                project_io::remove_autosave(std::path::Path::new(&path));
                Vec::new()
            }
            Command::SaveProject { path } => self.save_project(path.as_deref()),
            Command::Panic => {
                self.handle.send(EngineCommand::Panic);
                vec![self.position_event()]
            }
            Command::StopPreview => {
                if self.preview.is_none() {
                    return vec![Event::Preview { active: false }];
                }
                self.end_preview(true, true)
            }
            Command::Undo => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                if !self.session.can_undo() {
                    return error("rien à annuler".into());
                }
                let mut pre = if matches!(self.session.undo_label(), Some("Ajouter une piste" | "Supprimer une piste")) { self.end_preview(true, false) } else { Vec::new() };
                self.session.undo();
                pre.extend(self.finish_edit(name, Ok(())));
                pre
            }
            Command::Redo => {
                if let Some(refused) = self.refuse_while_recording(name) {
                    return refused;
                }
                if !self.session.can_redo() {
                    return error("rien à rétablir".into());
                }
                let mut pre = if matches!(self.session.redo_label(), Some("Ajouter une piste" | "Supprimer une piste")) { self.end_preview(true, false) } else { Vec::new() };
                self.session.redo();
                pre.extend(self.finish_edit(name, Ok(())));
                pre
            }
            Command::GetPeaks { clip, from, to, columns } => match self.peaks_for(clip, from, to, columns) {
                Ok(columns) => vec![Event::Peaks { clip, from, to, columns }],
                Err(message) => error(message),
            },
        }
    }

    /// Témoin d'entrée MIDI : relève les types de messages reçus par chaque piste, les retient `MIDI_HOLD` puis signale à la page
    /// l'ensemble des pistes actives quand il a changé.
    fn midi_activity_tick(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        for i in 0..self.nodes.len().min(MAX_METERED) {
            let v = self.meters.take_velocity(i);
            if v != 0 {
                out.push(Event::NoteVelocity { node: i as u32, input: v });
            }
        }
        const MIDI_HOLD: Duration = Duration::from_millis(150);
        let now = Instant::now();
        for i in 0..self.nodes.len().min(MAX_METERED) {
            let bits = self.meters.take_midi(i);
            if bits != 0 {
                let e = self.midi_act.entry(i as u32).or_insert((0, now));
                e.0 = if now.duration_since(e.1) > MIDI_HOLD { bits } else { e.0 | bits };
                e.1 = now;
            }
        }
        self.midi_act.retain(|_, (_, t)| now.duration_since(*t) <= MIDI_HOLD);
        let active: BTreeMap<String, u8> = self.midi_act.iter().map(|(k, (b, _))| (k.to_string(), *b)).collect();
        if active != self.midi_act_sent {
            self.midi_act_sent = active.clone();
            out.push(Event::MidiActivity { active });
        }
        out
    }

    /// À appeler environ 30 fois par seconde : position de lecture et niveaux de crête.
    pub fn tick(&mut self) -> Vec<Event> {
        let mut levels = BTreeMap::new();
        for i in 0..self.nodes.len().min(MAX_METERED) {
            let (l, r) = self.meters.take(i);
            levels.insert(i.to_string(), [l, r]);
            if self.monitor.is_some() {
                let now = Instant::now();
                let p = l.max(r);
                let e = self.peak_hold.entry(i as u32).or_insert((0.0, now));
                if p >= e.0 || now.duration_since(e.1) > Duration::from_millis(600) {
                    *e = (p, now);
                }
            }
        }
        let mut events = vec![self.position_event(), Event::Meters { levels }];
        events.extend(self.midi_activity_tick());
        events.extend(self.monitor_tick());
        // Le balayage des plugins installés est fini : on envoie l'inventaire.
        if let Some(list) = self.scan_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.scan_rx = None;
            self.catalog = Some(list.clone());
            events.push(Event::Plugins { plugins: list, scanning: false });
        }
        if self.prefs_dirty && self.last_prefs_save.elapsed() >= Duration::from_secs(1) {
            if let Err(message) = self.save_prefs() {
                events.push(Event::Error { command: "prefs".into(), message });
            }
        }
        if self.dirty && self.recording.is_none() && self.preview.is_none() && self.listen.is_none() && self.last_autosave.elapsed() >= self.autosave_every {
            self.last_autosave = Instant::now();
            if let Err(message) = self.autosave() {
                events.push(Event::Error { command: "autosave".into(), message });
            }
        }
        if self.stop_at.is_some_and(|at| self.handle.position() >= at) {
            self.stop_at = None;
            events.extend(self.begin_stop_recording());
            self.playing = false;
            self.handle.send(EngineCommand::Stop);
            self.settle_after_stop();
            self.send_return_seek();
            events.push(self.position_event());
        }
        events.extend(self.auto_stop_tick());
        events.extend(self.plugin_param_out_tick());
        events.extend(self.write_tick());
        events.extend(self.poll_recording());
        events.extend(self.export_tick());
        events.extend(self.import_tick());
        events.extend(self.align_tick());
        events.extend(self.convert_tick());
        // Un plugin qui a demandé à être relancé (latence changée...) l'est maintenant : la latence totale a pu changer.
        if matches!(self.handle.poll_restarts(self.session.graph()), Ok(n) if n > 0) {
            events.push(self.project_event());
        }
        events
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Clip désigné par un identifiant du protocole.
    fn find_clip(&self, id: u64) -> Option<ClipRef> {
        if id >= MIDI_CLIP_BASE {
            let mid = MidiClipId(id - MIDI_CLIP_BASE);
            return self.session.midi_clip(mid).map(|_| ClipRef::Midi(mid));
        }
        (0..self.session.graph().nodes.len()).find_map(|i| {
            let node = NodeId(i as u32);
            self.session.clip_ids(node).iter().find(|c| c.0 == id).map(|c| ClipRef::Audio(*c))
        })
    }

    fn owner_of_audio(&self, id: ClipId) -> Option<NodeId> {
        (0..self.session.graph().nodes.len()).map(|i| NodeId(i as u32)).find(|n| self.session.clip_ids(*n).contains(&id))
    }

    fn owner_of_midi(&self, id: MidiClipId) -> Option<NodeId> {
        (0..self.session.graph().nodes.len()).map(|i| NodeId(i as u32)).find(|n| self.session.midi_clip_ids(*n).contains(&id))
    }

    /// Rogne la fenêtre visible d'un clip de notes (début et/ou fin absolus, en échantillons) sans rien détruire.
    fn trim_midi(&mut self, id: MidiClipId, start: Option<u64>, end: Option<u64>) -> Result<(), SessionError> {
        let c = self.session.midi_clip(id).ok_or(SessionError::UnknownMidiClip(id))?.clone();
        let old_end = c.start.checked_add(c.len).ok_or(SessionError::TimeOverflow)?;
        let (mut s, mut len, mut offset) = (c.start, c.len, c.offset);
        if let Some(ns) = start {
            let nf = self.musical(ns)?;
            if nf >= old_end {
                return Err(SessionError::Edit(EditError::OutOfRange));
            }
            offset = if nf >= s { offset.checked_add(nf.checked_sub(s).ok_or(SessionError::TimeOverflow)?) } else { offset.checked_sub(s.checked_sub(nf).ok_or(SessionError::TimeOverflow)?) }.ok_or(SessionError::Edit(EditError::OutOfRange))?;
            len = old_end.checked_sub(nf).ok_or(SessionError::TimeOverflow)?;
            s = nf;
        }
        if let Some(ne) = end {
            let nf = self.musical(ne)?;
            if nf <= s {
                return Err(SessionError::Edit(EditError::OutOfRange));
            }
            len = nf.checked_sub(s).ok_or(SessionError::TimeOverflow)?;
        }
        self.session.set_midi_window(id, s, len, offset)
    }

    /// Position musicale (au tick de 960 par noire) d'un échantillon, pour les clips de notes.
    fn musical(&self, sample: u64) -> Result<Frac, SessionError> {
        let exact = self.session.tempo_map().to_frac(sample, self.session.sample_rate()).ok_or(SessionError::TimeOverflow)?;
        let ticks = TempoMap::to_ticks(exact, RECORD_PPQ).ok_or(SessionError::TimeOverflow)?;
        TempoMap::from_ticks(ticks, RECORD_PPQ).ok_or(SessionError::TimeOverflow)
    }

    /// Poignée de contrôle d'un plugin CLAP vivant (paramètres, état, latence), si `insert` en est un.
    /// Charge la banque de pages de contrôle (fichier de l'utilisateur) : à appeler au démarrage. Un fichier abîmé laisse la banque vide et l'erreur
    /// est montrée à la page ; il n'est réécrit que quand l'utilisateur change une page.
    pub fn set_control_pages_path(&mut self, path: Option<std::path::PathBuf>) {
        self.bank = crate::control_pages::Bank::new();
        self.bank_error = None;
        if let Some(p) = &path {
            match crate::control_pages::load(p) {
                Ok(b) => self.bank = b,
                Err(e) => self.bank_error = Some(e),
            }
        }
        self.bank_path = path;
    }

    fn control_pages_event(&self) -> Event {
        Event::ControlPages { entries: self.bank.entries(), error: self.bank_error.clone() }
    }

    /// La page de contrôle d'un plugin du projet, résolue d'après les paramètres qu'il expose (par identifiant, sinon par nom).
    pub(super) fn insert_page_event(&self, insert: u32) -> Vec<Event> {
        let Some(ph) = self.plugin_handle(insert) else { return vec![Event::Error { command: "get_insert_page".into(), message: format!("plugin inconnu : {insert}") }] };
        let Some(plugin_id) = self.meta.plugins.get(&insert).map(|p| p.clap_id.clone()) else { return vec![Event::Error { command: "get_insert_page".into(), message: "identité du plugin inconnue".into() }] };
        let Some(entry) = self.bank.get(&plugin_id) else { return vec![Event::InsertPage { insert, plugin_id, configured: false, params: Vec::new(), missing: Vec::new() }] };
        let exposed = ph.params();
        let list: Vec<(u32, String)> = exposed.iter().map(|p| (p.id, p.name.clone())).collect();
        let (found, missing) = crate::control_pages::resolve(&entry.params, &list);
        let params = found
            .into_iter()
            .map(|i| {
                let p = &exposed[i];
                let value = self.plugin_values.get(&(insert, p.id)).copied().or_else(|| ph.param_value(p.id)).unwrap_or(p.default);
                ParamEntry { id: p.id, name: p.name.clone(), module: p.module.clone(), min: p.min, max: p.max, default: p.default, value, text: ph.param_text(p.id, value).unwrap_or_default() }
            })
            .collect();
        vec![Event::InsertPage { insert, plugin_id, configured: true, params, missing }]
    }

    fn plugin_handle(&self, insert: u32) -> Option<daw_clap_host::PluginHandle> {
        self.handle.insert_control(insert)?.as_any().downcast_ref::<daw_clap_host::PluginHandle>().cloned()
    }

    /// Change le balayage des plugins installés (les tests y mettent une liste fixe).
    pub fn set_catalog_scan(&mut self, scan: CatalogScan) {
        self.scan = Some(scan);
    }

    // ---- entrées et enregistrement -----------------------------------------------------------------------------

    /// Préférences chargées au démarrage (fichier `path`, où elles seront réécrites) : le métronome en reprend les réglages.
    pub fn set_preferences(&mut self, prefs: Prefs, path: Option<std::path::PathBuf>) {
        let m = prefs.metronome;
        self.metro = Metro { enabled: m.enabled, only_recording: m.only_recording, volume: m.volume, count_in_bars: m.count_in_bars, pre_roll: m.pre_roll, post_roll: m.post_roll, pre_roll_bars: m.pre_roll_bars, pre_roll_unit: m.pre_roll_unit };
        self.prefs = prefs;
        self.prefs_path = path;
        self.prefs_dirty = false;
        self.apply_metronome();
    }

    /// Ce qui tourne (pilote, tampon, fils…), pour la fenêtre des réglages.
    pub fn set_runtime(&mut self, runtime: Runtime) {
        self.runtime = Some(runtime);
    }

    fn save_prefs(&mut self) -> Result<(), String> {
        self.prefs_dirty = false;
        self.last_prefs_save = Instant::now();
        match &self.prefs_path {
            Some(path) => prefs::save(path, &self.prefs),
            None => Ok(()),
        }
    }

    fn prefs_event(&self) -> Event {
        let env = |k: &str| std::env::var(k).ok();
        let restart_needed = self.runtime.as_ref().is_some_and(|r| {
            let want = prefs::resolve(&self.prefs, &env);
            let have = &r.effective;
            !prefs::same_driver(&want.driver, &have.driver) || (want.buffer, want.threads, want.ahead_ms, want.ahead_threads) != (have.buffer, have.threads, have.ahead_ms, have.ahead_threads)
        });
        Event::Prefs { prefs: self.prefs.clone(), running: self.runtime.clone(), restart_needed, path: self.prefs_path.as_ref().map(|p| p.to_string_lossy().into_owned()) }
    }

    /// Nombre de canaux d'entrée audio que le pilote a ouverts (0 : aucun).
    /// Dit au protocole combien de canaux de sortie le pilote a ouverts (retours de monitoring possibles au-delà des deux premiers).
    pub fn set_audio_outputs(&mut self, count: u32) {
        self.audio_outputs = count;
    }

    pub fn set_audio_inputs(&mut self, count: u32) {
        self.audio_inputs = count;
    }

    /// Latence aller-retour (échantillons) à retirer des prises pour les aligner sur la ligne de temps : latence annoncée par le
    /// pilote en entrée et en sortie, plus le délai des convertisseurs.
    pub fn set_record_compensation(&mut self, samples: u64) {
        self.record_compensation = samples;
    }

    /// Donne (une seule fois) l'entrée MIDI du moteur : à confier au fil qui reçoit le MIDI des appareils.
    pub fn take_midi_input(&mut self) -> Option<MidiInputTx> {
        self.midi_in.take().or_else(|| self.handle.take_midi_input())
    }

    /// Les appareils d'entrée MIDI que le système voit : proposés dans les filtres d'entrée des pistes.
    pub fn set_midi_devices(&mut self, devices: Vec<String>) {
        self.midi_devices = devices;
    }

    /// Pistes MIDI armées avec leur filtre d'entrée : un message de l'appareil `d`, canal `c`, ne leur est livré que s'il passe le filtre.
    pub fn armed_midi_routes(&self) -> Vec<(u32, Vec<daw_session::midi_filter::MidiRule>)> {
        self.armed_midi_tracks().into_iter().map(|n| (n, self.session.midi_filter(NodeId(n)).to_vec())).collect()
    }

    /// Pistes armées dont l'entrée est le MIDI : les messages des appareils doivent leur être livrés.
    pub fn armed_midi_tracks(&self) -> Vec<u32> {
        (0..self.nodes.len() as u32).filter(|&n| self.nodes[n as usize].arm && self.session.node_input(NodeId(n)).midi).collect()
    }

    /// L'armement et l'écoute du moteur pour une piste, d'après son entrée : audio → le moteur écoute et fournit le ou les canaux
    /// choisis ; MIDI → rien côté audio (les notes arrivent par l'entrée MIDI).
    fn apply_input(&mut self, node: u32) {
        let Some(st) = self.nodes.get(node as usize).copied() else { return };
        let inp = self.session.node_input(NodeId(node));
        if st.arm && !inp.midi {
            self.handle.send(EngineCommand::SetInput { node, armed: true, monitor: inp.monitor, source: Self::source_of(inp) });
        } else {
            self.handle.send(EngineCommand::SetInput { node, armed: false, monitor: false, source: RecordSource::Channel(0) });
        }
    }

    fn source_of(inp: NodeInput) -> RecordSource {
        if inp.stereo { RecordSource::Pair(inp.channels[0], inp.channels[1]) } else { RecordSource::Channel(inp.channels[0]) }
    }

    /// Refuse une commande qui n'a pas de sens pendant un enregistrement (déplacer la tête, changer la structure...).
    fn refuse_while_recording(&self, command: &str) -> Option<Vec<Event>> {
        self.recording.as_ref().map(|_| vec![Event::Error { command: command.into(), message: "arrête d'abord l'enregistrement".into() }])
    }

    fn recording_event(&self, active: bool, stopping: bool, overruns: u32, warnings: Vec<String>) -> Event {
        let (start, nodes) = self.recording.as_ref().map_or((0, Vec::new()), |r| (r.start, r.jobs.iter().map(|j| j.node).collect()));
        Event::Recording { active, stopping, overruns, warnings, start, nodes }
    }

    /// Arrête les prises déjà démarrées quand le démarrage d'une autre échoue.
    fn abort_takes(&mut self, jobs: Vec<TakeJob>) {
        for job in jobs {
            match job.kind {
                TakeKind::Audio { .. } => drop(self.handle.stop_record(job.node)),
                TakeKind::Midi { .. } => drop(self.handle.stop_midi_record(job.node)),
            }
        }
    }

    fn metronome_event(&self) -> Event {
        let m = self.metro;
        Event::Metronome { enabled: m.enabled, only_recording: m.only_recording, volume: m.volume, count_in_bars: m.count_in_bars, pre_roll: m.pre_roll, post_roll: m.post_roll, pre_roll_bars: m.pre_roll_bars, pre_roll_unit: m.pre_roll_unit }
    }

    /// Le clic sonne-t-il maintenant ? Allumé, et (si « seulement pendant l'enregistrement ») une prise est en cours.
    fn apply_metronome(&mut self) {
        let recording = self.recording.as_ref().is_some_and(|r| !r.stopping);
        let on = self.metro.enabled && (!self.metro.only_recording || recording);
        self.handle.send(EngineCommand::SetMetronome { enabled: on, gain: self.metro.volume as f32 });
    }

    /// Fabrique le clic (un coup par temps, accent sur le premier de chaque mesure) à partir des cartes de tempo et de chiffrage,
    /// et le confie au moteur. Refait seulement si ces cartes ont changé.
    fn refresh_click(&mut self) {
        let key = format!("{:?}|{:?}|{}", self.session.tempo_map().changes(), self.session.meter_map().changes(), self.session.sample_rate());
        if key == self.click_key {
            return;
        }
        self.click_key = key;
        let rate = self.session.sample_rate();
        let horizon = CLICK_HORIZON_SECONDS * rate as u64;
        let (tm, mm) = (self.session.tempo_map(), self.session.meter_map());
        let mut clicks = Vec::new();
        'bars: for bar in 0u32..20_000 {
            let (Some(start), meter) = (mm.bar_start(bar), mm.meter_at_bar(bar)) else { break };
            for beat in 0..meter.num() {
                let Some(at) = meter.beat_len().checked_mul_int(beat as u64).and_then(|d| start.checked_add(d)) else { break 'bars };
                let Some(pos) = tm.to_samples(at, rate) else { break 'bars };
                if pos > horizon {
                    break 'bars;
                }
                clicks.push(daw_engine::Click { pos, accent: beat == 0 });
            }
        }
        let track = daw_engine::ClickTrack::new(rate, clicks);
        drop(self.handle.set_click_track(track));
    }

    /// Pré-roll avant le point d'enregistrement `at` (échantillons) : en secondes, ou en mesures — la lecture part alors du début de la
    /// mesure située `pre_roll_bars` mesures avant celle qui contient `at`, d'après les cartes de tempo et de chiffrage.
    fn pre_roll_samples(&self, at: u64) -> u64 {
        let rate = self.session.sample_rate();
        match self.metro.pre_roll_unit {
            prefs::PreRollUnit::Seconds => (self.metro.pre_roll * rate as f64) as u64,
            prefs::PreRollUnit::Bars => {
                let (tm, mm) = (self.session.tempo_map(), self.session.meter_map());
                let start = tm.to_frac(at, rate).and_then(|f| mm.locate(f)).and_then(|(bar, _)| mm.bar_start(bar.saturating_sub(self.metro.pre_roll_bars))).and_then(|f| tm.to_samples(f, rate));
                start.map_or(0, |s| at.saturating_sub(s))
            }
        }
    }

    /// Pré-compte de `bars` mesures au tempo et au chiffrage de la position `pos` : clics à partir de 0, durée totale.
    fn count_in_for(&self, pos: u64, bars: u32) -> daw_engine::CountIn {
        let rate = self.session.sample_rate();
        let (tm, mm) = (self.session.tempo_map(), self.session.meter_map());
        let at = tm.to_frac(pos, rate).unwrap_or_else(|| Frac::new(0, 1).expect("zéro"));
        let bar = mm.locate(at).map_or(0, |(b, _)| b);
        let meter = mm.meter_at_bar(bar);
        // Un temps dure beat_len de ronde = 4 × beat_len noires ; le tempo est en noires par minute.
        let beat_seconds = meter.beat_len().as_f64() * 4.0 * 60.0 / tm.tempo_at(at).as_f64();
        let beat = (beat_seconds * rate as f64).round() as u64;
        let mut clicks = Vec::new();
        for b in 0..bars * meter.num() {
            clicks.push((b as u64 * beat, b % meter.num() == 0));
        }
        daw_engine::CountIn { clicks, frames: (bars * meter.num()) as u64 * beat }
    }

    fn start_recording(&mut self) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "record".into(), message }];
        if self.recording.is_some() {
            return error("un enregistrement est déjà en cours".into());
        }
        let Some(dir) = self.project_dir.clone() else { return error("pas encore de dossier pour les prises : utilise « Enregistrer sous » d'abord".into()) };
        // Punch allumé : la zone est la bande bleue (la sélection de la règle). Sans punch, une boucle empêche d'enregistrer.
        // Enregistrement en boucle : la même zone, mais la boucle continue de tourner et la prise ne se ferme pas toute seule.
        let loop_mode = self.loop_rec_on && !self.punch_on;
        let zone = if self.punch_on || loop_mode {
            match self.loop_range {
                Some(z) => Some(z),
                None if loop_mode => return error("enregistrement en boucle : pose d'abord une zone (bande bleue de la règle) ou éteins-le".into()),
                None => return error("punch allumé : pose d'abord une zone (bande bleue de la règle) ou éteins le punch".into()),
            }
        } else {
            None
        };
        if self.loop_range.is_some() && zone.is_none() {
            return error("désactive la boucle avant d'enregistrer (ou allume Punch pour enregistrer dans la zone)".into());
        }
        let armed: Vec<u32> = (0..self.nodes.len() as u32).filter(|&n| self.nodes[n as usize].arm).collect();
        if armed.is_empty() {
            return error("aucune piste armée : arme une piste (●) pour enregistrer".into());
        }
        let mut events = self.end_preview(true, false);
        // Départ de la prise : la position de la tête, ou le début de la zone de punch. Avec pré-compte ou pré-roll, la lecture démarre
        // avant et les prises attendent. Avec une zone de punch, les prises commencent et finissent exactement à ses bords.
        let head = self.handle.position();
        let (at, start_at, end_at) = match zone {
            Some(z) if self.playing && head >= z.end => return error("la tête de lecture est après la fin de la zone de punch : reviens avant ou déplace la zone".into()),
            Some(z) if self.playing && head >= z.start => (head, None, if loop_mode { None } else { Some(z.end) }),
            Some(z) => (z.start, Some(z.start), if loop_mode { None } else { Some(z.end) }),
            None => (head, if self.playing { None } else { Some(head) }, None),
        };
        let pre = if self.playing { 0 } else { self.pre_roll_samples(at) };
        let mut jobs: Vec<TakeJob> = Vec::new();
        for node in armed {
            let track_name = self.session.graph().nodes[node as usize].name.clone();
            let inp = self.session.node_input(NodeId(node));
            if inp.midi {
                match self.handle.start_midi_record_window(node, 8192, start_at, end_at) {
                    Ok(rx) => {
                        let number = 1 + self.session.midi_clip_ids(NodeId(node)).len() as u32;
                        jobs.push(TakeJob { node, track_name, number, kind: TakeKind::Midi { rx, events: Vec::new(), passes: Vec::new() } });
                    }
                    Err(e) => {
                        self.abort_takes(jobs);
                        return error(format!("prise MIDI de « {track_name} » : {e:?}"));
                    }
                }
                continue;
            }
            let (number, rel) = (1u32..)
                .map(|n| (n, format!("{}/{}-prise-{n:03}.wav", project_io::AUDIO_DIR, project_io::sanitize(&track_name))))
                .find(|(_, rel)| !dir.join(rel).exists() && !self.session.assets().iter().any(|(_, p)| p == rel))
                .expect("une suite infinie");
            if let Err(e) = std::fs::create_dir_all(dir.join(project_io::AUDIO_DIR)) {
                self.abort_takes(jobs);
                return error(format!("dossier des prises : {e}"));
            }
            let receiver = match self.handle.start_record_window(node, Self::source_of(inp), 6.0, start_at, end_at) {
                Ok(rx) => rx,
                Err(e) => {
                    self.abort_takes(jobs);
                    return error(format!("prise de « {track_name} » : {e:?}"));
                }
            };
            match Recording::start(receiver, dir.join(&rel)) {
                Ok(rec) => jobs.push(TakeJob { node, track_name, number, kind: TakeKind::Audio { rec, rel } }),
                Err(e) => {
                    self.handle.stop_record(node);
                    self.abort_takes(jobs);
                    return error(format!("fichier de prise « {rel} » : {e}"));
                }
            }
        }
        self.recording = Some(RecordingState { jobs, stopping: false, last_overruns: 0, start: at, graph_latency: u64::from(self.handle.latency_samples()), auto_end: end_at.is_some(), loop_suspended: zone.is_some() && self.loop_range.is_some() && !loop_mode, loop_zone: if loop_mode { zone.map(|z| (z.start, z.end)) } else { None }, loop_pass: 0, loop_last_pos: 0 });
        self.stop_at = None;
        if zone.is_some() && self.loop_range.is_some() && !loop_mode {
            self.handle.send(EngineCommand::SetLoop(None)); // la zone n'est plus bouclée pendant la prise : elle est rétablie à la fin
        }
        if !self.playing {
            // La lecture part de `at − pré-roll`, même sans pré-roll quand la tête n'est pas déjà sur le point de départ (début d'une zone
            // de punch) : sinon elle jouerait depuis la tête jusqu'à la zone.
            if pre > 0 || at != head {
                self.handle.send(EngineCommand::Seek(at.saturating_sub(pre)));
            }
            if self.metro.count_in_bars > 0 {
                let count_in = self.count_in_for(at.saturating_sub(pre), self.metro.count_in_bars);
                drop(self.handle.start_count_in(count_in));
            } else {
                self.handle.send(EngineCommand::Play);
            }
        }
        self.apply_metronome();
        self.playing = true;
        self.play_origin = Some(at);
        events.push(self.recording_event(true, false, 0, Vec::new()));
        events.push(self.position_event());
        events
    }

    /// Donne l'ordre d'arrêt : le moteur retire les prises ; les clips naissent quand l'écriture est finie (voir `poll_recording`).
    fn begin_stop_recording(&mut self) -> Vec<Event> {
        let Some(st) = self.recording.as_mut() else { return Vec::new() };
        if st.stopping {
            return Vec::new();
        }
        st.stopping = true;
        self.stop_at = None;
        for job in &st.jobs {
            match job.kind {
                TakeKind::Audio { .. } => drop(self.handle.stop_record(job.node)),
                TakeKind::Midi { .. } => drop(self.handle.stop_midi_record(job.node)),
            }
        }
        let overruns = st.last_overruns;
        self.apply_metronome();
        vec![self.recording_event(true, true, overruns, Vec::new())]
    }

    /// À chaque `tick` pendant un enregistrement : vide les files MIDI, signale les blocs perdus, et termine quand tout est écrit.
    fn poll_recording(&mut self) -> Vec<Event> {
        use std::sync::atomic::Ordering::{Acquire, Relaxed};
        let cur = self.handle.position();
        let Some(st) = self.recording.as_mut() else { return Vec::new() };
        let (mut overruns, mut ready) = (0u32, true);
        // Enregistrement en boucle : la tête est repassée en arrière depuis le dernier passage → un passage vient de finir. Les
        // événements reçus avant la fin de ce passage sont plus loin que la tête actuelle ; ceux d'après sont avant elle.
        let wrapped = st.loop_zone.is_some_and(|z| cur >= z.0 && cur < st.loop_last_pos);
        for job in &mut st.jobs {
            match &mut job.kind {
                TakeKind::Audio { rec, .. } => {
                    overruns += rec.status().overruns.load(Relaxed);
                    ready &= rec.status().closed.load(Acquire);
                }
                TakeKind::Midi { rx, events, passes } => {
                    for e in rx.take_events() {
                        passes.push(st.loop_pass + u32::from(wrapped && e.pos <= cur));
                        events.push(e);
                    }
                    ready &= rx.is_closed();
                }
            }
        }
        if let Some(z) = st.loop_zone {
            if wrapped {
                st.loop_pass += 1;
            }
            if cur >= z.0 && cur < z.1 {
                st.loop_last_pos = cur;
            }
        }
        // Punch-out : toutes les prises se sont fermées toutes seules à la fin de la zone ; on passe en « écriture des prises » sans
        // ordre d'arrêt, et la lecture continue.
        let auto_ended = !st.stopping && st.auto_end && ready;
        if auto_ended {
            st.stopping = true;
        }
        let (stopping, changed) = (st.stopping, overruns != st.last_overruns);
        st.last_overruns = overruns;
        let mut out = Vec::new();
        if changed || auto_ended {
            out.push(self.recording_event(true, stopping, overruns, Vec::new()));
        }
        if auto_ended {
            self.apply_metronome();
        }
        if stopping && ready {
            out.extend(self.finish_recording());
        }
        out
    }

    /// Les prises sont écrites : elles deviennent des clips posés à leur point de départ (moins la latence), en **une seule action
    /// annulable**. Ce qui est vide est écarté ; ce qui a manqué de blocs est signalé.
    fn finish_recording(&mut self) -> Vec<Event> {
        let Some(st) = self.recording.take() else { return Vec::new() };
        if st.loop_suspended {
            self.handle.send(EngineCommand::SetLoop(self.loop_range.map(|l| (l.start, l.end))));
        }
        let Some(dir) = self.project_dir.clone() else { return Vec::new() };
        // Latence de la carte (entrée + sortie + convertisseurs) **et** latence des plugins : la lecture que le musicien entend est en
        // retard de cette dernière sur la ligne de temps (compensation de latence du moteur).
        let comp = self.record_compensation + st.graph_latency;
        let (loop_zone, loop_pass) = (st.loop_zone, st.loop_pass);
        let (mut warnings, mut overruns) = (Vec::new(), 0u32);
        let mut audio: Vec<(u32, String, u32, String, std::sync::Arc<AudioData>, u64)> = Vec::new();
        let mut midi: Vec<(u32, String, u32, MidiTakeData, Vec<MidiTakeData>)> = Vec::new();
        for job in st.jobs {
            match job.kind {
                TakeKind::Audio { rec, rel } => match rec.finish() {
                    Ok(info) if info.frames == 0 => drop(std::fs::remove_file(&info.path)),
                    Ok(info) => {
                        overruns += info.overruns;
                        if info.overruns > 0 {
                            warnings.push(format!("« {} » : {} blocs perdus, la prise est abîmée (le disque ne suit pas)", job.track_name, info.overruns));
                        }
                        match project_io::load_wav(&dir.join(&rel)) {
                            Ok(data) => audio.push((job.node, job.track_name, job.number, rel, data, info.start_pos)),
                            Err(e) => warnings.push(format!("prise de « {} » illisible : {e}", job.track_name)),
                        }
                    }
                    Err(e) => warnings.push(format!("prise de « {} » : {e}", job.track_name)),
                },
                TakeKind::Midi { mut rx, mut events, mut passes } => {
                    for e in rx.take_events() {
                        passes.push(loop_pass);
                        events.push(e);
                    }
                    let whole = MidiTakeData { start_pos: rx.start_pos(), stop_pos: rx.stop_pos(), events: events.clone() };
                    let split = match loop_zone {
                        // Un passage = les événements de même numéro ; le premier part du début de la prise, les suivants du début de la
                        // zone, tous finissent à la fin de la zone sauf le dernier (arrêt de la prise).
                        Some(z) => {
                            let last = passes.iter().copied().max().unwrap_or(0).max(loop_pass);
                            (0..=last)
                                .map(|k| MidiTakeData {
                                    start_pos: if k == 0 { rx.start_pos() } else { z.0 },
                                    stop_pos: if k == last { rx.stop_pos() } else { z.1 },
                                    events: events.iter().zip(&passes).filter(|(_, &p)| p == k).map(|(e, _)| *e).collect(),
                                })
                                .collect()
                        }
                        None => Vec::new(),
                    };
                    midi.push((job.node, job.track_name, job.number, whole, split));
                }
            }
        }
        let mut events_out = Vec::new();
        if !audio.is_empty() || !midi.is_empty() {
            let result = self.session.group("Enregistrer", |s| {
                let (mut audio_ids, mut midi_ids) = (Vec::new(), Vec::new());
                for (node, track, number, rel, data, start_pos) in &audio {
                    s.register_asset(rel, std::sync::Arc::clone(data));
                    let frames = data.len();
                    // Le signal est arrivé `comp` échantillons plus tôt que sa place sur la ligne de temps : on recule le clip ;
                    // s'il tombe avant l'origine, on rogne la tête de la prise.
                    let passes = match loop_zone {
                        Some(zone) => loop_passes(zone, *start_pos, comp, frames),
                        None => vec![(0, frames)],
                    };
                    let mut built: Vec<(Clip, String)> = Vec::new();
                    for (k, (from, to)) in passes.into_iter().enumerate() {
                        // Le signal est arrivé `comp` échantillons plus tôt que sa place sur la ligne de temps : on recule le clip ;
                        // s'il tombe avant l'origine, on rogne la tête de la prise. (Passages suivants d'une boucle : ils commencent
                        // exactement au début de la zone.)
                        // Enregistrement en boucle : tous les passages sont rognés à la zone (le premier ne dépasse pas avant elle de la
                        // latence compensée : ce petit bout resterait seul, actif, à chaque changement de prise).
                        let floor = loop_zone.map_or(0, |z| z.0);
                        // `begin` peut tomber avant l'origine (ou avant la zone) : la tête de la prise est alors rognée.
                        let begin = if k == 0 { i128::from(*start_pos) - i128::from(comp) } else { i128::from(floor) };
                        let cut = ((i128::from(floor).max(begin) - begin) as u64).min((to - from).saturating_sub(1));
                        let at = (begin + cut as i128).max(0) as u64;
                        let clip = Clip::new(std::sync::Arc::clone(data), at, from + cut, to - from - cut, 1.0);
                        let label = if loop_zone.is_some() { format!("{track} prise {number}.{}", k + 1) } else { format!("{track} prise {number}") };
                        built.push((clip, label));
                    }
                    if loop_zone.is_some() {
                        // Chaque passage recouvre le précédent : le dernier est actif, les autres restent dessous.
                        let (clips, labels): (Vec<Clip>, Vec<String>) = built.into_iter().unzip();
                        for (id, label) in s.record_passes(NodeId(*node), clips)?.into_iter().zip(labels) {
                            audio_ids.push((id, label));
                        }
                    } else {
                        for (clip, label) in built {
                            audio_ids.push((s.record_over(NodeId(*node), clip)?, label));
                        }
                    }
                }
                for (node, track, number, take, passes) in &midi {
                    if loop_zone.is_some() {
                        // Un conteneur par passage qui contient quelque chose ; un passage vide ne crée rien.
                        for (k, id) in s.add_recorded_passes(NodeId(*node), passes, comp)?.into_iter().enumerate() {
                            midi_ids.push((id, format!("{track} prise {number}.{}", k + 1)));
                        }
                    } else if let Some(id) = s.add_recorded_take(NodeId(*node), take, comp)? {
                        midi_ids.push((id, format!("{track} prise {number}")));
                    }
                }
                Ok((audio_ids, midi_ids))
            });
            match result {
                Ok((audio_ids, midi_ids)) => {
                    for (id, name) in audio_ids {
                        self.meta.clip_names.insert(id.0, name);
                    }
                    for (id, name) in midi_ids {
                        self.meta.clip_names.insert(MIDI_CLIP_BASE + id.0, name);
                    }
                    events_out.extend(self.finish_edit("record", Ok(())));
                }
                Err(e) => warnings.push(format!("les prises n'ont pas pu être posées : {}", explain(&e))),
            }
        }
        events_out.push(self.recording_event(false, false, overruns, warnings));
        events_out
    }

    // ---- pistes et bus --------------------------------------------------------------------------------------------

    /// « Piste N » ou « Bus N » : le plus petit N qui ne porte pas encore ce nom.
    fn free_name(&self, label: &str) -> String {
        (1..).map(|n| format!("{label} {n}")).find(|c| !self.session.graph().nodes.iter().any(|n| &n.name == c)).expect("une suite infinie")
    }

    /// Des nœuds ont été ajoutés ou retirés (ou l'annulation l'a fait) : les numéros ont changé. On refait l'état de mixage de
    /// l'hôte d'après le graphe, on défait les armements (ils tenaient à des numéros) et on coupe les notes qui sonnaient (leur
    /// suivi tenait lui aussi à des numéros).
    fn resync_nodes(&mut self) {
        let old = self.nodes.len();
        self.last_structure = self.session.structure_version();
        self.nodes = self.session.graph().nodes.iter().map(|n| NodeState { gain: n.params.gain, pan: n.params.pan, mute: n.params.mute, solo: n.params.solo, invert: n.params.invert, arm: false }).collect();
        for node in 0..old.max(self.nodes.len()) as u32 {
            self.handle.send(EngineCommand::SetInput { node, armed: false, monitor: false, source: RecordSource::Left });
        }
        self.handle.send(EngineCommand::Panic);
    }

    // ---- projet : nouveau, ouvrir, enregistrer ------------------------------------------------------------------

    /// Nom affiché tant que le projet n'est pas enregistré (la démonstration s'appelle ainsi).
    pub fn set_name(&mut self, name: &str) {
        self.name = name.to_owned();
    }

    /// Change la façon de retrouver les plugins à l'ouverture d'un projet (les tests y mettent un plugin sans fichier).
    pub fn set_plugin_resolver(&mut self, resolver: PluginResolver) {
        self.resolver = resolver;
    }

    fn file_event(&self, warnings: Vec<String>) -> Event {
        Event::File { name: self.name.clone(), path: self.project_dir.as_ref().map(|d| d.to_string_lossy().into_owned()), dirty: self.dirty, generation: self.generation, warnings }
    }

    /// Enregistre le projet. Sans `path` : dans le dossier du projet ouvert. Un aperçu de fondu en cours est arrêté d'abord
    /// (ses solos ne doivent pas être enregistrés). Un échec ne change rien.
    /// Ce qu'il faut ajouter à la session pour l'écrire : données audio sans fichier rangées sous `audio/`, plugins (identité et
    /// état), et ce qui n'appartient qu'à l'interface. Utilisé par l'enregistrement et par la sauvegarde automatique.
    fn snapshot(&mut self) -> (Vec<PluginEntry>, UiMeta, Vec<String>) {
        // Données audio sans fichier (prises, importées) : rangées sous `audio/` d'après le nom de leur clip.
        let mut fresh: Vec<(Arc<AudioData>, String)> = Vec::new();
        for n in 0..self.session.graph().nodes.len() {
            for (i, c) in self.session.graph().nodes[n].clips.iter().enumerate() {
                let known = self.session.assets().iter().any(|(d, _)| Arc::ptr_eq(d, &c.data)) || fresh.iter().any(|(d, _)| Arc::ptr_eq(d, &c.data));
                if !known {
                    let id = self.session.clip_ids(NodeId(n as u32))[i];
                    fresh.push((Arc::clone(&c.data), format!("audio/{}.wav", self.meta.clip_names.get(&id.0).map_or("audio", String::as_str))));
                }
            }
        }
        for (data, path) in fresh {
            self.session.register_asset(&path, data);
        }
        // Plugins : identité et état de chaque effet ; ceux qu'on ne sait pas décrire sont signalés, ceux qu'on n'avait pas retrouvés gardés.
        let mut warnings = Vec::new();
        let mut plugins = Vec::new();
        for (n, node) in self.session.graph().nodes.iter().enumerate() {
            for ins in &node.inserts {
                let Some(pref) = self.meta.plugins.get(&ins.id) else {
                    warnings.push(format!("effet « {} » sur « {} » : identité inconnue, non enregistré", ins.name, node.name));
                    continue;
                };
                let state = self.handle.insert_control(ins.id).and_then(|c| c.save_state()).map(|b| project_io::base64_encode(&b));
                plugins.push(PluginEntry { node: n as u32, name: pref.name.clone(), clap_id: pref.clap_id.clone(), vendor: pref.vendor.clone(), file: pref.file.clone(), state, bypass: ins.bypass, sidechain: ins.sidechain.map(|s| s.0) });
            }
        }
        plugins.extend(self.kept_plugins.iter().cloned());
        let mut ui = UiMeta { auto_crossfade: self.auto_crossfade, ..UiMeta::default() };
        for n in 0..self.session.graph().nodes.len() as u32 {
            let c = self.session.node_color(NodeId(n));
            if !c.is_empty() {
                ui.node_colors.insert(n.to_string(), c.to_owned());
            }
        }
        ui.node_order = self.session.display_order().iter().map(|n| n.0).collect();
        ui.punch_on = self.punch_on;
        ui.aligns = self.aligns_for_file();
        ui.write_arms = self.write_arms_for_file();
        self.remember_window_places();
        ui.plugin_windows = self.window_places.iter().map(|(k, &(x, y))| (k.clone(), [x, y])).collect();
        for n in 0..self.session.graph().nodes.len() as u32 {
            let inp = self.session.node_input(NodeId(n));
            if inp != NodeInput::default() {
                let channels = if inp.midi { Vec::new() } else if inp.stereo { vec![inp.channels[0] as u32 + 1, inp.channels[1] as u32 + 1] } else { vec![inp.channels[0] as u32 + 1] };
                ui.inputs.insert(n.to_string(), UiInput { kind: if inp.midi { "midi" } else { "audio" }.into(), channels, monitor: inp.monitor });
            }
        }
        for n in 0..self.session.graph().nodes.len() {
            let node = NodeId(n as u32);
            let audio: Vec<String> = self.session.clip_ids(node).iter().zip(&self.session.graph().nodes[node.0 as usize].clips).map(|(id, c)| self.audio_clip_name(id.0, &c.data).unwrap_or_default()).collect();
            let midi: Vec<String> = self.session.midi_clip_ids(node).iter().map(|id| self.meta.clip_names.get(&(MIDI_CLIP_BASE + id.0)).cloned().unwrap_or_default()).collect();
            if audio.iter().any(|s| !s.is_empty()) {
                ui.audio_names.insert(n.to_string(), audio);
            }
            if midi.iter().any(|s| !s.is_empty()) {
                ui.midi_names.insert(n.to_string(), midi);
            }
        }
        (plugins, ui, warnings)
    }

    fn save_project(&mut self, path: Option<&str>) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "save_project".into(), message }];
        let dir = match (path, &self.project_dir) {
            (Some(p), _) => project_io::project_dir(p),
            (None, Some(d)) => d.clone(),
            (None, None) => return error("pas encore de dossier : utiliser « Enregistrer sous »".into()),
        };
        let mut events = self.end_preview(true, false);
        let (plugins, ui, warnings) = self.snapshot();
        match project_io::write_project(&dir, &mut self.session, plugins, ui, warnings) {
            Ok(warnings) => {
                // Les modifications sont enregistrées : la sauvegarde automatique n'a plus lieu d'être (celle du projet sans titre aussi).
                project_io::remove_autosave(&dir);
                if let Some(d) = self.untitled_autosave_dir() {
                    project_io::remove_autosave(&d);
                }
                self.name = project_io::project_name(&dir);
                self.project_dir = Some(dir);
                self.dirty = false;
                events.push(self.file_event(warnings));
                events
            }
            Err(message) => {
                events.extend(error(message));
                events
            }
        }
    }

    /// Remplace le projet ouvert : plugins vivants oubliés (les nouveaux sont créés à neuf), lecture arrêtée au début,
    /// armements défaits, caches de la page à vider (numéro de chargement).
    fn replace_project(&mut self, session: Session, meta: ProjectMeta, auto_crossfade: bool, dir: Option<std::path::PathBuf>, kept: Vec<PluginEntry>, warnings: Vec<String>) -> Vec<Event> {
        let mut events = self.close_all_plugin_uis();
        events.extend(self.end_preview(false, false));
        self.gui_cache.borrow_mut().clear();
        self.clipboard.clear();
        self.aligns.clear(); // les fiches de calage appartiennent à leur projet
        self.write_arms.clear(); // un autre projet : ses armements (et ce qui s'enregistrait) n'ont plus de sens
        self.plugin_values.clear();
        let old_nodes = self.nodes.len();
        self.handle.forget_inserts();
        if let Err(e) = self.handle.load_graph(session.graph()) {
            events.push(Event::Error { command: "open_project".into(), message: format!("le moteur a refusé le projet : {e:?}") });
            return events;
        }
        let new_nodes = session.graph().nodes.len();
        for node in 0..old_nodes.max(new_nodes) as u32 {
            self.handle.send(EngineCommand::SetInput { node, armed: false, monitor: false, source: RecordSource::Left });
        }
        self.handle.send(EngineCommand::Stop);
        self.handle.send(EngineCommand::Seek(0));
        self.handle.send(EngineCommand::SetLoop(None));
        self.play_origin = None;
        self.head_hint = None;
        self.nodes = session.graph().nodes.iter().map(|n| NodeState { gain: n.params.gain, pan: n.params.pan, mute: n.params.mute, solo: n.params.solo, invert: n.params.invert, arm: false }).collect();
        self.name = dir.as_deref().map_or_else(|| "Sans titre".to_owned(), project_io::project_name);
        self.last_structure = session.structure_version();
        self.session = session;
        self.meta = meta;
        self.peaks.clear();
        self.playing = false;
        self.loop_range = None;
        self.punch_on = false;
        self.loop_rec_on = false;
        self.auto_crossfade = auto_crossfade;
        self.session.set_seam_ms(if auto_crossfade { 10 } else { 0 });
        self.project_dir = dir;
        self.kept_plugins = kept;
        self.dirty = false;
        self.generation += 1;
        events.extend([self.file_event(warnings), self.project_event(), self.history_event(), self.position_event(), self.punch_event(), self.chords_event(Vec::new())]);
        events
    }

    /// Dossier de la sauvegarde automatique d'un projet jamais enregistré.
    fn untitled_autosave_dir(&self) -> Option<std::path::PathBuf> {
        self.autosave_root.as_ref().map(|r| r.join(UNTITLED_DIR))
    }

    /// Active la sauvegarde automatique : `root` accueille le projet sans titre, `every` est la période. Si une sauvegarde d'un projet
    /// sans titre est restée là (fermeture brutale), sa reprise est proposée à la prochaine commande `get_project`.
    pub fn set_autosave(&mut self, root: Option<std::path::PathBuf>, every: Duration) {
        self.autosave_root = root;
        self.autosave_every = every;
        self.last_autosave = Instant::now();
        self.recovery_offer = self.untitled_autosave_dir().and_then(|d| {
            project_io::autosave_time(&d).map(|t| Event::Autosave { path: d.to_string_lossy().into_owned(), untitled: true, modified: t })
        });
    }

    /// Écrit la sauvegarde automatique (à côté du projet, ou dans le dossier du projet sans titre) sans toucher au projet
    /// enregistré, au nom, ni à l'état « modifié ».
    fn autosave(&mut self) -> Result<(), String> {
        let dir = match (&self.project_dir, self.untitled_autosave_dir()) {
            (Some(d), _) => d.clone(),
            (None, Some(d)) => d,
            (None, None) => return Ok(()),
        };
        let (plugins, ui, warnings) = self.snapshot();
        project_io::write_project_file(&dir, project_io::AUTOSAVE_FILE, &mut self.session, plugins, ui, warnings).map(|_| ())
    }

    fn new_project(&mut self) -> Vec<Event> {
        self.forget_autosave();
        let rate = self.session.sample_rate();
        let mut g = GraphDesc::new();
        let track = g.add_track("Piste 1", TrackFormat::Stereo);
        let mut session = Session::new_at(g, rate);
        session.set_node_color_untracked(track, TRACK_COLORS[0]);
        self.replace_project(session, ProjectMeta::default(), true, None, Vec::new(), Vec::new())
    }

    /// Le projet courant est abandonné (nouveau, autre projet) : sa sauvegarde automatique et celle du projet sans titre disparaissent.
    fn forget_autosave(&mut self) {
        if let Some(d) = &self.project_dir {
            project_io::remove_autosave(d);
        }
        if let Some(d) = self.untitled_autosave_dir() {
            project_io::remove_autosave(&d);
        }
    }

    fn open_project(&mut self, path: &str, recover: bool) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: "open_project".into(), message }];
        let mut dir = std::path::PathBuf::from(path);
        if dir.file_name().is_some_and(|n| n == project_io::PROJECT_FILE || n == project_io::AUTOSAVE_FILE) {
            dir.pop(); // on a désigné le fichier plutôt que le dossier
        }
        let untitled = self.untitled_autosave_dir().is_some_and(|d| d == dir);
        let file = if recover { project_io::AUTOSAVE_FILE } else { project_io::PROJECT_FILE };
        // Projet à une autre fréquence que celle du moteur : on ne touche à rien et on propose une copie convertie (`convert_project`).
        let engine_rate = self.session.sample_rate();
        if let Ok(project_rate) = project_io::peek_sample_rate(&dir, file) {
            if project_rate != engine_rate {
                if recover {
                    return error(format!("cette sauvegarde automatique est à {project_rate} Hz et le moteur tourne à {engine_rate} Hz : ouvre d'abord le projet pour le convertir"));
                }
                return vec![Event::RateMismatch { path: dir.to_string_lossy().into_owned(), project_rate, engine_rate }];
            }
        }
        let loaded = match project_io::read_project_file(&dir, file) {
            Ok(l) => l,
            Err(e) => return error(e),
        };
        if !recover {
            self.forget_autosave();
        }
        if loaded.sample_rate != self.session.sample_rate() {
            return error(format!("ce projet est à {} Hz et le moteur tourne à {} Hz : la conversion n'est pas encore écrite", loaded.sample_rate, self.session.sample_rate()));
        }
        let mut session = loaded.session;
        let mut meta = ProjectMeta::default();
        for (k, v) in &loaded.ui.node_colors {
            if let Ok(id) = k.parse::<u32>() {
                session.set_node_color_untracked(NodeId(id), v);
            }
        }
        for (k, v) in &loaded.ui.inputs {
            let Ok(n) = k.parse::<u32>() else { continue };
            let valid = v.channels.iter().all(|&c| (1..=64).contains(&c));
            let mut inp = NodeInput { midi: v.kind == "midi", monitor: v.monitor, ..NodeInput::default() };
            if !inp.midi && valid && (1..=2).contains(&v.channels.len()) {
                inp.stereo = v.channels.len() == 2;
                inp.channels = [(v.channels[0] - 1) as u8, (*v.channels.get(1).unwrap_or(&v.channels[0]) - 1) as u8];
            }
            session.set_node_input_untracked(NodeId(n), inp);
        }
        if !loaded.ui.node_order.is_empty() {
            session.set_display_order_untracked(&loaded.ui.node_order.iter().map(|&n| NodeId(n)).collect::<Vec<_>>());
        }
        for (k, names) in &loaded.ui.audio_names {
            let Ok(n) = k.parse::<u32>() else { continue };
            for (id, name) in session.clip_ids(NodeId(n)).iter().zip(names) {
                if !name.is_empty() {
                    meta.clip_names.insert(id.0, name.clone());
                }
            }
        }
        for (k, names) in &loaded.ui.midi_names {
            let Ok(n) = k.parse::<u32>() else { continue };
            for (id, name) in session.midi_clip_ids(NodeId(n)).iter().zip(names) {
                if !name.is_empty() {
                    meta.clip_names.insert(MIDI_CLIP_BASE + id.0, name.clone());
                }
            }
        }
        // Plugins : retrouvés et recréés avec leur état (appliqué avant l'activation) ; les absents sont gardés tels quels.
        let mut warnings = Vec::new();
        // Fichiers référencés en place qui ont disparu : le projet s'ouvre, leurs clips sont du silence.
        for path in session.missing_assets() {
            let uses = (0..session.graph().nodes.len()).map(|n| session.graph().nodes[n].clips.iter().filter(|c| session.asset_path(&c.data) == Some(path.as_str())).count()).sum::<usize>();
            warnings.push(format!("fichier audio manquant : {path} ({uses} clip{} en silence) — remets-le à sa place et rouvre le projet", if uses > 1 { "s" } else { "" }));
        }
        let mut kept = Vec::new();
        // Place de chaque plugin dans la chaîne de sa piste (ordre du fichier) → identifiant du plugin recréé, s'il l'a été :
        // c'est ainsi que les courbes d'automation de leurs paramètres retrouvent leur plugin.
        let mut slots: HashMap<u32, Vec<Option<u32>>> = HashMap::new();
        for entry in loaded.plugins {
            if entry.node as usize >= session.graph().nodes.len() {
                warnings.push(format!("plugin « {} » : piste inexistante ({}), gardé tel quel", entry.name, entry.node));
                kept.push(entry);
                continue;
            }
            let slot = slots.entry(entry.node).or_default();
            slot.push(None);
            let slot_index = slot.len() - 1;
            let state = match entry.state.as_deref().map(project_io::base64_decode) {
                Some(Some(bytes)) => Some(bytes),
                Some(None) => {
                    warnings.push(format!("plugin « {} » : état illisible, réglages d'origine", entry.name));
                    None
                }
                None => None,
            };
            match (self.resolver)(&entry) {
                Some((factory, pref)) => {
                    if let Some(id) = session.add_insert(NodeId(entry.node), &entry.name, with_state(factory, state)) {
                        session.set_effect_bypass_untracked(id, entry.bypass);
                        if let Some(src) = entry.sidechain {
                            if let Err(e) = session.set_effect_sidechain_untracked(id, Some(NodeId(src))) {
                                warnings.push(format!("plugin « {} » : chaîne latérale ignorée ({})", entry.name, explain(&e)));
                            }
                        }
                        meta.plugins.insert(id, pref);
                        slots.get_mut(&entry.node).expect("créé plus haut")[slot_index] = Some(id);
                    }
                }
                None => {
                    warnings.push(format!("plugin « {} » introuvable sur cette machine : son état est conservé dans le projet", entry.name));
                    kept.push(entry);
                }
            }
        }
        // Courbes d'automation des paramètres de plugins : rattachées au plugin recréé à la même place de la chaîne.
        for lane in session.take_pending_insert_lanes() {
            let id = slots.get(&lane.node.0).and_then(|s| s.get(lane.chain_index)).copied().flatten();
            match id {
                Some(id) => {
                    if let Err(e) = session.attach_insert_lane(lane.node, id, lane.param, lane.points, lane.mode) {
                        warnings.push(format!("automation d'un paramètre de plugin : {e:?}, ignorée"));
                    }
                }
                None => warnings.push("automation d'un paramètre d'un plugin introuvable : courbe ignorée".to_owned()),
            }
        }
        // Un projet sans titre repris n'a toujours pas de dossier à lui : il reste « Sans titre », modifié.
        let offer = if recover { None } else { project_io::newer_autosave(&dir) };
        let mut events = self.replace_project(session, meta, loaded.ui.auto_crossfade, if untitled { None } else { Some(dir.clone()) }, kept, warnings);
        // Armements d'écriture : remis une fois les plugins recréés (ils se désignent par leur place dans la chaîne).
        for (name, [x, y]) in &loaded.ui.plugin_windows {
            self.window_places.insert(name.clone(), (*x, *y));
        }
        self.punch_on = loaded.ui.punch_on;
        events.push(self.punch_event());
        let mut arm_warnings = self.restore_write_arms(&loaded.ui.write_arms, &slots);
        arm_warnings.extend(self.restore_aligns(&loaded.ui.aligns));
        if !loaded.ui.write_arms.is_empty() {
            events.push(self.project_event());
        }
        if !arm_warnings.is_empty() {
            events.push(self.file_event(arm_warnings));
        }
        if recover {
            self.dirty = true;
            events.push(self.file_event(Vec::new()));
        }
        if let Some(modified) = offer {
            events.push(Event::Autosave { path: dir.to_string_lossy().into_owned(), untitled: false, modified });
        }
        events
    }

    /// Zone du fondu écouté, en échantillons absolus, et piste qui le porte.
    fn preview_zone(&self, target: PreviewTarget) -> Option<(u64, u64, NodeId)> {
        let (start, end, owner) = match target {
            PreviewTarget::Crossfade { out, inn } => {
                let (a, b) = (self.session.clip(out)?, self.session.clip(inn)?);
                (b.start, a.end(), self.owner_of_audio(out)?)
            }
            PreviewTarget::Single { clip, side_in } => {
                let c = self.session.clip(clip)?;
                let (s, e) = if side_in { (c.start, c.start + c.fade_in.len) } else { (c.end() - c.fade_out.len, c.end()) };
                (s, e, self.owner_of_audio(clip)?)
            }
        };
        (start < end).then_some((start, end, owner))
    }

    fn set_node_solo(&mut self, node: usize, solo: bool) {
        let Some(state) = self.nodes.get_mut(node) else { return };
        if state.solo == solo {
            return;
        }
        state.solo = solo;
        self.handle.send(EngineCommand::SetSolo { node: node as u32, solo });
        let params = NodeParams { gain: state.gain, pan: state.pan, mute: state.mute, solo, invert: state.invert };
        let _ = self.session.set_params_untracked(NodeId(node as u32), params);
    }

    /// (Ré)applique l'aperçu : solos (ceux de l'utilisateur d'abord rendus, puis la piste du fondu seule si demandé) et boucle
    /// sur la zone plus la marge. Renvoie la boucle posée, ou `None` si le fondu a disparu ou n'a plus de longueur.
    fn apply_preview(&mut self) -> Option<(u64, u64)> {
        let (target, margin_ms, solo, saved) = {
            let p = self.preview.as_ref()?;
            (p.target, p.margin_ms, p.solo, p.saved_solos.clone())
        };
        let (start, end, owner) = self.preview_zone(target)?;
        for (i, s) in saved.iter().enumerate() {
            let want = if solo { i == owner.0 as usize } else { *s };
            self.set_node_solo(i, want);
        }
        let margin = margin_ms * self.session.sample_rate() as u64 / 1000;
        let (from, to) = (start.saturating_sub(margin), end + margin);
        self.loop_range = Some(LoopRange { start: from, end: to });
        self.handle.send(EngineCommand::SetLoop(Some((from, to))));
        Some((from, to))
    }

    /// Termine l'aperçu (sans effet s'il n'y en a pas) : rend les solos, et selon les cas la boucle et la position avec l'état
    /// de lecture de l'utilisateur. Quand c'est lui qui reprend le transport, on ne lui reprend pas la position.
    fn end_preview(&mut self, restore_loop: bool, restore_pos: bool) -> Vec<Event> {
        let mut events = self.end_listen(restore_loop, restore_pos);
        let Some(p) = self.preview.take() else { return events };
        for (i, s) in p.saved_solos.iter().enumerate() {
            self.set_node_solo(i, *s);
        }
        if restore_loop {
            self.loop_range = p.saved_loop;
            self.handle.send(EngineCommand::SetLoop(p.saved_loop.map(|l| (l.start, l.end))));
        }
        if restore_pos {
            self.handle.send(EngineCommand::Seek(p.saved_pos));
            self.playing = p.saved_playing;
            self.handle.send(if self.playing { EngineCommand::Play } else { EngineCommand::Stop });
        }
        events.extend([Event::Preview { active: false }, self.project_event(), self.position_event()]);
        events
    }

    /// Après une édition : l'aperçu suit le fondu modifié (nouvelle boucle) ; s'il a disparu (annulation...), il se termine.
    fn refresh_preview(&mut self) -> Vec<Event> {
        if self.preview.is_none() {
            return Vec::new();
        }
        match self.apply_preview() {
            Some((from, to)) => {
                let pos = self.handle.position();
                if pos < from || pos >= to {
                    self.handle.send(EngineCommand::Seek(from));
                }
                vec![self.project_event(), self.position_event()]
            }
            None => self.end_preview(true, true),
        }
    }

    /// Après une édition de la session : recharge le graphe dans le moteur (les plugins déjà chargés sont conservés) puis
    /// renvoie projet et historique. Si le moteur refuse le graphe, l'édition est défaite et oubliée.
    fn finish_edit(&mut self, command: &str, result: Result<(), SessionError>) -> Vec<Event> {
        let error = |message: String| vec![Event::Error { command: command.into(), message }];
        if let Err(e) = result {
            return error(explain(&e));
        }
        match self.handle.load_graph(self.session.graph()) {
            Ok(()) => {
                self.dirty = true;
                let restructured = self.session.structure_version() != self.last_structure;
                if restructured {
                    self.resync_nodes();
                }
                let mut events = vec![self.project_event(), self.history_event()];
                if self.session.chord_revision() != self.chord_rev_sent {
                    events.push(self.chords_event(Vec::new()));
                }
                events.extend(self.refresh_preview());
                events
            }
            Err(e) => {
                self.session.rollback_last();
                error(format!("le moteur a refusé la modification : {e:?}"))
            }
        }
    }

    /// Les notes d'un clip, avec leurs positions exactes (pour le piano roll).
    fn notes_event(&self, clip: u64, mid: MidiClipId, selection: Vec<usize>) -> Option<Event> {
        let c = self.session.midi_clip(mid)?;
        let pair = |f: Frac| [f.num(), f.den()];
        Some(Event::Notes {
            clip,
            start: pair(c.start),
            len: pair(c.len),
            offset: pair(c.offset),
            notes: c.notes.iter().map(|n| NoteDetail { start: pair(n.start), len: pair(n.len), key: n.key, velocity: n.velocity, channel: n.channel, voice: n.voice }).collect(),
            selection,
            events: c
                .events
                .iter()
                .map(|e| {
                    let (kind, controller, key, value) = match e.kind {
                        MKind::ControlChange { controller, value } => ("cc", Some(controller), None, i32::from(value)),
                        MKind::PitchBend { value } => ("bend", None, None, i32::from(value)),
                        MKind::ChannelPressure { value } => ("pressure", None, None, i32::from(value)),
                        MKind::PolyPressure { key, value } => ("poly", None, Some(key), i32::from(value)),
                    };
                    EventDetail { start: pair(e.start), channel: e.channel, kind: kind.into(), controller, key, value }
                })
                .collect(),
        })
    }

    /// Après une modification de notes : le moteur reçoit le graphe, la page reçoit projet, historique et notes à jour.
    fn finish_notes_edit(&mut self, command: &str, clip: u64, mid: MidiClipId, result: Result<Vec<usize>, SessionError>) -> Vec<Event> {
        let selection = match &result {
            Ok(s) => s.clone(),
            Err(_) => Vec::new(),
        };
        let mut events = self.finish_edit(command, result.map(|_| ()));
        if !events.iter().any(|e| matches!(e, Event::Error { .. })) {
            events.extend(self.notes_event(clip, mid, selection));
        }
        events
    }

    fn history_event(&self) -> Event {
        Event::History {
            can_undo: self.session.can_undo(),
            can_redo: self.session.can_redo(),
            undo_label: self.session.undo_label().map(str::to_owned),
            redo_label: self.session.redo_label().map(str::to_owned),
        }
    }

    /// Nom d'un clip audio : le sien, sinon celui d'un clip qui partage ses données (morceau issu d'une découpe ou d'un
    /// enregistrement par-dessus, qui n'a pas reçu de nom propre).
    fn audio_clip_name(&self, id: u64, data: &Arc<AudioData>) -> Option<String> {
        if let Some(n) = self.meta.clip_names.get(&id) {
            return Some(n.clone());
        }
        let g = self.session.graph();
        (0..g.nodes.len()).find_map(|n| {
            let ids = self.session.clip_ids(NodeId(n as u32));
            g.nodes[n].clips.iter().zip(ids).find_map(|(c, cid)| if Arc::ptr_eq(&c.data, data) { self.meta.clip_names.get(&cid.0).cloned() } else { None })
        })
    }

    /// Dictionnaires de la saisie d'accords et de gammes (voir `Event::ChordCatalog`).
    fn chord_catalog_event() -> Event {
        Event::ChordCatalog {
            qualities: daw_session::quality_catalog::QUALITIES.iter().map(|(id, iv)| ((*id).to_owned(), iv.to_vec())).collect(),
            aliases: daw_session::quality_catalog::ALIASES.iter().map(|(a, id)| ((*a).to_owned(), (*id).to_owned())).collect(),
            scales: daw_session::scale_catalog::SCALES.iter().map(|(id, fr, en, iv)| ScaleInfo { id: (*id).to_owned(), fr: (*fr).to_owned(), en: (*en).to_owned(), intervals: iv.to_vec() }).collect(),
        }
    }

    /// La piste d'accords telle que la page la voit (format `flibuste-carte`).
    fn chords_event(&mut self, skipped: Vec<String>) -> Event {
        self.chord_rev_sent = self.session.chord_revision();
        let track = serde_json::from_str(&self.session.chord_track().to_carte_json(&self.name)).unwrap_or(serde_json::Value::Null);
        let structure = self.session.structure().iter().map(|s| SectionDto { at: [s.at.num(), s.at.den()], dur: [s.dur.num(), s.dur.den()], name: s.name.clone(), color: s.color.clone(), category: s.category.clone() }).collect();
        let lyrics = self.session.lyrics().iter().map(|l| LyricDto { at: [l.at.num(), l.at.den()], text: l.text.clone(), hyphen: l.hyphen, verse: l.verse, end: l.end.map(|e| [e.num(), e.den()]) }).collect();
        Event::Chords { revision: self.session.chord_revision(), track, skipped, structure, lyrics }
    }

    fn loop_record_event(&self) -> Event {
        Event::LoopRecord { enabled: self.loop_rec_on }
    }

    fn punch_event(&self) -> Event {
        Event::Punch { enabled: self.punch_on }
    }

    /// Après un arrêt : avec « retour à l'arrêt », la prochaine lecture repart du point de départ (pré-roll compris s'il est activé) ;
    /// sinon c'est une pause, la lecture reprendra où elle s'est arrêtée.
    fn settle_after_stop(&mut self) {
        if self.prefs.transport.return_on_stop && self.play_origin.is_some() {
            self.paused = false;
            self.head_hint = self.play_origin;
        } else {
            self.paused = true;
            self.head_hint = None;
        }
    }

    /// Envoie le retour de la tête au moteur après un arrêt, si `settle_after_stop` l'a décidé (à appeler après l'ordre `Stop`).
    fn send_return_seek(&mut self) {
        if self.prefs.transport.return_on_stop && !self.playing {
            if let Some(origin) = self.play_origin.take() {
                self.handle.send(EngineCommand::Seek(origin));
            }
        }
    }

    fn position_event(&self) -> Event {
        Event::Position { pos: self.handle.position(), playing: self.playing, loop_range: self.loop_range }
    }

    fn project_event(&self) -> Event {
        let g = self.session.graph();
        let tempo = self.session.tempo_map();
        let tempo_map = tempo.changes().iter().map(|(p, t)| TempoChange { pos: [p.num(), p.den()], num: t.num(), den: t.den() }).collect();
        let meters = self.session.meter_map().changes().iter().map(|(bar, m)| MeterChange { bar: *bar, num: m.num(), den: m.den() }).collect();
        // La sortie principale d'abord, puis les pistes et bus dans l'ordre d'affichage.
        let listed: Vec<usize> = std::iter::once(0).chain(self.session.display_order().iter().map(|n| n.0 as usize)).collect();
        let nodes = listed
            .iter()
            .map(|&i| (i, &g.nodes[i]))
            .map(|(i, n)| {
                let id = NodeId(i as u32);
                let st = self.nodes[i];
                let mut clips = Vec::new();
                for (k, c) in n.clips.iter().enumerate() {
                    let cid = self.session.clip_ids(id)[k].0;
                    clips.push(ClipInfo::Audio {
                        id: cid,
                        name: self.audio_clip_name(cid, &c.data).unwrap_or_else(|| format!("Clip {cid}")),
                        start: c.start,
                        len: c.len,
                        offset: c.offset,
                        source_len: c.data.len(),
                        gain: c.gain,
                        fade_in: c.fade_in.len,
                        fade_out: c.fade_out.len,
                        fade_in_curve: CurveSpec::from(&c.fade_in),
                        fade_out_curve: CurveSpec::from(&c.fade_out),
                        missing: self.session.is_missing(&c.data),
                        muted: c.muted,
                        take_group: c.take_group,
                        aligned: self.aligns.iter().any(|r| Arc::ptr_eq(&r.rendered, &c.data)),
                    });
                }
                for (k, mid) in self.session.midi_clip_ids(id).iter().enumerate() {
                    let cid = MIDI_CLIP_BASE + mid.0;
                    let mc = &n.midi_clips[k];
                    let mclip = self.session.midi_clip(*mid).expect("clip de la liste");
                    let (rate, tm) = (self.session.sample_rate(), self.session.tempo_map());
                    // Durées musicales → échantillons, au tempo du début du clip (affichage ; le moteur reste exact).
                    let dur = |f: Frac| -> u64 { tm.to_samples(mclip.start.checked_add(f).unwrap_or(mclip.start), rate).unwrap_or(0).saturating_sub(mc.start) };
                    clips.push(ClipInfo::Midi {
                        id: cid,
                        name: self.meta.clip_names.get(&cid).cloned().unwrap_or_else(|| format!("Notes {}", mid.0)),
                        start: mc.start,
                        len: mc.len,
                        offset: dur(mclip.offset),
                        source_len: 1_000_000_000_000,
                        fade_in: dur(mclip.fade_in),
                        fade_out: dur(mclip.fade_out),
                        notes: mc.notes.iter().map(|x| NoteInfo { start: x.start, len: x.len, key: x.key, velocity: x.velocity }).collect(),
                        muted: mclip.muted,
                        take_group: mclip.take_group,
                    });
                }
                NodeInfo {
                    id: i as u32,
                    name: n.name.clone(),
                    kind: match n.kind {
                        NodeKind::Master => "master",
                        NodeKind::Bus => "bus",
                        NodeKind::Track(TrackFormat::Mono) => "mono",
                        NodeKind::Track(TrackFormat::Stereo) => "stereo",
                    }
                    .into(),
                    color: match self.session.node_color(id) {
                        "" => "#9aa3b2".to_owned(),
                        c => c.to_owned(),
                    },
                    gain: st.gain,
                    pan: st.pan,
                    mute: st.mute,
                    solo: st.solo,
                    invert: st.invert,
                    arm: st.arm,
                    input: {
                        let inp = self.session.node_input(id);
                        if inp.midi {
                            InputSpec { kind: "midi".into(), channels: Vec::new() }
                        } else {
                            InputSpec { kind: "audio".into(), channels: if inp.stereo { vec![inp.channels[0] as u32 + 1, inp.channels[1] as u32 + 1] } else { vec![inp.channels[0] as u32 + 1] } }
                        }
                    },
                    monitor: self.session.node_input(id).monitor,
                    dynamics: DynamicsSpec::from_engine(self.session.input_dynamics(id)),
                    velocity_curve: VelocityCurveSpec::from_engine(self.session.velocity_curve(id)),
                    midi_filter: self.session.midi_filter(id).iter().map(|r| MidiRuleSpec { device: r.device.clone(), channels: if r.channels == daw_session::midi_filter::ALL_CHANNELS { Vec::new() } else { (1..=16u8).filter(|c| r.channels & (1u16 << (c - 1)) != 0).collect() } }).collect(),
                    inserts: n
                        .inserts
                        .iter()
                        .map(|i| InsertInfo {
                            id: i.id,
                            name: i.name.clone(),
                            vendor: self.meta.plugins.get(&i.id).map(|p| p.vendor.clone()).unwrap_or_default(),
                            bypass: i.bypass,
                            has_sidechain: self.plugin_handle(i.id).is_some_and(|h| h.has_sidechain()),
                            sidechain: i.sidechain.map(|s| s.0),
                            latency: self.plugin_handle(i.id).map_or(0, |h| h.latency()),
                            gui: self.gui_available(i.id),
                            ui_open: self.plugin_ui_open(i.id),
                        })
                        .collect(),
                    sends: n.sends.iter().map(|s| SendInfo { to: s.to.0, gain: s.gain, pre: s.pre, mute: s.mute, solo: s.solo }).collect(),
                    group: self.session.node_group(n.id),
                    automation: self.session.lanes(n.id).iter().filter(|l| match l.target {
                        AutoTarget::SendGain { to } => n.sends.iter().any(|s| s.to == to),
                        AutoTarget::InsertParam { insert, .. } => n.inserts.iter().any(|i| i.id == insert),
                        _ => true,
                    }).map(|l| LaneInfo {
                        target: lane_target_back(l.target),
                        mode: lane_mode_name(self.shown_lane_mode(n.id.0, l.target, l.mode)).to_owned(),
                        points: l.points.iter().map(|p| LanePoint { pos: [p.pos.num(), p.pos.den()], value: p.value, curve: if p.curve == AutoCurve::Step { "step" } else { "linear" }.to_owned() }).collect(),
                    }).collect(),
                    output: n.output.map(|o| o.0),
                    phys: n.phys.map(|p| ReturnOutput { first: p.first + 1, stereo: p.stereo, mono: p.mono || !p.stereo }),
                    return_groups: self.session.return_groups(n.id).map(|g| ReturnGroupsInfo {
                        groups: g.groups.iter().map(|x| GroupInfo { id: x.id, name: x.name.clone(), color: x.color.clone(), offsets: self.group_offset_infos(x.id) }).collect(),
                        members: g.of.iter().enumerate().filter(|(_, &gr)| gr != 0).map(|(i, &gr)| [i as u32, gr]).collect(),
                    }),
                    clips,
                }
            })
            .collect();
        Event::Project(ProjectInfo { audio_outputs: self.audio_outputs, audio_inputs: self.audio_inputs, sample_rate: self.session.sample_rate(), tempo_map, meters, latency_samples: self.handle.latency_samples(), auto_crossfade: self.auto_crossfade, write_arms: self.write_arm_infos(), midi_devices: self.midi_devices.clone(), groups: self.session.groups().iter().map(|g| GroupInfo { id: g.id, name: g.name.clone(), color: g.color.clone(), offsets: self.group_offset_infos(g.id) }).collect(), nodes })
    }

    /// Crêtes `[min, max]` de `columns` colonnes de la ligne de temps `[from, to[` pour un clip audio.
    fn peaks_for(&mut self, clip: u64, from: u64, to: u64, columns: usize) -> Result<Vec<[f32; 2]>, String> {
        let (node_idx, k) = (0..self.session.graph().nodes.len())
            .find_map(|i| self.session.clip_ids(NodeId(i as u32)).iter().position(|c| c.0 == clip).map(|k| (i, k)))
            .ok_or_else(|| format!("clip audio inconnu : {clip}"))?;
        let c = &self.session.graph().nodes[node_idx].clips[k];
        let (start, offset, data) = (c.start, c.offset, Arc::clone(&c.data));
        let key = Arc::as_ptr(&data) as usize;
        let peaks = Arc::clone(self.peaks.entry(key).or_insert_with(|| Arc::new(Peaks::build(&data))));
        // Les crêtes couvrent toutes les données du fichier, pas seulement la partie visible du clip : l'éditeur de fondu montre
        // la matière qu'un rognage découvrirait. Hors des données : colonnes vides (silence).
        let data_lo = start as f64 - offset as f64; // position sur la ligne de temps du premier échantillon du fichier
        let data_hi = data_lo + data.len() as f64;
        let (lo, hi) = ((from as f64).max(data_lo), (to as f64).min(data_hi));
        let span = (to - from) as f64;
        let mut out = Vec::with_capacity(columns);
        for col in 0..columns {
            let a = from as f64 + span * col as f64 / columns as f64;
            let b = from as f64 + span * (col + 1) as f64 / columns as f64;
            let (a, b) = (a.max(lo), b.min(hi));
            if b <= a {
                out.push([0.0, 0.0]);
                continue;
            }
            let da = (a - data_lo) as u64;
            let db = ((b - data_lo).ceil() as u64).max(da + 1);
            let (mut mn, mut mx) = (f32::INFINITY, f32::NEG_INFINITY);
            for ch in 0..peaks.channels() {
                for (l, h) in peaks.query(ch, da, db, 1) {
                    mn = mn.min(l);
                    mx = mx.max(h);
                }
            }
            out.push(if mn <= mx { [mn, mx] } else { [0.0, 0.0] });
        }
        let _: &AudioData = &data;
        Ok(out)
    }
}

/// `Option<Option<T>>` → `Option<Option<T>>` sans le dernier niveau d'échec : `None` extérieur = fraction invalide.
trait TransposeOk<T> {
    fn transpose_ok(self) -> Option<Option<T>>;
}

impl<T> TransposeOk<T> for Option<Option<T>> {
    fn transpose_ok(self) -> Option<Option<T>> {
        match self {
            None => Some(None),      // champ absent : inchangé
            Some(None) => None,      // fraction invalide
            Some(Some(v)) => Some(Some(v)),
        }
    }
}

fn lane_target(t: LaneTarget) -> AutoTarget {
    match t {
        LaneTarget::Gain => AutoTarget::Gain,
        LaneTarget::Pan => AutoTarget::Pan,
        LaneTarget::Send { to } => AutoTarget::SendGain { to: NodeId(to) },
        LaneTarget::Insert { insert, param } => AutoTarget::InsertParam { insert, param },
    }
}

fn lane_target_back(t: AutoTarget) -> LaneTarget {
    match t {
        AutoTarget::Gain => LaneTarget::Gain,
        AutoTarget::Pan => LaneTarget::Pan,
        AutoTarget::SendGain { to } => LaneTarget::Send { to: to.0 },
        AutoTarget::InsertParam { insert, param } => LaneTarget::Insert { insert, param },
    }
}

fn lane_mode(m: &str) -> AutoMode {
    match m {
        "trim" => AutoMode::Trim,
        "off" => AutoMode::Off,
        _ => AutoMode::Read,
    }
}

fn lane_mode_name(m: AutoMode) -> &'static str {
    match m {
        AutoMode::Read => "read",
        AutoMode::Trim => "trim",
        AutoMode::Off => "off",
    }
}
