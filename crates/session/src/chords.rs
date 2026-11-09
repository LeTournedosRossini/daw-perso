//! Piste d'accords : accords, gammes (qui peuvent se chevaucher), tonalités, appels de motifs et règle des mesures, en **temps musical
//! exact** (fractions de ronde). Seulement de l'information : le moteur ne la joue pas, les plugins la lisent (extension d'hôte).
//!
//! Échange : le format `flibuste-carte` version 1 (voir `FlibusteSuite/flibuste-neptune/docs/format-flibuste-carte.md`), lu et écrit par
//! Carte, prévu pour Scorsaire. Ce que ce module ne connaît pas est ignoré sans erreur et compté ; un fichier plus récent est refusé.

use std::collections::BTreeMap;

use daw_core_types::time::Frac;
use serde_json::{json, Map, Value};

use crate::quality_catalog::{ALIASES, QUALITIES};
use crate::scale_catalog::SCALES;

pub const MAX_MEASURES: usize = 4096;
pub const MAX_CHORDS: usize = 16384;
pub const MAX_SCALES: usize = 4096;
pub const MAX_KEYS: usize = 1024;
pub const MAX_PATTERNS: usize = 4096;

/// Plus grand intervalle d'une qualité : deux octaves (0 à 23 demi-tons), comme dans le dictionnaire de Scorsaire.
pub const MAX_INTERVAL: u8 = 23;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChordError {
    /// Fichier illisible (JSON invalide, mauvais format).
    Unreadable(String),
    /// Fichier d'une version plus récente que celle que nous lisons.
    TooNew { found: u32 },
    /// Trop d'éléments d'une voie.
    TooMany { voice: &'static str, max: usize },
    /// Donnée invalide (durée nulle, hauteur hors plage, accords qui se chevauchent...).
    Invalid(String),
}

impl std::fmt::Display for ChordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChordError::Unreadable(m) => write!(f, "carte illisible : {m}"),
            ChordError::TooNew { found } => write!(f, "carte de version {found} : plus récente que celle que ce logiciel lit"),
            ChordError::TooMany { voice, max } => write!(f, "trop d'éléments dans la voie « {voice} » (au plus {max})"),
            ChordError::Invalid(m) => write!(f, "carte invalide : {m}"),
        }
    }
}

impl std::error::Error for ChordError {}

const SHARP_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const FLAT_NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];

/// Une note : classe de hauteur (do = 0) et écriture (dièse ou bémol) conservée par élément.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pitch {
    pub pc: u8,
    pub sharp: bool,
}

impl Pitch {
    pub fn new(pc: u8, sharp: bool) -> Pitch {
        Pitch { pc: pc % 12, sharp }
    }

    /// `"C"`, `"F#"`, `"Bb"`, `"Ebb"` (lettres anglaises). Les doubles altérations donnent leur hauteur ; l'écriture retenue est celle
    /// de la dernière altération.
    pub fn parse(text: &str) -> Option<Pitch> {
        let mut chars = text.chars();
        let base: i32 = match chars.next()? {
            'C' | 'c' => 0,
            'D' | 'd' => 2,
            'E' | 'e' => 4,
            'F' | 'f' => 5,
            'G' | 'g' => 7,
            'A' | 'a' => 9,
            'B' | 'b' => 11,
            _ => return None,
        };
        let (mut pc, mut sharp) = (base, false);
        for c in chars {
            match c {
                '#' | '♯' => {
                    pc += 1;
                    sharp = true;
                }
                'b' | '♭' => {
                    pc -= 1;
                    sharp = false;
                }
                _ => return None,
            }
        }
        Some(Pitch { pc: pc.rem_euclid(12) as u8, sharp })
    }

