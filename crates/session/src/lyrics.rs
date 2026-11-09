//! Paroles du morceau : des syllabes posées à un instant exact de la ligne de temps musicale (fractions de ronde), sur un ou
//! plusieurs couplets. Indépendantes des notes : une syllabe n'est pas liée à une note (la partition l'aligne sur la note
//! au même instant, sinon sur sa propre colonne). Comme les sections, elles vivent dans la session (annulables, enregistrées).

use daw_core_types::time::Frac;

/// Nombre maximal de syllabes.
pub const MAX_SYLLABLES: usize = 16_384;
/// Longueur maximale d'une syllabe (en caractères).
pub const MAX_TEXT: usize = 40;
/// Couplets 1 à `MAX_VERSE` (comme la partition de Scorsaire).
pub const MAX_VERSE: u8 = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Syllable {
    pub at: Frac,
    pub text: String,
    /// La syllabe continue le mot : elle s'écrit « ma- » et se lie à la suivante.
    pub hyphen: bool,
    /// Numéro de couplet, de 1 à [`MAX_VERSE`].
    pub verse: u8,
    /// Fin du trait de prolongation (mélisme : la syllabe se prolonge sur plusieurs notes) ; `None` : pas de trait. Strictement après `at`.
    pub end: Option<Frac>,
}

/// Trie par instant puis par couplet (tri stable).
pub fn normalize(list: &mut [Syllable]) {
    list.sort_by(|a, b| a.at.cmp(&b.at).then(a.verse.cmp(&b.verse)));
}

/// Vérifie une liste triée : au plus [`MAX_SYLLABLES`], texte non vide sans espace ni caractère de contrôle (un mot ou une syllabe),
/// couplet de 1 à [`MAX_VERSE`], pas deux syllabes au même instant dans le même couplet.
pub fn validate(list: &[Syllable]) -> Result<(), String> {
    if list.len() > MAX_SYLLABLES {
        return Err(format!("trop de syllabes ({MAX_SYLLABLES} au plus)"));
    }
    for (i, s) in list.iter().enumerate() {
        if s.text.is_empty() || s.text.chars().count() > MAX_TEXT || s.text.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(format!("syllabe invalide « {} » (1 à {MAX_TEXT} caractères, sans espace)", s.text));
        }
        if s.verse < 1 || s.verse > MAX_VERSE {
            return Err(format!("couplet {} invalide (1 à {MAX_VERSE})", s.verse));
        }
        if s.end.is_some_and(|e| e <= s.at) {
            return Err("la fin d'un trait de prolongation doit suivre le début de la syllabe".into());
        }
        if let Some(n) = list.get(i + 1) {
            if n.at == s.at && n.verse == s.verse {
                return Err("deux syllabes au même instant dans le même couplet".into());
            }
        }
    }
    Ok(())
}
