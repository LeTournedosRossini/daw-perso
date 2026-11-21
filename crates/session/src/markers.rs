//! Marqueurs de repère : des points nommés (et colorés) posés à un instant exact de la ligne de temps musicale (fraction de ronde, donc
//! ils suivent le tempo). Posés à la main depuis la timeline, ou **en direct depuis une tablette** pendant une prise (« bonne prise »,
//! « à refaire »). Comme les sections, ils vivent dans la session : annulables, enregistrés dans le projet.

use daw_core_types::time::Frac;

/// Nombre maximal de marqueurs.
pub const MAX_MARKERS: usize = 4_096;
/// Longueur maximale d'un nom (en caractères).
pub const MAX_NAME: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    /// Identifiant stable (jamais réutilisé dans une session) : un marqueur déplacé ou renommé garde le sien.
    pub id: u32,
    pub at: Frac,
    pub name: String,
    /// Couleur `#rrggbb` ; `None` : la couleur par défaut de la page.
    pub color: Option<String>,
}

/// Trie par instant puis par identifiant.
pub fn normalize(list: &mut [Marker]) {
    list.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
}

/// `#rrggbb`.
fn is_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit())
}

/// Vérifie une liste : au plus [`MAX_MARKERS`], nom de 1 à [`MAX_NAME`] caractères sans caractère de contrôle, couleur `#rrggbb`,
/// identifiants distincts.
pub fn validate(list: &[Marker]) -> Result<(), String> {
    if list.len() > MAX_MARKERS {
        return Err(format!("trop de marqueurs ({MAX_MARKERS} au plus)"));
    }
    let mut ids: Vec<u32> = list.iter().map(|m| m.id).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|w| w[0] == w[1]) {
        return Err("deux marqueurs ont le même identifiant".into());
    }
    for m in list {
        let n = m.name.chars().count();
        if n == 0 || n > MAX_NAME || m.name.chars().any(char::is_control) || m.name.trim().is_empty() {
            return Err(format!("nom de marqueur invalide (1 à {MAX_NAME} caractères) : « {} »", m.name));
        }
        if m.color.as_deref().is_some_and(|c| !is_color(c)) {
            return Err("couleur de marqueur : #rrggbb".into());
        }
    }
    Ok(())
}
