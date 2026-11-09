//! Hébergement de plugins CLAP.
//!
//! Étape 3, première tranche : charger une bibliothèque `.clap` et lister les plugins qu'elle contient.
//! Tout le `unsafe` est ici et regroupé dans [`Bundle`], qui possède la bibliothèque chargée.

use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::plugin::clap_plugin_descriptor;
use clap_sys::version::clap_version_is_compatible;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub description: String,
    pub features: Vec<String>,
}

#[derive(Debug)]
pub enum LoadError {
    /// La bibliothèque n'a pas pu être chargée par le système.
    Load(String),
    /// Pas de symbole `clap_entry` : ce n'est pas un plugin CLAP.
    NoEntry,
    /// Version de CLAP incompatible.
    IncompatibleVersion { major: u32, minor: u32, revision: u32 },
    /// `init` du plugin a refusé de s'initialiser.
    InitFailed,
    NoFactory,
    BadPath,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Load(e) => write!(f, "chargement impossible : {e}"),
            LoadError::NoEntry => write!(f, "symbole clap_entry absent (pas un plugin CLAP ?)"),
            LoadError::IncompatibleVersion { major, minor, revision } => write!(f, "version CLAP {major}.{minor}.{revision} incompatible"),
            LoadError::InitFailed => write!(f, "le plugin a refusé de s'initialiser"),
            LoadError::NoFactory => write!(f, "pas de fabrique de plugins"),
            LoadError::BadPath => write!(f, "chemin invalide"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Texte C optionnel → `String` (vide si absent). Tolère l'UTF-8 invalide.
///
/// # Safety
/// `p` est soit nul, soit un pointeur vers une chaîne C terminée par NUL valide pendant l'appel.
unsafe fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// Bibliothèque `.clap` chargée et initialisée (`clap_entry.init` appelé). `deinit` puis déchargement à la destruction.
pub struct Bundle {
    path: PathBuf,
    entry: *const clap_plugin_entry,
    // Doit rester en vie tant que `entry` est utilisé ; déclarée après pour être libérée en dernier.
    // `None` pour une entrée statique (tests).
    _lib: Option<libloading::Library>,
}

impl Bundle {
    pub fn open(path: &Path) -> Result<Bundle, LoadError> {
        let cpath = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| LoadError::BadPath)?;
        // SAFETY: charger une bibliothèque exécute son code d'initialisation ; c'est inhérent à un hôte de plugins.
        let lib = unsafe { libloading::Library::new(path) }.map_err(|e| LoadError::Load(e.to_string()))?;
        // SAFETY: `clap_entry` est, par la spécification CLAP, une variable statique de type `clap_plugin_entry` ;
        // le symbole est donc l'adresse de cette structure, valide tant que la bibliothèque est chargée.
        let entry: *const clap_plugin_entry = unsafe {
            let sym: libloading::Symbol<*const clap_plugin_entry> = lib.get(b"clap_entry\0").map_err(|_| LoadError::NoEntry)?;
            *sym
        };
        if entry.is_null() {
            return Err(LoadError::NoEntry);
        }
        // SAFETY: `entry` pointe vers une structure statique valide (voir ci-dessus).
        let e = unsafe { &*entry };
        if !clap_version_is_compatible(e.clap_version) {
            return Err(LoadError::IncompatibleVersion { major: e.clap_version.major, minor: e.clap_version.minor, revision: e.clap_version.revision });
        }
        // SAFETY: appel de la fonction `init` du plugin avec un chemin C valide, comme l'exige la spécification.
        let ok = unsafe { e.init.map_or(false, |init| init(cpath.as_ptr())) };
        if !ok {
            return Err(LoadError::InitFailed);
        }
        Ok(Bundle { path: path.to_owned(), entry, _lib: Some(lib) })
    }

    /// Bundle à partir d'une entrée déjà présente dans le programme (tests : plugin compilé avec les tests).
    pub fn from_static_entry(entry: &'static clap_plugin_entry) -> Result<Bundle, LoadError> {
        if !clap_version_is_compatible(entry.clap_version) {
            return Err(LoadError::IncompatibleVersion { major: entry.clap_version.major, minor: entry.clap_version.minor, revision: entry.clap_version.revision });
        }
        // SAFETY: appel de `init` d'une entrée statique valide ; le chemin est une chaîne C valide.
        let ok = unsafe { entry.init.map_or(false, |init| init(c"statique".as_ptr())) };
        if !ok {
            return Err(LoadError::InitFailed);
        }
        Ok(Bundle { path: PathBuf::from("<statique>"), entry, _lib: None })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn factory(&self) -> Option<*const clap_plugin_factory> {
        // SAFETY: `entry` valide (voir `open`) ; `get_factory` reçoit un identifiant C valide.
        let f = unsafe { (*self.entry).get_factory.map(|g| g(CLAP_PLUGIN_FACTORY_ID.as_ptr())) }?;
        (!f.is_null()).then_some(f as *const clap_plugin_factory)
    }

    /// Liste des plugins contenus dans la bibliothèque.
    pub fn plugins(&self) -> Result<Vec<PluginInfo>, LoadError> {
        let factory = self.factory().ok_or(LoadError::NoFactory)?;
        // SAFETY: `factory` est valide tant que la bibliothèque est chargée ; les descripteurs renvoyés
        // et leurs chaînes appartiennent au plugin et restent valides jusqu'à `deinit`.
        unsafe {
            let f = &*factory;
            let count = f.get_plugin_count.map_or(0, |c| c(factory));
            let get = f.get_plugin_descriptor.ok_or(LoadError::NoFactory)?;
            let mut out = Vec::with_capacity(count as usize);
            for i in 0..count {
                let d: *const clap_plugin_descriptor = get(factory, i);
                if d.is_null() {
                    continue;
                }
                let d = &*d;
                let mut features = Vec::new();
                let mut p = d.features;
                while !p.is_null() && !(*p).is_null() {
                    features.push(text(*p));
                    p = p.add(1);
                }
                out.push(PluginInfo {
                    id: text(d.id),
                    name: text(d.name),
                    vendor: text(d.vendor),
                    version: text(d.version),
                    description: text(d.description),
                    features,
                });
            }
            Ok(out)
        }
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        // SAFETY: `entry` est encore valide (la bibliothèque n'est déchargée qu'après, par la destruction de `_lib`).
        unsafe {
            if let Some(deinit) = (*self.entry).deinit {
                deinit();
            }
        }
    }
}

/// Dossiers où chercher des plugins CLAP selon les conventions du système.
pub fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(target_os = "windows") {
        if let Some(common) = std::env::var_os("CommonProgramFiles") {
            dirs.push(PathBuf::from(common).join("CLAP"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Programs").join("Common").join("CLAP"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/lib/clap"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".clap"));
        }
    }
    if let Some(extra) = std::env::var_os("CLAP_PATH") {
        dirs.extend(std::env::split_paths(&extra));
    }
    dirs
}

/// Fichiers `.clap` d'un dossier, parcours récursif (Reaper et d'autres hôtes font de même), triés par chemin.
pub fn find_plugins(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() && p.extension().is_none_or(|e| e != "clap") {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "clap") {
                found.push(p);
            }
        }
    }
    found.sort();
    found
}

mod insert;
mod plugin;
pub use insert::{clap_insert_factory, ClapInsert};
pub use plugin::{FrChordTrack, FrItem, GuiSupport, ParamInfo, ParamOut, Plugin, PluginError, PluginHandle, PortsInfo, ProcessStatus};
pub use plugin::FR_EXT_CHORD_TRACK;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_clap_file_is_rejected_cleanly() {
        let dir = std::env::temp_dir().join(format!("daw-clap-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("faux.clap");
        std::fs::write(&fake, b"ceci n'est pas une bibliotheque").unwrap();
        assert!(matches!(Bundle::open(&fake), Err(LoadError::Load(_))));
        assert!(matches!(Bundle::open(&dir.join("absent.clap")), Err(LoadError::Load(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_plugins_is_recursive_and_sorted() {
        let dir = std::env::temp_dir().join(format!("daw-clap-find-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sous/dossier")).unwrap();
        for f in ["b.clap", "a.clap", "sous/c.clap", "sous/dossier/d.clap", "sous/lisez.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let names: Vec<String> = find_plugins(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["a.clap", "b.clap", "c.clap", "d.clap"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
