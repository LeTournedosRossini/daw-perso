//! Polarité inversée : une piste identique à une autre mais inversée s'annule exactement contre elle.

use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, TrackFormat};

const SR: u32 = 48_000;

fn noise(seed: u32, len: usize) -> Vec<f32> {
    let mut s = seed;
    (0..len)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 16_777_216.0 - 0.5
        })
        .collect()
}

/// Deux pistes qui jouent le même signal ; la seconde est inversée si `invert`.
fn pair(invert: bool) -> (Engine, daw_engine::EngineHandle, Vec<f32>, u32) {
    let signal = noise(3, 4_000);
    let data = AudioData::new(SR, vec![signal.clone(), signal.clone()]).unwrap();
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    let b = g.add_track("B", TrackFormat::Stereo);
    g.add_clip(a, Clip::whole(data.clone(), 0));
    g.add_clip(b, Clip::whole(data, 0));
    g.nodes[b.0 as usize].params.invert = invert;
    let (engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    (engine, handle, signal, b.0)
}

fn render(engine: &mut Engine, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    engine.process(&mut l, &mut r);
    (l, r)
}

#[test]
fn an_inverted_copy_cancels_exactly_against_the_original() {
    let (mut engine, _h, signal, _) = pair(true);
    let (l, r) = render(&mut engine, 2_000);
    assert!(l.iter().chain(&r).all(|&v| v == 0.0), "annulation exacte, échantillon pour échantillon");
    // Sans inversion, les deux pistes s'additionnent : le signal n'est pas nul (témoin).
    let (mut engine, _h, _, _) = pair(false);
    let (l, _) = render(&mut engine, 2_000);
    assert!(l.iter().any(|&v| v.abs() > 0.1));
    assert!(signal.iter().any(|&v| v != 0.0));
}

#[test]
fn toggling_the_polarity_while_playing_ramps_without_a_jump_then_inverts_exactly() {
    // Deux pistes qui jouent la même valeur constante : la somme vaut 0,8 ; piste B inversée, elle tombe à 0 par une rampe.
    let data = AudioData::new(SR, vec![vec![0.4f32; 6_000], vec![0.4f32; 6_000]]).unwrap();
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    let b = g.add_track("B", TrackFormat::Stereo);
    g.add_clip(a, Clip::whole(data.clone(), 0));
    g.add_clip(b, Clip::whole(data, 0));
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let (before, _) = render(&mut engine, 64);
    assert!(before.iter().all(|&v| (v - 0.8).abs() < 1e-6), "deux pistes identiques : 0,4 + 0,4");
    handle.send(Command::SetInvert { node: b.0, invert: true });
    let (l, _) = render(&mut engine, 1_000);
    // Rampe de 5 ms (240 échantillons à 48 kHz) : de 0,8 à 0 par petits pas réguliers, sans saut ni dépassement.
    let max_step = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(max_step < 0.8 / 200.0, "pas de saut dans la rampe : plus grand pas {max_step}");
    assert!(l.windows(2).all(|w| w[1] <= w[0] + 1e-6) && l.iter().all(|&v| v >= -1e-6), "descente monotone, sans dépassement");
    assert!(l[400..].iter().all(|&v| v == 0.0), "annulation exacte après la rampe");
    // Retour : remonte à 0,8.
    handle.send(Command::SetInvert { node: b.0, invert: false });
    let (back, _) = render(&mut engine, 1_000);
    assert!(back[500..].iter().all(|&v| (v - 0.8).abs() < 1e-6), "retour à l'addition");
}
