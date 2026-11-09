//! Enregistrement réel : moteur sur un fil, écriture disque sur un autre, fichier relu et comparé.

use std::path::PathBuf;

use daw_engine::{Command, Engine, GraphDesc, RecordSource, TrackFormat};
use daw_recorder::wav::read_wav;
use daw_recorder::Recording;

const SR: u32 = 48_000;

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("daw-rec-{}-{name}.wav", std::process::id()))
}

fn noise(seed: u32, len: usize) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 16_777_216.0 - 0.5
        })
        .collect()
}

#[test]
fn recorded_file_matches_the_input_sample_for_sample() {
    let mut g = GraphDesc::new();
    let t = g.add_track("voix", TrackFormat::Stereo).0;
    let (mut engine, mut handle) = Engine::new(SR, 128);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let path = temp("match");
    let rx = handle.start_record(t, RecordSource::Stereo, 6.0).unwrap();
    let rec = Recording::start(rx, &path).unwrap();

    // 5 secondes d'audio en blocs de 96 (qui ne divise pas la taille de bloc maximale du moteur).
    let (l, r) = (noise(1, 240_000), noise(2, 240_000));
    let (mut ol, mut or) = (vec![0.0f32; 96], vec![0.0f32; 96]);
    for k in (0..240_000).step_by(96) {
        let n = 96.min(240_000 - k);
        engine.process_io(&l[k..k + n], &r[k..k + n], &mut ol[..n], &mut or[..n]);
    }
    assert!(handle.stop_record(t));
    engine.process_io(&[], &[], &mut [], &mut []);
    let info = rec.finish().unwrap();

    assert_eq!((info.frames, info.channels, info.sample_rate, info.overruns, info.start_pos), (240_000, 2, SR, 0, 0));
    let wav = read_wav(&path).unwrap();
    assert_eq!(wav.frames(), 240_000);
    assert_eq!(wav.channel(0), l);
    assert_eq!(wav.channel(1), r);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn mono_take_starts_at_the_transport_position_and_stops_cleanly() {
    let mut g = GraphDesc::new();
    let t = g.add_track("gtr", TrackFormat::Mono).0;
    let (mut engine, mut handle) = Engine::new(SR, 256);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Seek(12_345));
    handle.send(Command::Play);
    let path = temp("mono");
    let rx = handle.start_record(t, RecordSource::Right, 1.0).unwrap();
    let rec = Recording::start(rx, &path).unwrap();
    let (l, r) = (noise(3, 10_000), noise(4, 10_000));
    let (mut ol, mut or) = (vec![0.0f32; 10_000], vec![0.0f32; 10_000]);
    engine.process_io(&l, &r, &mut ol, &mut or);
    handle.stop_record(t);
    engine.process_io(&[], &[], &mut [], &mut []);
    let info = rec.finish().unwrap();
    assert_eq!((info.frames, info.channels, info.start_pos), (10_000, 1, 12_345));
    assert_eq!(read_wav(&path).unwrap().samples, r);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_take_with_no_audio_is_a_valid_empty_file_and_a_bad_path_is_an_error() {
    let mut g = GraphDesc::new();
    let t = g.add_track("x", TrackFormat::Stereo).0;
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    let path = temp("empty");
    let rec = Recording::start(handle.start_record(t, RecordSource::Left, 0.5).unwrap(), &path).unwrap();
    handle.stop_record(t);
    engine.process_io(&[], &[], &mut [], &mut []);
    let info = rec.finish().unwrap();
    assert_eq!(info.frames, 0);
    assert_eq!(read_wav(&path).unwrap().frames(), 0);
    std::fs::remove_file(path).unwrap();

    let rx = handle.start_record(t, RecordSource::Left, 0.5).unwrap();
    assert!(Recording::start(rx, "/dossier/qui/n/existe/pas/prise.wav").is_err());
}

#[test]
fn engine_drop_without_stop_also_ends_the_writer() {
    let mut g = GraphDesc::new();
    let t = g.add_track("x", TrackFormat::Stereo).0;
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    let path = temp("abandon");
    let rec = Recording::start(handle.start_record(t, RecordSource::Left, 0.5).unwrap(), &path).unwrap();
    let (mut ol, mut or) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    engine.process_io(&[0.25; 64], &[0.0; 64], &mut ol, &mut or);
    drop(engine); // le moteur disparaît (arrêt brutal) : la file est abandonnée
    let info = rec.finish().unwrap();
    assert_eq!(info.frames, 64, "ce qui était dans la file est écrit, puis le fil s'arrête");
    std::fs::remove_file(path).unwrap();
}
