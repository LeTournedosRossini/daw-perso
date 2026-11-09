use daw_core_types::time::Frac;
use daw_engine::{GraphDesc, TrackFormat};
use daw_session::chords::{ChordError, ChordTrack, Pitch};
use daw_session::Session;

const SAMPLE: &str = r#"{
  "format": "flibuste-carte", "version": 1, "title": "Autumn Leaves", "from": "Neptune Carte", "unrolled": true,
  "timeSignatures": [ { "fromMeasure": 0, "ts": [4, 4] } ],
  "measures": [ { "at": [0, 1], "dur": [1, 4], "ts": [1, 4] }, { "at": [1, 4], "dur": [1, 1], "ts": [4, 4] } ],
  "qualities": { "m7": [0, 3, 7, 10], "7": [0, 4, 7, 10], "maj7": [0, 4, 7, 11], "zzz": [0, 5] },
  "chords": [
    { "at": [1, 4], "dur": [1, 1], "root": "D", "quality": "m7", "bass": null },
    { "at": [5, 4], "dur": [1, 1], "root": "G", "quality": "7", "bass": "B" },
    { "at": [9, 4], "dur": [1, 2], "nc": true },
    { "at": [11, 4], "dur": [1, 2], "root": "Bb", "quality": "maj7", "bass": "F" },
    { "at": [7, 2], "dur": [1, 1], "root": "C", "quality": "inconnue-de-moi" }
  ],
  "scales": [
    { "at": [1, 4], "dur": [2, 1], "root": "D", "scale": "dorian", "intervals": [0, 2, 3, 5, 7, 9, 10], "name": "Dorien" },
    { "at": [1, 2], "dur": [1, 1], "root": "G", "scale": "mixolydian" },
    { "at": [1, 1], "dur": [1, 1], "root": "F", "scale": "gamme-qui-n-existe-pas" }
  ],
  "keys": [ { "at": [0, 1], "root": "C", "mode": "major", "fifths": 0 }, { "at": [4, 1], "mode": "minor", "fifths": -1 } ],
  "globalKey": { "root": "C", "mode": "major", "fifths": 0 },
  "patterns": [ { "at": [1, 4], "dur": [2, 1], "slot": 12, "name": "Essai 1" }, { "at": [3, 1], "dur": [1, 1], "slot": 40 } ],
  "x": { "carte": { "peu importe": true } }
}"#;

fn load(text: &str) -> Result<(ChordTrack, daw_session::chords::ImportReport), ChordError> {
    ChordTrack::from_carte_json(&ChordTrack::default(), text)
}

#[test]
fn a_carte_file_is_read_with_exact_positions_and_what_cannot_be_read_is_counted() {
    let (t, report) = load(SAMPLE).unwrap();
    // Positions en fractions de ronde exactes, triées ; l'accord à qualité inconnue est ignoré et compté.
    assert_eq!(t.chords.len(), 4);
    assert_eq!(t.chords[0].start, Frac::new(1, 4).unwrap());
    assert_eq!((t.chords[0].root, t.chords[0].quality.as_str(), t.chords[0].bass), (Pitch::parse("D").unwrap(), "m7", None));
    assert_eq!(t.chords[1].bass, Pitch::parse("B"));
    assert!(t.chords[2].nc);
    assert_eq!((t.chords[3].root.pc, t.chords[3].root.sharp, t.chords[3].bass.unwrap().pc), (10, false, 5), "Si bémol, basse Fa");
    assert!(report.skipped.iter().any(|s| s.contains("qualité")), "{report:?}");
    // Gammes : intervalles du fichier, ou ceux du catalogue de Scorsaire quand l'identifiant est connu ; une inconnue sans intervalles est comptée.
    assert_eq!(t.scales.len(), 2);
    assert_eq!(t.scales[0].intervals, vec![0, 2, 3, 5, 7, 9, 10]);
    assert_eq!(t.scales[1].id.as_deref(), Some("mixolydian"));
    assert_eq!(t.scales[1].intervals, vec![0, 2, 4, 5, 7, 9, 10], "catalogue");
    assert!(report.skipped.iter().any(|s| s.contains("gamme")));
    // Tonalités : `fifths` suffit (sol mineur ? non : un bémol en mineur = ré mineur) ; la globale est lue.
    assert_eq!(t.keys.len(), 2);
    assert_eq!((t.keys[1].key.fifths, t.keys[1].key.minor, t.keys[1].key.root.pc), (-1, true, 2));
    assert_eq!(t.global_key.unwrap().fifths, 0);
    // Motifs : l'emplacement 40 n'existe pas (1 à 16).
    assert_eq!(t.patterns.len(), 1);
    assert!(report.skipped.iter().any(|s| s.contains("motif")));
    // Anacrouse : la première mesure d'une noire.
    assert_eq!((t.measures[0].len, t.measures[0].num, t.measures[0].den), (Frac::new(1, 4).unwrap(), 1, 4));
    // La qualité du fichier fait foi pour ce morceau et s'ajoute au dictionnaire.
    assert_eq!(t.qualities.get("zzz"), Some(&vec![0, 5]));
    assert_eq!(t.chord_mask(&t.chords[0]), Some(0b0100_1000_1001), "m7 = 0, 3, 7, 10");
}

