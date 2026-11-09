//! Retours de monitoring : un bus qui a une sortie physique sort sur des canaux de la carte son et n'alimente pas la sortie principale.

use std::sync::Arc;

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};
use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip, Command, Engine, GraphDesc, GraphError, PhysOut, TrackFormat};

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

const SR: u32 = 48_000;

/// Une piste qui joue (gauche 0,2 ; droite 0,6) vers la sortie principale, et envoie la moitié de son niveau au bus de retour.
fn graph(phys: Option<PhysOut>) -> (GraphDesc, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let bus = g.add_bus("retour");
    g.add_send(t, bus, 0.5);
    g.nodes[bus.0 as usize].phys = phys;
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![vec![0.2; 4_000], vec![0.6; 4_000]]).unwrap(), 0));
    (g, bus)
}

fn run(g: &GraphDesc, channels: usize, frames: usize, block: usize) -> Vec<Vec<f32>> {
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(g).unwrap();
    handle.send(Command::Play);
    let mut outs: Vec<Vec<f32>> = vec![vec![9.0; frames]; channels]; // 9,0 : un canal que personne n'écrit doit être mis à zéro
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        let mut parts: Vec<&mut [f32]> = outs.iter_mut().map(|c| &mut c[done..done + n]).collect();
        engine.process_outputs(&[], &mut parts);
        done += n;
    }
    outs
}

#[test]
fn a_stereo_return_gets_the_send_and_does_not_feed_the_main_output() {
    let (g, _) = graph(Some(PhysOut { first: 2, stereo: true, mono: false }));
    let o = run(&g, 4, 2_000, 64);
    // Sortie principale : seulement la piste (le bus de retour ne s'y ajoute pas).
    assert!((o[0][1_000] - 0.2).abs() < 1e-6 && (o[1][1_000] - 0.6).abs() < 1e-6, "principale : {} {}", o[0][1_000], o[1][1_000]);
    // Retour : la moitié de la piste, gauche et droite séparées.
    assert!((o[2][1_000] - 0.1).abs() < 1e-6 && (o[3][1_000] - 0.3).abs() < 1e-6, "retour : {} {}", o[2][1_000], o[3][1_000]);
    // Sans sortie physique (comportement d'avant) : le bus alimente la principale, rien sur les canaux supplémentaires.
    let (g, _) = graph(None);
    let o = run(&g, 4, 2_000, 64);
    assert!((o[0][1_000] - 0.3).abs() < 1e-6 && (o[1][1_000] - 0.9).abs() < 1e-6, "sans sortie physique : la somme");
    assert!(o[2].iter().chain(&o[3]).all(|&v| v == 0.0), "canaux sans bus : silence");
}

#[test]
fn mono_sum_goes_to_both_channels_or_to_a_single_one() {
    let (g, _) = graph(Some(PhysOut { first: 2, stereo: true, mono: true }));
    let o = run(&g, 4, 1_000, 64);
    assert!((o[2][500] - 0.2).abs() < 1e-6 && (o[3][500] - 0.2).abs() < 1e-6, "(0,1 + 0,3) / 2 sur les deux canaux");
    let (g, _) = graph(Some(PhysOut { first: 3, stereo: false, mono: false }));
    let o = run(&g, 4, 1_000, 64);
    assert!(o[2].iter().all(|&v| v == 0.0), "canal 2 non pris");
    assert!((o[3][500] - 0.2).abs() < 1e-6, "un canal seul : toujours la somme mono");
}

#[test]
fn two_returns_are_independent_and_the_result_does_not_depend_on_the_block_size() {
    let mut g = GraphDesc::new();
    let t = g.add_track("t", TrackFormat::Stereo);
    let a = g.add_bus("A");
    let b = g.add_bus("B");
    g.add_send(t, a, 1.0);
    g.add_send(t, b, 0.25);
    g.nodes[a.0 as usize].phys = Some(PhysOut { first: 2, stereo: true, mono: false });
    g.nodes[b.0 as usize].phys = Some(PhysOut { first: 4, stereo: true, mono: true });
    g.add_clip(t, Clip::whole(AudioData::new(SR, vec![vec![0.2; 3_000], vec![0.6; 3_000]]).unwrap(), 0));
    let reference = run(&g, 6, 3_000, 64);
    assert!((reference[2][1_000] - 0.2).abs() < 1e-6 && (reference[3][1_000] - 0.6).abs() < 1e-6);
    assert!((reference[4][1_000] - 0.1).abs() < 1e-6 && (reference[5][1_000] - 0.1).abs() < 1e-6, "B : un quart de 0,4 en somme mono");
    for block in [1, 7, 100, 333, 4_096] {
        assert_eq!(run(&g, 6, 3_000, block), reference, "bloc de {block}");
    }
    // Un appel avec seulement la paire principale ne casse rien (les retours sont simplement perdus).
    let two = run(&g, 2, 3_000, 64);
    assert_eq!(two[0], reference[0]);
}

