//! Test de charge : un projet lourd fait avec les plugins de Jean (seulement les siens : éditeur « Le Tournedos Rossini »), mesuré
//!
//! - **hors carte son** (`load-bench`) : le vrai moteur est appelé en boucle sans pilote et chaque appel est chronométré. Sert à
//!   répartir le coût **plugin par plugin** (`--per-plugin`) et à voir les pics isolés (allocation d'un plugin au déclenchement d'une
//!   voix, par exemple), sur n'importe quelle machine ;
//! - **dans le vrai callback du pilote** (`load-test`, ASIO sous Windows, JACK sous Linux) : même projet, chronométrage du callback
//!   réel et régularité des livraisons du pilote.
//!
//! Le chronométrage ne prouve rien sur ce qui s'entend (un clic peut échapper aux chiffres) : voir `CLAUDE.md`.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::time::Instant;

use daw_audio_io::playback::ControlFn;
use daw_audio_io::timing::{busy_bucket, BUSY_LABELS};
use daw_clap_host::{clap_insert_factory, default_search_dirs, find_plugins, Bundle, Plugin, PluginInfo};
use daw_core_types::NodeId;
use daw_engine::{PoolStats, AudioData, AutoCurve, AutoLane, AutoPoint, AutoTarget, Clip, Command, Engine, GraphDesc, MidiClip, Note, Pool, TrackFormat};

/// Seuls les plugins de Jean sont essayés (consigne : jamais les plugins tiers).
pub const VENDOR: &str = "Le Tournedos Rossini";

/// Durée de la boucle de lecture du projet de charge.
pub const LOOP_SECONDS: u32 = 8;

const SYNTHS: [&str; 3] = ["AnalogSynth", "FM6Op", "Orgamon"];
const STEPPED: u32 = 1; // CLAP_PARAM_IS_STEPPED

/// Un plugin de Jean trouvé sur cette machine.
pub struct Found {
    pub bundle: Arc<Bundle>,
    pub info: PluginInfo,
}

/// Les plugins de Jean installés (dossiers CLAP standard).
pub fn catalog() -> Vec<Found> {
    let mut out = Vec::new();
    for dir in default_search_dirs() {
        for file in find_plugins(&dir) {
            let Ok(bundle) = Bundle::open(&file) else { continue };
            let Ok(list) = bundle.plugins() else { continue };
            let bundle = Arc::new(bundle);
            for info in list.into_iter().filter(|i| i.vendor == VENDOR) {
                out.push(Found { bundle: Arc::clone(&bundle), info });
            }
        }
    }
    out.sort_by(|a, b| a.info.name.cmp(&b.info.name));
    out
}

pub(crate) fn find<'a>(cat: &'a [Found], name: &str) -> Option<&'a Found> {
    cat.iter().find(|f| f.info.name.eq_ignore_ascii_case(name))
}

/// Ce que contient le projet de charge.
#[derive(Debug, Clone)]
pub struct LoadSpec {
    /// Pistes stéréo portant un clip audio et la chaîne d'effets `fx`.
    pub tracks: usize,
    /// Noms des effets de chaque piste, dans l'ordre.
    pub fx: Vec<String>,
    /// Pistes d'instruments (AnalogSynth, FM6Op, Orgamon à tour de rôle) jouant des notes qui se recouvrent.
    pub synths: usize,
    /// Automation dense (un point toutes les 10 ms) sur un paramètre du premier effet de chaque piste.
    pub automate: bool,
}

pub struct Built {
    pub graph: GraphDesc,
    pub lines: Vec<String>,
    /// Nombre d'instances de plugins du projet.
    pub instances: usize,
}

/// Groupe de fils de calcul (voir `engine::Pool`) ; les fils de travail passent en priorité temps réel quand le système le permet.
pub fn make_pool(threads: usize) -> Option<Pool> {
    (threads >= 2).then(|| Pool::new(threads, Some(Arc::new(|_| {
        let _ = daw_audio_io::rt_priority::promote_current_thread();
    }))))
}

/// Source audio de 8 s, stéréo : tonalité et bruit qui varient (de quoi faire travailler un compresseur).
fn source(rate: u32, seed: usize) -> Arc<AudioData> {
    let n = rate as usize * LOOP_SECONDS as usize;
    let base = 110.0 * (1 + seed % 5) as f64;
    let mut s = 0x9e37_79b9_u32.wrapping_add(seed as u32 * 7919);
    let mut side = |detune: f64| -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f64 / rate as f64;
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (s >> 8) as f64 / 16_777_216.0 * 2.0 - 1.0;
                let env = 0.5 + 0.5 * (TAU * t / 1.3).sin().abs();
                (((TAU * base * (1.0 + detune) * t).sin() * 0.5 + noise * 0.15) * env * 0.5) as f32
            })
            .collect()
    };
    let (l, r) = (side(-0.002), side(0.002));
    AudioData::new(rate, vec![l, r]).expect("source valide")
}

