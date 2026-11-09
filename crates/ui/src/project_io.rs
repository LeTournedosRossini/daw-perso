//! Fichier de projet sur le disque : un **dossier** « Nom.daw » qui contient `projet.json` et un sous-dossier `audio/` de
//! fichiers WAV (chemins relatifs : on copie le dossier, le projet suit).
//!
//! `projet.json` enveloppe le format de la session (`daw_session::project`, avec ses migrations) et y ajoute ce que la session
//! ne connaît pas : les **plugins** (identifiant CLAP, nom, état en octets) et les réglages d'affichage (couleurs, noms de clips).
//! L'écriture est atomique : on écrit à côté puis on renomme, et la version précédente reste en `projet.json.bak`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use daw_engine::AudioData;
use daw_recorder::wav::{read_wav, WavWriter};
use daw_session::project::{is_external_path, AudioLoader, ProjectError};
use daw_session::Session;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FORMAT: &str = "daw-project";
pub const FILE_VERSION: u32 = 1;
pub const PROJECT_FILE: &str = "projet.json";
/// Sauvegarde automatique : même contenu que `projet.json`, dans un fichier voisin (le projet enregistré n'est jamais touché).
pub const AUTOSAVE_FILE: &str = "projet.autosave.json";
pub const AUDIO_DIR: &str = "audio";
pub const EXTENSION: &str = "daw";

/// Un plugin du projet : où il est (nœud), qui il est (identifiant CLAP), et son état (octets en base 64).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginEntry {
    pub node: u32,
    pub name: String,
    pub clap_id: String,
    #[serde(default)]
    pub vendor: String,
    /// Nom du fichier du plugin à l'enregistrement (pour le retrouver vite) ; facultatif.
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub state: Option<String>,
    /// Contourné à l'enregistrement.
    #[serde(default)]
    pub bypass: bool,
    /// Nœud dont le signal alimente la chaîne latérale du plugin (numéro de nœud du projet) ; absent : aucune.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<u32>,
}

/// Réglages d'affichage que la session ne connaît pas. Les clés d'objet JSON sont du texte : numéros de nœuds en chaînes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiMeta {
    #[serde(default)]
    pub node_colors: BTreeMap<String, String>,
    /// Noms des clips audio de chaque nœud, dans l'ordre des clips du nœud.
    #[serde(default)]
    pub audio_names: BTreeMap<String, Vec<String>>,
    /// Noms des clips de notes de chaque nœud, dans l'ordre.
    #[serde(default)]
    pub midi_names: BTreeMap<String, Vec<String>>,
    /// Entrée de chaque piste (clé : numéro de nœud) ; une piste absente garde l'entrée par défaut (audio, entrée 1, écoute).
    #[serde(default)]
    pub inputs: BTreeMap<String, UiInput>,
    /// Ordre d'affichage des pistes et bus (numéros de nœuds) ; vide = ordre naturel.
    #[serde(default)]
    pub node_order: Vec<u32>,
    #[serde(default = "yes")]
    pub auto_crossfade: bool,
    /// Paramètres armés pour l'écriture de l'automation (`touch`, `latch`, `write`).
    #[serde(default)]
    pub write_arms: Vec<UiWriteArm>,
    /// Position (x, y) de la fenêtre de chaque plugin, par nom de plugin.
    #[serde(default)]
    pub plugin_windows: BTreeMap<String, [i32; 2]>,
    /// Punch allumé (la zone est la bande bleue de la règle, qui n'est pas enregistrée).
    #[serde(default)]
    pub punch_on: bool,
    /// Prises calées sur un guide : réglages, analyse (octets en base 64) et d'où elles viennent, pour refaire le rendu.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aligns: Vec<UiAlign>,
}

