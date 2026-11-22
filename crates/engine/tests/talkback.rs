//! Talkback : la voix de la régie (un canal d'entrée) passe dans les retours visés, le reste de leur mix baisse pendant qu'on parle.

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};
use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, Command, Engine, EngineHandle, GraphDesc, PhysOut, TrackFormat};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

const SR: u32 = 48_000;
const VOICE: f32 = 0.2;

/// Piste qui joue 0,5 constant, envoyée aux retours R (nœud 2, sorties 3-4) et S (nœud 3, sorties 5-6) ; un bus ordinaire en nœud 4.
fn graph() -> (GraphDesc, NodeId, NodeId) {
    let data = AudioData::new(SR, vec![vec![0.5f32; 200_000], vec![0.5f32; 200_000]]).unwrap();
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let r = g.add_return("R");
    let s = g.add_return("S");
    let bus = g.add_bus("B");
    g.add_clip(t, Clip::whole(data, 0));
    g.nodes[r.0 as usize].phys = Some(PhysOut { first: 2, stereo: true, mono: false });
    g.nodes[s.0 as usize].phys = Some(PhysOut { first: 4, stereo: true, mono: false });
    g.add_send(t, r, 1.0);
    g.add_send(t, s, 1.0);
    let _ = bus;
    (g, r, s)
}

fn setup(g: &GraphDesc) -> (Engine, EngineHandle) {
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(g).unwrap();
    handle.send(Command::Play);
    let mut z = [[0.0f32; 8]; 6];
    let mut parts: Vec<&mut [f32]> = z.iter_mut().map(|c| &mut c[..]).collect();
    engine.process_outputs(&[], &mut parts);
    (engine, handle)
}

/// Rend `frames` échantillons ; l'entrée a deux canaux, le canal 1 porte la voix. Rend les canaux de sortie 2 à 5 (R gauche/droite, S gauche/droite).
fn render(engine: &mut Engine, frames: usize, block: usize) -> [Vec<f32>; 4] {
    let mut outs: [Vec<f32>; 6] = std::array::from_fn(|_| vec![0.0; frames]);
    let mic = vec![VOICE; frames];
    let silence = vec![0.0f32; frames];
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        let mut parts: Vec<&mut [f32]> = outs.iter_mut().map(|c| &mut c[done..done + n]).collect();
        engine.process_outputs(&[&silence[done..done + n], &mic[done..done + n]], &mut parts);
        done += n;
    }
    let [_, _, a, b, c, d] = outs;
    [a, b, c, d]
}

fn nodes_of(g: &GraphDesc) -> (u32, u32) {
    let (_, r, s) = (0, g.nodes.iter().position(|n| n.name == "R").unwrap() as u32, g.nodes.iter().position(|n| n.name == "S").unwrap() as u32);
    (r, s)
}

#[test]
fn the_voice_reaches_only_the_targeted_return_and_ducks_its_mix() {
    let _g = SERIAL.lock().unwrap();
    let (g, _, _) = graph();
    let (r, _) = nodes_of(&g);
    let (mut engine, mut h) = setup(&g);
    let o = render(&mut engine, 2_000, 64);
    assert!((o[0][1_999] - 0.5).abs() < 1e-6 && (o[2][1_999] - 0.5).abs() < 1e-6, "sans parole : le mix seul");
    h.send(Command::SetReturnTalk { node: r, gain: 1.0 });
    h.send(Command::SetTalkback { channel: 1, active: true, duck: 0.5 });
    let o = render(&mut engine, 2_000, 64);
    let end = o[0][1_999];
    assert!((end - (0.5 * 0.5 + VOICE)).abs() < 1e-5, "R : mix baissé de moitié + voix = {end}");
    assert_eq!(o[0], o[1], "gauche = droite");
    assert!((o[2][1_999] - 0.5).abs() < 1e-6, "S n'est pas visé : inchangé");
    h.send(Command::SetTalkback { channel: 1, active: false, duck: 0.5 });
    let o = render(&mut engine, 2_000, 64);
    assert!((o[0][1_999] - 0.5).abs() < 1e-6, "la parole s'arrête : le mix revient entier");
}

