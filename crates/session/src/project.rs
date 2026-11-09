//! Fichier de projet : JSON versionné, avec migrations.
//!
//! Choix (CLAUDE.md §14) : lisible, comparable avec `diff`, réparable à la main. Les fichiers audio ne sont **pas**
//! dans le projet : il les référence par chemin. Le chargement de l'audio est confié à l'appelant ([`AudioLoader`]),
//! ce qui laisse ce crate sans accès disque (il compile en WebAssembly).
//!
//! Version du format : [`CURRENT_VERSION`]. Pour changer le format : incrémenter la version, ajouter **à la fin** de
//! [`MIGRATIONS`] une fonction qui transforme le JSON de l'ancienne version vers la nouvelle, et ajouter un fichier
//! de la version précédente aux tests. Un fichier plus récent que le programme est refusé (jamais deviné).

use std::sync::Arc;

use daw_core_types::time::{Frac, Meter, MeterMap, Tempo, TempoMap};
use daw_core_types::NodeId;
use daw_engine::{AudioData, AutoCurve, AutoTarget, Clip, Fade, FadeShape, GraphDesc, GraphError, NodeKind, NodeParams, TrackFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::music::{MClip, MEvent, MKind, MLane, MNote, MPoint};
use crate::{PendingInsertLane, Session, Timing};

/// Migration du JSON d'une version vers la suivante.
pub type Migration = fn(&mut Value) -> Result<(), String>;

/// `MIGRATIONS[i]` fait passer un projet de la version `i + 1` à la version `i + 2`.
///
/// Historique : v1 = premier format (clips de notes en échantillons) ; v2 = temps musical (cartes des tempos et des
/// chiffrages, clips de notes et automation en fractions de ronde) ; v3 = événements de contrôle des clips de notes
/// (contrôleurs, molette de hauteur, pressions) ; v4 = fenêtre visible (`offset`) et fondus de vélocité des clips de notes ; v5 = trois formes de fondu de plus (`s_curve`, `slow`, `fast`) ; v9 = mode des courbes d'automation (`read`, `trim`, `off`) et courbes de niveau d'envoi ; v8 = envois « avant le fader » (`pre`) et vers la sortie principale ; v6 = courbe de fondu continue (`bend`, `smooth`) : les formes de la v5 deviennent des réglages de la courbe de base.
pub const MIGRATIONS: &[Migration] = &[v1_to_v2, v2_to_v3, v3_to_v4, v4_to_v5, v5_to_v6, v6_to_v7, v7_to_v8, v8_to_v9, v9_to_v10, v10_to_v11, v11_to_v12, v12_to_v13, v13_to_v14, v14_to_v15, v15_to_v16, v16_to_v17, v17_to_v18, v18_to_v19, v19_to_v20];
pub const CURRENT_VERSION: u32 = MIGRATIONS.len() as u32 + 1;

/// Fournit les données d'un fichier audio à partir du chemin inscrit dans le projet.
pub trait AudioLoader {
    fn load(&self, path: &str) -> Result<Arc<AudioData>, String>;
}

#[derive(Debug, PartialEq)]
pub enum ProjectError {
    /// JSON invalide ou ne correspondant pas au format.
    Parse(String),
    MissingVersion,
    /// Fichier écrit par une version plus récente du programme.
    TooNew { found: u32, supported: u32 },
    Migration { from: u32, error: String },
    Audio { path: String, error: String },
    /// Des données audio d'un clip n'ont pas été déclarées avec `register_asset`.
    UnregisteredAudio { node: NodeId },
    /// Le projet décrit un graphe invalide (référence à un nœud inexistant, boucle...).
    BadGraph(String),
    /// La conversion de fréquence a été interrompue par l'appelant.
    Cancelled,
    /// Fréquences d'échantillonnage différentes dans un même projet.
    SampleRate { path: String, found: u32, expected: u32 },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProjectError {}

// ---- format du fichier (version actuelle) ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct FileDto {
    version: u32,
    sample_rate: u32,
    pdc: bool,
    tempo_map: Vec<TempoChangeDto>,
    meters: Vec<MeterChangeDto>,
    assets: Vec<AssetDto>,
    nodes: Vec<NodeDto>,
    /// Piste d'accords au format `flibuste-carte` (objet JSON), absente si elle est vide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chord_track: Option<Value>,
    /// Carte de structure (sections nommées), absente si vide (format 14).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    structure: Vec<SectionDto>,
    /// Paroles (syllabes en fractions de ronde), absentes si vide : champ facultatif, le numéro de version ne change pas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    lyrics: Vec<LyricDto>,
    /// Groupes de pistes (format 15), absents si vide.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    groups: Vec<GroupDto>,
}

#[derive(Serialize, Deserialize)]
struct DynDto {
    enabled: bool,
    gate_db: f32,
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup_db: f32,
    #[serde(default)]
    record: bool,
}

#[derive(Serialize, Deserialize)]
struct VelCurveDto {
    enabled: bool,
    #[serde(default)]
    bend: i16,
    #[serde(default)]
    smooth: u16,
    min: u8,
    max: u8,
    #[serde(default)]
    fixed: u8,
}

#[derive(Serialize, Deserialize)]
struct MidiRuleDto {
    #[serde(default)]
    device: String,
    channels: u16,
}

#[derive(Serialize, Deserialize)]
struct RetGroupsDto {
    groups: Vec<GroupDto>,
    /// (nœud, groupe) de chaque membre.
    members: Vec<[u32; 2]>,
}

#[derive(Serialize, Deserialize)]
struct GroupDto {
    id: u32,
    name: String,
    #[serde(default)]
    color: String,
}

#[derive(Serialize, Deserialize)]
struct SectionDto {
    at: FracDto,
    dur: FracDto,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LyricDto {
    at: FracDto,
    text: String,
    #[serde(default, skip_serializing_if = "is_false")]
    hyphen: bool,
    #[serde(default = "one_verse", skip_serializing_if = "is_first_verse")]
    verse: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end: Option<FracDto>,
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn one_verse() -> u8 {
    1
}
fn is_first_verse(v: &u8) -> bool {
    *v == 1
}

/// Fraction de ronde `[numérateur, dénominateur]`.
type FracDto = [u64; 2];

#[derive(Serialize, Deserialize)]
struct TempoChangeDto {
    pos: FracDto,
    num: u32,
    den: u32,
}

#[derive(Serialize, Deserialize)]
struct MeterChangeDto {
    bar: u32,
    num: u32,
    den: u32,
}

#[derive(Serialize, Deserialize)]
struct LaneDto {
    /// `gain`, `pan` ou `send:<numéro du nœud destinataire>` (le niveau d'un envoi) ; `insert_param` n'est pas enregistré pour l'instant.
    target: String,
    /// `read`, `trim` ou `off` (absent des projets d'avant la version 9 : `read`).
    #[serde(default = "read_mode")]
    mode: String,
    points: Vec<PointDto>,
}

fn read_mode() -> String {
    "read".to_owned()
}

fn mode_name(m: daw_engine::AutoMode) -> &'static str {
    match m {
        daw_engine::AutoMode::Read => "read",
        daw_engine::AutoMode::Trim => "trim",
        daw_engine::AutoMode::Off => "off",
    }
}

fn mode_from(name: &str) -> Result<daw_engine::AutoMode, ProjectError> {
    match name {
        "read" => Ok(daw_engine::AutoMode::Read),
        "trim" => Ok(daw_engine::AutoMode::Trim),
        "off" => Ok(daw_engine::AutoMode::Off),
        other => Err(ProjectError::Parse(format!("mode d'automation inconnu : {other}"))),
    }
}

#[derive(Serialize, Deserialize)]
struct PointDto {
    pos: FracDto,
    value: f64,
    curve: String,
}

#[derive(Serialize, Deserialize)]
struct AssetDto {
    id: u32,
    path: String,
    /// Longueur (trames) et canaux des données ; servent seulement à remplacer un fichier référencé en place devenu introuvable.
    #[serde(default)]
    frames: u64,
    #[serde(default)]
    channels: u8,
}

#[derive(Serialize, Deserialize)]
struct NodeDto {
    kind: String,
    name: String,
    gain: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    /// Polarité inversée (absente des projets d'avant la version 7 : faux).
    #[serde(default)]
    invert: bool,
    output: Option<u32>,
    /// Sortie physique du bus (retour de monitoring) ; absente : pas de sortie physique (format 13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phys: Option<PhysDto>,
    /// Groupe du nœud (format 15), 0 ou absent : aucun.
    #[serde(default, skip_serializing_if = "is_zero")]
    group: u32,
    /// Groupes propres à ce retour (format 16) : ils remplacent ceux du projet dans son mix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ret_groups: Option<RetGroupsDto>,
    /// Jeton secret de l'adresse de tablette (retours ; format 17).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    monitor_token: String,
    /// Rôle de la télécommande du retour (facultatif, 0 si absent : le musicien ne règle que son retour).
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    monitor_role: u8,
    /// Filtre d'entrée MIDI de la piste (format 18) : absent ou vide, tout est accepté.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    midi_filter: Vec<MidiRuleDto>,
    /// Dynamique d'entrée de la piste (format 19) : absente, désactivée.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_dyn: Option<DynDto>,
    /// Courbe de réponse de la vélocité de la piste (format 20) : absente, désactivée.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vel_curve: Option<VelCurveDto>,
    sends: Vec<SendDto>,
    clips: Vec<ClipDto>,
    midi_clips: Vec<MidiClipDto>,
    automation: Vec<LaneDto>,
}

#[derive(Serialize, Deserialize)]
struct PhysDto {
    first: u8,
    stereo: bool,
    mono: bool,
}

#[derive(Serialize, Deserialize)]
struct SendDto {
    to: u32,
    gain: f32,
    /// Pris avant le fader (absent des projets d'avant la version 8 : faux).
    #[serde(default)]
    pre: bool,
    /// Coupé / solo dans la destination (absents des projets d'avant la version 15 : faux).
    #[serde(default)]
    mute: bool,
    #[serde(default)]
    solo: bool,
}

#[derive(Serialize, Deserialize)]
struct FadeDto {
    len: u64,
    shape: String,
    /// Courbure en millièmes (−1000 concave … +1000 convexe).
    bend: i16,
    /// Adoucissement en S en millièmes (0 … 1000).
    smooth: u16,
}

#[derive(Serialize, Deserialize)]
struct ClipDto {
    asset: u32,
    start: u64,
    offset: u64,
    len: u64,
    gain: f32,
    fade_in: FadeDto,
    fade_out: FadeDto,
    #[serde(default, skip_serializing_if = "is_false")]
    muted: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    take_group: u32,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Serialize, Deserialize)]
struct MidiClipDto {
    start: FracDto,
    len: FracDto,
    offset: FracDto,
    fade_in: FracDto,
    fade_out: FracDto,
    notes: Vec<NoteDto>,
    events: Vec<EventDto>,
    #[serde(default, skip_serializing_if = "is_false")]
    muted: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    take_group: u32,
}

/// Événement de contrôle : `type` vaut `control_change` (`controller`, `value`), `pitch_bend` (`value` de -8192 à 8191),
/// `channel_pressure` (`value`) ou `poly_pressure` (`key`, `value`).
#[derive(Serialize, Deserialize)]
struct EventDto {
    start: FracDto,
    channel: u8,
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    controller: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<u8>,
    value: i32,
}

fn event_to(e: &MEvent) -> EventDto {
    let (kind, controller, key, value) = match e.kind {
        MKind::ControlChange { controller, value } => ("control_change", Some(controller), None, value as i32),
        MKind::PitchBend { value } => ("pitch_bend", None, None, value as i32),
        MKind::ChannelPressure { value } => ("channel_pressure", None, None, value as i32),
        MKind::PolyPressure { key, value } => ("poly_pressure", None, Some(key), value as i32),
    };
    EventDto { start: frac_to(e.start), channel: e.channel, kind: kind.to_owned(), controller, key, value }
}

fn event_from(e: &EventDto) -> Result<MEvent, ProjectError> {
    let bad = |m: &str| ProjectError::Parse(m.to_owned());
    let byte = |v: i32| u8::try_from(v).map_err(|_| bad("valeur d'événement hors plage"));
    let kind = match e.kind.as_str() {
        "control_change" => MKind::ControlChange { controller: e.controller.ok_or_else(|| bad("contrôleur manquant"))?, value: byte(e.value)? },
        "pitch_bend" => MKind::PitchBend { value: i16::try_from(e.value).map_err(|_| bad("molette hors plage"))? },
        "channel_pressure" => MKind::ChannelPressure { value: byte(e.value)? },
        "poly_pressure" => MKind::PolyPressure { key: e.key.ok_or_else(|| bad("touche manquante"))?, value: byte(e.value)? },
        other => return Err(ProjectError::Parse(format!("type d'événement inconnu : {other}"))),
    };
    Ok(MEvent { start: frac_from(e.start)?, channel: e.channel, kind })
}

#[derive(Serialize, Deserialize)]
struct NoteDto {
    start: FracDto,
    len: FracDto,
    key: u8,
    velocity: f32,
    channel: u8,
    /// Voix d'affichage de la partition (1 à 4) ; absente = 1 (les anciens projets se relisent, un projet sans voix s'écrit sans ce champ).
    #[serde(default = "voice_one", skip_serializing_if = "is_voice_one")]
    voice: u8,
}

fn voice_one() -> u8 {
    1
}

fn is_voice_one(v: &u8) -> bool {
    *v == 1
}

fn frac_to(f: Frac) -> FracDto {
    [f.num(), f.den()]
}

fn frac_from(f: FracDto) -> Result<Frac, ProjectError> {
    Frac::new(f[0], f[1]).ok_or_else(|| ProjectError::Parse("fraction à dénominateur nul".into()))
}

/// Migration 1 → 2 : les clips de notes passent des échantillons au temps musical, avec le tempo implicite de la
/// version 1 (120 noires par minute, 4/4). Une ronde = 2 secondes = `2 × fréquence` échantillons.
fn v1_to_v2(v: &mut Value) -> Result<(), String> {
    let rate = v["sample_rate"].as_u64().filter(|&r| r > 0).ok_or("sample_rate manquant")?;
    let to_frac = |x: &Value| -> Result<Value, String> {
        let samples = x.as_u64().ok_or("position de note invalide")?;
        let f = Frac::new(samples, 2 * rate).ok_or("fraction invalide")?;
        Ok(serde_json::json!([f.num(), f.den()]))
    };
    for node in v["nodes"].as_array_mut().ok_or("nodes manquant")? {
        node["automation"] = Value::Array(Vec::new());
        for clip in node["midi_clips"].as_array_mut().ok_or("midi_clips manquant")? {
            clip["start"] = to_frac(&clip["start"])?;
            clip["len"] = to_frac(&clip["len"])?;
            for note in clip["notes"].as_array_mut().ok_or("notes manquantes")? {
                note["start"] = to_frac(&note["start"])?;
                note["len"] = to_frac(&note["len"])?;
            }
        }
    }
    v["tempo_map"] = serde_json::json!([{ "pos": [0, 1], "num": 120, "den": 1 }]);
    v["meters"] = serde_json::json!([{ "bar": 0, "num": 4, "den": 4 }]);
    Ok(())
}

/// Migration 2 → 3 : les clips de notes gagnent une liste d'événements de contrôle (vide).
fn v2_to_v3(v: &mut Value) -> Result<(), String> {
    for node in v["nodes"].as_array_mut().ok_or("nodes manquant")? {
        for clip in node["midi_clips"].as_array_mut().ok_or("midi_clips manquant")? {
            clip["events"] = Value::Array(Vec::new());
        }
    }
    Ok(())
}

/// Migration 3 → 4 : les clips de notes gagnent une fenêtre visible (décalage nul) et des fondus de vélocité (nuls).
fn v3_to_v4(v: &mut Value) -> Result<(), String> {
    for node in v["nodes"].as_array_mut().ok_or("nodes manquant")? {
        for clip in node["midi_clips"].as_array_mut().ok_or("midi_clips manquant")? {
            clip["offset"] = serde_json::json!([0, 1]);
            clip["fade_in"] = serde_json::json!([0, 1]);
            clip["fade_out"] = serde_json::json!([0, 1]);
        }
    }
    Ok(())
}

/// Migration 4 → 5 : aucune donnée à transformer (les formes de fondu ajoutées sont de nouvelles valeurs possibles).
fn v4_to_v5(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 5 → 6 : les fondus gagnent `bend` et `smooth` ; les trois formes ajoutées en v5 deviennent des réglages de la
/// courbe linéaire de base (`s_curve` = adoucissement 1000, `slow` = courbure +1000, `fast` = courbure −1000).
fn v5_to_v6(v: &mut Value) -> Result<(), String> {
    for node in v["nodes"].as_array_mut().ok_or("nodes manquant")? {
        for clip in node["clips"].as_array_mut().ok_or("clips manquant")? {
            for key in ["fade_in", "fade_out"] {
                let fade = clip.get_mut(key).ok_or("fondu manquant")?;
                let (shape, bend, smooth) = match fade["shape"].as_str().ok_or("forme de fondu manquante")? {
                    "s_curve" => ("linear", 0, 1000),
                    "slow" => ("linear", 1000, 0),
                    "fast" => ("linear", -1000, 0),
                    other => (if other == "equal_power" { "equal_power" } else { "linear" }, 0, 0),
                };
                fade["shape"] = Value::from(shape);
                fade["bend"] = Value::from(bend);
                fade["smooth"] = Value::from(smooth);
            }
        }
    }
    Ok(())
}

/// Migration 6 → 7 : chaque nœud gagne `invert` (polarité), faux pour tous les projets existants.
fn v6_to_v7(v: &mut Value) -> Result<(), String> {
    for node in v["nodes"].as_array_mut().ok_or("nodes manquant")? {
        node["invert"] = Value::from(false);
    }
    Ok(())
}

/// Migration 7 → 8 : chaque envoi gagne `pre` (avant le fader), faux pour tous les projets existants ; les envois peuvent désormais
/// viser la sortie principale (aucune donnée à transformer pour cela).
fn v7_to_v8(v: &mut Value) -> Result<(), String> {
    for node in v["nodes"].as_array_mut().ok_or("nodes manquant")? {
        for send in node["sends"].as_array_mut().ok_or("sends manquant")? {
            send["pre"] = Value::from(false);
        }
    }
    Ok(())
}

/// Migration 8 → 9 : chaque courbe d'automation gagne un `mode` (`read` : comportement des projets existants) ; les courbes peuvent
/// aussi viser le niveau d'un envoi (`send:<nœud>`), sans rien à transformer pour cela.
fn v8_to_v9(v: &mut Value) -> Result<(), String> {
    for node in v["nodes"].as_array_mut().ok_or("nodes manquant")? {
        for lane in node["automation"].as_array_mut().ok_or("automation manquante")? {
            lane["mode"] = Value::from("read");
        }
    }
    Ok(())
}

/// Migration 9 → 10 : un fichier audio peut être **référencé en place** (chemin absolu, hors du dossier du projet) ; l'entrée du
/// fichier porte alors sa longueur et son nombre de canaux (`frames`, `channels`) pour qu'un fichier devenu introuvable soit
/// remplacé par du silence sans perdre les clips. Rien à transformer : les champs sont facultatifs. La version signale seulement
/// aux programmes plus anciens qu'ils ne savent pas lire ces chemins.
fn v9_to_v10(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 10 → 11 : prises composites. Un clip audio peut être muet (`muted`) et appartenir à un groupe de prises (`take_group`) ;
/// les deux champs sont facultatifs et absents d'un ancien fichier (aucun muet, aucun groupe).
fn v10_to_v11(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 11 → 12 : piste d'accords (`chord_track`, au format `flibuste-carte`), facultative : absente = piste vide.
fn v11_to_v12(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 19 → 20 : courbe de réponse de la vélocité d'une piste (`vel_curve`), facultative : absente = désactivée.
fn v19_to_v20(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 18 → 19 : dynamique d'entrée d'une piste (`input_dyn`), facultative : absente = désactivée.
fn v18_to_v19(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 17 → 18 : filtre d'entrée MIDI d'une piste (`midi_filter`), facultatif : absent = tout accepter.
fn v17_to_v18(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 16 → 17 : jeton de l'adresse de tablette d'un retour (`monitor_token` du nœud), facultatif : absent = pas encore donné.
fn v16_to_v17(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 15 → 16 : groupes propres à un retour (`ret_groups` du nœud), facultatifs : absents = les groupes du projet.
fn v15_to_v16(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 14 → 15 : groupes de pistes (`groups`, `group` par nœud) et coupe / solo des envois (`mute`, `solo`), facultatifs :
/// absents = aucun groupe, aucun envoi coupé.
fn v14_to_v15(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 13 → 14 : carte de structure (`structure`), facultative : absente = aucune section.
fn v13_to_v14(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Migration 12 → 13 : sortie physique d'un bus (`phys`, retour de monitoring), facultative : absente = pas de sortie physique.
fn v12_to_v13(_: &mut Value) -> Result<(), String> {
    Ok(())
}

/// Vrai pour un chemin absolu (Linux / macOS `/…`, Windows `C:\…` ou `\\serveur\…`) : un fichier référencé en place.
pub fn is_external_path(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with('/') || path.starts_with("\\\\") || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
}

fn fade_to(f: Fade) -> FadeDto {
    FadeDto { len: f.len, shape: shape_name(f.shape).to_owned(), bend: f.bend, smooth: f.smooth }
}

/// Nom d'une forme de fondu dans le fichier de projet et dans le protocole de l'interface.
pub fn shape_name(s: FadeShape) -> &'static str {
    match s {
        FadeShape::Linear => "linear",
        FadeShape::EqualPower => "equal_power",
    }
}

pub fn shape_from_name(name: &str) -> Option<FadeShape> {
    FadeShape::ALL.into_iter().find(|s| shape_name(*s) == name)
}

fn fade_from(f: &FadeDto) -> Result<Fade, ProjectError> {
    let shape = shape_from_name(&f.shape).ok_or_else(|| ProjectError::Parse(format!("forme de fondu inconnue : {}", f.shape)))?;
    if !(-1000..=1000).contains(&f.bend) || f.smooth > 1000 {
        return Err(ProjectError::Parse("courbe de fondu hors plage".into()));
    }
    Ok(Fade::curved(f.len, shape, f.bend, f.smooth))
}

/// Applique les migrations nécessaires pour amener `value` à la dernière version de `migrations`.
pub fn migrate_with(mut value: Value, migrations: &[Migration]) -> Result<Value, ProjectError> {
    let current = migrations.len() as u32 + 1;
    let found = value.get("version").and_then(Value::as_u64).ok_or(ProjectError::MissingVersion)? as u32;
    if found > current {
        return Err(ProjectError::TooNew { found, supported: current });
    }
    if found == 0 {
        return Err(ProjectError::Parse("version 0 invalide".into()));
    }
    for v in found..current {
        migrations[(v - 1) as usize](&mut value).map_err(|error| ProjectError::Migration { from: v, error })?;
        value["version"] = Value::from(v + 1);
    }
    Ok(value)
}

impl Session {
    /// Sérialise le projet. Retourne le JSON et la liste des avertissements : les **effets insérés** ne sont pas encore
    /// enregistrés (leur identité et leur état viendront avec le registre de plugins) et sont ignorés — l'avertissement
    /// les nomme pour que l'interface puisse le dire à l'utilisateur.
    pub fn to_json(&self, sample_rate: u32) -> Result<(String, Vec<String>), ProjectError> {
        let mut warnings = Vec::new();
        let mut nodes = Vec::new();
        for n in &self.graph.nodes {
            for ins in &n.inserts {
                warnings.push(format!("effet « {} » sur « {} » non enregistré", ins.name, n.name));
            }
            let mut clips = Vec::new();
            for c in &n.clips {
                let asset = self.assets.iter().position(|(d, _)| Arc::ptr_eq(d, &c.data)).ok_or(ProjectError::UnregisteredAudio { node: n.id })?;
                clips.push(ClipDto { asset: asset as u32, start: c.start, offset: c.offset, len: c.len, gain: c.gain, fade_in: fade_to(c.fade_in), fade_out: fade_to(c.fade_out), muted: c.muted, take_group: c.take_group });
            }
            nodes.push(NodeDto {
                kind: match n.kind {
                    NodeKind::Master => "master",
                    NodeKind::Bus => "bus",
                    NodeKind::Track(TrackFormat::Mono) => "mono",
                    NodeKind::Track(TrackFormat::Stereo) => "stereo",
                }
                .to_owned(),
                name: n.name.clone(),
                gain: n.params.gain,
                pan: n.params.pan,
                mute: n.params.mute,
                solo: n.params.solo,
                invert: n.params.invert,
                phys: n.phys.map(|p| PhysDto { first: p.first, stereo: p.stereo, mono: p.mono }),
                group: self.node_group.get(n.id.0 as usize).copied().unwrap_or(0),
                monitor_token: self.monitor_tokens.get(n.id.0 as usize).cloned().unwrap_or_default(),
                monitor_role: self.monitor_roles.get(n.id.0 as usize).copied().unwrap_or(0),
                vel_curve: (n.vel_curve != daw_engine::VelocityCurve::default()).then(|| VelCurveDto { enabled: n.vel_curve.enabled, bend: n.vel_curve.bend, smooth: n.vel_curve.smooth, min: n.vel_curve.min, max: n.vel_curve.max, fixed: n.vel_curve.fixed }),
                input_dyn: (n.input_dyn != daw_engine::InputDynamics::default()).then(|| DynDto { enabled: n.input_dyn.enabled, gate_db: n.input_dyn.gate_db, threshold_db: n.input_dyn.threshold_db, ratio: n.input_dyn.ratio, attack_ms: n.input_dyn.attack_ms, release_ms: n.input_dyn.release_ms, makeup_db: n.input_dyn.makeup_db, record: n.input_dyn.record }),
                midi_filter: self.midi_filters.get(n.id.0 as usize).map(|v| v.iter().map(|r| MidiRuleDto { device: r.device.clone(), channels: r.channels }).collect()).unwrap_or_default(),
                ret_groups: self.ret_groups.get(n.id.0 as usize).and_then(|g| g.as_ref()).map(|g| RetGroupsDto {
                    groups: g.groups.iter().map(|x| GroupDto { id: x.id, name: x.name.clone(), color: x.color.clone() }).collect(),
                    members: g.of.iter().enumerate().filter(|(_, &gr)| gr != 0).map(|(i, &gr)| [i as u32, gr]).collect(),
                }),
                output: n.output.map(|o| o.0),
                sends: n.sends.iter().map(|s| SendDto { to: s.to.0, gain: s.gain, pre: s.pre, mute: s.mute, solo: s.solo }).collect(),
                clips,
                midi_clips: self.midi[n.id.0 as usize]
                    .iter()
                    .map(|(_, m)| MidiClipDto {
                        start: frac_to(m.start),
                        len: frac_to(m.len),
                        offset: frac_to(m.offset),
                        fade_in: frac_to(m.fade_in),
                        fade_out: frac_to(m.fade_out),
                        notes: m.notes.iter().map(|x| NoteDto { start: frac_to(x.start), len: frac_to(x.len), key: x.key, velocity: x.velocity, channel: x.channel, voice: x.voice }).collect(),
                        events: m.events.iter().map(event_to).collect(),
                        muted: m.muted,
                        take_group: m.take_group,
                    })
                    .collect(),
                automation: self.lanes[n.id.0 as usize]
                    .iter()
                    .filter_map(|l| {
                        let target = match l.target {
                            AutoTarget::Gain => "gain".to_owned(),
                            AutoTarget::Pan => "pan".to_owned(),
                            AutoTarget::SendGain { to } => format!("send:{}", to.0),
                            // Un plugin est désigné par sa place dans la chaîne du nœud (les identifiants changent à chaque ouverture).
                            // Une courbe dont le plugin a disparu (conservée pour l'annulation) n'est pas enregistrée.
                            AutoTarget::InsertParam { insert, param } => format!("insert:{}:{param}", n.inserts.iter().position(|i| i.id == insert)?),
                        };
                        let points = l
                            .points
                            .iter()
                            .map(|p| PointDto { pos: frac_to(p.pos), value: p.value, curve: match p.curve { AutoCurve::Linear => "linear", AutoCurve::Step => "step" }.to_owned() })
                            .collect();
                        Some(LaneDto { target, mode: mode_name(l.mode).to_owned(), points })
                    })
                    .collect(),
            });
        }
        let file = FileDto {
            version: CURRENT_VERSION,
            sample_rate,
            pdc: self.graph.pdc,
            tempo_map: self.timing.tempo.changes().iter().map(|(p, t)| TempoChangeDto { pos: frac_to(*p), num: t.num(), den: t.den() }).collect(),
            meters: self.timing.meters.changes().iter().map(|(b, m)| MeterChangeDto { bar: *b, num: m.num(), den: m.den() }).collect(),
            assets: self.assets.iter().enumerate().map(|(i, (d, p))| AssetDto { id: i as u32, path: p.clone(), frames: d.len(), channels: d.channel_count() as u8 }).collect(),
            nodes,
            chord_track: if self.chords.is_empty() { None } else { serde_json::from_str(&self.chords.to_carte_json("")).ok() },
            groups: self.groups.iter().map(|g| GroupDto { id: g.id, name: g.name.clone(), color: g.color.clone() }).collect(),
            lyrics: self.lyrics.iter().map(|l| LyricDto { at: frac_to(l.at), text: l.text.clone(), hyphen: l.hyphen, verse: l.verse, end: l.end.map(frac_to) }).collect(),
            structure: self.structure.iter().map(|s| SectionDto { at: frac_to(s.at), dur: frac_to(s.dur), name: s.name.clone(), color: s.color.clone(), category: s.category.clone() }).collect(),
        };
        let json = serde_json::to_string_pretty(&file).map_err(|e| ProjectError::Parse(e.to_string()))?;
        Ok((json, warnings))
    }

    /// Relit un projet (migré si besoin). L'historique d'annulation n'est pas conservé et les identifiants de clips
    /// sont attribués à neuf. Retourne la session et la fréquence d'échantillonnage du projet.
    pub fn from_json(json: &str, loader: &dyn AudioLoader) -> Result<(Session, u32), ProjectError> {
        Self::from_json_at_rate(json, loader, None, &mut |_, _| true)
    }

    /// Comme [`Session::from_json`], mais avec conversion de la fréquence d'échantillonnage : `target` = la fréquence voulue
    /// (`None` : celle du fichier). Le loader donne l'audio **à la fréquence d'origine du fichier**, la session le rééchantillonne
    /// (`core-types::resample`, déterministe). Ce qui est en **temps musical** (clips de notes, automation, tempos, chiffrages) n'a
    /// rien à convertir : il se retraduit tout seul en échantillons à la nouvelle fréquence. Ce qui est en **échantillons** (position,
    /// décalage et longueur des clips audio, longueur de leurs fondus) est mis à l'échelle (arrondi au plus proche) puis borné aux
    /// données converties (un clip n'est jamais plus long que son fichier ni vide). `progress(fait, total)` est appelée avant chaque
    /// fichier audio ; renvoyer `false` interrompt (`ProjectError::Cancelled`).
    pub fn from_json_at_rate(json: &str, loader: &dyn AudioLoader, target: Option<u32>, progress: &mut dyn FnMut(usize, usize) -> bool) -> Result<(Session, u32), ProjectError> {
        let value: Value = serde_json::from_str(json).map_err(|e| ProjectError::Parse(e.to_string()))?;
        let value = migrate_with(value, MIGRATIONS)?;
        let file: FileDto = serde_json::from_value(value).map_err(|e| ProjectError::Parse(e.to_string()))?;

        let (from, rate) = (file.sample_rate, target.unwrap_or(file.sample_rate));
        let converting = rate != from;
        let scale = |x: u64| ((x as u128 * rate as u128 * 2 + from as u128) / (2 * from as u128)) as u64;
        let mut assets: Vec<(Arc<AudioData>, String)> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        for a in &file.assets {
            if a.id as usize != assets.len() {
                return Err(ProjectError::Parse("identifiants de fichiers audio non consécutifs".into()));
            }
            if !progress(assets.len(), file.assets.len()) {
                return Err(ProjectError::Cancelled);
            }
            let data = match loader.load(&a.path) {
                Ok(data) => {
                    if data.sample_rate() != file.sample_rate {
                        return Err(ProjectError::SampleRate { path: a.path.clone(), found: data.sample_rate(), expected: file.sample_rate });
                    }
                    if converting {
                        let chans = (0..data.channel_count()).map(|c| daw_core_types::resample::resample(data.channel(c), from, rate)).collect();
                        AudioData::new(rate, chans).map_err(|e| ProjectError::Audio { path: a.path.clone(), error: format!("{e:?}") })?
                    } else {
                        data
                    }
                }
                // Un fichier référencé en place peut avoir été déplacé ou effacé : le projet s'ouvre, ses clips jouent du silence
                // (sur la seule partie qu'ils utilisent) et le fichier est signalé comme manquant.
                Err(_) if is_external_path(&a.path) => {
                    let extent = file.nodes.iter().flat_map(|n| n.clips.iter()).filter(|c| c.asset == a.id).map(|c| c.offset + c.len).max().unwrap_or(1).max(1);
                    let channels = (a.channels as usize).clamp(1, 2);
                    missing.push(a.path.clone());
                    let extent = if converting { scale(extent) + 1 } else { extent };
                    AudioData::new(rate, vec![vec![0.0; extent as usize]; channels]).map_err(|e| ProjectError::Audio { path: a.path.clone(), error: format!("{e:?}") })?
                }
                Err(error) => return Err(ProjectError::Audio { path: a.path.clone(), error }),
            };
            assets.push((data, a.path.clone()));
        }

        let mut g = GraphDesc::new();
        g.pdc = file.pdc;
        let bad = |m: &str| ProjectError::Parse(m.to_owned());
        if file.nodes.first().map(|n| n.kind.as_str()) != Some("master") {
            return Err(bad("le premier nœud doit être la sortie principale"));
        }
        for (i, n) in file.nodes.iter().enumerate() {
            let id = if i == 0 {
                NodeId::MASTER
            } else {
                match n.kind.as_str() {
                    "mono" => g.add_track(&n.name, TrackFormat::Mono),
                    "stereo" => g.add_track(&n.name, TrackFormat::Stereo),
                    "bus" => g.add_bus(&n.name),
                    _ => return Err(bad("type de nœud inconnu")),
                }
            };
            let node = g.node_mut(id).ok_or_else(|| bad("nœud introuvable"))?;
            node.name = n.name.clone();
            node.params = NodeParams { gain: n.gain, pan: n.pan, mute: n.mute, solo: n.solo, invert: n.invert };
            node.output = n.output.map(NodeId);
            node.phys = n.phys.as_ref().map(|p| daw_engine::PhysOut { first: p.first, stereo: p.stereo, mono: p.mono });
            node.sends = n.sends.iter().map(|s| daw_engine::SendDesc { to: NodeId(s.to), gain: s.gain, pre: s.pre, mute: s.mute, solo: s.solo }).collect();
            for c in &n.clips {
                let (data, _) = assets.get(c.asset as usize).ok_or_else(|| bad("clip : fichier audio inconnu"))?;
                let (mut fade_in, mut fade_out) = (fade_from(&c.fade_in)?, fade_from(&c.fade_out)?);
                let (start, offset, len) = if converting {
                    // Mise à l'échelle, puis bornes : jamais au-delà des données converties, jamais vide.
                    let total = data.len();
                    let offset = scale(c.offset).min(total.saturating_sub(1));
                    let end = scale(c.offset + c.len).min(total).max(offset + 1).min(total.max(offset + 1));
                    let len = (end - offset).max(1);
                    fade_in = fade_in.with_len(scale(c.fade_in.len).min(len));
                    fade_out = fade_out.with_len(scale(c.fade_out.len).min(len));
                    (scale(c.start), offset, len)
                } else {
                    (c.start, c.offset, c.len)
                };
                node.clips.push(Clip { data: data.clone(), start, offset, len, gain: c.gain, fade_in, fade_out, muted: c.muted, take_group: c.take_group });
            }
        }

        let mut tempo: Option<TempoMap> = None;
        for c in &file.tempo_map {
            let t = Tempo::new(c.num, c.den).ok_or_else(|| bad("tempo invalide"))?;
            let pos = frac_from(c.pos)?;
            match tempo.as_mut() {
                None if pos == Frac::ZERO => tempo = Some(TempoMap::new(t)),
                Some(m) => m.set_tempo(pos, t),
                None => return Err(bad("le premier tempo doit être à la position 0")),
            }
        }
        let mut meters: Option<MeterMap> = None;
        for c in &file.meters {
            let m = Meter::new(c.num, c.den).ok_or_else(|| bad("chiffrage invalide"))?;
            match meters.as_mut() {
                None if c.bar == 0 => meters = Some(MeterMap::new(m)),
                Some(map) => map.set_meter(c.bar, m),
                None => return Err(bad("le premier chiffrage doit être à la mesure 0")),
            }
        }
        let timing = Timing { tempo: tempo.ok_or_else(|| bad("carte des tempos vide"))?, meters: meters.ok_or_else(|| bad("carte des chiffrages vide"))? };

        // Le graphe de base (sans notes ni courbes) d'abord ; la session y ajoute la version en échantillons.
        let mut s = Session::with_timing(g, rate, timing);
        for (i, n) in file.nodes.iter().enumerate() {
            for m in &n.midi_clips {
                let notes = m
                    .notes
                    .iter()
                    .map(|x| Ok(MNote { start: frac_from(x.start)?, len: frac_from(x.len)?, key: x.key, velocity: x.velocity, channel: x.channel, voice: x.voice }))
                    .collect::<Result<Vec<_>, ProjectError>>()?;
                let events = m.events.iter().map(event_from).collect::<Result<Vec<_>, ProjectError>>()?;
                let clip = MClip { start: frac_from(m.start)?, len: frac_from(m.len)?, offset: frac_from(m.offset)?, notes, events, fade_in: frac_from(m.fade_in)?, fade_out: frac_from(m.fade_out)?, muted: m.muted, take_group: m.take_group };
                s.add_midi_clip(NodeId(i as u32), clip).map_err(|e| ProjectError::BadGraph(format!("clip de notes : {e:?}")))?;
            }
            for l in &n.automation {
                // `Err((place, paramètre))` : paramètre d'un plugin, qui n'existe pas encore (il est recréé par l'appelant).
                let target: Result<AutoTarget, (usize, u32)> = match l.target.as_str() {
                    "gain" => Ok(AutoTarget::Gain),
                    "pan" => Ok(AutoTarget::Pan),
                    other => {
                        if let Some(to) = other.strip_prefix("send:").and_then(|n| n.parse::<u32>().ok()) {
                            Ok(AutoTarget::SendGain { to: NodeId(to) })
                        } else if let Some((idx, param)) = other.strip_prefix("insert:").and_then(|r| r.split_once(':')).and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<u32>().ok()?))) {
                            Err((idx, param))
                        } else {
                            return Err(ProjectError::Parse(format!("cible d'automation inconnue : {other}")));
                        }
                    }
                };
                let mode = mode_from(&l.mode)?;
                let points = l
                    .points
                    .iter()
                    .map(|p| {
                        let curve = match p.curve.as_str() {
                            "linear" => AutoCurve::Linear,
                            "step" => AutoCurve::Step,
                            other => return Err(ProjectError::Parse(format!("courbe inconnue : {other}"))),
                        };
                        Ok(MPoint { pos: frac_from(p.pos)?, value: p.value, curve })
                    })
                    .collect::<Result<Vec<_>, ProjectError>>()?;
                match target {
                    Ok(target) => s.set_lane(NodeId(i as u32), MLane { target, points, mode }).map_err(|e| ProjectError::BadGraph(format!("automation : {e:?}")))?,
                    Err((chain_index, param)) => s.pending_insert_lanes.push(PendingInsertLane { node: NodeId(i as u32), chain_index, param, points, mode }),
                }
            }
        }
        if let Some(ct) = &file.chord_track {
            let (track, _) = crate::chords::ChordTrack::from_carte_json(&crate::chords::ChordTrack::default(), &ct.to_string()).map_err(|e| ProjectError::BadGraph(e.to_string()))?;
            s.chords = track;
            s.rebuild_chords();
        }
        if !file.structure.is_empty() {
            let mut list = Vec::new();
            for d in &file.structure {
                list.push(crate::structure::Section { at: frac_from(d.at)?, dur: frac_from(d.dur)?, name: d.name.clone(), color: d.color.clone(), category: d.category.clone() });
            }
            crate::structure::normalize(&mut list);
            crate::structure::validate(&list).map_err(ProjectError::BadGraph)?;
            s.structure = list;
        }
        if !file.lyrics.is_empty() {
            let mut list = Vec::new();
            for d in &file.lyrics {
                list.push(crate::lyrics::Syllable { at: frac_from(d.at)?, text: d.text.clone(), hyphen: d.hyphen, verse: d.verse, end: d.end.map(frac_from).transpose()? });
            }
            crate::lyrics::normalize(&mut list);
            crate::lyrics::validate(&list).map_err(ProjectError::BadGraph)?;
            s.lyrics = list;
        }
        if !file.groups.is_empty() || file.nodes.iter().any(|n| n.group != 0) {
            let groups: Vec<crate::groups::Group> = file.groups.iter().map(|g| crate::groups::Group { id: g.id, name: g.name.clone(), color: g.color.clone() }).collect();
            let members: Vec<(NodeId, u32)> = file.nodes.iter().enumerate().filter(|(_, n)| n.group != 0).map(|(i, n)| (NodeId(i as u32), n.group)).collect();
            s.set_groups("", groups, &members).map_err(|e| ProjectError::BadGraph(format!("groupes : {e:?}")))?;
            s.next_group_id = file.groups.iter().map(|g| g.id).max().unwrap_or(0);
        }
        for (i, n) in file.nodes.iter().enumerate() {
            if let Some(d) = &n.input_dyn {
                let dynamics = daw_engine::InputDynamics { enabled: d.enabled, gate_db: d.gate_db, threshold_db: d.threshold_db, ratio: d.ratio, attack_ms: d.attack_ms, release_ms: d.release_ms, makeup_db: d.makeup_db, record: d.record };
                s.set_input_dynamics_untracked(NodeId(i as u32), dynamics).map_err(|e| ProjectError::BadGraph(format!("dynamique d'entrée : {e:?}")))?;
            }
            if let Some(v) = &n.vel_curve {
                let curve = daw_engine::VelocityCurve { enabled: v.enabled, bend: v.bend, smooth: v.smooth, min: v.min, max: v.max, fixed: v.fixed };
                s.set_velocity_curve_untracked(NodeId(i as u32), curve).map_err(|e| ProjectError::BadGraph(format!("courbe de vélocité : {e:?}")))?;
            }
            if !n.midi_filter.is_empty() {
                let rules = n.midi_filter.iter().map(|r| crate::midi_filter::MidiRule { device: r.device.clone(), channels: r.channels }).collect();
                s.set_midi_filter_untracked(NodeId(i as u32), rules).map_err(|e| ProjectError::BadGraph(format!("filtre MIDI : {e:?}")))?;
            }
            if !n.monitor_token.is_empty() {
                s.set_monitor_token_untracked(NodeId(i as u32), &n.monitor_token);
            }
            if n.monitor_role != 0 {
                s.set_monitor_role_untracked(NodeId(i as u32), n.monitor_role);
            }
            if let Some(rg) = &n.ret_groups {
                let groups: Vec<crate::groups::Group> = rg.groups.iter().map(|g| crate::groups::Group { id: g.id, name: g.name.clone(), color: g.color.clone() }).collect();
                let members: Vec<(NodeId, u32)> = rg.members.iter().map(|m| (NodeId(m[0]), m[1])).collect();
                s.next_group_id = s.next_group_id.max(groups.iter().map(|g| g.id).max().unwrap_or(0));
                s.set_return_groups("", NodeId(i as u32), Some((groups, members))).map_err(|e| ProjectError::BadGraph(format!("groupes d'un retour : {e:?}")))?;
            }
        }
        s.graph.compile(rate, 64).map_err(|e: GraphError| ProjectError::BadGraph(e.to_string()))?;
        // Les ajouts ci-dessus sont des éditions : un projet qu'on vient d'ouvrir n'a pas d'historique.
        s.undo.clear();
        s.redo.clear();
        s.assets = assets;
        s.missing = missing;
        Ok((s, rate))
    }
}
