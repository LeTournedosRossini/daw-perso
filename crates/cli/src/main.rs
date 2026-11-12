//! Outils en ligne de commande du DAW : `devices`, `latency-test`.

mod demo;
mod load;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod midi_record;
mod record;
mod return_latency;
mod sync_test;

use daw_audio_io::alloc_guard::GuardAllocator;
use daw_audio_io::playback::PlaybackReport;
use daw_audio_io::latency::LatencyReport;
use daw_audio_io::timing::BUCKET_LABELS;

// Le garde-fou compte les allocations faites dans le callback audio (voir `alloc_guard`).
#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;

const USAGE: &str = "usage :
  daw-cli plugins [DOSSIER_OU_FICHIER.clap ...]
  daw-cli midi-info FICHIER.mid     (décrit un fichier MIDI standard : pistes, notes, tempo, chiffrage)
  daw-cli devices [--driver NOM]
  daw-cli play-demo [--seconds N] [--buffer N] [--rate HZ]
      Linux : [--out-l PORT] [--out-r PORT] [--no-connect]   Windows : [--driver NOM] [--out-ch N]
  daw-cli record [--seconds N] [--file prise.wav] [--source left|right|stereo] [--monitor] [--check clicks|tone] [--offset N]
      [--buffer N] [--rate HZ]   Linux : [--in-l PORT] [--in-r PORT] [--out-l PORT] [--out-r PORT] [--no-connect]
                                  Windows : [--driver NOM] [--in-ch N] [--out-ch N]
  daw-cli sync-test [--seconds N] [--driver NOM] [--buffer N] [--in-ch N] [--out-ch N] [--fx PLUGIN] [--ahead 50|100 [--ahead-threads N]] [--threads N] [--window N]
  daw-cli return-latency-test [--seconds N] [--driver NOM] [--buffer N] [--in-ch N] [--ret-out N] [--ret-in N] [--fx PLUGIN] [--ahead 50|100 [--ahead-threads N]] [--threads N] [--window N]
      (latence réelle d'un retour de monitoring : câbles sortie 1 → entrée A et sortie du retour (3) → entrée B ; ASIO seulement)
  daw-cli midi-record [--seconds N] [--tempo BPM] [--no-synth] [--buffer N]
      Linux (JACK) : [--midi-source PORT] [--out-l PORT] [--out-r PORT] [--no-connect]   Windows : [--midi-device N] [--driver NOM] [--out-ch N]
  daw-cli note-start [--plugins A,B] [--instances 1,8] [--seconds N] [--buffer N] [--rate HZ]
  daw-cli load-bench [--tracks N] [--fx A,B,C] [--synths N] [--automate] [--per-plugin] [--threads N] [--seconds N] [--buffer N] [--rate HZ]
      (banc hors carte son : le vrai moteur chargé des plugins de Jean, chaque appel chronométré ; --per-plugin : coût de chacun)
  daw-cli load-test [--tracks N] [--fx A,B,C] [--synths N] [--automate] [--with-export] [--threads N] [--ahead 50|100 [--ahead-threads N]] [--seconds N] [--buffer N]
      Linux : [--out-l PORT] [--out-r PORT] [--no-connect]   Windows : [--driver NOM] [--out-ch N] [--rate HZ]
      (même projet de charge dans le vrai callback du pilote)
  daw-cli latency-test [--seconds N] [--buffer N] [--rate HZ]
      Linux (JACK/PipeWire) : [--in PORT] [--out PORT]
      Windows (ASIO)        : [--driver NOM] [--in-ch N] [--out-ch N]   (canaux à partir de 1)";

/// Options `--nom valeur` ; les options sans valeur sont listées dans `FLAGS`.
struct Args(Vec<(String, Option<String>)>);

const FLAGS: &[&str] = &["--software-loopback", "--no-connect", "--monitor", "--no-synth", "--automate", "--per-plugin", "--with-export"];

impl Args {
    fn parse(args: &[String], known: &[&str]) -> Result<Args, String> {
        let mut out = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if !known.contains(&a.as_str()) {
                return Err(format!("option inconnue : {a}"));
            }
            if FLAGS.contains(&a.as_str()) {
                out.push((a.clone(), None));
            } else {
                let v = it.next().ok_or(format!("{a} demande une valeur"))?;
                out.push((a.clone(), Some(v.clone())));
            }
        }
        Ok(Args(out))
    }
    fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().rev().find(|(n, _)| n == name).and_then(|(_, v)| v.as_deref())
    }
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // utilisée par le backend JACK seulement
    fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(n, _)| n == name)
    }
    fn num(&self, name: &str) -> Result<Option<u32>, String> {
        self.get(name).map(|v| v.parse().map_err(|e| format!("{name} : {e}"))).transpose()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("devices") => backend::devices(&args[1..]),
        Some("latency-test") => backend::latency_test(&args[1..]),
        Some("play-demo") => backend::play_demo(&args[1..]),
        Some("load-test") => backend::load_test(&args[1..]),
        Some("load-bench") => load_bench(&args[1..]),
        Some("note-start") => note_start(&args[1..]),
        Some("record") => backend::record(&args[1..]),
        Some("sync-test") => backend::sync_test(&args[1..]),
        Some("return-latency-test") => backend::return_latency_test(&args[1..]),
        Some("midi-record") => backend::midi_record(&args[1..]),
        Some("plugins") => Ok(plugins(&args[1..])),
        Some("midi-info") => Ok(midi_info(&args[1..])),
        _ => Err(String::new()),
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            if !e.is_empty() {
                eprintln!("erreur : {e}");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

/// Liste les plugins CLAP : fichiers donnés, dossiers (récursif) ou, sans argument, les dossiers standard du système.
fn plugins(args: &[String]) -> i32 {
    use daw_clap_host::{default_search_dirs, find_plugins, Bundle};
    let targets: Vec<std::path::PathBuf> = if args.is_empty() { default_search_dirs().into_iter().filter(|d| d.is_dir()).collect() } else { args.iter().map(Into::into).collect() };
    let files: Vec<std::path::PathBuf> = targets
        .iter()
        .flat_map(|t| if t.is_dir() { find_plugins(t) } else { vec![t.clone()] })
        .collect();
    if files.is_empty() {
        eprintln!("aucun fichier .clap trouvé dans : {}", targets.iter().map(|t| t.display().to_string()).collect::<Vec<_>>().join(", "));
        return 1;
    }
    let mut failures = 0;
    for f in &files {
        match Bundle::open(f).and_then(|b| b.plugins()) {
            Ok(list) => {
                println!("{}", f.display());
                for p in list {
                    println!("  {} — {} {} [{}]", p.name, p.vendor, p.version, p.id);
                    if !p.features.is_empty() {
                        println!("    {}", p.features.join(", "));
                    }
                }
            }
            Err(e) => {
                failures += 1;
                println!("{} : ERREUR {e}", f.display());
            }
        }
    }
    println!("\n{} fichier(s), {} en erreur", files.len(), failures);
    i32::from(failures > 0)
}

/// Options communes de `record`.
fn record_options(a: &Args) -> Result<record::RecordOptions, String> {
    use daw_engine::RecordSource;
    Ok(record::RecordOptions {
        seconds: a.num("--seconds")?.unwrap_or(10),
        file: a.get("--file").unwrap_or("prise.wav").into(),
        source: match a.get("--source").unwrap_or("left") {
            "left" => RecordSource::Left,
            "right" => RecordSource::Right,
            "stereo" => RecordSource::Stereo,
            other => return Err(format!("--source : « {other} » inconnu (left, right, stereo)")),
        },
        monitor: a.has("--monitor"),
        reference: match a.get("--check") {
            None => None,
            Some("clicks") => Some(record::Reference::Clicks),
            Some("tone") => Some(record::Reference::Tone),
            Some(other) => return Err(format!("--check : « {other} » inconnu (clicks, tone)")),
        },
        offset: a.get("--offset").map(|v| v.parse::<i64>().map_err(|e| format!("--offset : {e}"))).transpose()?.unwrap_or(0),
    })
}

fn print_report(r: &LatencyReport) -> i32 {
    println!("Pilote : {} Hz, tampon {} échantillons", r.sample_rate, r.buffer_size);
    println!("Chemin : {} -> (programme) -> {}", r.input, r.output);
    let ms = |frames: f64| frames * 1000.0 / r.sample_rate as f64;
    if let Some((i, o)) = r.reported_latency {
        println!("Latences annoncées par le pilote : entrée {i} éch. ({:.3} ms), sortie {o} éch. ({:.3} ms)", ms(i as f64), ms(o as f64));
    }
    match &r.stats {
        None => println!("Aucune impulsion détectée ({} envoyées sans retour). Câble de bouclage branché ? Volume d'entrée trop bas ?", r.lost),
        Some(s) => {
            println!("Mesures : {} (perdues : {})", s.count, s.lost);
            println!("  min     {:>7} éch.  {:>8.3} ms", s.min, ms(s.min as f64));
            println!("  moyenne {:>9.1} éch.  {:>8.3} ms  ({:.2} tampons)", s.mean, ms(s.mean), s.mean / r.buffer_size.max(1) as f64);
            println!("  max     {:>7} éch.  {:>8.3} ms", s.max, ms(s.max as f64));
            println!("  gigue   {:>9.2} éch.  {:>8.3} ms (écart type)", s.jitter, ms(s.jitter));
        }
    }
    print_timing(r.timing.as_ref());
    println!("Xruns / dépassements : {}", r.xruns);
    println!("Allocations dans le callback : {}", r.callback_allocations);
    i32::from(r.callback_allocations > 0)
}

/// Contenu du projet de charge d'après les options (16 pistes × Compresseur → Egaliseur et 2 instruments par défaut).
fn load_spec(a: &Args) -> Result<load::LoadSpec, String> {
    Ok(load::LoadSpec {
        tracks: a.num("--tracks")?.unwrap_or(16) as usize,
        fx: a.get("--fx").unwrap_or("Compresseur,Egaliseur").split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
        synths: a.num("--synths")?.unwrap_or(2) as usize,
        automate: a.has("--automate"),
    })
}

/// `note-start` : coût d'un début de note dans un instrument, avec et sans notes, paramètre automatisé, piste muette (voir `load.rs`).
fn note_start(args: &[String]) -> Result<i32, String> {
    let a = Args::parse(args, &["--plugins", "--instances", "--seconds", "--buffer", "--rate"])?;
    let rate = a.num("--rate")?.unwrap_or(48_000);
    let (block, seconds) = (a.num("--buffer")?.unwrap_or(64) as usize, a.num("--seconds")?.unwrap_or(24));
    let plugins = a.get("--plugins").unwrap_or("Neptune,AnalogSynth,FM6Op").to_string();
    let instances = a.get("--instances").unwrap_or("1,8").to_string();
    let cat = load::catalog();
    if cat.is_empty() {
        return Err("aucun plugin de Jean trouvé dans les dossiers CLAP".into());
    }
    println!("Coût d'un début de note : blocs qui contiennent un début de note (une note toutes les 125 ms, accords de ~8 voix) contre les autres blocs.");
    for p in plugins.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        for n in instances.split(',').filter_map(|s| s.trim().parse::<usize>().ok()) {
            load::note_start_report(&cat, p, n.max(1), rate, block, seconds)?;
        }
    }
    Ok(0)
}

/// `load-bench` : banc hors carte son (voir `load.rs`).
fn load_bench(args: &[String]) -> Result<i32, String> {
    let a = Args::parse(args, &["--tracks", "--fx", "--synths", "--automate", "--per-plugin", "--seconds", "--buffer", "--rate", "--threads"])?;
    let spec = load_spec(&a)?;
    load::run_bench(&spec, a.num("--rate")?.unwrap_or(48_000), a.num("--buffer")?.unwrap_or(64) as usize, a.num("--seconds")?.unwrap_or(24), a.has("--per-plugin"), a.num("--threads")?.unwrap_or(1) as usize)
}

/// Les pires callbacks (instant, durée, intervalle depuis le précédent) et leur éventuelle périodicité.
fn print_slow(t: &daw_audio_io::timing::TimingSummary) {
    if t.slow.is_empty() {
        return;
    }
    let mut by_len = t.slow.clone();
    by_len.sort_by(|a, b| b.busy_ms.total_cmp(&a.busy_ms));
    println!("  les {} pires traitements (de plus de la moitié de la période), du plus long au plus court :", by_len.len().min(8));
    for c in by_len.iter().take(8) {
        println!("    à t = {:>9.1} ms : traitement {:>6.3} ms ({:>5.0} % de la période), intervalle depuis le callback précédent {:.3} ms", c.at_ms, c.busy_ms, 100.0 * c.busy_ms / t.nominal_ms, c.since_previous_ms);
    }
    let gaps = t.slow_gaps_ms();
    if !gaps.is_empty() {
        let shown: Vec<String> = gaps.iter().take(12).map(|g| format!("{g:.1}")).collect();
        println!("  écarts entre ces instants (ms) : {}", shown.join(", "));
    }
    for (label, period) in [("tic de l'ordonnanceur de Windows (15,625 ms)", 15.625), ("1 ms (salves du pilote)", 1.0)] {
        if let Some((share, all, phase)) = t.periodicity_vs_all(period) {
            let flag = if share > all + 0.3 { "  ← PÉRIODIQUE (bien au-dessus de l'ensemble des callbacks)" } else { "" };
            println!("  phase à {label} : {:.0} % des pires callbacks dans la même fenêtre (phase {phase:.2} ms) contre {:.0} % de tous les callbacks{flag}", 100.0 * share, 100.0 * all);
        }
    }
    for (label, period) in [("10 ms", 10.0), ("100 ms", 100.0)] {
        if let Some((share, phase)) = t.periodicity(period) {
            println!("  phase à {label} : {:.0} % des pires callbacks dans la même fenêtre (phase {phase:.2} ms ; au hasard : environ 20 %){}", 100.0 * share, if share > 0.7 { "  ← PÉRIODIQUE" } else { "" });
        }
    }
    let slow_after_long_gap = t.slow.iter().filter(|c| c.since_previous_ms > 1.5 * t.nominal_ms).count();
    println!("  parmi eux, {slow_after_long_gap} arrivent après un intervalle de plus de 1,5 période : le pilote nous a livré tard ; les {} autres arrivent à l'heure : c'est notre traitement qui a été lent (plugin, attente d'un fil, préemption)", t.slow.len() - slow_after_long_gap);
}

fn print_playback(r: &PlaybackReport) -> i32 {
    println!("Lecture : {} Hz, tampon {} échantillons, sortie {}", r.sample_rate, r.buffer_size, r.output);
    print_timing(r.timing.as_ref());
    println!("Xruns / dépassements : {} (le pilote ne les signale peut-être pas tous)", r.xruns);
    println!("Allocations dans le callback : {}", r.callback_allocations);
    i32::from(r.callback_allocations > 0)
}

/// Résumé court du callback pendant une prise : régularité et allocations.
fn print_playback_short(r: &PlaybackReport) -> i32 {
    if let Some(t) = r.timing.as_ref() {
        println!("Callback : traitement moyen {:.4} ms, max {:.4} ms ({:.1} % de la période) ; cycles irréguliers {:.1} %", t.busy_mean_ms, t.busy_max_ms, t.busy_max_percent(), t.irregular_percent());
    }
    println!("Allocations dans le callback : {}", r.callback_allocations);
    i32::from(r.callback_allocations > 0)
}

/// `midi-info` : lit un fichier MIDI standard et le décrit.
fn midi_info(args: &[String]) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage : daw-cli midi-info FICHIER.mid");
        return 2;
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path} : {e}");
            return 1;
        }
    };
    let file = match daw_session::midifile::parse(&bytes) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{path} : fichier MIDI illisible ({e:?})");
            return 1;
        }
    };
    println!("{path} : {} ticks par noire, {} pistes, {} messages ignorés", file.ppq, file.tracks.len(), file.skipped);
    for (tick, us) in &file.tempos {
        println!("  tempo à {tick} ticks : {:.2} noires par minute", 60_000_000.0 / *us as f64);
    }
    for (tick, n, d) in &file.meters {
        println!("  chiffrage à {tick} ticks : {n}/{}", 1u32 << d.min(&31));
    }
    for (i, t) in file.tracks.iter().enumerate() {
        let notes = t.events.iter().filter(|e| matches!(e.1, daw_engine::MidiMsg::NoteOn { .. })).count();
        let last = t.events.iter().map(|e| e.0).max().unwrap_or(0);
        println!("  piste {i} « {} » : {} messages dont {notes} notes, jusqu'à {last} ticks", t.name.as_deref().unwrap_or(""), t.events.len());
    }
    0
}

