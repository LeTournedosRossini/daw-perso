//! Piste d'accords vue par le moteur et par les plugins : un **instantané immuable**, trié, interrogé sans verrou ni allocation depuis
//! le fil audio. Positions en **noires** (comme `song_pos_beats` du transport CLAP) ; débuts inclus, fins exclues.
//!
//! La session le fabrique depuis sa piste d'accords (fractions de ronde exactes × 4) ; il est joint au graphe (`GraphDesc::chords`) et
//! libéré avec lui, côté contrôle.

use crate::time::MeterSeg;

/// Nature d'un élément (numéros partagés avec l'extension d'hôte C).
pub const KIND_CHORD: u32 = 0;
pub const KIND_KEY: u32 = 1;
pub const KIND_SCALE: u32 = 2;
pub const KIND_PATTERN: u32 = 3;
pub const KIND_MEASURE: u32 = 4;

/// `flags` : « pas d'accord » (un trou).
pub const FLAG_NC: u8 = 1;
/// `flags` : écriture en dièses (sinon bémols).
pub const FLAG_SHARP: u8 = 2;
/// `flags` : tonalité mineure.
pub const FLAG_MINOR: u8 = 4;

/// Pas de basse.
pub const NO_BASS: u8 = 255;
/// Longueur « infinie » (tonalité jusqu'au marqueur suivant, dernier de la voie).
pub const FOREVER: f64 = 1.0e12;

/// Un élément d'une voie. Selon `kind` :
/// - accord : `root` 0-11, `bass` 0-11 ou 255, `mask` = intervalles de la qualité, sur deux octaves (bit i = i demi-tons, 0 à 23) ;
/// - tonalité : `root`, `fifths`, mineur dans `flags`, `mask` = gamme majeure ou mineure naturelle ;
/// - gamme : `root`, `mask` ;
/// - appel de motif : `root` = emplacement 1 à 16 ;
/// - mesure : `root` = numérateur, `bass` = dénominateur (puis `index` = rang de la mesure depuis 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    pub start: f64,
    pub len: f64,
    pub root: u8,
    pub bass: u8,
    pub flags: u8,
    pub fifths: i8,
    /// Intervalles : bit i = i demi-tons depuis la fondamentale. Accord : **deux octaves** (0 à 23, une neuvième est 14) ; gamme et
    /// tonalité : 0 à 11 ; motif et mesure : 0.
    pub mask: u32,
    pub index: u32,
    pub name: [u8; 24],
}

impl Item {
    pub fn new(start: f64, len: f64) -> Item {
        Item { start, len, root: 0, bass: NO_BASS, flags: 0, fifths: 0, mask: 0, index: 0, name: [0; 24] }
    }

    /// Copie `text` (UTF-8, tronqué sans couper un caractère) dans le nom, terminé par un octet nul.
    pub fn with_name(mut self, text: &str) -> Item {
        let mut n = text.len().min(23);
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        self.name = [0; 24];
        self.name[..n].copy_from_slice(&text.as_bytes()[..n]);
        self
    }

    pub fn end(&self) -> f64 {
        self.start + self.len
    }

    fn covers(&self, beat: f64) -> bool {
        self.start <= beat && beat < self.start + self.len
    }
}

/// Les cinq voies, triées par début, plus ce qu'il faut pour la règle des mesures.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChordSnapshot {
    /// Change à chaque modification de la piste.
    pub revision: u32,
    pub chords: Vec<Item>,
    pub keys: Vec<Item>,
    pub scales: Vec<Item>,
    pub patterns: Vec<Item>,
    /// Règle du morceau (anacrouse...). Vide : les mesures se déduisent des chiffrages du projet (`meters`).
    pub measures: Vec<Item>,
    /// Tonalité avant le premier marqueur (ou pour tout le morceau).
    pub global_key: Option<Item>,
    pub meters: Vec<MeterSeg>,
    /// Plus longue gamme (pour borner la recherche en arrière des gammes qui se chevauchent).
    pub max_scale_len: f64,
}

/// Nombre maximal de gammes simultanées rendues par `lookup_all`.
pub const MAX_OVERLAP: usize = 8;

fn covering_sorted(v: &[Item], beat: f64) -> Option<Item> {
    let i = v.partition_point(|x| x.start <= beat);
    i.checked_sub(1).map(|k| v[k]).filter(|x| x.covers(beat))
}

impl ChordSnapshot {
    pub fn count(&self, kind: u32) -> u32 {
        let n = match kind {
            KIND_CHORD => self.chords.len(),
            KIND_KEY => self.keys.len() + usize::from(self.global_key.is_some()),
            KIND_SCALE => self.scales.len(),
            KIND_PATTERN => self.patterns.len(),
            KIND_MEASURE => self.measures.len(),
            _ => 0,
        };
        n as u32
    }

    /// L'élément de cette nature qui couvre `beat` ; pour les gammes qui se chevauchent, celui dont le début est le plus récent.
    pub fn lookup(&self, beat: f64, kind: u32) -> Option<Item> {
        let mut out = [Item::new(0.0, 0.0); 1];
        (self.lookup_all(beat, kind, &mut out) > 0).then_some(out[0])
    }

