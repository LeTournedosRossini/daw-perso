//! Commande `return-latency-test` : quelle latence entend un musicien dans **son retour** ?
//!
//! Deux câbles sur la carte : sortie 1 → entrée A (le « instrument », ici un bruit de référence joué par une piste) et sortie du retour
//! (3 par défaut) → entrée B. Une piste armée et écoutée reçoit l'entrée A, comme le ferait un instrument branché ; elle envoie son signal au
//! retour, qui sort sur la sortie 3. On enregistre l'entrée A et l'entrée B et on mesure leur retard sur la référence (corrélation croisée,
//! comme `sync-test`) :
//!
//!   retard A = sortie 1 → câble → entrée A            = délai de sortie + délai d'entrée de la carte (un « aller-retour » de carte)
//!   retard B = sortie 1 → entrée A → moteur → sortie 3 → câble → entrée B = retard A + (entrée A → moteur → sortie 3 → entrée B)
//!   latence du retour = retard B − retard A = délai d'entrée + passage dans le moteur + délai de sortie
//!
//! C'est exactement ce que vit le musicien (son instrument entre, traverse le moteur, ressort dans son casque). On la compare à l'attendu
//! (latences annoncées par le pilote + convertisseurs) : l'excédent est ce que **notre moteur** ajoute. Avec `--fx NOM`, une autre piste
//! passe par un plugin à latence (Digue : 480 échantillons) et envoie au même retour : l'écoute directe ne doit pas en être retardée.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use daw_audio_io::playback::{record_compensation, ControlFn, StreamInfo};
use daw_clap_host::clap_insert_factory;
use daw_core_types::NodeId;
use daw_engine::{AheadConfig, AudioData, Clip, Command, Engine, GraphDesc, InsertFactory, NodeParams, PhysOut, RecordSource, TrackFormat};
use daw_recorder::wav::read_wav;
use daw_recorder::Recording;

use crate::demo::EngineProc;
use crate::load;
use crate::sync_test::{analyze_sync, reference_signal, SyncReport};

#[derive(Debug, Clone)]
pub struct ReturnOptions {
    pub seconds: u32,
    /// Préfixe des deux fichiers de prise (`<préfixe>-entree.wav`, `<préfixe>-retour.wav`).
    pub prefix: PathBuf,
    /// Entrée A (1 = la première) : celle où revient la sortie 1.
    pub in_channel: u32,
    /// Sortie du retour (à partir de 3) et entrée B où elle revient.
    pub ret_out: u32,
    pub ret_in: u32,
    /// Plugin de Jean sur une autre piste qui envoie au même retour (sa latence ne doit pas retarder l'écoute directe).
    pub fx: Option<String>,
    pub ahead: Option<(u32, usize)>,
    pub threads: usize,
    pub window: u32,
}

/// Ce que contient le graphe de test.
pub struct Built {
    pub desc: GraphDesc,
    pub musician: NodeId,
    pub tap_in: NodeId,
    pub tap_ret: NodeId,
}

/// Le graphe de test. `fx` : (nom, usine) d'un plugin à latence placé sur une autre piste qui envoie au même retour.
pub fn build_graph(rate: u32, seconds: u32, ret_out: u32, fx: Option<(String, InsertFactory)>) -> Built {
    let mut g = GraphDesc::new();
    let musician = g.add_track("musicien", TrackFormat::Stereo);
    let tap_in = g.add_track("entrée", TrackFormat::Stereo);
    let tap_ret = g.add_track("sortie du retour", TrackFormat::Stereo);
    let reference = g.add_track("référence", TrackFormat::Stereo);
    let ret = g.add_bus("retour");
    let trash = g.add_bus("poubelle");
    g.node_mut(ret).expect("retour").phys = Some(PhysOut { first: (ret_out.max(3) - 1).min(255) as u8, stereo: false, mono: true });
    g.set_params(trash, NodeParams { mute: true, ..NodeParams::default() });
    let x = reference_signal(rate, seconds);
    g.add_clip(reference, Clip::whole(AudioData::new(rate, vec![x.clone(), x]).expect("deux canaux de même longueur"), 0));
    for t in [musician, tap_in, tap_ret] {
        g.set_output(t, trash);
    }
    g.add_send(musician, ret, 1.0);
    if let Some((name, factory)) = fx {
        let other = g.add_track("autre piste", TrackFormat::Stereo);
        g.add_insert(other, &name, factory).expect("piste créée");
        g.set_output(other, trash);
        g.add_send(other, ret, 1.0);
    }
    Built { desc: g, musician, tap_in, tap_ret }
}