#[test]
fn invalid_physical_outputs_are_refused() {
    let ok = PhysOut { first: 2, stereo: true, mono: false };
    let (mut g, bus) = graph(Some(ok));
    assert!(g.validate().is_ok());
    for bad in [PhysOut { first: 0, stereo: true, mono: false }, PhysOut { first: 1, stereo: false, mono: true }, PhysOut { first: 31, stereo: true, mono: false }, PhysOut { first: 40, stereo: false, mono: true }] {
        g.nodes[bus.0 as usize].phys = Some(bad);
        assert_eq!(g.validate().err(), Some(GraphError::BadPhysOut(bus)), "{bad:?}");
    }
    // Une piste ne sort pas sur la carte son (seulement un bus) ; deux bus ne partagent pas un canal.
    g.nodes[bus.0 as usize].phys = Some(ok);
    g.nodes[1].phys = Some(PhysOut { first: 6, stereo: false, mono: true });
    assert_eq!(g.validate().err(), Some(GraphError::BadPhysOut(NodeId(1))));
    g.nodes[1].phys = None;
    let other = g.add_bus("autre");
    g.nodes[other.0 as usize].phys = Some(PhysOut { first: 3, stereo: false, mono: true });
    assert!(matches!(g.validate().err(), Some(GraphError::BadPhysOut(_))), "le canal 3 est déjà pris par la paire 2-3");
    g.nodes[other.0 as usize].phys = Some(PhysOut { first: 4, stereo: false, mono: true });
    assert!(g.validate().is_ok());
}

#[test]
fn a_return_does_not_add_its_latency_to_the_main_output() {
    // Sans sortie physique, la latence du bus (plugin) compte pour la sortie principale ; avec, il n'y a plus de lien.
    let (g, _) = graph(None);
    assert_eq!(g.compile(SR, 64).unwrap().latency(), 0);
    let (g2, bus) = graph(Some(PhysOut { first: 2, stereo: true, mono: false }));
    assert_eq!(g2.compile(SR, 64).unwrap().extra_outputs(), 2, "deux canaux supplémentaires (2 et 3)");
    assert_eq!(g2.nodes[bus.0 as usize].main_output(), None);
}

#[test]
fn no_allocation_with_returns_across_changes() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (g, bus) = graph(Some(PhysOut { first: 2, stereo: true, mono: false }));
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::Play);
    let mut bufs = [[0.0f32; 64]; 4];
    let [a, b, c, d] = &mut bufs;
    let mut g2 = g.clone();
    g2.nodes[bus.0 as usize].phys = Some(PhysOut { first: 2, stereo: true, mono: true });
    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..300 {
            if i == 100 {
                handle.send(Command::Seek(10));
            }
            let mut parts: [&mut [f32]; 4] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..]];
            engine.process_outputs(&[], &mut parts);
        }
    }
    assert_eq!(violations(), 0, "allocation dans process_outputs()");
    // Le changement de graphe (somme mono) se fait aussi sans allocation dans le callback.
    handle.load_graph(&g2).unwrap();
    reset_violations();
    {
        let _rt = RtScope::enter();
        for _ in 0..50 {
            let mut parts: [&mut [f32]; 4] = [&mut a[..], &mut b[..], &mut c[..], &mut d[..]];
            engine.process_outputs(&[], &mut parts);
        }
    }
    assert_eq!(violations(), 0, "allocation après l'échange de graphe");
    let _ = Arc::new(0);
}

/// Retard fixe de 100 échantillons (le plugin lent d'une autre piste).
struct Slow(Vec<f32>, usize);
impl daw_engine::Insert for Slow {
    fn latency(&self) -> u32 {
        self.0.len() as u32
    }
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        for j in 0..in_l.len() {
            out_l[j] = self.0[self.1];
            self.0[self.1] = in_l[j];
            self.1 = (self.1 + 1) % self.0.len();
            out_r[j] = in_r[j];
        }
    }
    fn set_param(&mut self, _: u32, _: f64) {}
    fn reset(&mut self) {}
}

#[test]
fn a_musician_hears_himself_in_his_return_without_the_latency_of_other_tracks() {
    let mut g = GraphDesc::new();
    let live = g.add_track("musicien", TrackFormat::Mono);
    let slow = g.add_track("piste lente", TrackFormat::Stereo);
    let bus = g.add_bus("retour");
    g.add_send(live, bus, 1.0);
    g.add_send(slow, bus, 1.0);
    g.nodes[live.0 as usize].sends[0].pre = true;
    g.nodes[bus.0 as usize].phys = Some(PhysOut { first: 2, stereo: true, mono: false });
    g.add_insert(slow, "lent", Arc::new(|_, _, _| Ok(Box::new(Slow(vec![0.0; 100], 0)) as Box<dyn daw_engine::Insert>))).unwrap();
    let (mut engine, mut handle) = Engine::new(SR, 64);
    handle.load_graph(&g).unwrap();
    handle.send(Command::SetInput { node: live.0, armed: true, monitor: true, source: daw_engine::RecordSource::Channel(0) });
    handle.send(Command::Play);
    // Une impulsion à l'échantillon 10 sur l'entrée 1 ; la piste lente n'a pas de clip.
    let mut input = vec![0.0f32; 400];
    input[10] = 1.0;
    let mut outs = vec![vec![0.0f32; 400]; 4];
    let mut done = 0;
    while done < 400 {
        let n = 64.min(400 - done);
        let mut parts: Vec<&mut [f32]> = outs.iter_mut().map(|c| &mut c[done..done + n]).collect();
        engine.process_outputs(&[&input[done..done + n]], &mut parts);
        done += n;
    }
    let peak = outs[2].iter().position(|&v| v > 0.5).expect("l'impulsion arrive dans le retour");
    assert_eq!(peak, 10, "l'écoute directe n'est pas retardée par la compensation (la piste lente a 100 échantillons)");
}