fn print_timing(timing: Option<&daw_audio_io::timing::TimingSummary>) {
    if let Some(t) = timing {
        println!("Régularité des cycles ({} intervalles, période nominale {:.3} ms) :", t.measured, t.nominal_ms);
        println!("  intervalle min {:.3} / moyen {:.3} / max {:.3} ms, gigue {:.3} ms (écart type)", t.interval_min_ms, t.interval_mean_ms, t.interval_max_ms, t.interval_jitter_ms);
        let cells: Vec<String> = BUCKET_LABELS.iter().zip(t.buckets).map(|(l, n)| format!("{l}: {n}")).collect();
        println!("  répartition (intervalle / période) : {}", cells.join(" | "));
        println!("  en retard (> 1,1) : {}   collés (< 0,9) : {}   soit {:.1} % de cycles irréguliers", t.late(), t.bunched(), t.irregular_percent());
        println!("  traitement de notre callback : moyen {:.4} ms, max {:.4} ms ({:.1} % de la période)", t.busy_mean_ms, t.busy_max_ms, t.busy_max_percent());
        load::print_buckets(&t.busy_buckets);
        println!("  callbacks au-delà de la moitié de la période : {} ; au-delà de la période entière (coupure probable) : {}", t.heavy(), t.overruns());
        print_slow(t);
    }
}