#[test]
fn write_then_read_gives_the_same_track() {
    let (t, _) = load(SAMPLE).unwrap();
    let text = t.to_carte_json("Autumn Leaves");
    let (back, report) = ChordTrack::from_carte_json(&ChordTrack::default(), &text).unwrap();
    assert!(report.skipped.is_empty(), "{report:?}");
    assert_eq!(back.chords, t.chords);
    assert_eq!(back.scales, t.scales);
    assert_eq!(back.keys, t.keys);
    assert_eq!(back.global_key, t.global_key);
    assert_eq!(back.patterns, t.patterns);
    assert_eq!(back.measures, t.measures);
    assert_eq!(back.to_carte_json("Autumn Leaves"), text, "réécriture identique");
    // L'écriture (bémol, dièse) est celle de chaque accord.
    assert!(text.contains("\"Bb\"") && !text.contains("\"A#\""));
}

#[test]
fn present_voices_replace_and_absent_voices_are_left_alone() {
    let (t, _) = load(SAMPLE).unwrap();
    let only_patterns = r#"{ "format": "flibuste-carte", "version": 1, "patterns": [] }"#;
    let (merged, _) = ChordTrack::from_carte_json(&t, only_patterns).unwrap();
    assert!(merged.patterns.is_empty(), "voie présente, même vide : remplacée");
    assert_eq!(merged.chords, t.chords, "voie absente : intacte");
    assert_eq!(merged.scales, t.scales);
}