    /// Tous les éléments qui couvrent `beat`, le début le plus récent d'abord (au plus `out.len()` et `MAX_OVERLAP`).
    pub fn lookup_all(&self, beat: f64, kind: u32, out: &mut [Item]) -> usize {
        if out.is_empty() || !beat.is_finite() {
            return 0;
        }
        match kind {
            KIND_CHORD => covering_sorted(&self.chords, beat).map_or(0, |x| put(out, &[x])),
            KIND_PATTERN => covering_sorted(&self.patterns, beat).map_or(0, |x| put(out, &[x])),
            KIND_KEY => {
                let i = self.keys.partition_point(|k| k.start <= beat);
                let item = match i.checked_sub(1) {
                    Some(k) => {
                        let mut x = self.keys[k];
                        x.len = self.keys.get(k + 1).map_or(FOREVER, |n| n.start - x.start);
                        Some(x)
                    }
                    None => self.global_key.map(|mut g| {
                        g.start = -FOREVER;
                        g.len = self.keys.first().map_or(2.0 * FOREVER, |n| n.start + FOREVER);
                        g
                    }),
                };
                item.map_or(0, |x| put(out, &[x]))
            }
            KIND_SCALE => {
                let end = self.scales.partition_point(|s| s.start <= beat);
                let mut n = 0;
                let mut k = end;
                while k > 0 && n < out.len().min(MAX_OVERLAP) {
                    k -= 1;
                    let s = self.scales[k];
                    if s.start + self.max_scale_len <= beat {
                        break;
                    }
                    if s.covers(beat) {
                        out[n] = s;
                        n += 1;
                    }
                }
                n
            }
            KIND_MEASURE => self.measure_at(beat).map_or(0, |x| put(out, &[x])),
            _ => 0,
        }
    }

    /// Premier élément de cette nature qui **commence strictement après** `beat`.
    pub fn next_after(&self, beat: f64, kind: u32) -> Option<Item> {
        let v = match kind {
            KIND_CHORD => &self.chords,
            KIND_KEY => &self.keys,
            KIND_SCALE => &self.scales,
            KIND_PATTERN => &self.patterns,
            KIND_MEASURE if !self.measures.is_empty() => &self.measures,
            KIND_MEASURE => return self.measure_at(beat).and_then(|m| self.measure_at(m.end())),
            _ => return None,
        };
        v.get(v.partition_point(|x| x.start <= beat)).copied()
    }

    /// Rang (depuis 0) de l'élément de cette nature qui couvre `beat` (mesures et motifs : le rang dans la voie).
    pub fn index_at(&self, beat: f64, kind: u32) -> Option<u32> {
        match kind {
            KIND_MEASURE => self.measure_at(beat).map(|m| m.index),
            KIND_CHORD | KIND_SCALE | KIND_PATTERN | KIND_KEY => {
                let v = match kind {
                    KIND_CHORD => &self.chords,
                    KIND_SCALE => &self.scales,
                    KIND_PATTERN => &self.patterns,
                    _ => &self.keys,
                };
                let i = v.partition_point(|x| x.start <= beat).checked_sub(1)?;
                (kind == KIND_KEY || v[i].covers(beat)).then_some(i as u32)
            }
            _ => None,
        }
    }

    /// La mesure qui couvre `beat` : celle de la règle du morceau si elle existe, sinon déduite des chiffrages du projet.
    fn measure_at(&self, beat: f64) -> Option<Item> {
        if !self.measures.is_empty() {
            let i = self.measures.partition_point(|m| m.start <= beat).checked_sub(1)?;
            let mut m = self.measures[i];
            m.index = i as u32;
            return m.covers(beat).then_some(m);
        }
        let beat = beat.max(0.0);
        let mi = self.meters.partition_point(|m| m.start_beat <= beat).checked_sub(1)?;
        let m = self.meters[mi];
        let bar_len = f64::from(m.num) * 4.0 / f64::from(m.den.max(1));
        let bars_in = ((beat - m.start_beat) / bar_len).floor().max(0.0);
        let mut it = Item::new(m.start_beat + bars_in * bar_len, bar_len);
        it.root = m.num.min(255) as u8;
        it.bass = m.den.min(255) as u8;
        it.index = (m.start_bar - 1) + bars_in as u32;
        Some(it)
    }
}

