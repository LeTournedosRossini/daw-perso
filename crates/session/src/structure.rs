//! Carte de structure du morceau : des sections nommées (intro, couplet, refrain...) posées sur la ligne de temps musicale.
//! Indépendante de la piste d'accords. Positions et durées en fractions exactes de ronde, comme les accords.

use daw_core_types::time::Frac;

/// Nombre maximal de sections.
pub const MAX_SECTIONS: usize = 512;
/// Longueur maximale du nom (en caractères).
pub const MAX_NAME: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub at: Frac,
    pub dur: Frac,
    pub name: String,
    /// Couleur `#rrggbb` ; `None` : la page choisit d'après le nom.
    pub color: Option<String>,
    /// Catégorie (voir [`CATEGORIES`]) ; `None` : `custom`. Donne la couleur par défaut, comme dans Scorsaire. Le nom reste libre.
    pub category: Option<String>,
}

/// Les catégories de section, les mêmes identifiants que Scorsaire (`SECTION_CATEGORIES`).
pub const CATEGORIES: [&str; 11] = ["intro", "verse", "preChorus", "chorus", "bridge", "solo", "interlude", "outro", "coda", "letter", "custom"];

/// Trie par début (tri stable).
pub fn normalize(list: &mut [Section]) {
    list.sort_by(|a, b| a.at.cmp(&b.at));
}

/// Vérifie une liste déjà triée : au plus [`MAX_SECTIONS`], noms non vides et courts, couleurs `#rrggbb`, durées non nulles, pas de chevauchement.
pub fn validate(list: &[Section]) -> Result<(), String> {
    if list.len() > MAX_SECTIONS {
        return Err(format!("trop de sections ({} au plus)", MAX_SECTIONS));
    }
    let mut end: Option<Frac> = None;
    for s in list {
        let n = s.name.trim();
        if n.is_empty() || s.name.chars().count() > MAX_NAME {
            return Err(format!("nom de section invalide (1 à {MAX_NAME} caractères)"));
        }
        if let Some(c) = &s.color {
            let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit());
            if !ok {
                return Err(format!("couleur « {c} » invalide (attendu #rrggbb)"));
            }
        }
        if let Some(c) = &s.category {
            if !CATEGORIES.contains(&c.as_str()) {
                return Err(format!("catégorie « {c} » inconnue"));
            }
        }
        if s.dur.num() == 0 {
            return Err("une section a une durée nulle".into());
        }
        if let Some(e) = end {
            if s.at < e {
                return Err("deux sections se chevauchent".into());
            }
        }
        end = Some(s.at.checked_add(s.dur).ok_or("position trop grande")?);
    }
    Ok(())
}