#[test]
fn the_ramp_is_smooth_and_identical_for_any_block_size() {
    let _g = SERIAL.lock().unwrap();
    let (g, _, _) = graph();
    let (r, _) = nodes_of(&g);
    let mut reference: Option<Vec<f32>> = None;
    for block in [1usize, 7, 64, 333] {
        let (mut engine, mut h) = setup(&g);
        h.send(Command::SetReturnTalk { node: r, gain: 1.0 });
        let before = render(&mut engine, 500, block);
        h.send(Command::SetTalkback { channel: 1, active: true, duck: 0.5 });
        let o = render(&mut engine, 1_000, block);
        let mut all = before[0].clone();
        all.extend_from_slice(&o[0]);
        let step = all.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(step < 0.01, "pas de saut brutal (blocs de {block}) : {step}");
        match &reference {
            None => reference = Some(all),
            Some(r0) => assert_eq!(r0, &all, "identique pour des blocs de {block}"),
        }
    }
}

#[test]
fn bad_targets_and_missing_microphone_are_ignored() {
    let _g = SERIAL.lock().unwrap();
    let (g, _, _) = graph();
    let (mut engine, mut h) = setup(&g);
    h.send(Command::SetReturnTalk { node: 1, gain: 1.0 }); // une piste : ignorée
    h.send(Command::SetReturnTalk { node: 4, gain: 1.0 }); // un bus ordinaire : ignoré
    h.send(Command::SetReturnTalk { node: 0, gain: 1.0 }); // la sortie principale : ignorée
    h.send(Command::SetTalkback { channel: 1, active: true, duck: 0.5 });
    let o = render(&mut engine, 1_000, 64);
    assert!((o[0][999] - 0.5).abs() < 1e-6 && (o[2][999] - 0.5).abs() < 1e-6, "rien n'a changé");
    // Canal d'entrée qui n'existe pas : silence, pas de panique ; le mix baisse quand même (retour visé).
    let (r, _) = nodes_of(&g);
    h.send(Command::SetReturnTalk { node: r, gain: 1.0 });
    h.send(Command::SetTalkback { channel: 9, active: true, duck: 0.5 });
    let o = render(&mut engine, 1_000, 64);
    assert!((o[0][999] - 0.25).abs() < 1e-5, "voix absente : seulement la baisse du mix");
    h.send(Command::SetTalkback { channel: u32::MAX, active: true, duck: 0.5 });
    let o = render(&mut engine, 1_000, 64);
    assert!((o[0][999] - 0.5).abs() < 1e-6, "sans micro : plus rien");
}

#[test]
fn the_talkback_survives_a_graph_reload_and_does_not_allocate() {
    let _g = SERIAL.lock().unwrap();
    let (g, _, _) = graph();
    let (r, _) = nodes_of(&g);
    let (mut engine, mut h) = setup(&g);
    h.send(Command::SetReturnTalk { node: r, gain: 1.0 });
    h.send(Command::SetTalkback { channel: 1, active: true, duck: 0.5 });
    let _ = render(&mut engine, 2_000, 64);
    h.load_graph(&g).unwrap();
    let o = render(&mut engine, 2_000, 64);
    assert!((o[0][1_999] - 0.45).abs() < 1e-5, "la parole continue après le rechargement du graphe");
    let mic = vec![VOICE; 64];
    let silence = vec![0.0f32; 64];
    let mut bufs = [[0.0f32; 64]; 6];
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..400 {
            if i == 100 {
                h.send(Command::SetTalkback { channel: 1, active: false, duck: 0.3 });
            }
            if i == 200 {
                h.send(Command::SetTalkback { channel: 1, active: true, duck: 0.8 });
            }
            let [a, b, c, d, e, f] = &mut bufs;
            let mut parts: [&mut [f32]; 6] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..], &mut e[..], &mut f[..]];
            engine.process_outputs(&[&silence, &mic], &mut parts);
        }
    }
    assert_eq!(violations(), 0, "allocation dans le callback");
}
