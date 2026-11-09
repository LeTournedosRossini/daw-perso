//! Banque personnelle de pages de contrôle : pour chaque plugin (identifiant CLAP), les 16 paramètres au plus qu'on veut avoir sous la main sur une
//! tablette ou une surface de contrôle (seuil, rapport, attaque… d'un compresseur ; bandes d'un égaliseur). Un plugin peut exposer des milliers de
//! paramètres (Neptune : 3 630) : on en désigne **une fois** quelques-uns, sur cet ordinateur, et ils servent à **tous les projets** — la banque
//! est un fichier de l'utilisateur (`…/DAW/control-pages.json`), pas du projet. Une surface affiche ces paramètres par groupes de 8 (huit
//! encodeurs). Un paramètre se désigne par son identifiant CLAP (stable d'une version à l'autre d'un même plugin) et son nom, qui sert à le retrouver
//! si l'identifiant a changé.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const BANK_VERSION: u32 = 1;
/// Nombre maximal de paramètres d'une page (deux groupes de huit encodeurs).
pub const MAX_PARAMS: usize = 16;
const MAX_NAME: usize = 120;

/// Un paramètre de la page : son identifiant CLAP et son nom (pour l'affichage et pour le retrouver).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageParam {
    pub id: u32,
    pub name: String,
}

/// La page d'un plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageEntry {
    /// Identifiant CLAP du plugin.
    pub plugin_id: String,
    /// Nom du plugin (pour l'affichage de la banque).
    pub name: String,
    pub params: Vec<PageParam>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bank {
    pub version: u32,
    pub plugins: BTreeMap<String, PageEntry>,
}

/// Valide une page avant de l'enregistrer.
pub fn validate(entry: &PageEntry) -> Result<(), String> {
    if entry.plugin_id.trim().is_empty() || entry.plugin_id.len() > 200 {
        return Err("identifiant de plugin vide ou trop long".into());
    }
    if entry.name.len() > MAX_NAME {
        return Err("nom de plugin trop long".into());
    }
    if entry.params.len() > MAX_PARAMS {
        return Err(format!("une page a {MAX_PARAMS} paramètres au plus"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for p in &entry.params {
        if p.name.len() > MAX_NAME {
            return Err("nom de paramètre trop long".into());
        }
        if !seen.insert(p.id) {
            return Err(format!("le paramètre {} est en double", p.id));
        }
    }
    Ok(())
}

impl Bank {
    pub fn new() -> Bank {
        Bank { version: BANK_VERSION, plugins: BTreeMap::new() }
    }

    /// Met la page d'un plugin (une page sans paramètre la supprime).
    pub fn set(&mut self, entry: PageEntry) -> Result<(), String> {
        validate(&entry)?;
        if entry.params.is_empty() {
            self.plugins.remove(&entry.plugin_id);
        } else {
            self.plugins.insert(entry.plugin_id.clone(), entry);
        }
        Ok(())
    }

    pub fn get(&self, plugin_id: &str) -> Option<&PageEntry> {
        self.plugins.get(plugin_id)
    }

    pub fn entries(&self) -> Vec<PageEntry> {
        self.plugins.values().cloned().collect()
    }
}

/// Lit la banque ; un fichier absent donne une banque vide, un fichier abîmé ou plus récent une erreur (la banque reste vide : rien n'est écrasé
/// tant que l'utilisateur ne change pas une page).
pub fn load(path: &Path) -> Result<Bank, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Bank::new()),
        Err(e) => return Err(format!("lecture de {} : {e}", path.display())),
    };
    let bank: Bank = serde_json::from_str(&text).map_err(|e| format!("banque de pages de contrôle illisible : {e}"))?;
    if bank.version > BANK_VERSION {
        return Err(format!("banque de pages de contrôle de version {} (ce programme lit jusqu'à la {BANK_VERSION})", bank.version));
    }
    for (id, e) in &bank.plugins {
        if id != &e.plugin_id {
            return Err("banque de pages de contrôle incohérente (identifiant de plugin différent de sa clé)".into());
        }
        validate(e)?;
    }
    Ok(bank)
}

