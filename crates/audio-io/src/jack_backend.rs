//! Backend JACK (aussi servi par PipeWire via `pipewire-jack`). Linux uniquement.
//!
//! Sous Linux la basse latence n'est pas un objectif (décision du 4 novembre 2026) : ce backend
//! sert surtout à valider l'outillage (mesure, garde-fou d'allocation) avant ASIO.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jack::{AudioIn, AudioOut, Client, ClientOptions, Control, PortFlags, ProcessScope};

use crate::alloc_guard::{self, RtScope};
use crate::latency::{Event, LatencyReport, Rig, Stats};
use crate::playback::{ControlFn, PlaybackReport, StereoProcessor, TimedProcessor};
use crate::spsc::Spsc;

/// Ports audio visibles sur le serveur JACK.
pub struct PortLists {
    pub sample_rate: u32,
    pub buffer_size: u32,
    /// Ports d'où l'on peut lire (sorties des clients, dont les entrées physiques « capture »).
    pub sources: Vec<String>,
    /// Ports où l'on peut écrire (entrées des clients, dont les sorties physiques « playback »).
    pub sinks: Vec<String>,
    pub physical_sources: Vec<String>,
    pub physical_sinks: Vec<String>,
}

fn open_client(name: &str) -> Result<Client, String> {
    Client::new(name, ClientOptions::NO_START_SERVER)
        .map(|(c, _)| c)
        .map_err(|e| format!("connexion au serveur JACK impossible ({e}). Lancer jackd, ou utiliser pw-jack avec PipeWire"))
}

pub fn list_ports() -> Result<PortLists, String> {
    let c = open_client("daw-list")?;
    let audio = Some("32 bit float mono audio");
    Ok(PortLists {
        sample_rate: c.sample_rate() as u32,
        buffer_size: c.buffer_size(),
        sources: c.ports(None, audio, PortFlags::IS_OUTPUT),
        sinks: c.ports(None, audio, PortFlags::IS_INPUT),
        physical_sources: c.ports(None, audio, PortFlags::IS_OUTPUT | PortFlags::IS_PHYSICAL),
        physical_sinks: c.ports(None, audio, PortFlags::IS_INPUT | PortFlags::IS_PHYSICAL),
    })
}

#[derive(Debug, Clone, Default)]
pub struct LatencyOptions {
    pub seconds: u32,
    /// Taille de tampon à demander au serveur (sinon celle du serveur).
    pub buffer: Option<u32>,
    /// Port à lire (sinon première entrée physique).
    pub input: Option<String>,
    /// Port où écrire l'impulsion (sinon première sortie physique).
    pub output: Option<String>,
    /// Relie notre sortie directement à notre entrée : mesure l'overhead du serveur seul.
    pub software_loopback: bool,
}

struct Notifications {
    xruns: Arc<AtomicU32>,
}

impl jack::NotificationHandler for Notifications {
    fn xrun(&mut self, _: &Client) -> Control {
        self.xruns.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }
}

struct Process {
    input: jack::Port<AudioIn>,
    output: jack::Port<AudioOut>,
    rig: Rig,
}

impl jack::ProcessHandler for Process {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        // Un panic ne doit jamais traverser la frontière FFI : on le contient.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _rt = RtScope::enter();
            let input = self.input.as_slice(ps);
            let output = self.output.as_mut_slice(ps);
            self.rig.process(input, output);
        }));
        Control::Continue
    }
}

