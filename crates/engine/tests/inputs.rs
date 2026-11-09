//! Entrées multicanal : chaque piste armée écoute et enregistre le ou les canaux qu'on lui a assignés.

use daw_engine::{Command, Engine, GraphDesc, RecordSource, TrackFormat};

const SR: u32 = 48_000;

/// Dix canaux d'entrée : le canal `c` vaut `(c + 1) / 100 + j / 100_000` à l'échantillon `j` (reconnaissable).
fn inputs(frames: usize, channels: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|c| (0..frames).map(|j| (c + 1) as f32 / 100.0 + j as f32 / 100_000.0).collect()).collect()
}

fn refs(v: &[Vec<f32>]) -> Vec<&[f32]> {
    v.iter().map(Vec::as_slice).collect()
}

fn three_tracks() -> (Engine, daw_engine::EngineHandle, [u32; 3]) {
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    let b = g.add_track("B", TrackFormat::Stereo);
    let c = g.add_track("C", TrackFormat::Stereo);
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    engine.process(&mut [0.0; 8], &mut [0.0; 8]);
    (engine, handle, [a.0, b.0, c.0])
}

fn drain(rx: &mut daw_engine::RecordReceiver) -> Vec<f32> {
    let n = rx.consumer.slots();
    let chunk = rx.consumer.read_chunk(n).unwrap();
    let (a, b) = chunk.as_slices();
    let v = [a, b].concat();
    chunk.commit_all();
    v
}

#[test]
fn each_track_records_its_own_input_channels_whatever_the_block_size() {
    for call in [1usize, 7, 64, 100, 333] {
        let (mut engine, mut handle, [a, b, c]) = three_tracks();
        let mut ra = handle.start_record(a, RecordSource::Channel(2), 1.0).unwrap();
        let mut rb = handle.start_record(b, RecordSource::Channel(7), 1.0).unwrap();
        let mut rc = handle.start_record(c, RecordSource::Pair(4, 5), 1.0).unwrap();
        let frames = 1_000;
        let data = inputs(frames, 10);
        let (mut l, mut r) = (vec![0.0f32; call], vec![0.0f32; call]);
        let mut done = 0;
        while done < frames {
            let n = call.min(frames - done);
            let slices: Vec<&[f32]> = data.iter().map(|ch| &ch[done..done + n]).collect();
            engine.process_inputs(&slices, &mut l[..n], &mut r[..n]);
            done += n;
        }
        assert_eq!(drain(&mut ra), data[2], "A : canal 3 (indice 2), mono, appel de {call}");
        assert_eq!(drain(&mut rb), data[7], "B : canal 8 (indice 7), mono");
        let pair: Vec<f32> = (0..frames).flat_map(|j| [data[4][j], data[5][j]]).collect();
        assert_eq!(drain(&mut rc), pair, "C : paire 5-6 (indices 4 et 5), stéréo");
    }
}

#[test]
fn a_missing_channel_records_silence_and_the_old_sources_still_mean_channels_0_and_1() {
    let (mut engine, mut handle, [a, b, c]) = three_tracks();
    let mut ra = handle.start_record(a, RecordSource::Channel(9), 1.0).unwrap(); // pas ouvert
    let mut rb = handle.start_record(b, RecordSource::Right, 1.0).unwrap(); // = canal 1
    let mut rc = handle.start_record(c, RecordSource::Pair(0, 9), 1.0).unwrap(); // gauche réelle, droite absente
    let data = inputs(64, 3);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    engine.process_inputs(&refs(&data), &mut l, &mut r);
    assert!(drain(&mut ra).iter().all(|&v| v == 0.0));
    assert_eq!(drain(&mut rb), data[1]);
    let rc_data = drain(&mut rc);
    assert!(rc_data.iter().step_by(2).zip(&data[0]).all(|(x, y)| x == y) && rc_data.iter().skip(1).step_by(2).all(|&v| v == 0.0));
}

#[test]
fn monitoring_plays_the_assigned_channel_and_a_meter_shows_the_input_when_not_monitoring() {
    let (mut engine, mut handle, [a, b, _]) = three_tracks();
    handle.send(Command::Play);
    handle.send(Command::SetInput { node: a, armed: true, monitor: true, source: RecordSource::Channel(3) });
    handle.send(Command::SetInput { node: b, armed: true, monitor: false, source: RecordSource::Channel(5) });
    let meters = handle.meters();
    let data = inputs(256, 8);
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    // Deux appels : le premier prend les commandes en compte.
    engine.process_inputs(&refs(&data), &mut l, &mut r);
    engine.process_inputs(&refs(&data), &mut l, &mut r);
    // Seule la piste A est écoutée : la sortie est son canal (gain 1 au centre, piste stéréo), pas celui de B.
    for j in 100..200 {
        assert_eq!(l[j], data[3][j], "écoute du canal 4 à l'échantillon {j}");
    }
    let (pa_l, _) = meters.take(a as usize);
    let (pb_l, pb_r) = meters.take(b as usize);
    assert!(pa_l > 0.0399, "A : vumètre de la sortie écoutée ({pa_l})");
    // B n'est pas écoutée mais armée : son vumètre montre la crête de l'entrée 6 (valeur 0,06 + rampe).
    assert!((pb_l - (0.06 + 255.0 / 100_000.0)).abs() < 1e-6 && pb_l == pb_r, "B : crête de l'entrée ({pb_l})");
}

#[test]
fn no_inputs_at_all_means_silence_for_armed_tracks() {
    let (mut engine, mut handle, [a, ..]) = three_tracks();
    handle.send(Command::Play);
    handle.send(Command::SetInput { node: a, armed: true, monitor: true, source: RecordSource::Channel(0) });
    let mut rx = handle.start_record(a, RecordSource::Channel(0), 1.0).unwrap();
    let (mut l, mut r) = (vec![1.0f32; 128], vec![1.0f32; 128]);
    engine.process_inputs(&[], &mut l, &mut r);
    // Sans canal d'entrée la longueur utile n'est plus bornée par eux : le bloc entier est traité, en silence.
    assert!(l.iter().chain(&r).all(|&v| v == 0.0));
    assert_eq!(drain(&mut rx).len(), 128);
}