#[test]
fn newer_wrong_or_oversized_files_are_refused() {
    assert!(matches!(load(r#"{ "format": "flibuste-carte", "version": 2 }"#), Err(ChordError::TooNew { found: 2 })));
    assert!(matches!(load(r#"{ "format": "autre" }"#), Err(ChordError::Unreadable(_))));
    assert!(matches!(load("pas du json"), Err(ChordError::Unreadable(_))));
    let many: Vec<String> = (0..16_385u64).map(|i| format!(r#"{{ "at": [{i}, 1], "dur": [1, 1], "root": "C", "quality": "" }}"#)).collect();
    let big = format!(r#"{{ "format": "flibuste-carte", "version": 1, "chords": [{}] }}"#, many.join(","));
    assert!(matches!(load(&big), Err(ChordError::TooMany { voice: "accords", .. })));
    // Des accords qui se chevauchent sont refusés (une voie d'accords est une suite).
    let overlap = r#"{ "format": "flibuste-carte", "version": 1, "chords": [
        { "at": [0, 1], "dur": [2, 1], "root": "C", "quality": "" }, { "at": [1, 1], "dur": [1, 1], "root": "F", "quality": "" } ] }"#;
    assert!(matches!(load(overlap), Err(ChordError::Invalid(_))));
}

#[test]
fn pitch_names_keep_the_spelling() {
    for (text, pc, sharp) in [("C", 0, false), ("F#", 6, true), ("Gb", 6, false), ("Bb", 10, false), ("Ebb", 2, false), ("B#", 0, true)] {
        let p = Pitch::parse(text).unwrap();
        assert_eq!((p.pc, p.sharp), (pc, sharp), "{text}");
    }
    assert_eq!(Pitch::parse("F#").unwrap().name(), "F#");
    assert_eq!(Pitch::parse("Gb").unwrap().name(), "Gb");
    assert!(Pitch::parse("H").is_none() && Pitch::parse("").is_none() && Pitch::parse("C?").is_none());
}

fn session() -> Session {
    let mut g = GraphDesc::new();
    g.add_track("t", TrackFormat::Stereo);
    Session::new(g)
}

#[test]
fn the_session_imports_undoes_and_keeps_the_track_in_the_project() {
    let mut s = session();
    assert!(s.chord_track().is_empty());
    let rev = s.chord_revision();
    let report = s.import_carte(SAMPLE).unwrap();
    assert!(!report.skipped.is_empty());
    assert_eq!(s.chord_track().chords.len(), 4);
    assert_ne!(s.chord_revision(), rev);
    assert_eq!(s.undo_label(), Some("Importer une carte"));
    // Projet : écrit, relu, identique.
    struct NoAudio;
    impl daw_session::project::AudioLoader for NoAudio {
        fn load(&self, _: &str) -> Result<std::sync::Arc<daw_engine::AudioData>, String> {
            Err("pas d'audio".into())
        }
    }
    let (json, _) = s.to_json(48_000).unwrap();
    assert!(json.contains("\"chord_track\""));
    let (back, _) = Session::from_json(&json, &NoAudio).unwrap();
    assert_eq!(back.chord_track().chords, s.chord_track().chords);
    assert_eq!(back.chord_track().scales, s.chord_track().scales);
    assert_eq!(back.to_json(48_000).unwrap().0, json, "réécriture identique caractère pour caractère");
    // Annulation, rétablissement ; un fichier refusé ne laisse rien.
    assert!(s.undo());
    assert!(s.chord_track().is_empty());
    assert!(s.redo());
    assert_eq!(s.chord_track().chords.len(), 4);
    let before = s.chord_revision();
    assert!(s.import_carte(r#"{ "format": "flibuste-carte", "version": 9 }"#).is_err());
    assert_eq!(s.chord_revision(), before);
    // Une piste vide n'est pas écrite dans le projet.
    let (json2, _) = session().to_json(48_000).unwrap();
    assert!(!json2.contains("chord_track"));
}

#[test]
fn editing_the_track_is_one_undo_step_and_invalid_edits_leave_no_trace() {
    let mut s = session();
    s.edit_chord_track("Ajouter un accord", |t| {
        t.chords.push(daw_session::chords::ChordEv { start: Frac::ZERO, len: Frac::new(1, 1).unwrap(), root: Pitch::parse("A").unwrap(), quality: "m7".into(), bass: None, nc: false });
    })
    .unwrap();
    assert_eq!(s.undo_label(), Some("Ajouter un accord"));
    let r = s.edit_chord_track("Accord invalide", |t| t.chords[0].len = Frac::ZERO);
    assert!(r.is_err());
    assert_eq!(s.chord_track().chords[0].len, Frac::new(1, 1).unwrap());
    assert!(s.undo());
    assert!(s.chord_track().chords.is_empty());
}

#[test]
fn the_plugin_snapshot_is_in_beats_with_relative_masks_names_and_the_project_meters() {
    let mut s = session();
    s.import_carte(SAMPLE).unwrap();
    let snap = s.graph().chords.clone();
    assert_eq!(snap.revision, s.chord_revision());
    // Dm7 commence à [1, 4] de ronde = 1 noire, dure 4 noires.
    let dm7 = snap.lookup(1.5, 0).unwrap();
    assert_eq!((dm7.start, dm7.len, dm7.root, dm7.bass), (1.0, 4.0, 2, daw_engine::chords::NO_BASS));
    assert_eq!(dm7.mask, 0b0100_1000_1001);
    assert_eq!(&dm7.name[..3], b"Dm7");
    let g7 = snap.lookup(5.5, 0).unwrap();
    assert_eq!((g7.root, g7.bass), (7, 11));
    assert_eq!(&g7.name[..4], b"G7/B");
    assert!(snap.lookup(9.5, 0).unwrap().flags & daw_engine::chords::FLAG_NC != 0, "pas d'accord : un trou signalé");
    let scales = snap.lookup(3.0, 2).unwrap();
    assert_eq!(scales.root, 7, "la gamme de sol (posée à la noire 2) est plus récente que celle de ré");
    // Tonalité : au-delà du marqueur de la ronde 4 (noire 16), mineur à un bémol.
    let k = snap.lookup(17.0, 1).unwrap();
    assert!(k.flags & daw_engine::chords::FLAG_MINOR != 0 && k.fifths == -1);
    // Règle du fichier : anacrouse d'une noire, puis des mesures de 4.
    let m = snap.lookup(0.5, 4).unwrap();
    assert_eq!((m.len, m.root, m.bass, m.index), (1.0, 1, 4, 0));
    // Sans règle dans le fichier, les mesures viennent des chiffrages du projet.
    let mut t = session();
    t.edit_chord_track("x", |c| c.global_key = Some(daw_session::chords::KeyDef { root: Pitch::parse("C").unwrap(), minor: false, fifths: 0 })).unwrap();
    let m = t.graph().chords.lookup(5.0, 4).unwrap();
    assert_eq!((m.start, m.len, m.index), (4.0, 4.0, 1));
    // Annulation : l'instantané suit.
    assert!(s.undo());
    assert!(s.graph().chords.lookup(1.5, 0).is_none());
}

#[test]
fn qualities_are_scorsaires_with_two_octave_intervals_aliases_and_free_qualities() {
    let t = ChordTrack::default();
    assert_eq!(t.qualities.len(), 31, "le dictionnaire de Scorsaire");
    assert_eq!(t.qualities["add9"], vec![0, 4, 7, 14], "une neuvième est à 14, pas à 2");
    assert_eq!(t.qualities["sus2"], vec![0, 2, 7]);
    assert_eq!(t.qualities["13"], vec![0, 4, 7, 10, 14, 17, 21]);
    let file = r#"{ "format": "flibuste-carte", "version": 1, "chords": [
      { "at": [0, 1], "dur": [1, 4], "root": "C", "quality": "maj7" },
      { "at": [1, 4], "dur": [1, 4], "root": "D", "quality": "°7" },
      { "at": [1, 2], "dur": [1, 4], "root": "E", "quality": "m(maj7)" },
      { "at": [3, 4], "dur": [1, 4], "root": "F", "quality": "6/9" },
      { "at": [1, 1], "dur": [1, 4], "root": "G", "quality": "Δ" },
      { "at": [5, 4], "dur": [1, 4], "root": "A", "quality": "[0, 4, 7, 13]" },
      { "at": [6, 4], "dur": [1, 4], "root": "B", "quality": "2" },
      { "at": [7, 4], "dur": [1, 4], "root": "C", "quality": "add2" },
      { "at": [2, 1], "dur": [1, 4], "root": "D", "quality": "[0, 4, 24]" } ] }"#;
    let (t, report) = ChordTrack::from_carte_json(&ChordTrack::default(), file).unwrap();
    let q: Vec<&str> = t.chords.iter().map(|c| c.quality.as_str()).collect();
    assert_eq!(q, vec!["M7", "o7", "mMaj7", "69", "M7", "[0,4,7,13]"], "alias ramenés aux identifiants de Scorsaire, qualité libre normalisée");
    assert_eq!(report.skipped.len(), 3, "`2`, `add2` et un intervalle de 24 sont refusés : {report:?}");
    // Une qualité libre est écrite et relue ; son nom est « C[0,4,7,13] ».
    assert_eq!(t.qualities["[0,4,7,13]"], vec![0, 4, 7, 13]);
    let text = t.to_carte_json("x");
    let (back, report) = ChordTrack::from_carte_json(&ChordTrack::default(), &text).unwrap();
    assert!(report.skipped.is_empty() && back.chords == t.chords, "{report:?}");
    // Les intervalles voyagent sur deux octaves jusqu'au plugin : `9` = bits 0, 4, 7, 10, 14.
    let mut s = session();
    s.import_carte(r#"{ "format": "flibuste-carte", "version": 1, "chords": [
      { "at": [0, 1], "dur": [1, 1], "root": "C", "quality": "9" }, { "at": [1, 1], "dur": [1, 1], "root": "C", "quality": "add9" }, { "at": [2, 1], "dur": [1, 1], "root": "C", "quality": "sus2" },
      { "at": [3, 1], "dur": [1, 1], "root": "C", "quality": "[0,4,7,13]" } ] }"#)
        .unwrap();
    let snap = s.graph().chords.clone();
    let m = |beat| snap.lookup(beat, 0).unwrap();
    assert_eq!(m(1.0).mask, (1 << 0) | (1 << 4) | (1 << 7) | (1 << 10) | (1 << 14), "9");
    assert_eq!(m(5.0).mask, (1 << 0) | (1 << 4) | (1 << 7) | (1 << 14), "add9");
    assert_eq!(m(9.0).mask, (1 << 0) | (1 << 2) | (1 << 7), "sus2");
    assert_ne!(m(5.0).mask, m(9.0).mask, "add9 n'est pas sus2");
    assert_eq!(&m(13.0).name[..12], b"C[0,4,7,13]\0");
}