/// Où lire l'audio d'un clip : le fichier (chemin du projet), la position du clip dans le projet, son décalage et sa longueur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiAlignSrc {
    pub asset: String,
    pub start: u64,
    pub offset: u64,
    pub len: u64,
    /// Autres clips lus avec celui-ci comme une seule bande (un double ou un guide fait de plusieurs clips d'une même piste).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more: Vec<UiAlignSrc>,
}

/// Une prise calée telle qu'elle est enregistrée.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiAlign {
    /// Fichier audio de la prise rendue.
    pub asset: String,
    pub name: String,
    pub guide_name: String,
    /// Fiche de réglages (JSON de la bibliothèque de calage).
    pub settings: String,
    /// Analyse en octets (base 64).
    pub analysis: String,
    pub guide: UiAlignSrc,
    pub source: UiAlignSrc,
    pub window_start: u64,
    pub window_len: u64,
}

/// Un paramètre armé pour l'écriture de l'automation : `target` = `gain`, `pan`, `send:<nœud>` ou `insert:<place dans la chaîne>:<paramètre>`
/// (un plugin se désigne par sa place : les identifiants changent à chaque ouverture, comme pour les courbes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiWriteArm {
    pub node: u32,
    pub target: String,
    pub mode: String,
}

/// Entrée d'une piste telle qu'elle est enregistrée : `kind` `audio` (1 ou 2 canaux, numérotés à partir de 1) ou `midi`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiInput {
    pub kind: String,
    #[serde(default)]
    pub channels: Vec<u32>,
    #[serde(default = "yes")]
    pub monitor: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: String,
    version: u32,
    session: Value,
    #[serde(default)]
    plugins: Vec<PluginEntry>,
    #[serde(default)]
    ui: UiMeta,
}

/// Ce que contient un projet relu.
pub struct Loaded {
    pub session: Session,
    pub sample_rate: u32,
    pub plugins: Vec<PluginEntry>,
    pub ui: UiMeta,
}

/// Le dossier du projet pour un chemin saisi : l'extension `.daw` est ajoutée si elle manque.
pub fn project_dir(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION)) {
        p
    } else {
        let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        name.push(format!(".{EXTENSION}"));
        p.with_file_name(name)
    }
}

/// Nom du projet affiché : le nom du dossier sans l'extension.
pub fn project_name(dir: &Path) -> String {
    dir.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Projet".into())
}

// ---- base 64 (sans dépendance : l'état d'un plugin est un tableau d'octets) -----------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let t: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if t.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| B64.iter().position(|&x| x == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(t.len() / 4 * 3);
    for (i, q) in t.chunks(4).enumerate() {
        let last = i == t.len() / 4 - 1;
        let pad = q.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (k, &c) in q.iter().enumerate() {
            n = n << 6 | if k >= 4 - pad { 0 } else { val(c)? };
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

// ---- écriture ---------------------------------------------------------------------------------------------------------

/// Écrit `bytes` dans `path` sans jamais laisser un fichier à moitié écrit : fichier voisin `.tmp`, synchronisé, puis
/// renommé ; l'ancienne version est conservée à côté en `.bak`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if path.exists() {
        let _ = fs::copy(path, path.with_file_name(format!("{name}.bak")));
    }
    fs::rename(&tmp, path)
}

pub(crate) fn write_wav_file(path: &Path, data: &AudioData) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    let channels = data.channel_count();
    let mut w = WavWriter::create(&tmp, data.sample_rate(), channels as u16)?;
    let frames = data.len() as usize;
    let mut block = Vec::with_capacity(4096 * channels);
    for start in (0..frames).step_by(4096) {
        block.clear();
        for i in start..(start + 4096).min(frames) {
            for c in 0..channels {
                block.push(data.channel(c)[i]);
            }
        }
        w.write_samples(&block)?;
    }
    w.finish()?;
    fs::rename(&tmp, path)
}

pub(crate) fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { "audio".into() } else { s }
}