/// Notes denses : doubles croches sur toute la boucle, tenues une demi-seconde (environ 8 voix en même temps).
fn dense_notes(rate: u32, track: usize) -> Vec<Note> {
    let sixteenth = rate as u64 / 8;
    let total = LOOP_SECONDS as u64 * 8;
    let chord = [45u8, 52, 57, 60, 64, 67, 71, 76];
    (0..total).map(|i| Note { start: i * sixteenth, len: (rate as u64 / 2).min((total - i) * sixteenth), key: chord[(i as usize + track) % chord.len()] - 12 + 12 * ((i as usize / 8) % 2) as u8, velocity: 0.6 + 0.3 * ((i % 3) as f32 / 2.0), channel: 0 }).collect()
}

/// Premier paramètre continu du plugin (de préférence automatisable) : celui qu'on fera bouger.
fn automatable_param(f: &Found) -> Option<(u32, f64, f64)> {
    let plugin = Plugin::instantiate(&f.bundle, &f.info.id).ok()?;
    let params = plugin.params();
    let ok = |p: &&daw_clap_host::ParamInfo| p.flags & STEPPED == 0 && p.max > p.min && p.min.is_finite() && p.max.is_finite();
    let p = params.iter().filter(ok).find(|p| p.flags & 32 != 0).or_else(|| params.iter().find(ok))?;
    Some((p.id, p.min, p.max))
}

pub fn build_load(rate: u32, spec: &LoadSpec, cat: &[Found]) -> Result<Built, String> {
    let known: Vec<&str> = cat.iter().map(|f| f.info.name.as_str()).collect();
    let mut fx = Vec::new();
    for name in &spec.fx {
        fx.push(find(cat, name).ok_or_else(|| format!("plugin « {name} » introuvable parmi ceux de Jean ({})", known.join(", ")))?);
    }
    let mut g = GraphDesc::new();
    let mut lines = Vec::new();
    let mut instances = 0;
    g.node_mut(NodeId::MASTER).expect("sortie principale").params.gain = 0.4;
    let sources: Vec<Arc<AudioData>> = (0..spec.tracks.clamp(1, 4)).map(|k| source(rate, k)).collect();
    for t in 0..spec.tracks {
        let node = g.add_track(&format!("piste {}", t + 1), TrackFormat::Stereo);
        g.add_clip(node, Clip::whole(Arc::clone(&sources[t % sources.len()]), 0));
        let mut first_insert = None;
        for f in &fx {
            let id = g.add_insert(node, &f.info.name, clap_insert_factory(Arc::clone(&f.bundle), f.info.id.clone())).expect("la piste vient d'être créée");
            first_insert.get_or_insert((id, *f));
            instances += 1;
        }
        if let (true, Some((insert, f))) = (spec.automate, first_insert) {
            if let Some((param, lo, hi)) = automatable_param(f) {
                let step = (rate as u64 / 100).max(1);
                let points = (0..LOOP_SECONDS as u64 * 100)
                    .map(|k| {
                        let t_s = k as f64 / 100.0;
                        let v = 0.5 + 0.4 * (TAU * t_s / (1.7 + 0.13 * t as f64)).sin();
                        AutoPoint { time: k * step, value: lo + (hi - lo) * v, curve: AutoCurve::Linear }
                    })
                    .collect();
                g.set_automation(node, AutoLane::new(AutoTarget::InsertParam { insert, param }, points));
            }
        }
    }
    if spec.synths > 0 {
        let avail: Vec<&Found> = SYNTHS.iter().filter_map(|n| find(cat, n)).collect();
        if avail.is_empty() {
            lines.push("aucun instrument de Jean trouvé : pas de piste d'instrument".into());
        } else {
            for k in 0..spec.synths {
                let f = avail[k % avail.len()];
                let node = g.add_track(&format!("{} {}", f.info.name, k + 1), TrackFormat::Stereo);
                g.add_insert(node, &f.info.name, clap_insert_factory(Arc::clone(&f.bundle), f.info.id.clone())).expect("la piste vient d'être créée");
                g.add_midi_clip(node, MidiClip::new(0, LOOP_SECONDS as u64 * rate as u64, Arc::new(dense_notes(rate, k))));
                instances += 1;
            }
        }
    }
    lines.push(format!(
        "projet de charge : {} pistes × [{}]{}, {} instrument(s), {} instances de plugins{}",
        spec.tracks,
        spec.fx.join(" → "),
        if spec.automate { " (avec automation à 100 points/s)" } else { "" },
        if spec.synths > 0 { spec.synths } else { 0 },
        instances,
        if instances == 0 { " (aucun plugin !)" } else { "" }
    ));
    Ok(Built { graph: g, lines, instances })
}

