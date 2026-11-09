//! L'extension d'hôte `fr.flibuste.chord-track/1` : un plugin lit les accords, gammes, tonalités, motifs et mesures de la piste d'accords
//! pendant `process`, par la disposition C documentée dans `docs/fr-flibuste-chord-track.h`.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle, Plugin};
use daw_engine::chords::{ChordSnapshot, Item, FLAG_SHARP, KIND_CHORD};
use daw_engine::{AudioData, Clip, Command, GraphDesc, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_test_plugin::{entry, probe_chords_outside_process, seen};

const SR: u32 = 48_000;

fn item(start: f64, len: f64, root: u8) -> Item {
    let mut i = Item::new(start, len);
    i.root = root;
    i
}

fn snapshot() -> ChordSnapshot {
    let mut dm7 = item(0.0, 4.0, 2);
    dm7.mask = 0b0100_1000_1001; // m7 : 0, 3, 7, 10
    let dm7 = dm7.with_name("Dm7");
    let mut g7 = item(4.0, 4.0, 7);
    g7.bass = 11;
    g7.flags = FLAG_SHARP;
    g7.mask = 0b0100_0100_0100_1001_0001; // 7 : 0, 4, 7, 10 ; plus une neuvième à 14 (sur deux octaves)
    let g7 = g7.with_name("G7/B");
    let mut key = item(0.0, 0.0, 0);
    key.fifths = -2;
    ChordSnapshot {
        revision: 7,
        chords: vec![dm7, g7, item(10.0, 2.0, 0)],
        scales: vec![item(0.0, 8.0, 2), item(4.0, 4.0, 7)],
        max_scale_len: 8.0,
        global_key: Some(key),
        patterns: vec![item(4.0, 4.0, 12)],
        meters: vec![daw_engine::MeterSeg { start_beat: 0.0, start_bar: 1, num: 4, den: 4 }],
        ..Default::default()
    }
}

fn graph(with_chords: bool) -> GraphDesc {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![vec![0.1; 192_000], vec![0.1; 192_000]]).unwrap(), 0));
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    g.add_insert(t, "Gain de test", clap_insert_factory(bundle, "com.daw-perso.test-gain".into())).unwrap();
    if with_chords {
        g.chords = Arc::new(snapshot());
    }
    g
}

fn play(g: GraphDesc, seconds_x10: u64) {
    let mut spec = RenderSpec::new(SR, SR as u64 * seconds_x10 / 10, g);
    spec.block = 256;
    spec.events.push((0, Command::Play));
    render(&spec).unwrap();
}

#[test]
fn a_plugin_reads_the_chord_track_during_process_and_nothing_outside_it() {
    // Un seul test : le plugin de test garde « ce qu'il a lu » dans des variables globales.

    // Sans piste : l'extension existe, mais elle ne trouve rien.
    play(graph(false), 20);
    let c = seen::last_chords().unwrap();
    assert!(c.extension && c.chord_root == -1 && c.chord_count == 0 && c.revision == 0, "{c:?}");

    // 3 s à 120 bpm = 6 noires : on est dans G7/B (4 à 8) ; le dernier bloc commence un peu avant 6 noires.
    play(graph(true), 30);
    let c = seen::last_chords().unwrap();
    assert!(c.extension);
    assert_eq!(c.revision, 7);
    assert_eq!((c.chord_root, c.chord_start, c.chord_len, c.chord_bass), (7, 4.0, 4.0, 11), "accord actif : sol, de la noire 4, sur 4 noires, basse si");
    assert_eq!(c.chord_mask, 0b0100_0100_0100_1001_0001, "intervalles sur deux octaves : le bit 14 (neuvième) arrive jusqu'au plugin");
    assert_eq!(&c.chord_name[..4], b"G7/B");
    assert_eq!(c.scales, 2, "deux gammes se chevauchent à la noire 6");
    assert_eq!((c.scale_roots[0], c.scale_roots[1]), (7, 2), "la plus récente d'abord");
    assert_eq!(c.next_chord_start, 10.0, "prochain changement : après le trou");
    assert_eq!(c.key_fifths, -2, "tonalité globale");
    assert_eq!((c.measure_index, c.measure_len), (1, 4.0), "mesures déduites des chiffrages : la 2ᵉ");
    assert_eq!(c.pattern_slot, 12);
    assert_eq!(c.chord_count, 3);

    // Plus tôt dans la piste : l'accord change avec la position (sol à la noire 6, ré à la noire 1).
    play(graph(true), 5);
    let c = seen::last_chords().unwrap();
    assert_eq!((c.chord_root, c.chord_start), (2, 0.0), "à 0,5 s = noire 1 : ré mineur 7");
    assert_eq!(c.scales, 1);

    // Hors de `process` (autre fil, interface du plugin...), même avec une piste donnée au graphe : « rien » (pas d'instantané valable).
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let _plugin = Plugin::instantiate(&bundle, "com.daw-perso.test-gain").unwrap();
    probe_chords_outside_process();
    assert!(!seen::last_chords().unwrap().lookup_when_not_processing, "l'extension ne répond pas hors du traitement");
    let _ = KIND_CHORD;
}

#[test]
fn the_c_layout_matches_the_header() {
    use daw_clap_host::{FrChordTrack, FrItem, FR_EXT_CHORD_TRACK};
    use std::mem::{offset_of, size_of};
    assert_eq!((size_of::<FrItem>(), std::mem::align_of::<FrItem>()), (56, 8));
    let offsets = [
        offset_of!(FrItem, start_beats),
        offset_of!(FrItem, length_beats),
        offset_of!(FrItem, root),
        offset_of!(FrItem, bass),
        offset_of!(FrItem, flags),
        offset_of!(FrItem, fifths),
        offset_of!(FrItem, intervals),
        offset_of!(FrItem, index),
        offset_of!(FrItem, name),
    ];
    assert_eq!(offsets, [0, 8, 16, 17, 18, 19, 20, 24, 28]);
    assert_eq!(size_of::<FrChordTrack>(), 6 * size_of::<usize>(), "six pointeurs de fonctions, dans l'ordre du fichier d'en-tête");
    assert_eq!(FR_EXT_CHORD_TRACK.to_str().unwrap(), "fr.flibuste.chord-track/1");
}