/// Chemin relatif sous `audio/` pour un fichier de la session : ceux qui y sont déjà restent, les autres (chemins
/// symboliques de la démonstration, prises) y sont rangés sous leur nom ; un nom déjà pris reçoit un numéro.
fn audio_path(current: &str, taken: &[String]) -> String {
    let rel = Path::new(current);
    let in_audio = rel.components().next().is_some_and(|c| c.as_os_str() == AUDIO_DIR) && rel.components().count() == 2;
    let base = if in_audio { current.to_owned() } else { format!("{AUDIO_DIR}/{}.wav", sanitize(rel.file_stem().and_then(|s| s.to_str()).unwrap_or("audio"))) };
    if !taken.contains(&base) {
        return base;
    }
    let stem = base.trim_end_matches(".wav").to_owned();
    (2..).map(|n| format!("{stem}-{n}.wav")).find(|c| !taken.contains(c)).expect("une suite infinie")
}

/// Enregistre le projet dans `dir` : crée le dossier, écrit les fichiers audio qui n'y sont pas encore, puis `projet.json`
/// (atomiquement). Les données audio sans chemin doivent avoir été déclarées (`Session::register_asset`) avant l'appel.
/// Retourne les avertissements (par exemple un effet qu'on ne sait pas enregistrer).
pub fn write_project(dir: &Path, session: &mut Session, plugins: Vec<PluginEntry>, ui: UiMeta, warnings: Vec<String>) -> Result<Vec<String>, String> {
    write_project_file(dir, PROJECT_FILE, session, plugins, ui, warnings)
}

/// Comme `write_project`, mais vers le fichier `file` du dossier (les fichiers audio sont rangés dans `audio/` comme d'habitude).
pub fn write_project_file(dir: &Path, file_name: &str, session: &mut Session, plugins: Vec<PluginEntry>, ui: UiMeta, mut warnings: Vec<String>) -> Result<Vec<String>, String> {
    let audio = dir.join(AUDIO_DIR);
    fs::create_dir_all(&audio).map_err(|e| format!("dossier du projet : {e}"))?;
    // Chaque fichier audio sous `audio/` (chemins déjà rangés d'abord, pour que les nouveaux ne leur prennent pas leur nom).
    let assets: Vec<(Arc<AudioData>, String)> = session.assets().to_vec();
    let mut taken: Vec<String> = Vec::new();
    let mut targets: Vec<Option<String>> = vec![None; assets.len()];
    for pass in 0..2 {
        for (i, (_, path)) in assets.iter().enumerate() {
            if is_external_path(path) {
                continue; // référencé en place : on n'y touche pas
            }
            let settled = Path::new(path).starts_with(AUDIO_DIR) && Path::new(path).components().count() == 2;
            if (pass == 0) == settled {
                let t = audio_path(path, &taken);
                taken.push(t.clone());
                targets[i] = Some(t);
            }
        }
    }
    for ((data, old), target) in assets.iter().zip(targets) {
        let Some(target) = target else { continue };
        if *old != target {
            session.register_asset(&target, Arc::clone(data));
        }
        let file = dir.join(&target);
        if !file.exists() {
            write_wav_file(&file, data).map_err(|e| format!("fichier audio « {target} » : {e}"))?;
        }
    }
    let (json, w) = session.to_json(session.sample_rate()).map_err(|e| format!("projet : {e:?}"))?;
    // La session signale les effets qu'elle ne sait pas décrire ; ici ils sont enregistrés à part (`plugins`).
    warnings.extend(w.into_iter().filter(|m| !m.starts_with("effet « ")));
    let file = ProjectFile { format: FORMAT.into(), version: FILE_VERSION, session: serde_json::from_str(&json).map_err(|e| e.to_string())?, plugins, ui };
    let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(file_name), text.as_bytes()).map_err(|e| format!("écriture de {file_name} : {e}"))?;
    Ok(warnings)
}

// ---- lecture ----------------------------------------------------------------------------------------------------------