/// Résultat d'un passage hors carte son.
#[derive(Debug, Clone)]
pub struct Bench {
    pub calls: usize,
    pub period_ms: f64,
    pub mean_ms: f64,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub p999_ms: f64,
    pub max_ms: f64,
    pub buckets: [u64; 7],
    pub latency: u32,
    /// Les appels les plus longs : (position dans la boucle en ms, durée en ms, temps écoulé depuis le début de la mesure en s), du plus long au plus court.
    pub worst: Vec<(f64, f64, f64)>,
    /// Endroits de la boucle (à deux blocs près) où un appel a dépassé la moitié de la période **dans tous les passages complets** :
    /// ce qui se reproduit au même endroit vient du projet (début d'une voix, effet), pas du système. Vide si moins de deux passages.
    pub repeatable: Vec<f64>,
    pub passes: f64,
}

impl Bench {
    pub fn pct(&self, ms: f64) -> f64 {
        100.0 * ms / self.period_ms
    }
}

/// Fait tourner le vrai moteur sans pilote, `seconds` secondes de lecture en boucle, et chronomètre chaque appel (taille de tampon
/// `block`). Les 50 premiers appels (chauffe) ne comptent pas.
pub fn bench(graph: &GraphDesc, rate: u32, block: usize, seconds: u32, threads: usize) -> Result<Bench, String> {
    let (mut engine, mut handle) = Engine::new(rate, block.max(256));
    if let Some(p) = make_pool(threads) {
        engine.set_pool(p);
    }
    handle.load_graph(graph).map_err(|e| format!("graphe refusé par le moteur : {e:?}"))?;
    handle.send(Command::SetLoop(Some((0, LOOP_SECONDS as u64 * rate as u64))));
    handle.send(Command::Play);
    let zeros = vec![0.0f32; block];
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let total = (seconds as usize * rate as usize / block).max(1);
    let mut times: Vec<u64> = Vec::with_capacity(total);
    for i in 0..total + 50 {
        let t0 = Instant::now();
        engine.process_io(&zeros, &zeros, &mut l, &mut r);
        let dt = t0.elapsed().as_nanos() as u64;
        if i >= 50 {
            times.push(dt);
        }
        if i % 256 == 0 {
            handle.collect_garbage();
        }
    }
    let period_ms = block as f64 * 1000.0 / rate as f64;
    let mut buckets = [0u64; 7];
    for &t in &times {
        buckets[busy_bucket(100.0 * (t as f64 / 1e6) / period_ms)] += 1;
    }
    let mean = times.iter().sum::<u64>() as f64 / times.len() as f64 / 1e6;
    let loop_ms = LOOP_SECONDS as f64 * 1000.0;
    let mut by_time: Vec<(usize, u64)> = times.iter().copied().enumerate().collect();
    by_time.sort_unstable_by(|a, b| b.1.cmp(&a.1));
    let worst = by_time.iter().take(6).map(|&(i, t)| ((((i + 50) as f64 * block as f64 * 1000.0 / rate as f64) % loop_ms), t as f64 / 1e6, (i + 50) as f64 * block as f64 / rate as f64)).collect();
    // Pics reproductibles : au même endroit de la boucle (à un bloc près) dans **tous** les passages complets. Un pic isolé du système
    // n'a aucune raison de tomber trois fois de suite au même bloc ; ce qui revient à chaque passage vient du projet.
    let loop_samples = LOOP_SECONDS as u64 * rate as u64;
    let slot = 2 * block as u64;
    let mut seen: std::collections::BTreeMap<u64, std::collections::BTreeSet<u64>> = std::collections::BTreeMap::new();
    for (i, &t) in times.iter().enumerate() {
        if t as f64 / 1e6 > 0.5 * period_ms {
            let pos = (i as u64 + 50) * block as u64;
            seen.entry((pos % loop_samples) / slot).or_default().insert(pos / loop_samples);
        }
    }
    let passes = (times.len() as f64 + 50.0) * block as f64 / loop_samples as f64;
    let full = passes.floor() as u64;
    let repeatable: Vec<f64> = if full >= 2 {
        seen.iter().filter(|(_, p)| (0..full).all(|k| p.contains(&k))).map(|(k, _)| (*k * slot) as f64 * 1000.0 / rate as f64).collect()
    } else {
        Vec::new()
    };
    let mut sorted = times;
    sorted.sort_unstable();
    let q = |p: f64| sorted[(((sorted.len() - 1) as f64) * p).round() as usize] as f64 / 1e6;
    Ok(Bench { calls: sorted.len(), period_ms, mean_ms: mean, p50_ms: q(0.5), p99_ms: q(0.99), p999_ms: q(0.999), max_ms: *sorted.last().unwrap() as f64 / 1e6, buckets, latency: handle.latency_samples(), worst, repeatable, passes })
}