#[derive(Default)]
pub struct Shared {
    pub rec_in: Option<Recording>,
    pub rec_ret: Option<Recording>,
    pub latency: Option<(u32, u32)>,
    pub graph_latency: u32,
    pub driver: String,
    pub buffer: u32,
    pub inputs: u32,
    pub outputs: u32,
}

pub type Slot = Rc<RefCell<Shared>>;

pub fn make(opts: &ReturnOptions, info: &StreamInfo, slot: &Slot) -> Result<(EngineProc, ControlFn), String> {
    let rate = info.sample_rate;
    if info.inputs < opts.in_channel.max(opts.ret_in) {
        return Err(format!("le pilote n'a ouvert que {} entrée(s) : il en faut au moins {}", info.inputs, opts.in_channel.max(opts.ret_in)));
    }
    if info.outputs < opts.ret_out {
        return Err(format!("le pilote n'a ouvert que {} sortie(s) : le retour demandé est sur la sortie {}", info.outputs, opts.ret_out));
    }
    let fx = match &opts.fx {
        None => None,
        Some(name) => {
            let cat = load::catalog();
            let f = load::find(&cat, name).ok_or_else(|| format!("plugin « {name} » introuvable parmi ceux de Jean ({})", cat.iter().map(|f| f.info.name.as_str()).collect::<Vec<_>>().join(", ")))?;
            Some((f.info.name.clone(), clap_insert_factory(Arc::clone(&f.bundle), f.info.id.clone())))
        }
    };
    let built = build_graph(rate, opts.seconds, opts.ret_out, fx);
    let (mut engine, mut handle) = Engine::new(rate, 256);
    if let Some(p) = load::make_pool(opts.threads) {
        engine.set_pool(p);
    }
    if let Some((ms, n)) = opts.ahead {
        engine.enable_ahead(&mut handle, &AheadConfig { lookahead_ms: ms, block: 256, threads: n.max(1), on_start: Some(Arc::new(|_| { let _ = daw_audio_io::rt_priority::promote_current_thread(); })) });
    }
    handle.load_graph(&built.desc).map_err(|e| format!("graphe refusé : {e:?}"))?;
    let src_in = RecordSource::Channel(opts.in_channel.saturating_sub(1).min(255) as u8);
    let src_ret = RecordSource::Channel(opts.ret_in.saturating_sub(1).min(255) as u8);
    // Le musicien : armé et écouté, il reçoit l'entrée A, l'envoie au retour **et** l'enregistre (prise A).
    handle.send(Command::SetInput { node: built.musician.0, armed: true, monitor: true, source: src_in });
    handle.send(Command::SetInput { node: built.tap_ret.0, armed: true, monitor: false, source: src_ret });
    let rx_in = handle.start_record(built.musician.0, src_in, 4.0).map_err(|_| "file de commandes pleine au départ".to_string())?;
    let rx_ret = handle.start_record(built.tap_ret.0, src_ret, 4.0).map_err(|_| "file de commandes pleine au départ".to_string())?;
    {
        let mut s = slot.borrow_mut();
        s.rec_in = Some(Recording::start(rx_in, with_suffix(&opts.prefix, "entree")).map_err(|e| format!("création du fichier de prise : {e}"))?);
        s.rec_ret = Some(Recording::start(rx_ret, with_suffix(&opts.prefix, "retour")).map_err(|e| format!("création du fichier de prise : {e}"))?);
        s.graph_latency = handle.latency_samples();
        s.driver = info.driver.clone();
        s.buffer = info.buffer_size;
        s.inputs = info.inputs;
        s.outputs = info.outputs;
    }
    handle.send(Command::Play);
    let (slot2, latency, stop_at) = (slot.clone(), Arc::clone(&info.latency), opts.seconds as f64 + 0.4);
    let (tap_a, tap_b) = (built.musician.0, built.tap_ret.0);
    let mut stopped = false;
    let control: ControlFn = Box::new(move |elapsed| {
        handle.collect_garbage();
        if slot2.borrow().latency.is_none() {
            slot2.borrow_mut().latency = latency.get();
        }
        if elapsed.as_secs_f64() >= stop_at && !stopped {
            handle.stop_record(tap_a);
            handle.stop_record(tap_b);
            stopped = true;
        }
        true
    });
    Ok((EngineProc(engine), control))
}