/// Lit les fichiers audio d'un projet : seulement des chemins relatifs qui restent dans le dossier du projet.
struct DiskLoader<'a> {
    dir: &'a Path,
}

impl AudioLoader for DiskLoader<'_> {
    fn load(&self, path: &str) -> Result<Arc<AudioData>, String> {
        if is_external_path(path) {
            // Fichier référencé en place : n'importe quel format géré, mono ou stéréo (la fréquence est vérifiée par la session).
            let d = daw_recorder::decode::decode_file(Path::new(path)).map_err(|e| e.to_string())?;
            if d.audio.channels > 2 {
                return Err(format!("{path} : plus de 2 canaux (non référençable en place)"));
            }
            let channels: Vec<Vec<f32>> = (0..d.audio.channels as usize).map(|c| d.audio.channel(c)).collect();
            return AudioData::new(d.audio.sample_rate, channels).map_err(|e| format!("{path} : {e:?}"));
        }
        let rel = Path::new(path);
        if rel.is_absolute() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(format!("chemin refusé (hors du dossier du projet) : {path}"));
        }
        let wav = read_wav(&self.dir.join(rel)).map_err(|e| format!("{path} : {e}"))?;
        let channels: Vec<Vec<f32>> = (0..wav.channels.min(2) as usize).map(|c| wav.channel(c)).collect();
        AudioData::new(wav.sample_rate, channels).map_err(|e| format!("{path} : {e:?}"))
    }
}

/// Lit un fichier WAV flottant (une prise) comme données audio du moteur.
pub(crate) fn load_wav(path: &Path) -> Result<Arc<AudioData>, String> {
    let wav = read_wav(path).map_err(|e| format!("{} : {e}", path.display()))?;
    let channels: Vec<Vec<f32>> = (0..wav.channels.min(2) as usize).map(|c| wav.channel(c)).collect();
    AudioData::new(wav.sample_rate, channels).map_err(|e| format!("{} : {e:?}", path.display()))
}

