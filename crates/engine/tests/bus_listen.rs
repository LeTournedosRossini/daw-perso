//! Écoute de contrôle (AFL) d'un bus ou d'un retour : il entre seul dans la sortie principale, par son fader et ses effets, sans rien changer
//! aux sorties physiques des retours.

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};
use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, NodeParams, PhysOut, TrackFormat};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

const SR: u32 = 48_000;

fn clip(l: f32, r: f32) -> Clip {
    Clip::whole(AudioData::new(SR, vec![vec![l; 200_000], vec![r; 200_000]]).unwrap(), 0)
}

/// Nœuds : 1 = A (0,2 / 0,6, envoie la moitié au retour), 2 = B (0,1 vers le sous-groupe), 3 = sous-groupe G, 4 = retour R (sorties 3-4).
fn rig(master_gain: f32, r_mono: bool) -> GraphDesc {
    let mut g = GraphDesc::new();
    let a = g.add_track("A", TrackFormat::Stereo);
    let b = g.add_track("B", TrackFormat::Stereo);
    let grp = g.add_bus("G");
    let r = g.add_return("R");
    g.nodes[r.0 as usize].phys = Some(PhysOut { first: 2, stereo: true, mono: r_mono });
    g.set_output(b, grp);
    g.add_send(a, r, 0.5);
    g.add_clip(a, clip(0.2, 0.6));
    g.add_clip(b, clip(0.1, 0.1));
    g.set_params(daw_core_types::NodeId(0), NodeParams { gain: master_gain, ..NodeParams::default() });
    g
}

/// Joue, applique les commandes, rend `frames` échantillons (par blocs de `block`) ; rend (principale G, principale D, retour G, retour D).
fn run(g: &GraphDesc, commands: &[Command], frames: usize, block: usize) -> [Vec<f32>; 4] {
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(g).unwrap();
    handle.send(Command::Play);
    for c in commands {
        handle.send(*c);
    }
    let mut outs: [Vec<f32>; 4] = std::array::from_fn(|_| vec![0.0; frames]);
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        let mut parts: Vec<&mut [f32]> = outs.iter_mut().map(|c| &mut c[done..done + n]).collect();
        engine.process_outputs(&[], &mut parts);
        done += n;
    }
    outs
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn without_listening_the_mix_is_the_normal_one() {
    let o = run(&rig(1.0, false), &[], 4_000, 64);
    assert!(near(o[0][3_000], 0.3) && near(o[1][3_000], 0.7), "principale : {} {}", o[0][3_000], o[1][3_000]);
    assert!(near(o[2][3_000], 0.1) && near(o[3][3_000], 0.3), "retour : {} {}", o[2][3_000], o[3][3_000]);
}

#[test]
fn listening_to_a_return_puts_it_alone_in_the_main_output_without_touching_its_physical_outputs() {
    let o = run(&rig(1.0, false), &[Command::SetListen { node: 4 }], 4_000, 64);
    assert!(near(o[0][3_000], 0.1) && near(o[1][3_000], 0.3), "la sortie principale ne porte que le retour : {} {}", o[0][3_000], o[1][3_000]);
    assert!(near(o[2][3_000], 0.1) && near(o[3][3_000], 0.3), "le musicien entend la même chose : {} {}", o[2][3_000], o[3][3_000]);
}

#[test]
fn a_mono_return_is_heard_as_the_musician_hears_it() {
    let o = run(&rig(1.0, true), &[Command::SetListen { node: 4 }], 4_000, 64);
    assert!(near(o[0][3_000], 0.2) && near(o[1][3_000], 0.2), "somme mono sur les deux côtés : {} {}", o[0][3_000], o[1][3_000]);
}

#[test]
fn listening_to_an_ordinary_bus_hears_what_leaves_it() {
    let o = run(&rig(1.0, false), &[Command::SetListen { node: 3 }], 4_000, 64);
    assert!(near(o[0][3_000], 0.1) && near(o[1][3_000], 0.1), "le sous-groupe seul : {} {}", o[0][3_000], o[1][3_000]);
}