pub fn save(path: &Path, bank: &Bank) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("dossier {} : {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(bank).map_err(|e| e.to_string())?;
    crate::project_io::write_atomic(path, text.as_bytes()).map_err(|e| format!("écriture de {} : {e}", path.display()))
}

/// Retrouve les paramètres d'une page parmi ceux que le plugin expose : par identifiant, sinon par nom (sans tenir compte de la casse).
/// Rend les indices dans `exposed` (`ids`/`names`, mêmes positions) et les noms qu'on n'a pas retrouvés.
pub fn resolve(page: &[PageParam], exposed: &[(u32, String)]) -> (Vec<usize>, Vec<String>) {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for p in page {
        let by_id = exposed.iter().position(|(id, name)| *id == p.id && (p.name.is_empty() || name.eq_ignore_ascii_case(&p.name)));
        let idx = by_id.or_else(|| exposed.iter().position(|(_, name)| !p.name.is_empty() && name.eq_ignore_ascii_case(&p.name))).or_else(|| exposed.iter().position(|(id, _)| *id == p.id));
        match idx {
            Some(i) if !found.contains(&i) => found.push(i),
            _ => missing.push(p.name.clone()),
        }
    }
    (found, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: &str, params: &[(u32, &str)]) -> PageEntry {
        PageEntry { plugin_id: id.into(), name: id.into(), params: params.iter().map(|(i, n)| PageParam { id: *i, name: (*n).into() }).collect() }
    }

    #[test]
    fn a_page_is_validated_and_an_empty_page_removes_the_plugin() {
        let mut b = Bank::new();
        b.set(page("com.x.comp", &[(1, "Seuil"), (2, "Rapport")])).unwrap();
        assert_eq!(b.get("com.x.comp").unwrap().params.len(), 2);
        assert!(b.set(page("", &[(1, "a")])).is_err());
        assert!(b.set(page("p", &[(1, "a"), (1, "b")])).is_err(), "doublon");
        let many: Vec<(u32, String)> = (0..17).map(|i| (i, format!("p{i}"))).collect();
        let many_ref: Vec<(u32, &str)> = many.iter().map(|(i, n)| (*i, n.as_str())).collect();
        assert!(b.set(page("p", &many_ref)).is_err(), "17 paramètres : trop");
        b.set(page("com.x.comp", &[])).unwrap();
        assert!(b.get("com.x.comp").is_none());
    }

    #[test]
    fn the_bank_is_saved_and_read_back_and_bad_files_are_refused() {
        let dir = std::env::temp_dir().join(format!("daw-bank-{}", std::process::id()));
        let f = dir.join("control-pages.json");
        assert_eq!(load(&f).unwrap(), Bank::new(), "pas de fichier : banque vide");
        let mut b = Bank::new();
        b.set(page("com.x.eq", &[(10, "Gain 1"), (11, "Fréq 1")])).unwrap();
        save(&f, &b).unwrap();
        assert_eq!(load(&f).unwrap(), b);
        std::fs::write(&f, "pas du json").unwrap();
        assert!(load(&f).unwrap_err().contains("illisible"));
        std::fs::write(&f, r#"{"version":99,"plugins":{}}"#).unwrap();
        assert!(load(&f).unwrap_err().contains("version 99"));
        std::fs::write(&f, r#"{"version":1,"plugins":{"a":{"plugin_id":"b","name":"","params":[]}}}"#).unwrap();
        assert!(load(&f).unwrap_err().contains("incohérente"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parameters_are_found_by_id_then_by_name() {
        let exposed = vec![(1u32, "Seuil".to_string()), (2, "Rapport".to_string()), (7, "Attaque".to_string())];
        let want = vec![PageParam { id: 2, name: "Rapport".into() }, PageParam { id: 99, name: "attaque".into() }, PageParam { id: 5, name: "Inexistant".into() }, PageParam { id: 1, name: "Autre nom".into() }];
        let (found, missing) = resolve(&want, &exposed);
        // 2 par identifiant ; « attaque » par son nom (identifiant changé) ; « Inexistant » absent ; 1 retrouvé par son identifiant malgré un nom différent.
        assert_eq!(found, vec![1, 2, 0]);
        assert_eq!(missing, vec!["Inexistant".to_string()]);
    }
}
