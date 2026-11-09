//! Le transport (position en temps, tempo, chiffrage, boucle) arrive aux plugins CLAP.

use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::{AudioData, Clip, Command, GraphDesc, MeterSeg, TempoSeg, TimeMap, TrackFormat};
use daw_offline_render::{render, RenderSpec};
use daw_test_plugin::{entry, seen};

const SR: u32 = 48_000;

fn graph() -> GraphDesc {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![vec![0.1; 96_000], vec![0.1; 96_000]]).unwrap(), 0));
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    g.add_insert(t, "Gain de test", clap_insert_factory(bundle, "com.daw-perso.test-gain".into())).unwrap();
    // 120 bpm pendant 1 s (2 noires), puis 60 bpm ; 4/4 puis 3/4 à la mesure 2 (noire 4).
    g.time_map = Arc::new(TimeMap {
        tempo: vec![TempoSeg { start_sample: 0, start_beat: 0.0, bpm: 120.0 }, TempoSeg { start_sample: 48_000, start_beat: 2.0, bpm: 60.0 }],
        meters: vec![MeterSeg { start_beat: 0.0, start_bar: 1, num: 4, den: 4 }, MeterSeg { start_beat: 4.0, start_bar: 2, num: 3, den: 4 }],
    });
    g
}

#[test]
fn plugins_receive_position_tempo_signature_and_playing_flag() {
    // Tout le transport tient dans un seul test : le plugin de test garde « le dernier transport » dans des variables globales.
    use clap_sys::events::*;
    let mut spec = RenderSpec::new(SR, 48_000 + 24_000, graph()); // 1,5 s : on s'arrête au milieu du palier à 60 bpm
    spec.block = 256;
    spec.events.push((0, Command::Play));
    render(&spec).unwrap();
    let s = seen::last();
    assert!(s.given && s.calls > 0);
    assert!(s.flags & CLAP_TRANSPORT_IS_PLAYING != 0, "en lecture");
    assert!(s.flags & CLAP_TRANSPORT_HAS_TEMPO != 0 && s.flags & CLAP_TRANSPORT_HAS_BEATS_TIMELINE != 0 && s.flags & CLAP_TRANSPORT_HAS_TIME_SIGNATURE != 0);
    assert_eq!(s.tempo, 60.0, "deuxième palier");
    // Dernier bloc : proche de 1,5 s = noire 2 + 0,5 = 2,5 ; la compensation de latence décale la lecture de 64 échantillons.
    assert!((s.beats - 2.5).abs() < 0.02, "noire {}", s.beats);
    assert!((s.seconds - 1.5).abs() < 0.02, "seconde {}", s.seconds);
    assert_eq!((s.tsig_num, s.tsig_den, s.bar_number), (4, 4, 1));

    // Passé la noire 4 (2,5 s) : mesure 2 en 3/4, début de mesure à la noire 4.
    let mut spec = RenderSpec::new(SR, 48_000 + 96_000, graph());
    spec.block = 256;
    spec.events.push((0, Command::Play));
    spec.events.push((0, Command::SetLoop(Some((24_000, 72_000)))));
    render(&spec).unwrap();
    let s = seen::last();
    assert!(s.flags & CLAP_TRANSPORT_IS_LOOP_ACTIVE != 0, "boucle active");
    // Boucle de 0,5 s à 1,5 s : en noires 1,0 à 2,5 (120 bpm jusqu'à 1 s, puis 60 bpm).
    assert!((s.loop_start_beats - 1.0).abs() < 1e-6 && (s.loop_end_beats - 2.5).abs() < 1e-6, "{s:?}");
}