#[test]
fn the_listened_signal_goes_through_the_main_fader() {
    let o = run(&rig(0.5, false), &[Command::SetListen { node: 4 }], 4_000, 64);
    assert!(near(o[0][3_000], 0.05) && near(o[1][3_000], 0.15), "moitié du niveau : {} {}", o[0][3_000], o[1][3_000]);
}

#[test]
fn ending_the_listening_brings_the_normal_mix_back_and_a_track_cannot_be_listened_to() {
    let o = run(&rig(1.0, false), &[Command::SetListen { node: 4 }, Command::SetListen { node: u32::MAX }], 4_000, 64);
    assert!(near(o[0][3_000], 0.3) && near(o[1][3_000], 0.7));
    let o = run(&rig(1.0, false), &[Command::SetListen { node: 1 }], 4_000, 64);
    assert!(near(o[0][3_000], 0.3), "une piste ne s'écoute pas par ce moyen : {}", o[0][3_000]);
    let o = run(&rig(1.0, false), &[Command::SetListen { node: 0 }, Command::SetListen { node: 99 }], 4_000, 64);
    assert!(near(o[0][3_000], 0.3), "la sortie principale et un numéro inconnu sont ignorés");
}

#[test]
fn switching_the_listening_from_one_bus_to_another() {
    let o = run(&rig(1.0, false), &[Command::SetListen { node: 4 }, Command::SetListen { node: 3 }], 4_000, 64);
    assert!(near(o[0][3_000], 0.1) && near(o[1][3_000], 0.1), "le dernier choisi : {} {}", o[0][3_000], o[1][3_000]);
}

#[test]
fn the_entry_and_exit_are_ramped_and_the_result_does_not_depend_on_the_block_size() {
    let g = rig(1.0, false);
    let reference = run(&g, &[Command::SetListen { node: 4 }], 3_000, 1);
    // Entrée en rampe : le premier échantillon n'est pas déjà au niveau de l'écoute, ni le dernier de la rampe encore au mixage normal.
    assert!(reference[0][0] > 0.1 && reference[0][0] < 0.3, "premier échantillon : {}", reference[0][0]);
    assert!(near(reference[0][2_999], 0.1));
    for block in [7usize, 64, 100, 333] {
        let o = run(&g, &[Command::SetListen { node: 4 }], 3_000, block);
        assert_eq!(o[0], reference[0], "bloc {block}");
        assert_eq!(o[1], reference[1], "bloc {block}");
    }
}

#[test]
fn listening_does_not_allocate_in_the_callback() {
    let _guard = SERIAL.lock().unwrap();
    let g = rig(1.0, true);
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let mut bufs = [[0.0f32; 64]; 4];
    let [a, b, c, d] = &mut bufs;
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..300 {
            if i == 50 {
                handle.send(Command::SetListen { node: 4 });
            }
            if i == 150 {
                handle.send(Command::SetListen { node: 3 });
            }
            if i == 250 {
                handle.send(Command::SetListen { node: u32::MAX });
            }
            let mut parts: [&mut [f32]; 4] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..]];
            engine.process_outputs(&[], &mut parts);
        }
    }
    assert_eq!(violations(), 0, "allocation dans le callback");
    // Rechargement du graphe pendant l'écoute : l'écoute survit.
    handle.send(Command::SetListen { node: 4 });
    for _ in 0..100 {
        let mut parts: [&mut [f32]; 4] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..]];
        engine.process_outputs(&[], &mut parts);
    }
    handle.load_graph(&g).unwrap();
    reset_violations();
    {
        let _rt = RtScope::enter();
        for _ in 0..100 {
            let mut parts: [&mut [f32]; 4] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..]];
            engine.process_outputs(&[], &mut parts);
        }
    }
    assert_eq!(violations(), 0, "allocation après l'échange de graphe");
    assert!(near(a[63], 0.2), "l'écoute survit au rechargement du graphe : {}", a[63]);
}