pub fn print_bench(b: &Bench) {
    println!("  {} appels mesurés", b.calls);
    println!(
        "  période {:.3} ms · traitement moyen {:.4} ms ({:.1} %), médian {:.4}, p99 {:.4}, p99,9 {:.4}, max {:.4} ms ({:.1} % de la période) · latence ajoutée {} éch.",
        b.period_ms,
        b.mean_ms,
        b.pct(b.mean_ms),
        b.p50_ms,
        b.p99_ms,
        b.p999_ms,
        b.max_ms,
        b.pct(b.max_ms),
        b.latency
    );
    print_buckets(&b.buckets);
    if b.max_ms > 0.5 * b.period_ms {
        let cells: Vec<String> = b.worst.iter().map(|(at, ms, wall)| format!("{ms:.2} ms à t = {at:.0} ms de la boucle ({wall:.1} s après le début)")).collect();
        println!("  appels les plus longs : {}", cells.join(" · "));
        println!("  {}", repeat_line(b));
    }
}

fn repeat_line(b: &Bench) -> String {
    if b.passes < 2.0 {
        "reproductibilité : moins de deux passages de la boucle, impossible de distinguer un pic du projet d'un bruit du système (mesurer au moins 2 × 8 s)".into()
    } else if b.repeatable.is_empty() {
        format!("reproductibilité : {:.1} passages, aucun pic ne revient à chaque passage au même endroit de la boucle → bruit du système (ordonnanceur, autres processus)", b.passes)
    } else {
        let at: Vec<String> = b.repeatable.iter().take(6).map(|t| format!("{t:.0} ms")).collect();
        format!("reproductibilité : {:.1} passages, {} pic(s) reviennent au même endroit de la boucle à chaque passage (≈ {}) → vient du projet, pas du système", b.passes, b.repeatable.len(), at.join(", "))
    }
}

pub fn print_buckets(buckets: &[u64; 7]) {
    let cells: Vec<String> = BUSY_LABELS.iter().zip(buckets).map(|(l, n)| format!("{l}: {n}")).collect();
    println!("  répartition du temps de traitement (en % de la période) : {}", cells.join(" | "));
}

/// Analyse hors carte son : le projet complet, ou (`per_plugin`) chaque plugin seul pour répartir le coût.
pub fn run_bench(spec: &LoadSpec, rate: u32, block: usize, seconds: u32, per_plugin: bool, threads: usize) -> Result<i32, String> {
    let cat = catalog();
    if cat.is_empty() {
        return Err(format!("aucun plugin de « {VENDOR} » trouvé dans les dossiers CLAP"));
    }
    println!("Plugins de Jean trouvés : {}", cat.iter().map(|f| f.info.name.as_str()).collect::<Vec<_>>().join(", "));
    println!("Banc hors carte son : {rate} Hz, tampon {block}, {seconds} s par mesure.\n");
    let built = build_load(rate, spec, &cat)?;
    for l in &built.lines {
        println!("{l}");
    }
    if built.instances == 0 {
        return Err("le projet ne contient aucun plugin : rien à mesurer (--fx ou --synths)".into());
    }
    let whole = bench(&built.graph, rate, block, seconds, 1)?;
    println!("Un seul fil (comme aujourd'hui) :");
    print_bench(&whole);
    if threads >= 2 {
        let par = bench(&built.graph, rate, block, seconds, threads)?;
        println!("\n{threads} fils de calcul (le fil audio + {} fils de travail) :", threads - 1);
        print_bench(&par);
        println!("  gain sur le temps moyen : ×{:.2} ; pire appel : {:.1} % → {:.1} % de la période ; appels > moitié de la période : {} → {}", whole.mean_ms / par.mean_ms, whole.pct(whole.max_ms), par.pct(par.max_ms), whole.buckets[4..].iter().sum::<u64>(), par.buckets[4..].iter().sum::<u64>());
    }
    if !per_plugin {
        return Ok(i32::from(whole.buckets[6] > 0));
    }
    // Coût de chaque plugin : `tracks` instances du plugin seul, comparées à la même charge sans plugin.
    println!("\nCoût par plugin ({} instances de chacun, sans automation ; la base est le moteur seul avec {} pistes audio) :", spec.tracks.max(1), spec.tracks.max(1));
    let base_spec = LoadSpec { tracks: spec.tracks.max(1), fx: vec![], synths: 0, automate: false };
    let base = bench(&build_load(rate, &base_spec, &cat)?.graph, rate, block, seconds, 1)?;
    println!("  {:<14} moyen {:>6.2} % · p99,9 {:>6.2} % · max {:>6.2} %  (la base)", "(aucun)", base.pct(base.mean_ms), base.pct(base.p999_ms), base.pct(base.max_ms));
    let mut worst: Vec<(String, f64, f64)> = Vec::new();
    for f in &cat {
        let is_synth = f.info.features.iter().any(|x| x == "instrument") || SYNTHS.iter().any(|s| s.eq_ignore_ascii_case(&f.info.name));
        let one = if is_synth {
            LoadSpec { tracks: 0, fx: vec![], synths: 0, automate: false }
        } else {
            LoadSpec { tracks: spec.tracks.max(1), fx: vec![f.info.name.clone()], synths: 0, automate: false }
        };
        let g = if is_synth {
            // Un instrument : autant d'instances que de pistes demandées, avec les notes denses.
            let mut gd = GraphDesc::new();
            for k in 0..spec.tracks.max(1) {
                let node = gd.add_track(&format!("{} {}", f.info.name, k + 1), TrackFormat::Stereo);
                gd.add_insert(node, &f.info.name, clap_insert_factory(Arc::clone(&f.bundle), f.info.id.clone())).expect("piste créée");
                gd.add_midi_clip(node, MidiClip::new(0, LOOP_SECONDS as u64 * rate as u64, Arc::new(dense_notes(rate, k))));
            }
            gd
        } else {
            build_load(rate, &one, &cat)?.graph
        };
        let b = match bench(&g, rate, block, seconds, 1) {
            Ok(b) => b,
            Err(e) => {
                println!("  {:<14} refusé : {e}", f.info.name);
                continue;
            }
        };
        let cost = b.pct(b.mean_ms) - base.pct(base.mean_ms);
        println!(
            "  {:<14} moyen {:>6.2} % · p99,9 {:>6.2} % · max {:>6.2} %  → coût ≈ {:>5.2} % de la période pour {} instance(s), latence {} éch.{}",
            f.info.name,
            b.pct(b.mean_ms),
            b.pct(b.p999_ms),
            b.pct(b.max_ms),
            cost.max(0.0),
            spec.tracks.max(1),
            b.latency,
            if b.buckets[6] > 0 { "  ⚠ dépasse la période" } else { "" }
        );
        if b.max_ms > 0.5 * b.period_ms {
            let cells: Vec<String> = b.worst.iter().take(4).map(|(at, ms, wall)| format!("{ms:.2} ms à t = {at:.0} ms ({wall:.1} s)")).collect();
            println!("                 appels les plus longs : {}", cells.join(" · "));
            println!("                 {}", repeat_line(&b));
        }
        worst.push((f.info.name.clone(), b.pct(b.max_ms), b.pct(b.p999_ms)));
    }
    worst.sort_by(|a, b| b.1.total_cmp(&a.1));
    println!("\nPics les plus hauts (max, % de la période) : {}", worst.iter().take(3).map(|w| format!("{} {:.1} %", w.0, w.1)).collect::<Vec<_>>().join(" · "));
    Ok(i32::from(whole.buckets[6] > 0))
}

