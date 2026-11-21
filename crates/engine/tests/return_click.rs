//! Clic du métronome dans les retours : chaque retour reçoit le clic à son propre niveau, indépendamment de la sortie principale
//! (métronome éteint ou non), à l'échantillon exact, sans allocation.

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};
use daw_engine::{Click, ClickTrack, Command, CountIn, Engine, EngineHandle, GraphDesc, PhysOut};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

const SR: u32 = 48_000;

/// Clic normal `[1, 0,5, 0,25]`, accentué `[2, 1]`.
fn click_track(clicks: &[(u64, bool)]) -> ClickTrack {
    ClickTrack { clicks: clicks.iter().map(|&(pos, accent)| Click { pos, accent }).collect(), normal: vec![1.0, 0.5, 0.25], accent: vec![2.0, 1.0] }
}

const CLICKS: [(u64, bool); 4] = [(100, true), (480, false), (481, false), (1_439, true)];

/// Nœud 1 = retour R sur les sorties 3-4, nœud 2 = retour S sur 5-6 (stéréo), nœud 3 = un bus ordinaire.
fn graph(click_r: f32, click_s: f32) -> GraphDesc {
    let mut g = GraphDesc::new();
    let r = g.add_return("R");
    let s = g.add_return("S");
    let _bus = g.add_bus("B");
    g.nodes[r.0 as usize].phys = Some(PhysOut { first: 2, stereo: true, mono: false });
    g.nodes[s.0 as usize].phys = Some(PhysOut { first: 4, stereo: true, mono: false });
    g.nodes[r.0 as usize].click = click_r;
    g.nodes[s.0 as usize].click = click_s;
    g
}

fn setup(g: &GraphDesc, metronome: bool) -> (Engine, EngineHandle) {
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(g).unwrap();
    handle.set_click_track(click_track(&CLICKS)).unwrap();
    handle.send(Command::SetMetronome { enabled: metronome, gain: 1.0 });
    handle.send(Command::Play);
    let mut z = [[0.0f32; 8]; 6];
    let mut parts: Vec<&mut [f32]> = z.iter_mut().map(|c| &mut c[..]).collect();
    engine.process_outputs(&[], &mut parts); // prend en compte les messages
    handle.send(Command::Seek(0));
    (engine, handle)
}

/// Rend `frames` échantillons par appels de `block` ; rend les six canaux (principale G/D, retour R G/D, retour S G/D).
fn render(engine: &mut Engine, frames: usize, block: usize) -> [Vec<f32>; 6] {
    let mut outs: [Vec<f32>; 6] = std::array::from_fn(|_| vec![0.0; frames]);
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        let mut parts: Vec<&mut [f32]> = outs.iter_mut().map(|c| &mut c[done..done + n]).collect();
        engine.process_outputs(&[], &mut parts);
        done += n;
    }
    outs
}

fn expected(gain: f32, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    for &(pos, accent) in &CLICKS {
        let wave: &[f32] = if accent { &[2.0, 1.0] } else { &[1.0, 0.5, 0.25] };
        for (k, s) in wave.iter().enumerate() {
            let at = pos as usize + k;
            if at < frames {
                out[at] += s * gain;
            }
        }
    }
    out
}

#[test]
fn each_return_hears_the_click_at_its_own_level_even_when_the_main_click_is_off() {
    let _g = SERIAL.lock().unwrap();
    for block in [1usize, 7, 64, 333] {
        let (mut engine, _h) = setup(&graph(0.5, 0.25), false);
        let o = render(&mut engine, 2_000, block);
        assert!(o[0].iter().chain(&o[1]).all(|&x| x == 0.0), "la sortie principale reste muette (métronome éteint), blocs de {block}");
        assert_eq!(o[2], expected(0.5, 2_000), "retour R, blocs de {block}");
        assert_eq!(o[3], expected(0.5, 2_000), "R : mêmes échantillons à gauche et à droite");
        assert_eq!(o[4], expected(0.25, 2_000), "retour S, blocs de {block}");
    }
}

