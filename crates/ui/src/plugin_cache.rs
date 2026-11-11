//! Cache disque du balayage des plugins installés.
//!
//! Ouvrir chaque fichier `.clap` pour lire ses descriptifs coûte de l'ordre d'une seconde sous Windows (une cinquantaine de fichiers). Le
//! cache retient, par fichier, ce qu'il contenait la dernière fois, avec sa date et sa taille : un fichier inchangé n'est **pas rouvert**.
//! Un fichier modifié (ou ajouté) est relu ; un fichier disparu quitte la liste ; un fichier qui ne se charge pas est retenu comme tel
//! (il n'est pas réessayé tant qu'il ne change pas). Cache absent, abîmé ou d'une autre version : on balaye tout, comme avant.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::protocol::PluginListing;

const VERSION: u32 = 1;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    files: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    path: String,
    /// Date de modification (nanosecondes depuis 1970) et taille : le fichier est « le même » si les deux sont égales.
    mtime: u64,
    size: u64,
    /// Faux : le fichier n'a pas pu être chargé (rien à lister).
    ok: bool,
    plugins: Vec<PluginListing>,
}

/// Ce que le balayage a fait (pour le journal et les tests).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanStats {
    /// Fichiers dont la liste vient du cache.
    pub cached: usize,
    /// Fichiers ouverts pour de bon.
    pub opened: usize,
}

/// Date et taille d'un fichier. Un paquet (dossier, macOS) : la date la plus récente et la taille cumulée de son contenu.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    fn walk(p: &Path, depth: u32) -> Option<(u64, u64)> {
        let meta = std::fs::metadata(p).ok()?;
        let own = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos() as u64;
        if !meta.is_dir() {
            return Some((own, meta.len()));
        }
        let (mut m, mut s) = (own, 0u64);
        if depth < 4 {
            for e in std::fs::read_dir(p).ok()?.flatten() {
                if let Some((em, es)) = walk(&e.path(), depth + 1) {
                    m = m.max(em);
                    s += es;
                }
            }
        }
        Some((m, s))
    }
    walk(path, 0)
}

fn load(path: &Path) -> Cache {
    let Ok(text) = std::fs::read_to_string(path) else { return Cache::default() };
    match serde_json::from_str::<Cache>(&text) {
        Ok(c) if c.version == VERSION => c,
        _ => Cache::default(),
    }
}