/// Temps (ns) de chaque appel après 50 appels de chauffe, lecture en boucle (même méthode que `bench`).
fn call_times(graph: &GraphDesc, rate: u32, block: usize, seconds: u32) -> Result<Vec<u64>, String> {
    let (mut engine, mut handle) = Engine::new(rate, block.max(256));
    handle.load_graph(graph).map_err(|e| format!("graphe refusé par le moteur : {e:?}"))?;
    handle.send(Command::SetLoop(Some((0, LOOP_SECONDS as u64 * rate as u64))));
    handle.send(Command::Play);
    let zeros = vec![0.0f32; block];
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let total = (seconds as usize * rate as usize / block).max(1);
    let mut times = Vec::with_capacity(total);
    for i in 0..total + 50 {
        let t0 = Instant::now();
        engine.process_io(&zeros, &zeros, &mut l, &mut r);
        let dt = t0.elapsed().as_nanos() as u64;
        if i >= 50 {
            times.push(dt);
        }
        if i % 256 == 0 {
            handle.collect_garbage();
        }
    }
    Ok(times)
}

/// Sépare les appels qui **contiennent un début de note** (un des `starts`, en échantillons depuis le début de la boucle) des autres.
/// L'appel `i` (les 50 appels de chauffe étant retirés) couvre `[(i + 50) × block, +block)` modulo la boucle. Retourne (avec, sans).
pub fn split_by_note_start(times: &[u64], starts: &[u64], loop_samples: u64, block: u64) -> (Vec<u64>, Vec<u64>) {
    let (mut with, mut without) = (Vec::new(), Vec::new());
    for (i, &t) in times.iter().enumerate() {
        let pos = (i as u64 + 50) * block % loop_samples;
        if starts.iter().any(|&s| s >= pos && s < pos + block) {
            with.push(t);
        } else {
            without.push(t);
        }
    }
    (with, without)
}

