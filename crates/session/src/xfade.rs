//! Longueur d'un fondu enchaîné : la zone où `a` (qui sort) et `b` (qui entre) se chevauchent. La changer = rogner les
//! bords des deux clips, **sans déplacer leur contenu** (non destructif, annulable). Les courbes des fondus sont conservées.

use daw_engine::{Clip, EditError, Fade, FadeShape};

/// Ce qui reste en place quand la longueur change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Le milieu du fondu : `a` et `b` s'allongent ou se raccourcissent de chaque côté.
    Center,
    /// Le début du fondu (début de `b`) : seule la fin de `a` bouge.
    Start,
    /// La fin du fondu (fin de `a`) : seul le début de `b` bouge.
    End,
}

/// Les deux clips doivent former une paire « a puis b » qui se chevauchent, comme celles que reconnaît le fondu automatique.
fn check(a: &Clip, b: &Clip) -> Result<(), EditError> {
    if a.start < b.start && b.start < a.end() && a.end() <= b.end() {
        Ok(())
    } else {
        Err(EditError::NoOverlap)
    }
}

/// Début de `b` et fin de `a` pour une longueur donnée (en entiers signés : le résultat peut sortir de la ligne de temps).
fn edges(a: &Clip, b: &Clip, len: u64, anchor: Anchor) -> (i128, i128) {
    let (s, e, l) = (b.start as i128, a.end() as i128, len as i128);
    match anchor {
        Anchor::Start => (s, s + l),
        Anchor::End => (e - l, e),
        Anchor::Center => {
            let bs = (s + e - l).div_euclid(2);
            (bs, bs + l)
        }
    }
}

/// Longueurs possibles `(min, max)` selon l'ancrage : limitées par les données audio disponibles de chaque côté, par le
/// début de `a` et par la fin de `b` (l'ordre « a commence avant b, finit avant la fin de b » est conservé).
pub fn length_bounds(a: &Clip, b: &Clip, anchor: Anchor) -> Result<(u64, u64), EditError> {
    check(a, b)?;
    let (s, e) = (b.start as i128, a.end() as i128);
    let tail_a = (a.data.len() - a.offset - a.len) as i128; // données après la fin de a
    let head_b = b.offset as i128; // données avant le début de b
    let lo = (a.start as i128 + 1).max(s - head_b); // plus petit début de b possible
    let hi = (b.end() as i128).min(e + tail_a); // plus grande fin de a possible
    let max = match anchor {
        Anchor::Start => hi - s,
        Anchor::End => e - lo,
        Anchor::Center => (s + e - 2 * lo).min(2 * hi + 1 - s - e),
    };
    Ok((1, max.max(1) as u64))
}

/// Les deux clips avec un fondu enchaîné de `len` échantillons. Refuse une longueur hors des bornes (`OutOfRange`).
pub fn resize(a: &Clip, b: &Clip, len: u64, anchor: Anchor) -> Result<(Clip, Clip), EditError> {
    let (_, max) = length_bounds(a, b, anchor)?;
    if len < 1 || len > max {
        return Err(EditError::OutOfRange);
    }
    let (bs, ae) = edges(a, b, len, anchor);
    let (bs, ae) = (u64::try_from(bs).map_err(|_| EditError::OutOfRange)?, u64::try_from(ae).map_err(|_| EditError::OutOfRange)?);
    let mut na = if ae == a.end() { a.clone() } else { a.trim_end(ae)? };
    let mut nb = if bs == b.start { b.clone() } else { b.trim_start(bs)? };
    // Courbes conservées ; un clip sans fondu préalable reçoit la puissance constante, comme le fondu automatique.
    na.fade_out = if a.fade_out.len > 0 { a.fade_out.with_len(len) } else { Fade::new(len, FadeShape::EqualPower) };
    nb.fade_in = if b.fade_in.len > 0 { b.fade_in.with_len(len) } else { Fade::new(len, FadeShape::EqualPower) };
    Ok((na, nb))
}

/// Raccord entre deux clips qui se touchent exactement (`a` finit où `b` commence) : un petit fondu enchaîné **centré sur la
/// jonction autant que la matière le permet**, fait avec l'audio que chacun garde de part et d'autre (non destructif : `a` s'allonge,
/// `b` commence plus tôt, contenu en place). Un clip sans matière de son côté (le début d'une prise neuve n'a rien avant lui) laisse
/// l'autre fournir tout le recouvrement : le fondu part alors de la jonction. La longueur visée est réduite à ce que la matière
/// permet (et à la moitié de chaque clip) ; `None` si elle tombe sous 2 échantillons ou si `b` n'est que la suite de `a` (mêmes
/// données, contiguës : rien à raccorder). Courbe **linéaire** : des prises d'un même passage sont corrélées (somme plate) et la
/// courbe en puissance constante, dont la pente est infinie au départ, laisse un petit saut audible.
pub fn seam(a: &Clip, b: &Clip, target: u64) -> Option<(Clip, Clip)> {
    if a.end() != b.start || (std::sync::Arc::ptr_eq(&a.data, &b.data) && a.offset + a.len == b.offset) {
        return None;
    }
    // Chaque fondu doit tenir dans son clip une fois celui-ci prolongé : `fade_out` de `a` (longueur ha + hb) ≤ a.len + ha, donc
    // hb ≤ a.len ; `fade_in` de `b` ≤ b.len + hb, donc ha ≤ b.len. (Un morceau très court d'un côté, comme les quelques échantillons
    // que laisse la compensation de latence avant une zone, ne doit pas recevoir un fondu plus long que lui.)
    let tail_a = a.data.len().saturating_sub(a.offset + a.len).min(a.len / 2).min(b.len);
    let head_b = b.offset.min(b.len / 2).min(a.len);
    // Moitié de chaque côté si possible ; ce qui manque d'un côté est pris de l'autre.
    let hb = (target / 2).min(head_b);
    let ha = (target - hb).min(tail_a);
    let hb = (target - ha).min(head_b);
    let len = ha + hb;
    if len < 2 {
        return None;
    }
    let mut na = if ha > 0 { a.trim_end(a.end() + ha).ok()? } else { a.clone() };
    let mut nb = if hb > 0 { b.trim_start(b.start - hb).ok()? } else { b.clone() };
    na.fade_out = Fade::new(len, FadeShape::Linear);
    nb.fade_in = Fade::new(len, FadeShape::Linear);
    Some((na, nb))
}