    pub fn name(&self) -> &'static str {
        let table = if self.sharp { &SHARP_NAMES } else { &FLAT_NAMES };
        table[usize::from(self.pc % 12)]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChordEv {
    pub start: Frac,
    pub len: Frac,
    pub root: Pitch,
    /// Identifiant de qualité (suffixe) ; la table est `ChordTrack::qualities`.
    pub quality: String,
    pub bass: Option<Pitch>,
    /// « Pas d'accord » : un trou.
    pub nc: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScaleEv {
    pub start: Frac,
    pub len: Frac,
    pub root: Pitch,
    /// Identifiant du catalogue de gammes de Scorsaire, s'il est connu (`None` : gamme libre).
    pub id: Option<String>,
    /// Demi-tons depuis la fondamentale : fait foi pour le morceau.
    pub intervals: Vec<u8>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyDef {
    pub root: Pitch,
    pub minor: bool,
    /// Nombre de quintes de −7 à 7 (négatif = bémols), comme l'armure de Scorsaire.
    pub fifths: i8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyEv {
    pub start: Frac,
    pub key: KeyDef,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatternEv {
    pub start: Frac,
    pub len: Frac,
    /// Emplacement 1 à 16 d'un Arpège ou d'un instrument de la suite.
    pub slot: u8,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeasureEv {
    pub start: Frac,
    pub len: Frac,
    pub num: u16,
    pub den: u16,
}

/// Toute la piste. Les vecteurs sont **triés par début** (`normalize`) ; les accords ne se chevauchent pas, les gammes le peuvent.
#[derive(Debug, Clone, PartialEq)]
pub struct ChordTrack {
    pub chords: Vec<ChordEv>,
    pub scales: Vec<ScaleEv>,
    pub keys: Vec<KeyEv>,
    pub global_key: Option<KeyDef>,
    pub patterns: Vec<PatternEv>,
    /// Règle des mesures du morceau (anacrouse, mesures de longueur quelconque). Vide : la carte des chiffrages de la session.
    pub measures: Vec<MeasureEv>,
    pub qualities: BTreeMap<String, Vec<u8>>,
}

impl Default for ChordTrack {
    fn default() -> Self {
        ChordTrack {
            chords: Vec::new(),
            scales: Vec::new(),
            keys: Vec::new(),
            global_key: None,
            patterns: Vec::new(),
            measures: Vec::new(),
            qualities: QUALITIES.iter().map(|(k, v)| ((*k).to_owned(), v.to_vec())).collect(),
        }
    }
}

impl ChordTrack {
    /// Aucun élément dans aucune voie.
    pub fn is_empty(&self) -> bool {
        self.chords.is_empty() && self.scales.is_empty() && self.keys.is_empty() && self.global_key.is_none() && self.patterns.is_empty() && self.measures.is_empty()
    }

    /// Trie chaque voie par début (tri stable : à début égal, l'ordre d'écriture est conservé).
    pub fn normalize(&mut self) {
        self.chords.sort_by_key(|c| c.start);
        self.scales.sort_by_key(|c| c.start);
        self.keys.sort_by_key(|c| c.start);
        self.patterns.sort_by_key(|c| c.start);
        self.measures.sort_by_key(|c| c.start);
    }

    /// Vérifie limites, durées, hauteurs, qualités connues et accords sans chevauchement. À appeler après `normalize`.
    pub fn validate(&self) -> Result<(), ChordError> {
        let too_many = |voice, max, n: usize| if n > max { Err(ChordError::TooMany { voice, max }) } else { Ok(()) };
        too_many("mesures", MAX_MEASURES, self.measures.len())?;
        too_many("accords", MAX_CHORDS, self.chords.len())?;
        too_many("gammes", MAX_SCALES, self.scales.len())?;
        too_many("tonalités", MAX_KEYS, self.keys.len())?;
        too_many("motifs", MAX_PATTERNS, self.patterns.len())?;
        let zero = Frac::ZERO;
        let bad = |what: &str, i: usize| ChordError::Invalid(format!("{what} {} : durée nulle ou position impossible", i + 1));
        for (i, c) in self.chords.iter().enumerate() {
            if c.len == zero || c.start.checked_add(c.len).is_none() {
                return Err(bad("accord", i));
            }
            if !c.nc && !self.qualities.contains_key(&c.quality) {
                return Err(ChordError::Invalid(format!("accord {} : qualité « {} » inconnue", i + 1, c.quality)));
            }
        }
        for w in self.chords.windows(2) {
            if w[0].start.checked_add(w[0].len).is_some_and(|end| end > w[1].start) {
                return Err(ChordError::Invalid("des accords se chevauchent".into()));
            }
        }
        for (i, s) in self.scales.iter().enumerate() {
            if s.len == zero || s.start.checked_add(s.len).is_none() || s.intervals.is_empty() || s.intervals.iter().any(|&x| x > 11) {
                return Err(bad("gamme", i));
            }
        }
        for (i, p) in self.patterns.iter().enumerate() {
            if p.len == zero || p.start.checked_add(p.len).is_none() || !(1..=16).contains(&p.slot) {
                return Err(bad("motif", i));
            }
        }
        for (i, m) in self.measures.iter().enumerate() {
            if m.len == zero || m.start.checked_add(m.len).is_none() || m.num == 0 || m.den == 0 {
                return Err(bad("mesure", i));
            }
        }
        for k in self.keys.iter().map(|k| k.key).chain(self.global_key) {
            if !(-7..=7).contains(&k.fifths) {
                return Err(ChordError::Invalid("tonalité : quintes hors de −7 à 7".into()));
            }
        }
        for iv in self.qualities.values() {
            if iv.iter().any(|&x| x > MAX_INTERVAL) {
                return Err(ChordError::Invalid("qualité : demi-tons hors de 0 à 23".into()));
            }
        }
        Ok(())
    }

    /// Intervalles d'un accord sur **deux octaves**, relatifs à la fondamentale (bit i = i demi-tons, de 0 à 23) : une neuvième est 14,
    /// pas 2. `None` : accord inconnu ou « pas d'accord ».
    pub fn chord_mask(&self, c: &ChordEv) -> Option<u32> {
        if c.nc {
            return None;
        }
        self.qualities.get(&c.quality).map(|iv| iv.iter().fold(0u32, |m, &i| m | (1 << u32::from(i.min(MAX_INTERVAL)))))
    }

    /// Retrouve l'identifiant d'une qualité écrite par quelqu'un : tel quel s'il est dans le dictionnaire (celui du fichier fait foi),
    /// sinon par ses écritures acceptées (`maj7` → `M7`, `°7` → `o7`...), sinon une qualité libre `[0,4,7,14]` (demi-tons de 0 à 23),
    /// ajoutée au dictionnaire. `2` et `add2` ne sont pas acceptés.
    pub fn resolve_quality(&mut self, text: &str) -> Option<String> {
        if self.qualities.contains_key(text) {
            return Some(text.to_owned());
        }
        if let Some((_, id)) = ALIASES.iter().find(|(a, _)| *a == text) {
            return self.qualities.contains_key(*id).then(|| (*id).to_owned());
        }
        let inner = text.strip_prefix('[')?.strip_suffix(']')?;
        let iv: Option<Vec<u8>> = inner.split(',').map(|x| x.trim().parse::<u8>().ok().filter(|&n| n <= MAX_INTERVAL)).collect();
        let mut iv = iv.filter(|v| !v.is_empty())?;
        iv.sort_unstable();
        iv.dedup();
        let id = format!("[{}]", iv.iter().map(u8::to_string).collect::<Vec<_>>().join(","));
        self.qualities.entry(id.clone()).or_insert(iv);
        Some(id)
    }
}

/// Ce qu'un import a laissé de côté (rien n'est perdu en silence).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub skipped: Vec<String>,
}

fn frac(v: &Value) -> Option<Frac> {
    let a = v.as_array()?;
    Frac::new(a.first()?.as_u64()?, a.get(1)?.as_u64()?)
}

fn pitch(v: &Value) -> Option<Pitch> {
    Pitch::parse(v.as_str()?)
}

fn key_def(v: &Value) -> Option<KeyDef> {
    let root = v.get("root").and_then(pitch);
    let minor = v.get("mode").and_then(Value::as_str).map(|m| m == "minor");
    let fifths = v.get("fifths").and_then(Value::as_i64).filter(|f| (-7..=7).contains(f)).map(|f| f as i8);
    match (root, minor, fifths) {
        (Some(root), Some(minor), Some(fifths)) => Some(KeyDef { root, minor, fifths }),
        // `fifths` seul : la tonique se déduit du mode (majeur par défaut) ; `root` + `mode` seuls : les quintes se déduisent.
        (None, m, Some(fifths)) => {
            let minor = m.unwrap_or(false);
            let pc = (i32::from(fifths) * 7).rem_euclid(12) + if minor { 9 } else { 0 };
            Some(KeyDef { root: Pitch::new((pc.rem_euclid(12)) as u8, fifths >= 0), minor, fifths })
        }
        (Some(root), Some(minor), None) => {
            let major_pc = if minor { (i32::from(root.pc) + 3) % 12 } else { i32::from(root.pc) };
            let fifths = (0..12i32).map(|k| if k <= 6 { k } else { k - 12 }).find(|k| (k * 7).rem_euclid(12) == major_pc).unwrap_or(0) as i8;
            Some(KeyDef { root, minor, fifths })
        }
        _ => None,
    }
}

fn intervals_of(v: &Value, max: u8) -> Option<Vec<u8>> {
    let a = v.as_array()?;
    let mut out = Vec::with_capacity(a.len());
    for x in a {
        let n = x.as_u64()?;
        if n > u64::from(max) {
            return None;
        }
        out.push(n as u8);
    }
    Some(out)
}

impl ChordTrack {
    /// Lit un fichier `flibuste-carte` (ou `flibuste-accords`) et le fusionne avec `existing` : une voie **présente** dans le fichier
    /// (même vide) remplace celle de `existing`, une voie absente n'est pas touchée. Rend la piste et ce qui a été ignoré.
    pub fn from_carte_json(existing: &ChordTrack, text: &str) -> Result<(ChordTrack, ImportReport), ChordError> {
        let root: Value = serde_json::from_str(text).map_err(|e| ChordError::Unreadable(e.to_string()))?;
        match root.get("format").and_then(Value::as_str) {
            Some("flibuste-carte" | "flibuste-accords") => {}
            _ => return Err(ChordError::Unreadable("ce n'est pas un fichier flibuste-carte".into())),
        }
        let version = root.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
        if version > 1 {
            return Err(ChordError::TooNew { found: version });
        }
        let mut track = existing.clone();
        let mut report = ImportReport::default();
        let mut skip = |what: String| report.skipped.push(what);

        // Le dictionnaire du fichier fait foi pour ce morceau ; on garde celui de l'existant pour ce qu'il ne dit pas.
        if let Some(q) = root.get("qualities").and_then(Value::as_object) {
            for (id, iv) in q {
                match intervals_of(iv, MAX_INTERVAL) {
                    Some(iv) => {
                        track.qualities.insert(id.clone(), iv);
                    }
                    None => skip(format!("qualité « {id} » ignorée (intervalles illisibles)")),
                }
            }
        }
        if let Some(a) = root.get("measures").and_then(Value::as_array) {
            track.measures = a
                .iter()
                .enumerate()
                .filter_map(|(i, m)| {
                    let ts = m.get("ts").and_then(Value::as_array);
                    let (num, den) = (ts.and_then(|t| t.first()).and_then(Value::as_u64), ts.and_then(|t| t.get(1)).and_then(Value::as_u64));
                    match (frac(m.get("at")?), frac(m.get("dur")?), num, den) {
                        (Some(start), Some(len), Some(n), Some(d)) if n > 0 && d > 0 && n < 65536 && d < 65536 => Some(MeasureEv { start, len, num: n as u16, den: d as u16 }),
                        _ => {
                            skip(format!("mesure {} ignorée", i + 1));
                            None
                        }
                    }
                })
                .collect();
        }
        if let Some(a) = root.get("chords").and_then(Value::as_array) {
            track.chords = a
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let nc = c.get("nc").and_then(Value::as_bool).unwrap_or(false);
                    let (start, len) = (c.get("at").and_then(frac), c.get("dur").and_then(frac));
                    let (Some(start), Some(len)) = (start, len) else {
                        skip(format!("accord {} ignoré (position illisible)", i + 1));
                        return None;
                    };
                    if nc {
                        return Some(ChordEv { start, len, root: Pitch::new(0, true), quality: String::new(), bass: None, nc: true });
                    }
                    let root = c.get("root").and_then(pitch);
                    let written = c.get("quality").and_then(Value::as_str).unwrap_or("").to_owned();
                    let bass = c.get("bass").filter(|b| !b.is_null()).and_then(pitch);
                    let resolved = track.resolve_quality(&written);
                    match (root, resolved) {
                        (Some(root), Some(quality)) => Some(ChordEv { start, len, root, quality, bass, nc: false }),
                        (Some(_), None) => {
                            skip(format!("accord {} ignoré (qualité « {written} » inconnue)", i + 1));
                            None
                        }
                        (None, _) => {
                            skip(format!("accord {} ignoré (fondamentale illisible)", i + 1));
                            None
                        }
                    }
                })
                .collect();
        }
        if let Some(a) = root.get("scales").and_then(Value::as_array) {
            track.scales = a
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    let (start, len, rt) = (s.get("at").and_then(frac), s.get("dur").and_then(frac), s.get("root").and_then(pitch));
                    let id = s.get("scale").and_then(Value::as_str).map(str::to_owned);
                    let known = id.as_deref().and_then(|id| SCALES.iter().find(|(k, ..)| *k == id));
                    let intervals = s.get("intervals").and_then(|v| intervals_of(v, 11)).or_else(|| known.map(|k| k.3.to_vec()));
                    let name = s.get("name").and_then(Value::as_str).map(str::to_owned).or_else(|| known.map(|k| k.1.to_owned())).or_else(|| id.clone()).unwrap_or_default();
                    match (start, len, rt, intervals) {
                        (Some(start), Some(len), Some(root), Some(intervals)) if !intervals.is_empty() => Some(ScaleEv { start, len, root, id: known.map(|k| k.0.to_owned()), intervals, name }),
                        _ => {
                            skip(format!("gamme {} ignorée (ni intervalles ni identifiant connu)", i + 1));
                            None
                        }
                    }
                })
                .collect();
        }
        if let Some(a) = root.get("keys").and_then(Value::as_array) {
            track.keys = a
                .iter()
                .enumerate()
                .filter_map(|(i, k)| match (k.get("at").and_then(frac), key_def(k)) {
                    (Some(start), Some(key)) => Some(KeyEv { start, key }),
                    _ => {
                        skip(format!("tonalité {} ignorée", i + 1));
                        None
                    }
                })
                .collect();
        }
        if let Some(g) = root.get("globalKey") {
            track.global_key = key_def(g);
            if track.global_key.is_none() && !g.is_null() {
                skip("tonalité globale ignorée".into());
            }
        }
        if let Some(a) = root.get("patterns").and_then(Value::as_array) {
            track.patterns = a
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    let slot = p.get("slot").and_then(Value::as_u64).filter(|s| (1..=16).contains(s));
                    match (p.get("at").and_then(frac), p.get("dur").and_then(frac), slot) {
                        (Some(start), Some(len), Some(slot)) => Some(PatternEv { start, len, slot: slot as u8, name: p.get("name").and_then(Value::as_str).unwrap_or("").to_owned() }),
                        _ => {
                            skip(format!("appel de motif {} ignoré (emplacement de 1 à 16 attendu)", i + 1));
                            None
                        }
                    }
                })
                .collect();
        }
        track.normalize();
        track.validate()?;
        Ok((track, report))
    }

    /// Écrit la piste au format `flibuste-carte` version 1. Les positions sont celles de la session (fractions de ronde exactes).
    pub fn to_carte_json(&self, title: &str) -> String {
        let fr = |f: Frac| json!([f.num(), f.den()]);
        let key_json = |k: &KeyDef| json!({ "root": k.root.name(), "mode": if k.minor { "minor" } else { "major" }, "fifths": k.fifths });
        let used: std::collections::BTreeSet<&str> = self.chords.iter().filter(|c| !c.nc).map(|c| c.quality.as_str()).collect();
        let qualities: Map<String, Value> = self.qualities.iter().filter(|(k, _)| used.contains(k.as_str())).map(|(k, v)| (k.clone(), json!(v))).collect();
        let mut o = Map::new();
        o.insert("format".into(), json!("flibuste-carte"));
        o.insert("version".into(), json!(1));
        o.insert("title".into(), json!(title));
        o.insert("from".into(), json!("DAW"));
        o.insert("unrolled".into(), json!(true));
        if !self.measures.is_empty() {
            o.insert("measures".into(), Value::Array(self.measures.iter().map(|m| json!({ "at": fr(m.start), "dur": fr(m.len), "ts": [m.num, m.den] })).collect()));
        }
        o.insert("qualities".into(), Value::Object(qualities));
        o.insert(
            "chords".into(),
            Value::Array(
                self.chords
                    .iter()
                    .map(|c| {
                        if c.nc {
                            json!({ "at": fr(c.start), "dur": fr(c.len), "nc": true })
                        } else {
                            json!({ "at": fr(c.start), "dur": fr(c.len), "root": c.root.name(), "quality": c.quality, "bass": c.bass.map(|b| b.name()) })
                        }
                    })
                    .collect(),
            ),
        );
        o.insert(
            "scales".into(),
            Value::Array(
                self.scales
                    .iter()
                    .map(|s| {
                        let mut v = json!({ "at": fr(s.start), "dur": fr(s.len), "root": s.root.name(), "intervals": s.intervals, "name": s.name });
                        if let Some(id) = &s.id {
                            v["scale"] = json!(id);
                        }
                        v
                    })
                    .collect(),
            ),
        );
        o.insert("keys".into(), Value::Array(self.keys.iter().map(|k| {
            let mut v = key_json(&k.key);
            v["at"] = fr(k.start);
            v
        }).collect()));
        if let Some(g) = &self.global_key {
            o.insert("globalKey".into(), key_json(g));
        }
        o.insert("patterns".into(), Value::Array(self.patterns.iter().map(|p| json!({ "at": fr(p.start), "dur": fr(p.len), "slot": p.slot, "name": p.name })).collect()));
        serde_json::to_string_pretty(&Value::Object(o)).unwrap_or_default()
    }
}