fn with_suffix(prefix: &std::path::Path, suffix: &str) -> PathBuf {
    let mut s = prefix.as_os_str().to_owned();
    s.push(format!("-{suffix}.wav"));
    PathBuf::from(s)
}

/// Les deux mesures et ce qu'on en tire.
#[derive(Debug, Clone, PartialEq)]
pub struct ReturnReport {
    /// Sortie 1 → entrée A.
    pub a: SyncReport,
    /// Sortie 1 → entrée A → moteur → sortie du retour → entrée B.
    pub b: SyncReport,
    /// Latence du retour pour le musicien (échantillons, interpolée) : retard B − retard A.
    pub live: f64,
}

/// Mesure le retard des deux prises sur la référence. `center` : où chercher le retard A (aller-retour de carte attendu, plus la latence
/// de compensation des plugins qui retarde la lecture principale) ; le retard B est cherché autour de A + l'aller-retour de carte.
pub fn analyze_return(take_a: &[f32], start_a: u64, take_b: &[f32], start_b: u64, reference: &[f32], center: i64, card: i64, window: i64, skip: usize) -> Option<ReturnReport> {
    let a = analyze_sync(take_a, reference, start_a, center, window, skip)?;
    let b = analyze_sync(take_b, reference, start_b, a.lag_int + card, window, skip)?;
    let live = b.lag - a.lag;
    Some(ReturnReport { a, b, live })
}

/// Tolérance sur l'écart avec l'attendu (échantillons) : le délai des convertisseurs varie de ±6 d'un lancement à l'autre (mesuré).
/// Seul un retour **plus long** que prévu est un problème : « plus court » veut dire que les convertisseurs de la carte retardent un peu
/// moins que les 72 échantillons supposés (mesuré : −9 échantillons à 16 et 64, soit 0,2 ms), pas que le moteur fait mieux ou pire.
pub const TOLERANCE: f64 = 8.0;
/// Au-delà, un retour plus court que prévu est suspect (compensation mal calculée, mauvais câblage).
pub const TOO_SHORT: f64 = -30.0;