#[cfg(target_os = "linux")]
mod backend {
    use super::*;
    use daw_audio_io::jack_backend::{self, LatencyOptions};

    pub fn devices(args: &[String]) -> Result<i32, String> {
        Args::parse(args, &[])?;
        let p = jack_backend::list_ports().map_err(|e| e)?;
        println!("Serveur JACK : {} Hz, tampon {} échantillons", p.sample_rate, p.buffer_size);
        for (title, list, phys) in [
            ("Entrées physiques (capture)", &p.sources, &p.physical_sources),
            ("Sorties physiques (lecture)", &p.sinks, &p.physical_sinks),
        ] {
            println!("\n{title} :");
            for name in phys {
                println!("  {name}");
            }
            println!("Autres ports ({}) :", list.len() - phys.len());
            for name in list.iter().filter(|n| !phys.contains(n)) {
                println!("  {name}");
            }
        }
        Ok(0)
    }

    pub fn play_demo(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--buffer", "--out-l", "--out-r", "--no-connect"])?;
        let o = jack_backend::PlayOptions {
            seconds: a.num("--seconds")?.unwrap_or(16),
            buffer: a.num("--buffer")?,
            left: a.get("--out-l").map(str::to_owned),
            right: a.get("--out-r").map(str::to_owned),
            in_left: None,
            in_right: None,
            no_connect: a.has("--no-connect"),
            midi_in: false,
            midi_source: None,
        };
        match jack_backend::run_playback(&o, crate::demo::make) {
            Ok(r) => Ok(print_playback(&r)),
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    pub fn load_test(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--tracks", "--fx", "--synths", "--automate", "--with-export", "--threads", "--ahead", "--ahead-threads", "--seconds", "--buffer", "--out-l", "--out-r", "--no-connect"])?;
        let spec = load_spec(&a)?;
        let o = jack_backend::PlayOptions {
            seconds: a.num("--seconds")?.unwrap_or(20),
            buffer: a.num("--buffer")?,
            left: a.get("--out-l").map(str::to_owned),
            right: a.get("--out-r").map(str::to_owned),
            in_left: None,
            in_right: None,
            no_connect: a.has("--no-connect"),
            midi_in: false,
            midi_source: None,
        };
        match jack_backend::run_playback(&o, load::make_for_driver(spec, a.has("--with-export"), a.num("--threads")?.unwrap_or(1) as usize, a.num("--ahead")?.map(|ms| (ms as u32, a.num("--ahead-threads").ok().flatten().unwrap_or(2) as usize)))) {
            Ok(r) => Ok(print_playback(&r)),
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    pub fn midi_record(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--tempo", "--no-synth", "--midi-source", "--out-l", "--out-r", "--no-connect", "--buffer"])?;
        let seconds: u32 = a.num("--seconds")?.unwrap_or(10);
        let bpm: u32 = a.num("--tempo")?.unwrap_or(120);
        let with_synth = !a.has("--no-synth");
        let o = jack_backend::PlayOptions {
            seconds: seconds + 3,
            buffer: a.num("--buffer")?,
            left: a.get("--out-l").map(str::to_owned),
            right: a.get("--out-r").map(str::to_owned),
            in_left: None,
            in_right: None,
            no_connect: a.has("--no-connect"),
            midi_in: true,
            midi_source: a.get("--midi-source").map(str::to_owned),
        };
        let slot: midi_record::Slot = Default::default();
        let s2 = slot.clone();
        println!("Jouez sur le clavier MIDI pendant {seconds} s (tempo {bpm}, 4/4)...");
        match jack_backend::run_playback(&o, move |rate| midi_record::make(seconds, with_synth, rate, &s2, None)) {
            Ok(r) => Ok(midi_record::report(&slot, r.sample_rate, bpm).max(print_playback_short(&r))),
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    pub fn record(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--file", "--source", "--monitor", "--check", "--offset", "--buffer", "--in-l", "--in-r", "--out-l", "--out-r", "--no-connect"])?;
        let opts = record_options(&a)?;
        let o = jack_backend::PlayOptions {
            seconds: opts.seconds + 1,
            buffer: a.num("--buffer")?,
            left: a.get("--out-l").map(str::to_owned),
            right: a.get("--out-r").map(str::to_owned),
            in_left: a.get("--in-l").map(str::to_owned),
            in_right: a.get("--in-r").map(str::to_owned),
            no_connect: a.has("--no-connect"),
            midi_in: false,
            midi_source: None,
        };
        let slot: record::Slot = Default::default();
        let s2 = slot.clone();
        let o2 = opts.clone();
        match jack_backend::run_playback(&o, move |rate| record::make(&o2, &s2, rate)) {
            Ok(r) => {
                println!("Lecture : {} Hz, tampon {} échantillons", r.sample_rate, r.buffer_size);
                Ok(record::report(&opts, &slot, r.sample_rate).max(print_playback_short(&r)))
            }
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    /// `return-latency-test` : seulement avec ASIO (plusieurs sorties et latences annoncées).
    pub fn return_latency_test(_: &[String]) -> Result<i32, String> {
        eprintln!("return-latency-test : pour l'instant seulement avec ASIO (Windows) : il faut plusieurs sorties et les latences de la carte.");
        Ok(1)
    }

    /// `sync-test` : seulement avec un pilote qui annonce ses latences (ASIO) ; pas encore écrit pour JACK.
    pub fn sync_test(_: &[String]) -> Result<i32, String> {
        eprintln!("sync-test : pour l'instant seulement avec ASIO (Windows) ; JACK ne donne pas les latences de la carte comme le fait ASIO.");
        Ok(1)
    }

    pub fn latency_test(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--buffer", "--in", "--out", "--software-loopback"])?;
        let o = LatencyOptions {
            seconds: a.num("--seconds")?.unwrap_or(10),
            buffer: a.num("--buffer")?,
            input: a.get("--in").map(str::to_owned),
            output: a.get("--out").map(str::to_owned),
            software_loopback: a.has("--software-loopback"),
        };
        match jack_backend::run_latency_test(&o) {
            Ok(r) => Ok(print_report(&r)),
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod backend {
    use super::*;
    use daw_audio_io::asio_backend::{self, Driver, LatencyOptions};

    pub fn devices(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--driver"])?;
        let midi = daw_audio_io::winmm_midi::list_inputs();
        println!("Entrées MIDI ({}) :", midi.len());
        for (i, m) in midi.iter().enumerate() {
            println!("  {i} : {m}");
        }
        let drivers = asio_backend::list_drivers();
        println!("Pilotes ASIO installés ({}) :", drivers.len());
        for d in &drivers {
            println!("  {}", d.name);
        }
        if let Some(q) = a.get("--driver") {
            match Driver::open(q) {
                Ok(d) => {
                    let i = d.info();
                    println!("\n{} :", i.name);
                    println!("  canaux : {} entrées, {} sorties", i.input_channels, i.output_channels);
                    println!("  tampon : min {}, max {}, préféré {}", i.buffer_min, i.buffer_max, i.buffer_preferred);
                    println!("  fréquence actuelle : {} Hz", i.sample_rate);
                    println!("  latences annoncées : entrée {} éch., sortie {} éch.", i.input_latency, i.output_latency);
                }
                Err(e) => {
                    eprintln!("erreur : {e}");
                    return Ok(1);
                }
            }
        }
        Ok(0)
    }

    pub fn play_demo(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--buffer", "--rate", "--driver", "--out-ch"])?;
        let o = asio_backend::PlayOptions {
            seconds: a.num("--seconds")?.unwrap_or(16),
            driver: a.get("--driver").unwrap_or("UMC").to_owned(),
            buffer: a.num("--buffer")?,
            sample_rate: a.num("--rate")?,
            output_channel: a.num("--out-ch")?.unwrap_or(1),
            input_channel: 0,
            input_count: 0,
            output_count: 0,
        };
        match asio_backend::run_playback(&o, crate::demo::make) {
            Ok(r) => Ok(print_playback(&r)),
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    pub fn load_test(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--tracks", "--fx", "--synths", "--automate", "--with-export", "--threads", "--ahead", "--ahead-threads", "--seconds", "--buffer", "--rate", "--driver", "--out-ch"])?;
        let spec = load_spec(&a)?;
        let o = asio_backend::PlayOptions {
            seconds: a.num("--seconds")?.unwrap_or(20),
            driver: a.get("--driver").unwrap_or("UMC").to_owned(),
            buffer: a.num("--buffer")?,
            sample_rate: a.num("--rate")?,
            output_channel: a.num("--out-ch")?.unwrap_or(1),
            input_channel: 0,
            input_count: 0,
            output_count: 0,
        };
        match asio_backend::run_playback(&o, load::make_for_driver(spec, a.has("--with-export"), a.num("--threads")?.unwrap_or(1) as usize, a.num("--ahead")?.map(|ms| (ms as u32, a.num("--ahead-threads").ok().flatten().unwrap_or(2) as usize)))) {
            Ok(r) => Ok(print_playback(&r)),
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    pub fn midi_record(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--tempo", "--no-synth", "--midi-device", "--buffer", "--rate", "--driver", "--out-ch"])?;
        let seconds: u32 = a.num("--seconds")?.unwrap_or(10);
        let bpm: u32 = a.num("--tempo")?.unwrap_or(120);
        let with_synth = !a.has("--no-synth");
        let device: u32 = a.num("--midi-device")?.unwrap_or(0);
        let inputs = daw_audio_io::winmm_midi::list_inputs();
        if inputs.is_empty() {
            eprintln!("aucune entrée MIDI détectée par Windows (clavier non branché, ou pilote absent)");
            return Ok(1);
        }
        let Some(name) = inputs.get(device as usize) else {
            eprintln!("entrée MIDI {device} inexistante ; disponibles : {}", inputs.iter().enumerate().map(|(i, n)| format!("{i}={n}")).collect::<Vec<_>>().join(", "));
            return Ok(1);
        };
        let o = asio_backend::PlayOptions {
            seconds: seconds + 3,
            driver: a.get("--driver").unwrap_or("UMC").to_owned(),
            buffer: a.num("--buffer")?,
            sample_rate: a.num("--rate")?,
            output_channel: a.num("--out-ch")?.unwrap_or(1),
            input_channel: 0,
            input_count: 0,
            output_count: 0,
        };
        let slot: midi_record::Slot = Default::default();
        let shared: midi_record::SharedInput = Default::default();
        // Entrée ouverte avant le pilote audio : ce que l'on joue avant que le moteur existe est simplement perdu.
        let sh = shared.clone();
        let _input = daw_audio_io::winmm_midi::MidiInput::open(device, move |bytes| {
            if let (Some(tx), Some(msg)) = (sh.lock().unwrap_or_else(|e| e.into_inner()).as_mut(), daw_engine::parse_midi(bytes)) {
                let _ = tx.push(midi_record::KEYBOARD_NODE, msg, 0);
            }
        })?;
        println!("Entrée MIDI : {name}");
        println!("Jouez sur le clavier MIDI pendant {seconds} s (tempo {bpm}, 4/4)...");
        let (s2, sh2) = (slot.clone(), shared.clone());
        match asio_backend::run_playback(&o, move |rate| midi_record::make(seconds, with_synth, rate, &s2, Some(&sh2))) {
            Ok(r) => Ok(midi_record::report(&slot, r.sample_rate, bpm).max(print_playback_short(&r))),
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    pub fn record(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--file", "--source", "--monitor", "--check", "--offset", "--buffer", "--rate", "--driver", "--in-ch", "--out-ch"])?;
        let opts = record_options(&a)?;
        let o = asio_backend::PlayOptions {
            seconds: opts.seconds + 1,
            driver: a.get("--driver").unwrap_or("UMC").to_owned(),
            buffer: a.num("--buffer")?,
            sample_rate: a.num("--rate")?,
            output_channel: a.num("--out-ch")?.unwrap_or(1),
            input_channel: a.num("--in-ch")?.unwrap_or(1),
            input_count: 0,
            output_count: 0,
        };
        let slot: record::Slot = Default::default();
        let s2 = slot.clone();
        let o2 = opts.clone();
        match asio_backend::run_playback(&o, move |rate| record::make(&o2, &s2, rate)) {
            Ok(r) => {
                println!("Lecture : {} Hz, tampon {} échantillons", r.sample_rate, r.buffer_size);
                Ok(record::report(&opts, &slot, r.sample_rate).max(print_playback_short(&r)))
            }
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    /// `sync-test` : une prise rebouclée revient-elle exactement à sa place ? (voir `sync_test.rs`)
    pub fn sync_test(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--buffer", "--rate", "--driver", "--in-ch", "--out-ch", "--file", "--fx", "--ahead", "--ahead-threads", "--threads", "--window"])?;
        let opts = sync_test::SyncOptions {
            seconds: a.num("--seconds")?.unwrap_or(8),
            file: a.get("--file").unwrap_or("sync-test.wav").into(),
            in_channel: a.num("--in-ch")?.unwrap_or(1),
            fx: a.get("--fx").map(str::to_owned),
            ahead: a.num("--ahead")?.map(|ms| (ms, a.num("--ahead-threads").ok().flatten().unwrap_or(2) as usize)),
            threads: a.num("--threads")?.unwrap_or(1) as usize,
            window: a.num("--window")?.unwrap_or(1500),
        };
        let o = asio_backend::PlayOptions {
            seconds: opts.seconds + 3,
            driver: a.get("--driver").unwrap_or("UMC").to_owned(),
            buffer: a.num("--buffer")?,
            sample_rate: a.num("--rate")?,
            output_channel: a.num("--out-ch")?.unwrap_or(1),
            input_channel: 1,
            input_count: u32::MAX,
            output_count: 0,
        };
        let slot: sync_test::Slot = Default::default();
        let (s2, o2) = (slot.clone(), opts.clone());
        match asio_backend::run_playback_ex(&o, move |info| sync_test::make(&o2, info, &s2).unwrap_or_else(|e| {
            eprintln!("erreur : {e}");
            std::process::exit(1)
        })) {
            Ok(r) => {
                println!("Lecture : {} Hz, tampon {} échantillons", r.sample_rate, r.buffer_size);
                Ok(sync_test::report(&opts, &slot, r.sample_rate).max(print_playback_short(&r)))
            }
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    /// `return-latency-test` : latence réelle d'un retour de monitoring (voir `return_latency.rs`).
    pub fn return_latency_test(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--buffer", "--rate", "--driver", "--in-ch", "--ret-out", "--ret-in", "--file", "--fx", "--ahead", "--ahead-threads", "--threads", "--window"])?;
        let opts = return_latency::ReturnOptions {
            seconds: a.num("--seconds")?.unwrap_or(8),
            prefix: a.get("--file").unwrap_or("retour-latence").into(),
            in_channel: a.num("--in-ch")?.unwrap_or(1),
            ret_out: a.num("--ret-out")?.unwrap_or(3),
            ret_in: a.num("--ret-in")?.unwrap_or(2),
            fx: a.get("--fx").map(str::to_owned),
            ahead: a.num("--ahead")?.map(|ms| (ms, a.num("--ahead-threads").ok().flatten().unwrap_or(2) as usize)),
            threads: a.num("--threads")?.unwrap_or(1) as usize,
            window: a.num("--window")?.unwrap_or(2500),
        };
        if opts.ret_out < 3 {
            return Err("--ret-out : un retour sort sur la sortie 3 ou plus (1 et 2 sont la sortie principale)".into());
        }
        if opts.in_channel == opts.ret_in {
            return Err("--in-ch et --ret-in doivent être deux entrées différentes".into());
        }
        let o = asio_backend::PlayOptions {
            seconds: opts.seconds + 3,
            driver: a.get("--driver").unwrap_or("UMC").to_owned(),
            buffer: a.num("--buffer")?,
            sample_rate: a.num("--rate")?,
            output_channel: 1,
            input_channel: 1,
            input_count: u32::MAX,
            output_count: u32::MAX,
        };
        let slot: return_latency::Slot = Default::default();
        let (s2, o2) = (slot.clone(), opts.clone());
        match asio_backend::run_playback_ex(&o, move |info| return_latency::make(&o2, info, &s2).unwrap_or_else(|e| {
            eprintln!("erreur : {e}");
            std::process::exit(1)
        })) {
            Ok(r) => {
                println!("Lecture : {} Hz, tampon {} échantillons", r.sample_rate, r.buffer_size);
                Ok(return_latency::report(&opts, &slot, r.sample_rate).max(print_playback_short(&r)))
            }
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    pub fn latency_test(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--buffer", "--rate", "--driver", "--in-ch", "--out-ch"])?;
        let o = LatencyOptions {
            seconds: a.num("--seconds")?.unwrap_or(10),
            driver: a.get("--driver").unwrap_or("UMC").to_owned(),
            buffer: a.num("--buffer")?,
            sample_rate: a.num("--rate")?,
            input_channel: a.num("--in-ch")?.unwrap_or(1),
            output_channel: a.num("--out-ch")?.unwrap_or(1),
        };
        match asio_backend::run_latency_test(&o) {
            Ok(r) => Ok(print_report(&r)),
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod backend {
    use super::*;
    use daw_audio_io::coreaudio_backend::{self, PlayOptions};

    pub fn devices(_: &[String]) -> Result<i32, String> {
        let list = coreaudio_backend::list_devices()?;
        println!("Périphériques CoreAudio ({}) :", list.len());
        for d in &list {
            println!("  {}{} : {} entrées, {} sorties, {} Hz", d.name, if d.default_output { " (sortie par défaut)" } else { "" }, d.inputs, d.outputs, d.sample_rate);
        }
        Ok(0)
    }

    pub fn play_demo(args: &[String]) -> Result<i32, String> {
        let a = Args::parse(args, &["--seconds", "--buffer", "--device"])?;
        let o = PlayOptions { seconds: a.num("--seconds")?.unwrap_or(16), device: a.get("--device").map(str::to_owned), buffer: a.num("--buffer")?, input_count: 0, output_count: 0 };
        match coreaudio_backend::run_playback(&o, crate::demo::make) {
            Ok(r) => Ok(print_playback(&r)),
            Err(e) => {
                eprintln!("erreur : {e}");
                Ok(1)
            }
        }
    }

    fn not_yet(what: &str) -> Result<i32, String> {
        eprintln!("{what} : pas encore écrit pour CoreAudio (seuls `devices` et `play-demo` le sont).");
        Ok(1)
    }
    pub fn latency_test(_: &[String]) -> Result<i32, String> {
        not_yet("latency-test")
    }
    pub fn load_test(_: &[String]) -> Result<i32, String> {
        not_yet("load-test")
    }
    pub fn midi_record(_: &[String]) -> Result<i32, String> {
        not_yet("midi-record")
    }
    pub fn record(_: &[String]) -> Result<i32, String> {
        not_yet("record")
    }
    pub fn sync_test(_: &[String]) -> Result<i32, String> {
        not_yet("sync-test")
    }
    pub fn return_latency_test(_: &[String]) -> Result<i32, String> {
        not_yet("return-latency-test")
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod backend {
    pub fn devices(_: &[String]) -> Result<i32, String> {
        eprintln!("aucun backend audio sur ce système");
        Ok(1)
    }
    pub fn latency_test(a: &[String]) -> Result<i32, String> {
        devices(a)
    }
    pub fn play_demo(a: &[String]) -> Result<i32, String> {
        devices(a)
    }
    pub fn load_test(a: &[String]) -> Result<i32, String> {
        devices(a)
    }
    pub fn midi_record(_: &[String]) -> Result<i32, String> {
        eprintln!("midi-record : l'entrée MIDI de cette plateforme n'est pas encore écrite (seul Linux/JACK l'est ; Windows viendra avec WinMM/WinRT).");
        Ok(1)
    }

    pub fn record(a: &[String]) -> Result<i32, String> {
        devices(a)
    }
    pub fn sync_test(a: &[String]) -> Result<i32, String> {
        devices(a)
    }
    pub fn return_latency_test(a: &[String]) -> Result<i32, String> {
        devices(a)
    }
}