/// (n, médiane, p99, max) en millisecondes d'un lot de temps.
fn quartiles(v: &[u64]) -> (usize, f64, f64, f64) {
    if v.is_empty() {
        return (0, 0.0, 0.0, 0.0);
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let q = |p: f64| s[(((s.len() - 1) as f64) * p).round() as usize] as f64 / 1e6;
    (s.len(), q(0.5), q(0.99), *s.last().unwrap() as f64 / 1e6)
}

/// Mesure, pour un instrument, le coût du bloc qui contient un début de note comparé aux autres blocs, dans quatre situations :
/// notes seules, sans note, sans note avec un paramètre automatisé (événements de paramètre à chaque case de 32 échantillons),
/// notes avec le même paramètre automatisé, et notes sur une piste muette. `instances` instances jouent les mêmes notes en même temps.
pub fn note_start_report(cat: &[Found], name: &str, instances: usize, rate: u32, block: usize, seconds: u32) -> Result<(), String> {
    let f = find(cat, name).ok_or_else(|| format!("plugin « {name} » introuvable parmi ceux de Jean ({})", cat.iter().map(|f| f.info.name.as_str()).collect::<Vec<_>>().join(", ")))?;
    let period_ms = block as f64 * 1000.0 / rate as f64;
    let loop_samples = LOOP_SECONDS as u64 * rate as u64;
    let starts: Vec<u64> = dense_notes(rate, 0).iter().map(|n| n.start).collect();
    let param = automatable_param(f);
    println!("\n{} — {instances} instance(s), tampon {block}, {rate} Hz, {seconds} s par situation (période {period_ms:.3} ms)", f.info.name);
    let situations: [(&str, bool, bool, bool); 5] = [("notes seules", true, false, false), ("sans note", false, false, false), ("sans note, paramètre automatisé", false, true, false), ("notes + paramètre automatisé", true, true, false), ("notes, piste muette", true, false, true)];
    for (label, notes, automate, muted) in situations {
        if automate && param.is_none() {
            println!("  {label:<34} (aucun paramètre continu à automatiser)");
            continue;
        }
        let mut g = GraphDesc::new();
        g.node_mut(NodeId::MASTER).expect("sortie principale").params.gain = 0.4;
        for k in 0..instances {
            let node = g.add_track(&format!("{} {}", f.info.name, k + 1), TrackFormat::Stereo);
            let insert = g.add_insert(node, &f.info.name, clap_insert_factory(Arc::clone(&f.bundle), f.info.id.clone())).expect("piste créée");
            if notes {
                g.add_midi_clip(node, MidiClip::new(0, loop_samples, Arc::new(dense_notes(rate, k))));
            }
            if let (true, Some((pid, lo, hi))) = (automate, param) {
                let step = (rate as u64 / 100).max(1);
                let points = (0..LOOP_SECONDS as u64 * 100)
                    .map(|j| AutoPoint { time: j * step, value: lo + (hi - lo) * (0.5 + 0.4 * (TAU * (j as f64 / 100.0) / 1.7).sin()), curve: AutoCurve::Linear })
                    .collect();
                g.set_automation(node, AutoLane::new(AutoTarget::InsertParam { insert, param: pid }, points));
            }
            if muted {
                g.node_mut(node).expect("piste").params.mute = true;
            }
        }
        let times = call_times(&g, rate, block, seconds)?;
        let (with, without) = split_by_note_start(&times, &starts, loop_samples, block as u64);
        let pct = |ms: f64| 100.0 * ms / period_ms;
        let (n1, m1, p1, x1) = quartiles(&with);
        let (n2, m2, p2, x2) = quartiles(&without);
        let all = quartiles(&times);
        if notes {
            println!("  {label:<34} blocs avec début de note ({n1}) : médiane {m1:.3} ms ({:.1} %), p99 {p1:.3}, max {x1:.3} ms ({:.1} %) · autres blocs ({n2}) : médiane {m2:.3} ms ({:.1} %), max {x2:.3} ms ({:.1} %) · pic/médiane des autres ×{:.1}", pct(m1), pct(x1), pct(m2), pct(x2), if m2 > 0.0 { x1 / m2 } else { 0.0 });
        } else {
            println!("  {label:<34} tous les blocs ({}) : médiane {:.3} ms ({:.1} %), p99 {:.3}, max {:.3} ms ({:.1} %)", all.0, all.1, pct(all.1), all.2, all.3, pct(all.3));
        }
        let _ = (p2, x2);
    }
    Ok(())
}

/// Affiche, à la fin de la lecture, combien de fois et combien de temps le fil audio a **attendu** un fil de travail.
struct PoolReport(Arc<PoolStats>);

impl Drop for PoolReport {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        let s = &self.0;
        let (runs, waits, total, max, long) = (s.runs.load(Relaxed), s.waits.load(Relaxed), s.wait_total_ns.load(Relaxed), s.wait_max_ns.load(Relaxed), s.long_waits.load(Relaxed));
        eprintln!(
            "attente des fils de travail par le fil audio : {waits} attente(s) sur {runs} tâches parallèles, total {:.2} ms, plus longue {:.3} ms, dont {long} de plus de 0,1 ms",
            total as f64 / 1e6,
            max as f64 / 1e6
        );
    }
}

