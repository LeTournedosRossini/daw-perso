//! Liste des projets récents : les dossiers `.daw` ouverts ou enregistrés le plus récemment, du plus récent au plus ancien. Fichier de
//! l'utilisateur (`…/DAW/recent.json`), commun à tous les projets. Un fichier absent, abîmé ou plus récent que le programme donne une liste vide
//! (jamais d'erreur : ce n'est qu'un confort) et n'est réécrit que quand la liste change.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
/// Nombre de projets gardés.
pub const MAX: usize = 10;

#[derive(Serialize, Deserialize)]
struct RecentFile {
    version: u32,
    projects: Vec<String>,
}

/// Lit la liste ; vide si le fichier manque ou n'est pas lisible.
pub fn load(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(file) = serde_json::from_str::<RecentFile>(&text) else { return Vec::new() };
    if file.version > VERSION {
        return Vec::new();
    }
    let mut list = Vec::new();
    for p in file.projects {
        push(&mut list, p);
    }
    list.reverse(); // `push` met en tête : l'ordre du fichier est celui d'affichage
    list.truncate(MAX);
    list
}

/// Écrit la liste (fichier voisin puis renommage : jamais de fichier à moitié écrit).
pub fn save(path: &Path, list: &[String]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("projets récents : {e}"))?;
    }
    let text = serde_json::to_string_pretty(&RecentFile { version: VERSION, projects: list.to_vec() }).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("projets récents : {e}"))
}

/// Met `project` en tête (sans doublon, `MAX` au plus).
pub fn push(list: &mut Vec<String>, project: String) {
    let same = |a: &str, b: &str| a == b || (cfg!(windows) && a.eq_ignore_ascii_case(b));
    list.retain(|p| !same(p, &project));
    list.insert(0, project);
    list.truncate(MAX);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_duplicates_and_capped() {
        let mut l = Vec::new();
        for n in 0..15 {
            push(&mut l, format!("/p/{n}.daw"));
        }
        assert_eq!(l.len(), MAX);
        assert_eq!(l[0], "/p/14.daw");
        push(&mut l, "/p/10.daw".into());
        assert_eq!(l[0], "/p/10.daw");
        assert_eq!(l.iter().filter(|p| *p == "/p/10.daw").count(), 1);
        assert_eq!(l.len(), MAX);
    }

    #[test]
    fn file_round_trip_and_bad_files_give_an_empty_list() {
        let dir = std::env::temp_dir().join(format!("daw-recent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("recent.json");
        assert!(load(&f).is_empty());
        let list = vec!["/a.daw".to_string(), "/b.daw".to_string()];
        save(&f, &list).unwrap();
        assert_eq!(load(&f), list);
        std::fs::write(&f, "pas du json").unwrap();
        assert!(load(&f).is_empty());
        std::fs::write(&f, r#"{"version":99,"projects":["/x.daw"]}"#).unwrap();
        assert!(load(&f).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
