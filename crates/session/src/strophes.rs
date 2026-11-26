//! Texte des paroles en strophes (Couplet 1, Refrain, Pont…) : du texte, sans mélodie ni instant. Une strophe peut être **liée à une
//! carte de la structure** (par le nom de la section) ; plusieurs sections de même nom (deux refrains) partagent alors le même texte.
//! Indépendant des syllabes posées sur la ligne de temps (voir `lyrics`) : on écrit d'abord le texte, on le pose ensuite sur des notes.

use crate::structure::CATEGORIES;

pub const MAX_STROPHES: usize = 256;
pub const MAX_LINES: usize = 512;
pub const MAX_LINE: usize = 400;
pub const MAX_NAME: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Strophe {
    /// Nom affiché (« Couplet 1 », « Refrain »…), libre.
    pub name: String,
    /// Catégorie de la structure (voir `structure::CATEGORIES`) ; `None` : `custom`.
    pub category: Option<String>,
    /// Une phrase par ligne (jamais de retour à la ligne dans une ligne).
    pub lines: Vec<String>,
    /// Nom de la section de la structure à laquelle la strophe est liée ; `None` : par le nom de la strophe.
    pub link: Option<String>,
}

/// Vérifie la liste : au plus [`MAX_STROPHES`], noms de 1 à [`MAX_NAME`] caractères, catégories connues, lignes courtes sans caractère de contrôle.
pub fn validate(list: &[Strophe]) -> Result<(), String> {
    if list.len() > MAX_STROPHES {
        return Err(format!("trop de strophes ({MAX_STROPHES} au plus)"));
    }
    for s in list {
        let n = s.name.trim();
        if n.is_empty() || n.chars().count() > MAX_NAME || s.name.chars().any(|c| c.is_control()) {
            return Err(format!("nom de strophe invalide (1 à {MAX_NAME} caractères)"));
        }
        if let Some(c) = &s.category {
            if !CATEGORIES.contains(&c.as_str()) {
                return Err(format!("catégorie de strophe inconnue : {c}"));
            }
        }
        if let Some(l) = &s.link {
            if l.trim().is_empty() || l.chars().count() > MAX_NAME || l.chars().any(|c| c.is_control()) {
                return Err("lien de strophe invalide".into());
            }
        }
        if s.lines.len() > MAX_LINES {
            return Err(format!("trop de lignes dans « {} » ({MAX_LINES} au plus)", s.name));
        }
        if s.lines.iter().any(|l| l.chars().count() > MAX_LINE || l.chars().any(|c| c.is_control())) {
            return Err(format!("ligne invalide dans « {} » ({MAX_LINE} caractères au plus, sans retour à la ligne)", s.name));
        }
    }
    Ok(())
}