/// Écriture atomique (fichier voisin puis renommage) : un arrêt en cours d'écriture ne laisse jamais un cache à moitié écrit.
fn save(path: &Path, cache: &Cache) {
    let Ok(text) = serde_json::to_string(cache) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Balaye `files` : pour chacun, la liste du cache si le fichier n'a pas changé, sinon `open` (qui rend `None` si le fichier ne se
/// charge pas). Le cache est réécrit seulement s'il a changé. Rend la liste triée comme l'inventaire (éditeur puis nom).
pub fn scan_files(files: &[PathBuf], cache_path: Option<&Path>, mut open: impl FnMut(&Path) -> Option<Vec<PluginListing>>) -> (Vec<PluginListing>, ScanStats) {
    let old = cache_path.map(load).unwrap_or_default();
    let mut stats = ScanStats::default();
    let mut entries: Vec<Entry> = Vec::with_capacity(files.len());
    for file in files {
        let key = file.to_string_lossy().into_owned();
        let Some((mtime, size)) = stamp(file) else { continue };
        if let Some(e) = old.files.iter().find(|e| e.path == key && e.mtime == mtime && e.size == size) {
            stats.cached += 1;
            entries.push(e.clone());
            continue;
        }
        stats.opened += 1;
        let listing = open(file);
        entries.push(Entry { path: key, mtime, size, ok: listing.is_some(), plugins: listing.unwrap_or_default() });
    }
    if let Some(path) = cache_path {
        let unchanged = old.files.len() == entries.len() && stats.opened == 0;
        if !unchanged {
            save(path, &Cache { version: VERSION, files: entries.clone() });
        }
    }
    let mut out: Vec<PluginListing> = entries.into_iter().flat_map(|e| e.plugins).collect();
    out.sort_by_key(|p| (p.vendor.to_lowercase(), p.name.to_lowercase()));
    (out, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(id: &str, file: &str) -> PluginListing {
        PluginListing { clap_id: id.into(), name: id.into(), vendor: "V".into(), file: file.into(), description: String::new(), features: vec![] }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("daw-plugin-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn unchanged_files_are_not_reopened_and_changes_are_picked_up() {
        let d = dir("a");
        let (a, b) = (d.join("a.clap"), d.join("b.clap"));
        std::fs::write(&a, "aaaa").unwrap();
        std::fs::write(&b, "bb").unwrap();
        let cache = d.join("cache.json");
        let mut opened = Vec::new();
        let open = |opened: &mut Vec<String>, p: &Path| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            opened.push(name.clone());
            Some(vec![listing(&format!("id-{name}"), &name)])
        };
        let files = vec![a.clone(), b.clone()];
        let (list, s) = scan_files(&files, Some(&cache), |p| open(&mut opened, p));
        assert_eq!((s.cached, s.opened, list.len()), (0, 2, 2));
        // Deuxième balayage : tout vient du cache, rien n'est rouvert.
        opened.clear();
        let (list2, s) = scan_files(&files, Some(&cache), |p| open(&mut opened, p));
        assert_eq!((s.cached, s.opened), (2, 0));
        assert!(opened.is_empty());
        assert_eq!(list, list2, "même liste, même ordre");
        // Un fichier modifié (taille différente) est relu, l'autre reste en cache.
        std::fs::write(&a, "aaaaaaaa").unwrap();
        let (_, s) = scan_files(&files, Some(&cache), |p| open(&mut opened, p));
        assert_eq!((s.cached, s.opened), (1, 1));
        assert_eq!(opened, vec!["a.clap".to_string()]);
        // Un fichier disparu quitte la liste ; un fichier ajouté entre.
        std::fs::remove_file(&b).unwrap();
        let c = d.join("c.clap");
        std::fs::write(&c, "c").unwrap();
        opened.clear();
        let (list3, s) = scan_files(&[a.clone(), c.clone()], Some(&cache), |p| open(&mut opened, p));
        assert_eq!((s.cached, s.opened, list3.len()), (1, 1, 2));
        assert!(list3.iter().all(|p| p.file != "b.clap"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_that_does_not_load_is_remembered_and_not_retried_until_it_changes() {
        let d = dir("b");
        let bad = d.join("bad.clap");
        std::fs::write(&bad, "x").unwrap();
        let cache = d.join("cache.json");
        let mut tries = 0;
        let (list, _) = scan_files(&[bad.clone()], Some(&cache), |_| {
            tries += 1;
            None
        });
        assert!(list.is_empty() && tries == 1);
        let (_, s) = scan_files(&[bad.clone()], Some(&cache), |_| {
            tries += 1;
            None
        });
        assert_eq!((s.cached, tries), (1, 1), "pas réessayé");
        std::fs::write(&bad, "xyz").unwrap();
        scan_files(&[bad], Some(&cache), |_| {
            tries += 1;
            None
        });
        assert_eq!(tries, 2, "réessayé après modification");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_or_foreign_cache_means_a_full_scan_and_no_cache_path_works_too() {
        let d = dir("c");
        let a = d.join("a.clap");
        std::fs::write(&a, "a").unwrap();
        let cache = d.join("cache.json");
        std::fs::write(&cache, "{ pas du json").unwrap();
        let (_, s) = scan_files(&[a.clone()], Some(&cache), |p| Some(vec![listing("x", &p.to_string_lossy())]));
        assert_eq!((s.cached, s.opened), (0, 1));
        // Autre version : ignoré.
        std::fs::write(&cache, r#"{"version":99,"files":[]}"#).unwrap();
        let (_, s) = scan_files(&[a.clone()], Some(&cache), |_| Some(vec![]));
        assert_eq!(s.opened, 1);
        // Sans chemin de cache : balayage complet à chaque fois, rien d'écrit.
        let (_, s) = scan_files(&[a], None, |_| Some(vec![]));
        assert_eq!((s.cached, s.opened), (0, 1));
        let _ = std::fs::remove_dir_all(&d);
    }
}