fn mask_of(intervals: &[u8]) -> u32 {
    intervals.iter().fold(0u32, |m, &i| m | (1 << u32::from(i % 12)))
}

impl ChordTrack {
    /// L'instantané que lisent les plugins : positions en noires (fraction de ronde × 4), voies triées, masques relatifs à la
    /// fondamentale. `meters` : chiffrages du projet, pour déduire la règle des mesures quand le fichier n'en donne pas.
    pub fn snapshot(&self, meters: &[daw_engine::MeterSeg], revision: u32) -> daw_engine::ChordSnapshot {
        use daw_engine::chords::{FLAG_MINOR, FLAG_NC, FLAG_SHARP, NO_BASS};
        use daw_engine::ChordItem;
        let beats = |f: Frac| f.as_f64() * 4.0;
        let item = |start: Frac, len: Frac| ChordItem::new(beats(start), beats(len));
        let spell = |p: Pitch| if p.sharp { FLAG_SHARP } else { 0 };
        let chords = self
            .chords
            .iter()
            .map(|c| {
                let mut i = item(c.start, c.len);
                i.root = c.root.pc;
                i.flags = spell(c.root) | if c.nc { FLAG_NC } else { 0 };
                i.bass = c.bass.map_or(NO_BASS, |b| b.pc);
                i.mask = self.chord_mask(c).unwrap_or(0);
                let name = if c.nc { "N.C.".to_owned() } else { format!("{}{}{}", c.root.name(), c.quality, c.bass.map_or(String::new(), |b| format!("/{}", b.name()))) };
                i.with_name(&name)
            })
            .collect();
        let scales: Vec<ChordItem> = self
            .scales
            .iter()
            .map(|s| {
                let mut i = item(s.start, s.len);
                i.root = s.root.pc;
                i.flags = spell(s.root);
                i.mask = mask_of(&s.intervals);
                i.with_name(&s.name)
            })
            .collect();
        let key_item = |k: &KeyDef, start: f64| {
            let mut i = ChordItem::new(start, 0.0);
            i.root = k.root.pc;
            i.fifths = k.fifths;
            i.flags = spell(k.root) | if k.minor { FLAG_MINOR } else { 0 };
            i.mask = mask_of(if k.minor { &[0, 2, 3, 5, 7, 8, 10] } else { &[0, 2, 4, 5, 7, 9, 11] });
            i.with_name(&format!("{} {}", k.root.name(), if k.minor { "minor" } else { "major" }))
        };
        let patterns = self
            .patterns
            .iter()
            .map(|p| {
                let mut i = item(p.start, p.len);
                i.root = p.slot;
                i.with_name(&p.name)
            })
            .collect();
        let measures = self
            .measures
            .iter()
            .map(|m| {
                let mut i = item(m.start, m.len);
                i.root = m.num.min(255) as u8;
                i.bass = m.den.min(255) as u8;
                i
            })
            .collect();
        daw_engine::ChordSnapshot {
            revision,
            chords,
            keys: self.keys.iter().map(|k| key_item(&k.key, beats(k.start))).collect(),
            max_scale_len: scales.iter().map(|s| s.len).fold(0.0, f64::max),
            scales,
            patterns,
            measures,
            global_key: self.global_key.as_ref().map(|k| key_item(k, 0.0)),
            meters: meters.to_vec(),
        }
    }
}