/// Simule un export qui tourne pendant la lecture : un fil construit le même projet avec son propre moteur et le fait rendre sans
/// s'arrêter (comme le fait l'export de l'application, qui partage le processeur avec le fil audio). Arrêté et compté à la disparition.
struct ExportLoad {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<(u64, u32)>>,
}

impl ExportLoad {
    fn start(spec: LoadSpec, rate: u32) -> ExportLoad {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("export-simule".into())
            .spawn(move || {
                let built = build_load(rate, &spec, &catalog()).expect("projet valide (déjà construit pour la lecture)");
                let (mut engine, mut handle) = Engine::new(rate, 512);
                handle.load_graph(&built.graph).expect("graphe valide");
                handle.send(Command::SetLoop(Some((0, LOOP_SECONDS as u64 * rate as u64))));
                handle.send(Command::Play);
                let (zeros, mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512], vec![0.0f32; 512]);
                let mut blocks = 0u64;
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    engine.process_io(&zeros, &zeros, &mut l, &mut r);
                    blocks += 1;
                    if blocks % 256 == 0 {
                        handle.collect_garbage();
                    }
                }
                (blocks, rate)
            })
            .expect("fil d'export simulé");
        ExportLoad { stop, thread: Some(thread) }
    }
}

impl Drop for ExportLoad {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some((blocks, rate)) = self.thread.take().and_then(|t| t.join().ok()) {
            eprintln!("export simulé : {:.0} s de son rendu pendant la lecture", blocks as f64 * 512.0 / rate as f64);
        }
    }
}