pub fn run_latency_test(opts: &LatencyOptions) -> Result<LatencyReport, String> {
    let client = open_client("daw-latency")?;
    if let Some(n) = opts.buffer {
        client.set_buffer_size(n).map_err(|e| format!("taille de tampon {n} refusée : {e}"))?;
    }
    let sample_rate = client.sample_rate() as u32;
    let in_port = client.register_port("in", AudioIn::default()).map_err(|e| e.to_string())?;
    let out_port = client.register_port("out", AudioOut::default()).map_err(|e| e.to_string())?;
    let (my_in, my_out) = (
        in_port.name().map_err(|e| e.to_string())?,
        out_port.name().map_err(|e| e.to_string())?,
    );

    let audio = Some("32 bit float mono audio");
    let phys_src = client.ports(None, audio, PortFlags::IS_OUTPUT | PortFlags::IS_PHYSICAL);
    let phys_snk = client.ports(None, audio, PortFlags::IS_INPUT | PortFlags::IS_PHYSICAL);
    let source = if opts.software_loopback {
        my_out.clone()
    } else {
        opts.input.clone().or_else(|| phys_src.first().cloned()).ok_or("aucune entrée physique trouvée (voir `devices`)")?
    };
    let sink = if opts.software_loopback {
        my_in.clone()
    } else {
        opts.output.clone().or_else(|| phys_snk.first().cloned()).ok_or("aucune sortie physique trouvée (voir `devices`)")?
    };

    let queue = Arc::new(Spsc::new(4096));
    let xruns = Arc::new(AtomicU32::new(0));
    alloc_guard::reset_violations();
    let active = client
        .activate_async(
            Notifications { xruns: Arc::clone(&xruns) },
            Process { input: in_port, output: out_port, rig: Rig::new(sample_rate, Arc::clone(&queue)) },
        )
        .map_err(|e| e.to_string())?;

    // Connexions : [source] -> notre entrée ; notre sortie -> [sink].
    if opts.software_loopback {
        active.as_client().connect_ports_by_name(&my_out, &my_in).map_err(|e| e.to_string())?;
    } else {
        active.as_client().connect_ports_by_name(&source, &my_in).map_err(|e| format!("{source} -> {my_in} : {e}"))?;
        active.as_client().connect_ports_by_name(&my_out, &sink).map_err(|e| format!("{my_out} -> {sink} : {e}"))?;
    }

    // PipeWire applique le changement de tampon de façon asynchrone : on relit la valeur effective.
    std::thread::sleep(Duration::from_millis(200));
    let buffer_size = active.as_client().buffer_size();

    let (mut latencies, mut lost) = (Vec::new(), 0usize);
    let end = Instant::now() + Duration::from_secs(opts.seconds.max(1) as u64);
    while Instant::now() < end {
        while let Some(v) = queue.pop() {
            match Event::decode(v) {
                Event::Latency(n) => latencies.push(n),
                Event::Lost => lost += 1,
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let callback_allocations = alloc_guard::violations();
    // Une fois désactivé, le gestionnaire nous est rendu : on peut lire le chronométrage sans conflit.
    let timing = active.deactivate().ok().and_then(|(_, _, p)| p.rig.timing());

    // La première impulsion peut tomber pendant l'établissement des connexions : on l'écarte.
    let skip = usize::from(latencies.len() > 2);
    Ok(LatencyReport {
        sample_rate,
        buffer_size,
        input: source,
        output: sink,
        stats: Stats::from_samples(&latencies[skip..], lost),
        lost,
        xruns: xruns.load(Ordering::Relaxed),
        callback_allocations,
        reported_latency: None,
        timing,
    })
}


#[derive(Debug, Clone, Default)]
pub struct PlayOptions {
    pub seconds: u32,
    pub buffer: Option<u32>,
    /// Ports de sortie gauche/droite (sinon les deux premières sorties physiques).
    pub left: Option<String>,
    pub right: Option<String>,
    /// Ports d'entrée à lire (sinon les deux premières entrées physiques).
    pub in_left: Option<String>,
    pub in_right: Option<String>,
    /// Ne rien connecter (essai silencieux : le programme tourne mais on ne l'entend pas).
    pub no_connect: bool,
    /// Ouvre un port d'entrée MIDI et transmet ses messages au processeur (`StereoProcessor::midi`).
    pub midi_in: bool,
    /// Port MIDI à relier (sinon la première sortie MIDI physique).
    pub midi_source: Option<String>,
}

struct Play<P> {
    in_left: jack::Port<AudioIn>,
    in_right: jack::Port<AudioIn>,
    left: jack::Port<AudioOut>,
    right: jack::Port<AudioOut>,
    midi: Option<jack::Port<jack::MidiIn>>,
    tp: TimedProcessor<P>,
}

impl<P: StereoProcessor> jack::ProcessHandler for Play<P> {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        let ok = catch_unwind(AssertUnwindSafe(|| {
            let _rt = RtScope::enter();
            if let Some(m) = &self.midi {
                for ev in m.iter(ps) {
                    self.tp.midi(ev.time, ev.bytes);
                }
            }
            let (il, ir) = (self.in_left.as_slice(ps), self.in_right.as_slice(ps));
            let l = self.left.as_mut_slice(ps);
            let r = self.right.as_mut_slice(ps);
            self.tp.process(il, ir, l, r);
        }))
        .is_ok();
        if !ok {
            // Un panic ne doit pas laisser de bruit : silence.
            self.left.as_mut_slice(ps).fill(0.0);
            self.right.as_mut_slice(ps).fill(0.0);
        }
        Control::Continue
    }
}

/// Joue le processeur fabriqué par `make` (qui reçoit la fréquence du serveur) pendant `opts.seconds`
/// secondes, ou jusqu'à ce que la fonction de contrôle retourne `false`.
pub fn run_playback<P: StereoProcessor>(
    opts: &PlayOptions,
    make: impl FnOnce(u32) -> (P, ControlFn),
) -> Result<PlaybackReport, String> {
    let client = open_client("daw-play")?;
    if let Some(n) = opts.buffer {
        client.set_buffer_size(n).map_err(|e| format!("taille de tampon {n} refusée : {e}"))?;
    }
    let sample_rate = client.sample_rate() as u32;
    let in_left = client.register_port("in_l", AudioIn::default()).map_err(|e| e.to_string())?;
    let in_right = client.register_port("in_r", AudioIn::default()).map_err(|e| e.to_string())?;
    let (my_in_l, my_in_r) = (in_left.name().map_err(|e| e.to_string())?, in_right.name().map_err(|e| e.to_string())?);
    let left = client.register_port("out_l", AudioOut::default()).map_err(|e| e.to_string())?;
    let right = client.register_port("out_r", AudioOut::default()).map_err(|e| e.to_string())?;
    let (my_l, my_r) = (left.name().map_err(|e| e.to_string())?, right.name().map_err(|e| e.to_string())?);
    let midi_port = if opts.midi_in { Some(client.register_port("midi_in", jack::MidiIn::default()).map_err(|e| e.to_string())?) } else { None };
    let my_midi = midi_port.as_ref().map(|p| p.name().map_err(|e| e.to_string())).transpose()?;
    let phys = client.ports(None, Some("32 bit float mono audio"), PortFlags::IS_INPUT | PortFlags::IS_PHYSICAL);
    let dest_l = opts.left.clone().or_else(|| phys.first().cloned());
    let dest_r = opts.right.clone().or_else(|| phys.get(1).cloned()).or_else(|| dest_l.clone());

    let (processor, mut control) = make(sample_rate);
    let xruns = Arc::new(AtomicU32::new(0));
    alloc_guard::reset_violations();
    let active = client
        .activate_async(Notifications { xruns: Arc::clone(&xruns) }, Play { in_left, in_right, left, right, midi: midi_port, tp: TimedProcessor::new(processor, sample_rate) })
        .map_err(|e| e.to_string())?;
    if !opts.no_connect {
        let (dl, dr) = (dest_l.ok_or("aucune sortie physique trouvée (voir `devices`)")?, dest_r.ok_or("aucune sortie physique trouvée")?);
        active.as_client().connect_ports_by_name(&my_l, &dl).map_err(|e| format!("{my_l} -> {dl} : {e}"))?;
        active.as_client().connect_ports_by_name(&my_r, &dr).map_err(|e| format!("{my_r} -> {dr} : {e}"))?;
        // Entrées : celles qu'on a demandées, sinon les deux premières entrées physiques ; leur absence n'est pas une erreur.
        let sources = active.as_client().ports(None, Some("32 bit float mono audio"), PortFlags::IS_OUTPUT | PortFlags::IS_PHYSICAL);
        let src_l = opts.in_left.clone().or_else(|| sources.first().cloned());
        let src_r = opts.in_right.clone().or_else(|| sources.get(1).cloned()).or_else(|| src_l.clone());
        if let Some(sl) = src_l {
            active.as_client().connect_ports_by_name(&sl, &my_in_l).map_err(|e| format!("{sl} -> {my_in_l} : {e}"))?;
        }
        if let Some(sr) = src_r {
            active.as_client().connect_ports_by_name(&sr, &my_in_r).map_err(|e| format!("{sr} -> {my_in_r} : {e}"))?;
        }
    }
    if let (Some(mine), false) = (my_midi.as_ref(), opts.no_connect) {
        let sources = active.as_client().ports(None, Some("8 bit raw midi"), PortFlags::IS_OUTPUT | PortFlags::IS_PHYSICAL);
        match opts.midi_source.clone().or_else(|| sources.first().cloned()) {
            Some(src) => active.as_client().connect_ports_by_name(&src, mine).map_err(|e| format!("{src} -> {mine} : {e}"))?,
            None => eprintln!("aucune entrée MIDI physique trouvée : à relier à la main ({mine}), ou option --midi-source"),
        }
    }
    std::thread::sleep(Duration::from_millis(200)); // PipeWire applique le tampon de façon asynchrone
    let buffer_size = active.as_client().buffer_size();

    let start = Instant::now();
    let end = Duration::from_secs(opts.seconds.max(1) as u64);
    loop {
        let elapsed = start.elapsed();
        if elapsed >= end || !control(elapsed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let callback_allocations = alloc_guard::violations();
    let timing = active.deactivate().ok().and_then(|(_, _, p)| p.tp.timing());
    Ok(PlaybackReport {
        sample_rate,
        buffer_size,
        output: if opts.no_connect { "(non connecté)".to_owned() } else { format!("{} + {}", opts.left.clone().unwrap_or_else(|| "sortie physique 1".into()), opts.right.clone().unwrap_or_else(|| "sortie physique 2".into())) },
        timing,
        xruns: xruns.load(Ordering::Relaxed),
        callback_allocations,
    })
}