fn put(out: &mut [Item], items: &[Item]) -> usize {
    let n = items.len().min(out.len());
    out[..n].copy_from_slice(&items[..n]);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(start: f64, len: f64, root: u8) -> Item {
        let mut i = Item::new(start, len);
        i.root = root;
        i
    }

    fn snap() -> ChordSnapshot {
        let mut s = ChordSnapshot::default();
        s.chords = vec![chord(0.0, 4.0, 2), chord(4.0, 4.0, 7), chord(12.0, 2.0, 0)];
        s.scales = vec![chord(0.0, 8.0, 2), chord(2.0, 2.0, 7), chord(3.0, 6.0, 9)];
        s.max_scale_len = 8.0;
        s.meters = vec![MeterSeg { start_beat: 0.0, start_bar: 1, num: 4, den: 4 }, MeterSeg { start_beat: 8.0, start_bar: 3, num: 3, den: 4 }];
        let mut k = chord(0.0, 0.0, 0);
        k.fifths = 0;
        s.global_key = Some(k);
        s.keys = vec![{ let mut k = chord(8.0, 0.0, 9); k.flags = FLAG_MINOR; k.fifths = 0; k }];
        s.patterns = vec![chord(4.0, 4.0, 12)];
        s
    }

    #[test]
    fn chords_cover_from_start_included_to_end_excluded() {
        let s = snap();
        assert_eq!(s.lookup(0.0, KIND_CHORD).unwrap().root, 2);
        assert_eq!(s.lookup(3.999, KIND_CHORD).unwrap().root, 2);
        assert_eq!(s.lookup(4.0, KIND_CHORD).unwrap().root, 7);
        assert!(s.lookup(8.0, KIND_CHORD).is_none() && s.lookup(11.0, KIND_CHORD).is_none(), "trou entre deux accords");
        assert_eq!(s.lookup(13.5, KIND_CHORD).unwrap().root, 0);
        assert!(s.lookup(14.0, KIND_CHORD).is_none() && s.lookup(-1.0, KIND_CHORD).is_none());
        assert!(s.lookup(f64::NAN, KIND_CHORD).is_none());
    }

    #[test]
    fn overlapping_scales_come_most_recent_first() {
        let s = snap();
        let mut out = [Item::new(0.0, 0.0); 8];
        assert_eq!(s.lookup_all(3.5, KIND_SCALE, &mut out), 3);
        assert_eq!((out[0].root, out[1].root, out[2].root), (9, 7, 2), "début le plus récent d'abord");
        assert_eq!(s.lookup(3.5, KIND_SCALE).unwrap().root, 9, "lookup = la première");
        assert_eq!(s.lookup_all(5.0, KIND_SCALE, &mut out), 2, "la gamme de 2 à 4 est finie");
        assert_eq!(s.lookup_all(20.0, KIND_SCALE, &mut out), 0);
    }

    #[test]
    fn next_after_is_strict_and_per_kind() {
        let s = snap();
        assert_eq!(s.next_after(0.0, KIND_CHORD).unwrap().start, 4.0);
        assert_eq!(s.next_after(4.0, KIND_CHORD).unwrap().start, 12.0);
        assert!(s.next_after(12.0, KIND_CHORD).is_none());
        assert_eq!(s.next_after(0.0, KIND_PATTERN).unwrap().start, 4.0);
        assert_eq!(s.next_after(0.0, KIND_KEY).unwrap().start, 8.0);
    }

    #[test]
    fn keys_last_until_the_next_marker_and_the_global_key_comes_first() {
        let s = snap();
        let a = s.lookup(1.0, KIND_KEY).unwrap();
        assert!(a.flags & FLAG_MINOR == 0 && a.len > 1e9, "avant le premier marqueur : la tonalité globale");
        let b = s.lookup(9.0, KIND_KEY).unwrap();
        assert!(b.flags & FLAG_MINOR != 0 && b.root == 9 && b.start == 8.0);
    }

    #[test]
    fn measures_come_from_the_project_meters_without_an_explicit_ruler() {
        let s = snap();
        let m = s.lookup(5.0, KIND_MEASURE).unwrap();
        assert_eq!((m.start, m.len, m.root, m.bass, m.index), (4.0, 4.0, 4, 4, 1));
        let m = s.lookup(9.5, KIND_MEASURE).unwrap();
        assert_eq!((m.start, m.len, m.root, m.bass, m.index), (8.0, 3.0, 3, 4, 2), "3/4 depuis la mesure 3");
        assert_eq!(s.index_at(12.0, KIND_MEASURE), Some(3));
        assert_eq!(s.next_after(9.5, KIND_MEASURE).unwrap().start, 11.0);
    }

    #[test]
    fn an_explicit_ruler_wins_and_can_start_with_a_short_pickup_bar() {
        let mut s = snap();
        s.measures = vec![
            { let mut m = Item::new(0.0, 1.0); m.root = 1; m.bass = 4; m },
            { let mut m = Item::new(1.0, 4.0); m.root = 4; m.bass = 4; m },
            { let mut m = Item::new(5.0, 4.0); m.root = 4; m.bass = 4; m },
        ];
        let a = s.lookup(0.5, KIND_MEASURE).unwrap();
        assert_eq!((a.len, a.root, a.index), (1.0, 1, 0), "anacrouse d'une noire");
        assert_eq!(s.lookup(6.0, KIND_MEASURE).unwrap().index, 2);
        assert_eq!(s.next_after(0.5, KIND_MEASURE).unwrap().start, 1.0);
        assert!(s.lookup(9.0, KIND_MEASURE).is_none(), "hors de la règle");
    }

    #[test]
    fn names_are_truncated_on_a_character_boundary() {
        let i = Item::new(0.0, 1.0).with_name("é".repeat(30).as_str());
        let n = i.name.iter().position(|&b| b == 0).unwrap();
        assert!(std::str::from_utf8(&i.name[..n]).is_ok() && n <= 23);
    }
}