/// Fabrique, pour un pilote audio, le moteur chargé du projet de charge (lecture en boucle, démarrée tout de suite) et sa fonction de
/// contrôle (qui ne fait que vider les rebuts). La fréquence est celle du pilote.
pub fn make_for_driver(spec: LoadSpec, with_export: bool, threads: usize, ahead: Option<(u32, usize)>) -> impl FnOnce(u32) -> (crate::demo::EngineProc, ControlFn) {
    move |rate| {
        let cat = catalog();
        let built = build_load(rate, &spec, &cat).unwrap_or_else(|e| {
            eprintln!("erreur : {e}");
            std::process::exit(1);
        });
        for l in &built.lines {
            eprintln!("{l}");
        }
        let (mut engine, mut handle) = Engine::new(rate, 256);
        let mut pool_report = None;
        if let Some(p) = make_pool(threads) {
            eprintln!("calcul parallèle : {} fils (le fil audio + {} fils de travail)", p.threads(), p.threads() - 1);
            pool_report = Some(PoolReport(p.stats()));
            engine.set_pool(p);
        }
        // Calcul en avance des pistes de lecture : à activer avant le premier graphe.
        if let Some((ms, n)) = ahead {
            engine.enable_ahead(&mut handle, &daw_engine::AheadConfig { lookahead_ms: ms, block: 256, threads: n.max(1), on_start: Some(Arc::new(|_| {
                let _ = daw_audio_io::rt_priority::promote_current_thread();
            })) });
            eprintln!("calcul en avance : {ms} ms, blocs de 256, {} fil(s) de rendu", n.max(1));
        }
        handle.load_graph(&built.graph).unwrap_or_else(|e| {
            eprintln!("erreur : graphe refusé par le moteur : {e:?}");
            std::process::exit(1);
        });
        eprintln!("latence ajoutée par les plugins : {} échantillons", handle.latency_samples());
        handle.send(Command::SetLoop(Some((0, LOOP_SECONDS as u64 * rate as u64))));
        handle.send(Command::Play);
        // Le fil d'export simulé vit autant que la fonction de contrôle (donc autant que la lecture).
        let export = with_export.then(|| {
            eprintln!("export simulé pendant la lecture : un second moteur, même projet, rend sans s'arrêter");
            ExportLoad::start(spec.clone(), rate)
        });
        let control: ControlFn = Box::new(move |_| {
            let _keep = (&export, &pool_report);
            handle.collect_garbage();
            true
        });
        (crate::demo::EngineProc(engine), control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_engine::{Insert, InsertFactory};

    /// Un « plugin » qui consomme du temps (pour tester le banc sans dépendre des plugins installés).
    struct Burn(u32);
    impl Insert for Burn {
        fn latency(&self) -> u32 {
            0
        }
        fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
            let mut x = 0.0f32;
            for _ in 0..self.0 {
                x = (x + 0.37).sin();
            }
            for i in 0..out_l.len() {
                out_l[i] = in_l[i] + x * 1e-9;
                out_r[i] = in_r[i];
            }
        }
        fn set_param(&mut self, _: u32, _: f64) {}
        fn reset(&mut self) {}
    }

    fn graph_with(cost: u32, tracks: usize) -> GraphDesc {
        let mut g = GraphDesc::new();
        let data = source(48_000, 0);
        for k in 0..tracks {
            let n = g.add_track(&format!("p{k}"), TrackFormat::Stereo);
            g.add_clip(n, Clip::whole(Arc::clone(&data), 0));
            let f: InsertFactory = Arc::new(move |_, _, _| Ok(Box::new(Burn(cost)) as Box<dyn Insert>));
            g.add_insert(n, "burn", f);
        }
        g
    }

    #[test]
    fn the_bench_measures_more_time_for_a_heavier_graph_and_counts_every_call() {
        let light = bench(&graph_with(1, 4), 48_000, 64, 1, 1).unwrap();
        let heavy = bench(&graph_with(20_000, 4), 48_000, 64, 1, 1).unwrap();
        assert_eq!(light.calls, 48_000 / 64);
        assert_eq!(light.buckets.iter().sum::<u64>() as usize, light.calls, "chaque appel tombe dans une classe");
        assert!(heavy.mean_ms > light.mean_ms * 3.0, "{} contre {}", heavy.mean_ms, light.mean_ms);
        assert!(heavy.p999_ms >= heavy.p99_ms && heavy.p99_ms >= heavy.p50_ms && heavy.max_ms >= heavy.p999_ms);
        assert_eq!(light.latency, 0);
        assert!(light.period_ms > 1.33 && light.period_ms < 1.34);
    }

    #[test]
    fn a_graph_that_cannot_keep_up_is_reported_as_overrunning() {
        // 64 échantillons à 48 kHz = 1,33 ms : 4 pistes × 200 000 sinus dépassent largement la période.
        let b = bench(&graph_with(200_000, 4), 48_000, 64, 1, 1).unwrap();
        assert!(b.buckets[6] > 0, "{:?}", b.buckets);
    }

    #[test]
    fn calls_are_split_by_whether_they_contain_a_note_start() {
        // Boucle de 1 000 échantillons, blocs de 100, notes aux échantillons 120 et 450 : les appels 50 + i couvrent [(50+i)·100 mod 1000, +100).
        let times: Vec<u64> = (0..20).map(|i| i as u64).collect();
        let (with, without) = split_by_note_start(&times, &[120, 450], 1_000, 100);
        // Position de l'appel i = ((50 + i) × 100) mod 1000 = (i mod 10) × 100 : il contient 120 quand i mod 10 = 1, et 450 quand i mod 10 = 4.
        assert_eq!(with, vec![1, 4, 11, 14]);
        assert_eq!(without.len(), 16);
        assert!(!without.contains(&1) && !without.contains(&14));
        // Une note pile sur une frontière de bloc appartient au bloc qui commence là, pas au précédent.
        let (with, _) = split_by_note_start(&times, &[300], 1_000, 100);
        assert_eq!(with, vec![3, 13]);
        let q = quartiles(&[1_000_000, 2_000_000, 3_000_000, 10_000_000]);
        assert_eq!(q.0, 4);
        assert!((q.3 - 10.0).abs() < 1e-9);
        assert_eq!(quartiles(&[]), (0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn dense_notes_are_valid_overlap_and_fit_the_loop() {
        let rate = 48_000;
        let notes = dense_notes(rate, 2);
        assert_eq!(notes.len(), LOOP_SECONDS as usize * 8);
        let total = LOOP_SECONDS as u64 * rate as u64;
        assert!(notes.iter().all(|n| n.start + n.len <= total && n.len > 0 && n.key < 128 && n.velocity > 0.0 && n.velocity <= 1.0));
        // Au moins 4 notes sonnent en même temps à mi-parcours.
        let mid = total / 2;
        assert!(notes.iter().filter(|n| n.start <= mid && mid < n.start + n.len).count() >= 3);
        let mut g = GraphDesc::new();
        let t = g.add_track("x", TrackFormat::Stereo);
        g.add_midi_clip(t, MidiClip::new(0, total, Arc::new(notes)));
        g.compile(rate, 256).expect("notes valides");
    }

    #[test]
    fn the_source_is_deterministic_audible_and_does_not_clip() {
        let (a, b) = (source(44_100, 3), source(44_100, 3));
        assert_eq!(a.channel(0), b.channel(0));
        assert_ne!(source(44_100, 1).channel(0), a.channel(0));
        let peak = a.channel(0).iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.1 && peak < 0.9, "{peak}");
    }

    #[test]
    fn an_unknown_plugin_name_is_refused_with_the_list_of_known_ones() {
        let spec = LoadSpec { tracks: 1, fx: vec!["Inconnu".into()], synths: 0, automate: false };
        let e = build_load(48_000, &spec, &[]).err().expect("refusé");
        assert!(e.contains("Inconnu") && e.contains("introuvable"), "{e}");
    }
}