pub fn report(opts: &ReturnOptions, slot: &Slot, rate: u32) -> i32 {
    let (rec_in, rec_ret, latency, graph_latency, driver, buffer, inputs, outputs) = {
        let mut s = slot.borrow_mut();
        (s.rec_in.take(), s.rec_ret.take(), s.latency, s.graph_latency, s.driver.clone(), s.buffer, s.inputs, s.outputs)
    };
    let (Some(rec_in), Some(rec_ret)) = (rec_in, rec_ret) else {
        eprintln!("erreur : les prises n'ont pas démarré");
        return 1;
    };
    let (info_a, info_b) = match (rec_in.finish(), rec_ret.finish()) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("erreur d'écriture d'une prise : {e}");
            return 1;
        }
    };
    let Some((lat_in, lat_out)) = latency else {
        eprintln!("erreur : le pilote n'a pas annoncé ses latences");
        return 1;
    };
    let card = record_compensation(lat_in, lat_out);
    let ms = |s: f64| s * 1000.0 / rate as f64;
    println!("\nPilote : {driver}, {rate} Hz, tampon {buffer}, {inputs} entrée(s), {outputs} sortie(s)");
    println!("Câbles : sortie 1 → entrée {} ; sortie {} (retour) → entrée {}", opts.in_channel, opts.ret_out, opts.ret_in);
    println!("Attendu pour un passage direct (annoncé {lat_in} + {lat_out} + {} convertisseurs) : {card} échantillons ({:.3} ms)", daw_audio_io::playback::CONVERTER_DELAY, ms(card as f64));
    if let Some(name) = &opts.fx {
        println!("Autre piste avec « {name} » (latence {graph_latency} éch.) qui envoie au même retour : elle ne doit PAS retarder l'écoute directe.");
    }
    println!("Prises : {} et {} trames, blocs perdus par le disque : {} / {}", info_a.frames, info_b.frames, info_a.overruns, info_b.overruns);
    let (wa, wb) = match (read_wav(&info_a.path), read_wav(&info_b.path)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("erreur de relecture : {e}");
            return 1;
        }
    };
    let (ta, tb) = (wa.channel(0), wb.channel(0));
    let peak = |t: &[f32]| t.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    println!("Crêtes : entrée {} {:.3} ({:.1} dBFS), entrée {} {:.3} ({:.1} dBFS)", opts.in_channel, peak(&ta), 20.0 * peak(&ta).max(1e-9).log10(), opts.ret_in, peak(&tb), 20.0 * peak(&tb).max(1e-9).log10());
    if peak(&ta) < 0.02 {
        println!("Aucun signal sur l'entrée {} : câble sortie 1 → entrée {} branché ? gain d'entrée trop bas ?", opts.in_channel, opts.in_channel);
        return 1;
    }
    if peak(&tb) < 0.02 {
        println!("Aucun signal sur l'entrée {} : câble sortie {} → entrée {} branché ? gain d'entrée trop bas ?", opts.ret_in, opts.ret_out, opts.ret_in);
        return 1;
    }
    let reference = reference_signal(rate, opts.seconds);
    let Some(r) = analyze_return(&ta, info_a.start_pos, &tb, info_b.start_pos, &reference, card as i64 + graph_latency as i64, card as i64, opts.window as i64, rate as usize / 2) else {
        println!("Analyse impossible : prises trop courtes ou signal sans rapport avec la référence.");
        return 1;
    };
    println!("\nSortie 1 → entrée {} : {:.2} éch. ({:.3} ms), corrélation {:.4}", opts.in_channel, r.a.lag, ms(r.a.lag), r.a.corr);
    println!("Sortie 1 → entrée {} → moteur → sortie {} → entrée {} : {:.2} éch. ({:.3} ms), corrélation {:.4}", opts.in_channel, opts.ret_out, opts.ret_in, r.b.lag, ms(r.b.lag), r.b.corr);
    let excess = r.live - card as f64;
    println!("\n=> LATENCE DU RETOUR (ce que le musicien entend) : {:.2} échantillons = {:.3} ms", r.live, ms(r.live));
    println!("   Attendu pour un passage direct : {card} éch. ({:.3} ms) ; écart : {:+.2} éch. ({:+.3} ms){}", ms(card as f64), excess, ms(excess), if excess <= TOLERANCE && excess >= TOO_SHORT { if excess < -TOLERANCE { " — notre moteur n'ajoute rien (les convertisseurs de la carte retardent un peu moins que les 72 échantillons supposés)" } else { " — notre moteur n'ajoute rien de mesurable" } } else if excess > 0.0 { " — plus long que prévu" } else { " — beaucoup plus court que prévu : câblage ou compensation à vérifier" });
    let ok = r.a.corr > 0.9 && r.b.corr > 0.9 && !r.a.at_edge && !r.b.at_edge && (TOO_SHORT..=TOLERANCE).contains(&excess) && info_a.overruns == 0 && info_b.overruns == 0;
    if r.a.at_edge || r.b.at_edge {
        println!("   Le pic est au bord de la plage essayée (±{} éch.) : élargir avec --window.", opts.window);
    }
    println!("{}", if ok { "\nRésultat : OK" } else { "\nRésultat : PROBLÈME ou écart au-delà de la tolérance (voir ci-dessus)" });
    i32::from(!ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::Insert;

    /// Retard pur annoncé comme latence (un limiteur à anticipation).
    struct Delay {
        ring: Vec<f32>,
        pos: usize,
    }
    impl Insert for Delay {
        fn latency(&self) -> u32 {
            self.ring.len() as u32
        }
        fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
            for j in 0..in_l.len() {
                out_l[j] = self.ring[self.pos];
                out_r[j] = self.ring[self.pos];
                self.ring[self.pos] = 0.5 * (in_l[j] + in_r[j]);
                self.pos = (self.pos + 1) % self.ring.len();
            }
        }
        fn set_param(&mut self, _: u32, _: f64) {}
        fn reset(&mut self) {
            self.ring.fill(0.0);
        }
    }

    fn delay(n: usize) -> InsertFactory {
        Arc::new(move |_, _, _| Ok(Box::new(Delay { ring: vec![0.0; n], pos: 0 }) as Box<dyn Insert>))
    }

    /// Carte simulée : chaque sortie revient sur l'entrée qui lui est câblée après `loop_delay` échantillons (sortie + entrée).
    /// Rend (latence mesurée du retour, latence du graphe).
    fn simulate(loop_delay: usize, plugin: usize, block: usize) -> (f64, u32) {
        let rate = 48_000;
        let seconds = 4;
        let fx = (plugin > 0).then(|| ("retard".to_string(), delay(plugin)));
        let built = build_graph(rate, seconds, 3, fx);
        let (mut engine, mut handle) = Engine::new(rate, 256);
        handle.load_graph(&built.desc).unwrap();
        let graph_latency = handle.latency_samples();
        let (a, b) = (RecordSource::Channel(0), RecordSource::Channel(1));
        handle.send(Command::SetInput { node: built.musician.0, armed: true, monitor: true, source: a });
        handle.send(Command::SetInput { node: built.tap_ret.0, armed: true, monitor: false, source: b });
        let mut rx_a = handle.start_record(built.musician.0, a, 8.0).unwrap();
        let mut rx_b = handle.start_record(built.tap_ret.0, b, 8.0).unwrap();
        handle.send(Command::Play);
        let total = rate as usize * seconds as usize;
        let mut back_a = std::collections::VecDeque::from(vec![0.0f32; loop_delay]);
        let mut back_b = std::collections::VecDeque::from(vec![0.0f32; loop_delay]);
        let mut bufs = vec![vec![0.0f32; block]; 3];
        let mut done = 0;
        while done < total {
            let n = block.min(total - done);
            let in_a: Vec<f32> = back_a.drain(..n).collect();
            let in_b: Vec<f32> = back_b.drain(..n).collect();
            let mut refs: Vec<&mut [f32]> = bufs.iter_mut().map(|v| &mut v[..n]).collect();
            engine.process_outputs(&[&in_a, &in_b], &mut refs);
            back_a.extend(bufs[0][..n].iter().copied()); // sortie 1 → entrée A
            back_b.extend(bufs[2][..n].iter().copied()); // sortie 3 (retour) → entrée B
            done += n;
            handle.collect_garbage();
        }
        let take = |rx: &mut daw_engine::RecordReceiver| {
            let n = rx.consumer.slots();
            let chunk = rx.consumer.read_chunk(n).unwrap();
            let (x, y) = chunk.as_slices();
            let v = [x, y].concat();
            chunk.commit_all();
            v
        };
        let (ta, tb) = (take(&mut rx_a), take(&mut rx_b));
        let reference = reference_signal(rate, seconds);
        let r = analyze_return(&ta, 0, &tb, 0, &reference, loop_delay as i64 + graph_latency as i64, loop_delay as i64, 400, rate as usize / 2).expect("analyse");
        (r.live, graph_latency)
    }

    #[test]
    fn the_return_latency_is_the_card_round_trip_and_the_engine_adds_nothing() {
        for (loop_delay, block) in [(300, 64), (260, 100), (512, 64)] {
            let (live, _) = simulate(loop_delay, 0, block);
            assert!((live - loop_delay as f64).abs() < 0.2, "boucle {loop_delay}, bloc {block} : retour mesuré à {live}");
        }
    }

    #[test]
    fn a_plugin_with_latency_on_another_track_sending_to_the_same_return_does_not_delay_the_live_input() {
        let (live, latency) = simulate(300, 480, 64);
        // L'autre piste sort aussi dans la sortie principale (par le bus muet) : la compensation retarde la lecture principale de 480, et
        // les deux mesures la portent. L'écoute directe, elle, n'est jamais compensée : la différence reste l'aller-retour de la carte.
        assert_eq!(latency, 480);
        assert!((live - 300.0).abs() < 0.2, "l'écoute directe ne doit pas être retardée par les 480 échantillons de l'autre piste : {live}");
    }
}