/// Fréquence d'échantillonnage déclarée par le projet `file_name` de `dir`, sans charger l'audio (lecture du seul JSON).
pub fn peek_sample_rate(dir: &Path, file_name: &str) -> Result<u32, String> {
    let text = fs::read_to_string(dir.join(file_name)).map_err(|e| format!("lecture de {} : {e}", dir.join(file_name).display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{file_name} illisible : {e}"))?;
    v["session"]["sample_rate"].as_u64().map(|r| r as u32).ok_or_else(|| format!("{file_name} : fréquence d'échantillonnage introuvable"))
}

/// Pourquoi une conversion de projet s'arrête.
#[derive(Debug, PartialEq)]
pub enum ConvertError {
    Cancelled,
    Failed(String),
}

/// Copie le projet `src` dans le **nouveau** dossier `dst` converti à la fréquence `rate` : l'audio est rééchantillonné, les positions
/// des clips audio mises à l'échelle, ce qui est en temps musical (notes, automation, tempos) suit tout seul ; les plugins (identité
/// et état) et ce qui n'appartient qu'à l'interface sont repris tels quels. **`src` n'est jamais modifié.** Les fichiers référencés en
/// place (hors du dossier) deviennent des copies converties sous `audio/` du nouveau projet — sauf ceux qui sont introuvables, qui
/// restent référencés (et manquants). Le travail se fait dans `dst.part`, renommé à la fin : un échec ou une annulation ne laisse rien.
/// `progress(fait, total)` est appelée avant chaque fichier audio ; renvoyer `false` annule. Retourne les avertissements.
pub fn convert_project(src: &Path, dst: &Path, rate: u32, progress: &mut dyn FnMut(usize, usize) -> bool) -> Result<Vec<String>, ConvertError> {
    let fail = |m: String| ConvertError::Failed(m);
    let file = src.join(PROJECT_FILE);
    let text = fs::read_to_string(&file).map_err(|e| fail(format!("lecture de {} : {e}", file.display())))?;
    let pf: ProjectFile = serde_json::from_str(&text).map_err(|e| fail(format!("{PROJECT_FILE} illisible : {e}")))?;
    if pf.format != FORMAT {
        return Err(fail(format!("ce dossier n'est pas un projet (format « {} »)", pf.format)));
    }
    if pf.version > FILE_VERSION {
        return Err(fail(format!("projet écrit par une version plus récente du programme (format {})", pf.version)));
    }
    let session_json = serde_json::to_string(&pf.session).map_err(|e| fail(e.to_string()))?;
    let (mut session, _) = Session::from_json_at_rate(&session_json, &DiskLoader { dir: src }, Some(rate), progress).map_err(|e| match e {
        ProjectError::Cancelled => ConvertError::Cancelled,
        other => fail(explain(other)),
    })?;
    // Fichiers référencés en place et retrouvés : ils sont convertis, donc ne peuvent plus être référencés → copies sous `audio/`.
    let missing: Vec<String> = session.missing_assets().to_vec();
    for (data, path) in session.assets().to_vec() {
        if is_external_path(&path) && !missing.contains(&path) {
            let stem = Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            session.register_asset(&format!("{AUDIO_DIR}/{}.wav", sanitize(&stem)), data);
        }
    }
    let part = dst.with_file_name(format!("{}.part", dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let _ = fs::remove_dir_all(&part);
    // Les fiches de calage comptent en échantillons (analyse, fenêtres) : à une autre fréquence elles n'ont plus de sens ; les prises rendues restent.
    let mut ui = pf.ui;
    ui.aligns.clear();
    let result = write_project(&part, &mut session, pf.plugins, ui, Vec::new()).map_err(fail).and_then(|w| fs::rename(&part, dst).map(|_| w).map_err(|e| fail(format!("{} : {e}", dst.display()))));
    if result.is_err() {
        let _ = fs::remove_dir_all(&part);
    }
    result
}

pub fn read_project(dir: &Path) -> Result<Loaded, String> {
    read_project_file(dir, PROJECT_FILE)
}

pub fn read_project_file(dir: &Path, file_name: &str) -> Result<Loaded, String> {
    let file = dir.join(file_name);
    let text = fs::read_to_string(&file).map_err(|e| format!("lecture de {} : {e}", file.display()))?;
    let pf: ProjectFile = serde_json::from_str(&text).map_err(|e| format!("{file_name} illisible : {e}"))?;
    if pf.format != FORMAT {
        return Err(format!("ce dossier n'est pas un projet (format « {} »)", pf.format));
    }
    if pf.version > FILE_VERSION {
        return Err(format!("projet écrit par une version plus récente du programme (format {} ; ce programme comprend jusqu'à {FILE_VERSION})", pf.version));
    }
    let session_json = serde_json::to_string(&pf.session).map_err(|e| e.to_string())?;
    let (session, sample_rate) = Session::from_json(&session_json, &DiskLoader { dir }).map_err(explain)?;
    Ok(Loaded { session, sample_rate, plugins: pf.plugins, ui: pf.ui })
}

fn explain(e: ProjectError) -> String {
    match e {
        ProjectError::Audio { path, error } => format!("fichier audio introuvable ou illisible : {path} ({error})"),
        ProjectError::TooNew { found, supported } => format!("projet écrit par une version plus récente (session {found} > {supported})"),
        other => format!("projet invalide : {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_length_and_refuses_garbage() {
        for len in 0..40usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&bytes)).unwrap(), bytes, "longueur {len}");
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert!(base64_decode("TWF").is_none(), "longueur invalide");
        assert!(base64_decode("TW$u").is_none(), "caractère invalide");
        assert!(base64_decode("TQ==TQ==").is_none(), "remplissage au milieu");
    }

    #[test]
    fn project_dir_adds_the_extension_and_name_drops_it() {
        assert_eq!(project_dir("/tmp/Chanson"), PathBuf::from("/tmp/Chanson.daw"));
        assert_eq!(project_dir("/tmp/Chanson.daw"), PathBuf::from("/tmp/Chanson.daw"));
        assert_eq!(project_dir("/tmp/Chanson.DAW"), PathBuf::from("/tmp/Chanson.DAW"));
        assert_eq!(project_name(Path::new("/tmp/Ma chanson.daw")), "Ma chanson");
    }

    #[test]
    fn audio_paths_are_tidied_under_audio_without_collisions() {
        assert_eq!(audio_path("demo/batterie.wav", &[]), "audio/batterie.wav");
        assert_eq!(audio_path("audio/prise-1.wav", &[]), "audio/prise-1.wav");
        assert_eq!(audio_path("demo/batterie.wav", &["audio/batterie.wav".into()]), "audio/batterie-2.wav");
        assert_eq!(audio_path("a b/é!.wav", &[]), "audio/é.wav");
        assert_eq!(audio_path("", &[]), "audio/audio.wav");
    }

    #[test]
    fn atomic_write_replaces_the_file_and_keeps_a_backup() {
        let dir = std::env::temp_dir().join(format!("daw-atomic-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("projet.json");
        write_atomic(&f, b"un").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"un");
        assert!(!dir.join("projet.json.bak").exists(), "pas de copie à la première écriture");
        write_atomic(&f, b"deux").unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"deux");
        assert_eq!(fs::read(dir.join("projet.json.bak")).unwrap(), b"un");
        assert!(!dir.join("projet.json.tmp").exists(), "aucun fichier temporaire ne reste");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_loader_refuses_relative_paths_that_leave_the_project_folder_and_treats_absolute_ones_as_external_files() {
        let l = DiskLoader { dir: Path::new("/tmp/x") };
        // Chemins relatifs : jamais en dehors du dossier du projet.
        for bad in ["../secret.wav", "audio/../../x.wav"] {
            assert!(l.load(bad).unwrap_err().contains("hors du dossier"), "{bad}");
        }
        // Chemins absolus : fichiers référencés en place (format 10). Ils ne sont lus que s'ils sont de l'audio gérés ;
        // tout le reste échoue sans rien révéler de leur contenu.
        let dir = std::env::temp_dir().join(format!("daw-loader-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let text = dir.join("secret.wav");
        fs::write(&text, b"mot de passe : 1234").unwrap();
        for bad in [text.to_string_lossy().into_owned(), "/etc/passwd".to_owned(), "/n/existe/pas.flac".to_owned()] {
            let e = l.load(&bad).unwrap_err();
            assert!(!e.contains("1234") && !e.contains("root:"), "{e}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}

// ---- sauvegarde automatique ------------------------------------------------------------------------------------------

fn seconds(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Date (secondes depuis 1970) de la sauvegarde automatique de `dir`, s'il y en a une.
pub fn autosave_time(dir: &Path) -> Option<u64> {
    fs::metadata(dir.join(AUTOSAVE_FILE)).ok()?.modified().ok().map(seconds)
}

/// Date de la sauvegarde automatique si elle est **plus récente** que le projet enregistré (sinon : rien à proposer).
pub fn newer_autosave(dir: &Path) -> Option<u64> {
    let auto = fs::metadata(dir.join(AUTOSAVE_FILE)).ok()?.modified().ok()?;
    let saved = fs::metadata(dir.join(PROJECT_FILE)).ok()?.modified().ok()?;
    (auto > saved).then(|| seconds(auto))
}

/// Supprime la sauvegarde automatique de `dir` (et son ancienne version `.bak`) ; sans effet s'il n'y en a pas.
pub fn remove_autosave(dir: &Path) {
    for name in [AUTOSAVE_FILE.to_owned(), format!("{AUTOSAVE_FILE}.bak"), format!("{AUTOSAVE_FILE}.tmp")] {
        let _ = fs::remove_file(dir.join(name));
    }
}
