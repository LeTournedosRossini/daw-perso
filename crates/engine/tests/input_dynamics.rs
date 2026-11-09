//! Dynamique d'entrée d'une piste : niveaux, écoute, prise brute ou traitée, continuité à un changement de graphe.

use daw_engine::{Command, Engine, EngineHandle, GraphDesc, InputDynamics, RecordSource, TrackFormat};

const SR: u32 = 48_000;

fn sine(level_db: f32, n: usize) -> Vec<f32> {
    let a = 10f32.powf(level_db / 20.0);
    (0..n).map(|i| a * (std::f32::consts::TAU * 440.0 * (i as f32) / SR as f32).sin()).collect()
}

fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

fn db(x: f32) -> f32 {
    20.0 * x.log10()
}

/// Piste mono armée et écoutée sur l'entrée 1, vers la sortie principale (gain 1).
fn rig(dyn_: InputDynamics) -> (Engine, EngineHandle, u32, GraphDesc) {
    let mut g = GraphDesc::new();
    let t = g.add_track("voix", TrackFormat::Stereo);
    g.node_mut(t).unwrap().input_dyn = dyn_;
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::SetInput { node: t.0, armed: true, monitor: true, source: RecordSource::Channel(0) });
    engine.process(&mut [0.0; 8], &mut [0.0; 8]);
    (engine, handle, t.0, g)
}

/// Fait passer `x` par l'entrée 1 ; rend la sortie principale (gauche).
fn play(engine: &mut Engine, x: &[f32]) -> Vec<f32> {
    let (mut out, mut r) = (Vec::new(), vec![0.0f32; 64]);
    for chunk in x.chunks(64) {
        let mut l = vec![0.0f32; chunk.len()];
        engine.process_inputs(&[chunk], &mut l, &mut r[..chunk.len()]);
        out.extend(l);
    }
    out
}

fn loud_compressor() -> InputDynamics {
    InputDynamics { enabled: true, threshold_db: -20.0, ratio: 4.0, attack_ms: 1.0, release_ms: 50.0, ..Default::default() }
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
fn disabled_the_input_passes_through_untouched_and_enabled_the_monitored_level_follows_the_compressor() {
    let x = sine(-8.0, SR as usize);
    let (mut engine, _h, _t, _g) = rig(InputDynamics::default());
    let y = play(&mut engine, &x);
    assert_eq!(y, x, "désactivée, aucune différence au bit près");
    let (mut engine, _h, _t, _g) = rig(loud_compressor());
    let y = play(&mut engine, &x);
    let out = db(peak(&y[SR as usize / 2..]));
    assert!((out + 17.0).abs() < 0.7, "−8 dB avec seuil −20 et rapport 4 → −17 dB : {out:.2}");
}

#[test]
fn the_take_stays_raw_unless_the_option_prints_the_processed_signal() {
    let x = sine(-8.0, SR as usize / 2);
    for (record, expect_processed) in [(false, false), (true, true)] {
        let (mut engine, mut handle, t, _g) = rig(InputDynamics { record, ..loud_compressor() });
        let mut rx = handle.start_record(t, RecordSource::Channel(0), 2.0).unwrap();
        handle.send(Command::Play);
        play(&mut engine, &x);
        handle.stop_record(t);
        engine.process(&mut [0.0; 8], &mut [0.0; 8]);
        let take = drain(&mut rx);
        assert_eq!(take.len(), x.len());
        let tail = &take[x.len() / 2..];
        if expect_processed {
            assert!((db(peak(tail)) + 17.0).abs() < 0.8, "prise traitée : {:.2} dB", db(peak(tail)));
        } else {
            assert_eq!(take, x, "prise brute : exactement l'entrée");
        }
    }
}

#[test]
fn a_gate_silences_the_noise_floor_of_an_input() {
    let cfg = InputDynamics { enabled: true, gate_db: -50.0, threshold_db: 0.0, ratio: 1.0, ..Default::default() };
    let (mut engine, _h, _t, _g) = rig(cfg);
    let mut x = sine(-70.0, SR as usize / 2);
    x.extend(sine(-20.0, SR as usize / 2));
    let y = play(&mut engine, &x);
    assert!(peak(&y[SR as usize / 4..SR as usize / 2]) < 0.1 * peak(&x[SR as usize / 4..SR as usize / 2]));
    assert!((peak(&y[SR as usize * 3 / 4..]) / peak(&x[SR as usize * 3 / 4..]) - 1.0).abs() < 0.05);
}

#[test]
fn reloading_the_graph_keeps_the_envelope_so_the_level_does_not_jump() {
    let x = sine(-8.0, SR as usize);
    let (mut engine, mut handle, t, mut g) = rig(loud_compressor());
    let first = play(&mut engine, &x[..SR as usize / 2]);
    // Un autre réglage (rapport 8) : le nouveau graphe reprend l'enveloppe de l'ancien, la piste reste armée et écoutée.
    g.node_mut(daw_core_types::NodeId(t)).unwrap().input_dyn = InputDynamics { ratio: 8.0, ..loud_compressor() };
    handle.load_graph(&g).unwrap();
    let second = play(&mut engine, &x[SR as usize / 2..]);
    let before = peak(&first[first.len() - 480..]);
    let after = peak(&second[..480]);
    // Une enveloppe remise à zéro laisserait passer l'attaque entière (+10 dB) : ici le niveau reste dans la zone de compression.
    assert!(db(after) < db(before) + 3.0, "avant {:.2} dB, juste après {:.2} dB", db(before), db(after));
    assert!(db(after) < -8.0 - 6.0, "toujours comprimé après le changement : {:.2} dB", db(after));
}