#[test]
fn the_main_click_and_the_return_clicks_are_independent() {
    let _g = SERIAL.lock().unwrap();
    // Métronome allumé à 0,8 ; R sans clic ; S à 1.
    let (mut engine, mut h) = setup(&graph(0.0, 1.0), true);
    h.send(Command::SetMetronome { enabled: true, gain: 0.8 });
    let o = render(&mut engine, 2_000, 64);
    assert_eq!(o[0], expected(0.8, 2_000), "sortie principale");
    assert!(o[2].iter().all(|&x| x == 0.0), "R n'a pas demandé le clic");
    assert_eq!(o[4], expected(1.0, 2_000), "S : clic entier, quel que soit le volume de la sortie principale");
}

#[test]
fn a_level_set_live_and_a_non_return_target_are_handled() {
    let _g = SERIAL.lock().unwrap();
    let (mut engine, mut h) = setup(&graph(0.0, 0.0), false);
    let o = render(&mut engine, 600, 64);
    assert!(o.iter().all(|c| c.iter().all(|&x| x == 0.0)), "aucun clic nulle part");
    h.send(Command::SetReturnClick { node: 1, gain: 0.5 });
    h.send(Command::SetReturnClick { node: 3, gain: 1.0 }); // un bus ordinaire : ignoré
    h.send(Command::SetReturnClick { node: 0, gain: 1.0 }); // la sortie principale : ignorée
    let o = render(&mut engine, 2_000, 64);
    let e = expected(0.5, 2_600);
    assert_eq!(o[2], e[600..2_600].to_vec(), "R reçoit le clic dès qu'on le règle");
    assert!(o[0].iter().all(|&x| x == 0.0));
    h.send(Command::SetReturnClick { node: 1, gain: 0.0 });
    let o = render(&mut engine, 2_000, 64);
    assert!(o[2].iter().all(|&x| x == 0.0), "remis à zéro : plus de clic");
}

#[test]
fn the_count_in_is_heard_in_the_returns_that_want_the_click() {
    let _g = SERIAL.lock().unwrap();
    let (mut engine, mut h) = Engine::new(SR, 64);
    h.load_graph(&graph(1.0, 0.0)).unwrap();
    h.set_click_track(click_track(&[])).unwrap();
    h.send(Command::SetMetronome { enabled: false, gain: 1.0 });
    let mut z = [[0.0f32; 8]; 6];
    let mut parts: Vec<&mut [f32]> = z.iter_mut().map(|c| &mut c[..]).collect();
    engine.process_outputs(&[], &mut parts);
    // Pré-compte de 1 000 échantillons avec deux clics (le métronome étant éteint).
    h.start_count_in(CountIn { clicks: vec![(100, true), (600, false)], frames: 1_000 }).unwrap();
    let o = render(&mut engine, 1_200, 64);
    let mut e = vec![0.0f32; 1_200];
    for (k, s) in [2.0f32, 1.0].iter().enumerate() {
        e[100 + k] += s;
    }
    for (k, s) in [1.0f32, 0.5, 0.25].iter().enumerate() {
        e[600 + k] += s;
    }
    assert_eq!(o[2], e, "R entend le pré-compte");
    assert_eq!(o[0], e, "la sortie principale aussi, comme avant");
    assert!(o[4].iter().all(|&x| x == 0.0), "S n'a pas demandé le clic");
}

#[test]
fn the_return_click_does_not_allocate_in_the_callback() {
    let _g = SERIAL.lock().unwrap();
    let (mut engine, mut h) = setup(&graph(0.7, 0.3), false);
    let mut bufs = [[0.0f32; 64]; 6];
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..400 {
            if i == 100 {
                h.send(Command::SetReturnClick { node: 1, gain: 0.2 });
            }
            if i == 200 {
                h.send(Command::Seek(0));
            }
            let [a, b, c, d, e, f] = &mut bufs;
            let mut parts: [&mut [f32]; 6] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..], &mut e[..], &mut f[..]];
            engine.process_outputs(&[], &mut parts);
        }
    }
    assert_eq!(violations(), 0, "allocation dans le callback");
}
