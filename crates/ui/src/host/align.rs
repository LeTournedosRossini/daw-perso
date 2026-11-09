//! Calage de prises (`daw-align`) : un **guide** (un clip) et des **doubles** (d'autres clips) ; chaque double est lu sur la fenêtre de temps du
//! guide, analysé puis recalé sans changer sa hauteur, sur un fil de travail (progression, annulation). Le résultat est **une prise de plus**
//! dans le groupe de prises du double (la nouvelle est active, l'original reste dessous : voir `Session::record_over`), le tout en une seule
//! action annulable. Rien n'est ajouté au projet tant que le travail n'est pas fini.
//!
//! Chaque prise calée garde sa **fiche** : réglages (JSON), analyse en octets, et d'où elle vient (guide, double, fenêtre). Cela permet de
//! **refaire le rendu** avec d'autres réglages (une prise de plus) sans refaire l'analyse, ou de refaire l'analyse. Les fiches sont enregistrées
//! avec le projet (`UiMeta::aligns`), la prise rendue étant un fichier audio ordinaire de `audio/`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use daw_align::{AlignError, Analysis, Audio, Guide};
use daw_core_types::NodeId;
use daw_engine::{AudioData, Clip};
use daw_session::peaks::Peaks;
use daw_session::ClipId;
use daw_engine::Command as EngineCommand;

use super::{explain, Host};
use crate::project_io::{self, UiAlign, UiAlignSrc};
use crate::protocol::{AlignedClip, Command, Event};

/// Un clip à lire : ses données et sa géométrie dans le projet (position, décalage dans les données, longueur).
#[derive(Clone)]
pub(super) struct Piece {
    pub(super) data: Arc<AudioData>,
    pub(super) start: u64,
    pub(super) offset: u64,
    pub(super) len: u64,
}

/// Une source audio : un ou **plusieurs clips d'une même piste lus comme une seule bande** (les trous sont du silence ; si deux clips se
/// recouvrent, le plus tardif l'emporte). Le premier morceau donne le nom et le gain.
#[derive(Clone)]
pub(super) struct Src {
    pub(super) pieces: Vec<Piece>,
}

impl Src {
    /// Plusieurs clips, rangés par position.
    fn of_many(clips: &[Clip]) -> Src {
        let mut pieces: Vec<Piece> = clips.iter().map(|c| Piece { data: Arc::clone(&c.data), start: c.start, offset: c.offset, len: c.len }).collect();
        pieces.sort_by_key(|p| p.start);
        Src { pieces }
    }

    /// Nombre de voies de la bande (celles du premier morceau ; un morceau d'un autre format est lu avec ses voies communes).
    fn channels(&self) -> usize {
        self.pieces.iter().map(|p| p.data.channel_count()).max().unwrap_or(1)
    }

    /// Audio entrelacé de la fenêtre `[ws, ws + wlen)` de la ligne de temps (silence hors des clips), et son nombre de voies.
    fn window(&self, ws: u64, wlen: u64) -> (Vec<f32>, usize) {
        let ch = self.channels();
        let mut out = vec![0.0f32; wlen as usize * ch];
        for p in &self.pieces {
            let pch = p.data.channel_count();
            let from = ws.max(p.start);
            let to = (ws + wlen).min(p.start + p.len);
            for t in from..to {
                let src = (p.offset + (t - p.start)) as usize;
                if src >= p.data.len() as usize {
                    break;
                }
                let dst = (t - ws) as usize * ch;
                for c in 0..ch {
                    // Un morceau mono dans une bande stéréo remplit ses deux voies.
                    out[dst + c] = p.data.channel(c.min(pch - 1))[src];
                }
            }
        }
        (out, ch)
    }

    /// Recouvre-t-il la fenêtre ?
    fn overlaps(&self, ws: u64, wlen: u64) -> bool {
        self.pieces.iter().any(|p| p.start < ws + wlen && ws < p.start + p.len)
    }

    /// Les intervalles de la fenêtre qui n'ont aucune matière (trous entre les clips, bords), en échantillons relatifs à `ws`.
    fn gaps(&self, ws: u64, wlen: u64) -> Vec<(u64, u64)> {
        let mut cover: Vec<(u64, u64)> = self.pieces.iter().filter_map(|p| {
            let a = ws.max(p.start);
            let b = (ws + wlen).min(p.start + p.len);
            (a < b).then_some((a - ws, b - ws))
        }).collect();
        cover.sort();
        let mut gaps = Vec::new();
        let mut at = 0u64;
        for (a, b) in cover {
            if a > at {
                gaps.push((at, a));
            }
            at = at.max(b);
        }
        if at < wlen {
            gaps.push((at, wlen));
        }
        gaps
    }

    /// Une clé qui identifie le contenu (pour partager l'analyse d'un guide).
    fn key(&self) -> Vec<(usize, u64, u64, u64)> {
        self.pieces.iter().map(|p| (Arc::as_ptr(&p.data) as usize, p.start, p.offset, p.len)).collect()
    }
}

/// La fiche d'une prise calée (voir le début du module).
pub(super) struct AlignRecord {
    /// Les données de la prise rendue : c'est par elles qu'un clip retrouve sa fiche (une découpe garde les mêmes données).
    pub(super) rendered: Arc<AudioData>,
    /// Nom du double d'origine.
    name: String,
    guide_name: String,
    settings: String,
    analysis: Vec<u8>,
    guide: Src,
    source: Src,
    window: (u64, u64),
}

/// Un double à caler (ou une prise à refaire).
struct Item {
    name: String,
    guide_name: String,
    node: NodeId,
    gain: f32,
    source: Src,
    guide: Src,
    window: (u64, u64),
    settings: String,
    /// Analyse enregistrée à réutiliser (seul le rendu est refait) ; `None` : analyser.
    analysis: Option<Vec<u8>>,
    consonants: Option<Vec<(f64, f64)>>,
}

struct Output {
    index: usize,
    rendered: Arc<AudioData>,
    peaks: Arc<Peaks>,
    analysis: Vec<u8>,
    fell_back: bool,
}

enum Msg {
    Stage { fraction: f64, name: String },
    Done(Output),
    Warning(String),
    Finished,
    Cancelled,
}

pub(super) struct AlignJob {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    total: u32,
    done: u32,
    fraction: f64,
    current: String,
    items: Vec<Item>,
    results: Vec<Output>,
    warnings: Vec<String>,
    label: &'static str,
}

/// Mode d'analyse d'une fiche (il fait partie du guide : un guide par mode).
fn analysis_mode(settings: &str) -> String {
    serde_json::from_str::<serde_json::Value>(settings).ok().and_then(|v| v.pointer("/general/analysis_mode").and_then(|m| m.as_str().map(str::to_owned))).unwrap_or_else(|| "same_pitch".into())
}

/// Rappel de progression d'une étape : `lo` à `hi` de la part du double `index` ; annule si l'utilisateur l'a demandé.
fn stage_cb<'a>(tx: &'a Sender<Msg>, cancel: &'a AtomicBool, name: &'a str, index: usize, count: f64, lo: f64, hi: f64) -> impl FnMut(f64) -> bool + 'a {
    move |f: f64| {
        let within = lo + (hi - lo) * f.clamp(0.0, 1.0);
        let _ = tx.send(Msg::Stage { fraction: (index as f64 + within) / count, name: name.to_owned() });
        cancel.load(Ordering::Relaxed)
    }
}

fn deinterleave(samples: &[f32], channels: usize) -> Vec<Vec<f32>> {
    (0..channels).map(|c| samples.iter().skip(c).step_by(channels).copied().collect()).collect()
}

/// Un échec d'un double : annulé → `true` (le travail s'arrête) ; sinon un avertissement et on passe au suivant.
fn failed(tx: &Sender<Msg>, name: &str, e: AlignError) -> bool {
    if e == AlignError::Cancelled {
        let _ = tx.send(Msg::Cancelled);
        return true;
    }
    let _ = tx.send(Msg::Warning(format!("« {name} » : {e}")));
    false
}

type GuideKey = (Vec<(usize, u64, u64, u64)>, u64, u64, String);

fn work(items: Vec<Item>, rate: u32, tx: Sender<Msg>, cancel: Arc<AtomicBool>) {
    let count = items.len().max(1) as f64;
    let mut guides: HashMap<GuideKey, Guide> = HashMap::new();
    let sample_rate = rate as f64;
    for (i, it) in items.iter().enumerate() {
        let _ = tx.send(Msg::Stage { fraction: i as f64 / count, name: it.name.clone() });
        let (ws, wl) = it.window;
        let (dub, dch) = it.source.window(ws, wl);
        let dub_audio = Audio { samples: &dub, channels: dch, sample_rate };
        let mut analysis = match &it.analysis {
            Some(bytes) => match Analysis::from_bytes(bytes) {
                Ok(a) => a,
                Err(e) => {
                    failed(&tx, &it.name, e);
                    continue;
                }
            },
            None => {
                let key: GuideKey = (it.guide.key(), ws, wl, analysis_mode(&it.settings));
                if !guides.contains_key(&key) {
                    let (g, gch) = it.guide.window(ws, wl);
                    let mut cb = stage_cb(&tx, &cancel, &it.name, i, count, 0.0, 0.1);
                    match Guide::new(Audio { samples: &g, channels: gch, sample_rate }, Some(&it.settings), Some(&mut cb)) {
                        Ok(gd) => {
                            guides.insert(key.clone(), gd);
                        }
                        Err(e) => {
                            if failed(&tx, &it.name, e) {
                                return;
                            }
                            continue;
                        }
                    }
                }
                let mut cb = stage_cb(&tx, &cancel, &it.name, i, count, 0.1, 0.8);
                match guides[&key].analyse(dub_audio, Some(&it.settings), Some(&mut cb)) {
                    Ok(a) => a,
                    Err(e) => {
                        if failed(&tx, &it.name, e) {
                            return;
                        }
                        continue;
                    }
                }
            }
        };
        if let Some(c) = &it.consonants {
            if let Err(e) = analysis.set_consonants(c) {
                failed(&tx, &it.name, e);
                continue;
            }
        }
        let mut cb = stage_cb(&tx, &cancel, &it.name, i, count, 0.8, 1.0);
        let (out, info) = match analysis.render(dub_audio, Some(&it.settings), Some(&mut cb)) {
            Ok(r) => r,
            Err(e) => {
                if failed(&tx, &it.name, e) {
                    return;
                }
                continue;
            }
        };
        let bytes = match analysis.to_bytes() {
            Ok(b) => b,
            Err(e) => {
                failed(&tx, &it.name, e);
                continue;
            }
        };
        let rendered = match AudioData::new(rate, deinterleave(&out, dch)) {
            Ok(d) => d,
            Err(e) => {
                let _ = tx.send(Msg::Warning(format!("« {} » : {e:?}", it.name)));
                continue;
            }
        };
        let peaks = Arc::new(Peaks::build(&rendered));
        let _ = tx.send(Msg::Done(Output { index: i, rendered, peaks, analysis: bytes, fell_back: info.fell_back }));
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Msg::Cancelled);
            return;
        }
    }
    let _ = tx.send(Msg::Finished);
}

impl Host {
    pub(super) fn refuse_while_aligning(&self, command: &str) -> Option<Vec<Event>> {
        self.align.as_ref().map(|_| vec![Event::Error { command: command.into(), message: "un calage est en cours : attends sa fin ou annule-le".into() }])
    }

    fn align_error(command: &str, message: String) -> Vec<Event> {
        vec![Event::Error { command: command.into(), message }]
    }

    /// Fiche d'un clip (celle de la prise rendue dont il montre les données).
    fn align_record_of(&self, clip: u64) -> Option<&AlignRecord> {
        let c = self.session.clip(ClipId(clip))?;
        self.aligns.iter().find(|r| Arc::ptr_eq(&r.rendered, &c.data))
    }

    fn clip_display_name(&self, id: ClipId, c: &Clip) -> String {
        self.audio_clip_name(id.0, &c.data).unwrap_or_else(|| format!("Clip {}", id.0))
    }

    pub(super) fn start_align(&mut self, cmd: Command) -> Vec<Event> {
        let command = cmd.name();
        if self.align.is_some() {
            return Self::align_error(command, "un calage est déjà en cours".into());
        }
        for refused in [self.refuse_while_recording(command), self.refuse_while_importing(command), self.refuse_while_converting(command)] {
            if let Some(r) = refused {
                return r;
            }
        }
        let default = daw_align::default_settings_json();
        let mut initial_warnings: Vec<String> = Vec::new();
        let (items, label) = match cmd {
            Command::AlignClips { guide, guide_more, doubles, settings } => {
                let settings = settings.unwrap_or(default);
                if let Err(e) = daw_align::validate_settings(&settings) {
                    return Self::align_error(command, format!("réglages : {e}"));
                }
                let Some(gc) = self.session.clip(ClipId(guide)).cloned() else { return Self::align_error(command, format!("guide inconnu : {guide}")) };
                if gc.len == 0 {
                    return Self::align_error(command, "le guide est vide".into());
                }
                let guide_node = self.owner_of_audio(ClipId(guide));
                // Le guide peut être plusieurs clips de sa piste, lus comme une bande ; la fenêtre va du début du premier à la fin du dernier.
                let mut guide_clips = vec![gc.clone()];
                for g in guide_more {
                    let Some(c) = self.session.clip(ClipId(g)).cloned() else { return Self::align_error(command, format!("guide inconnu : {g}")) };
                    if self.owner_of_audio(ClipId(g)) != guide_node {
                        return Self::align_error(command, "les clips du guide doivent être sur la même piste".into());
                    }
                    guide_clips.push(c);
                }
                let wstart = guide_clips.iter().map(|c| c.start).min().unwrap_or(0);
                let wend = guide_clips.iter().map(|c| c.end()).max().unwrap_or(0);
                let window = (wstart, wend - wstart);
                let guide_src = Src::of_many(&guide_clips);
                let mut guide_name = self.clip_display_name(ClipId(guide), &gc);
                if guide_clips.len() > 1 {
                    guide_name = format!("{guide_name} + {}", guide_clips.len() - 1);
                }
                // Les doubles : les clips d'une même piste forment un seul double (une bande), dans l'ordre où ils sont donnés.
                let mut groups: Vec<(NodeId, Vec<(ClipId, Clip)>)> = Vec::new();
                for d in doubles {
                    let Some(dc) = self.session.clip(ClipId(d)).cloned() else { return Self::align_error(command, format!("double inconnu : {d}")) };
                    let Some(node) = self.owner_of_audio(ClipId(d)) else { return Self::align_error(command, format!("double inconnu : {d}")) };
                    match groups.iter_mut().find(|g| g.0 == node) {
                        Some(g) => g.1.push((ClipId(d), dc)),
                        None => groups.push((node, vec![(ClipId(d), dc)])),
                    }
                }
                let mut items = Vec::new();
                let mut notes = Vec::new();
                for (node, members) in groups {
                    let clips: Vec<Clip> = members.iter().map(|m| m.1.clone()).collect();
                    let source = Src::of_many(&clips);
                    let first = &members[0];
                    let mut name = self.clip_display_name(first.0, &first.1);
                    if members.len() > 1 {
                        name = format!("{name} + {}", members.len() - 1);
                    }
                    if guide_node == Some(node) {
                        return Self::align_error(command, format!("« {name} » est sur la même piste que le guide « {guide_name} » : la prise calée le recouvrirait. Mets le guide sur une autre piste."));
                    }
                    if !source.overlaps(window.0, window.1) {
                        return Self::align_error(command, format!("« {name} » ne recouvre pas la fenêtre du guide « {guide_name} »"));
                    }
                    // Les trous (et les bords) de la bande : rien à caler à cet endroit, le résultat y est du silence.
                    let rate = self.session.sample_rate() as u64;
                    let long: Vec<(u64, u64)> = source.gaps(window.0, window.1).into_iter().filter(|g| g.1 - g.0 >= rate / 10).collect();
                    for g in long.iter().take(3) {
                        let secs = |x: u64| x as f64 / rate as f64;
                        notes.push(format!("« {name} » : {:.1} s sans matière à {:.1} s du début du guide", secs(g.1 - g.0), secs(g.0)));
                    }
                    if long.len() > 3 {
                        notes.push(format!("« {name} » : {} autres trous", long.len() - 3));
                    }
                    items.push(Item { name, guide_name: guide_name.clone(), node, gain: first.1.gain, source, guide: guide_src.clone(), window, settings: settings.clone(), analysis: None, consonants: None });
                }
                initial_warnings = notes;
                (items, "Caler des prises")
            }
            Command::AlignRedo { clips, settings, reanalyse, consonants, general_only } => {
                if let Some(s) = &settings {
                    if let Err(e) = daw_align::validate_settings(s) {
                        return Self::align_error(command, format!("réglages : {e}"));
                    }
                }
                let mut items = Vec::new();
                for id in clips {
                    let Some(r) = self.align_record_of(id) else { return Self::align_error(command, format!("le clip {id} n'est pas une prise calée")) };
                    let Some(node) = self.owner_of_audio(ClipId(id)) else { return Self::align_error(command, format!("clip inconnu : {id}")) };
                    let gain = self.session.clip(ClipId(id)).map_or(1.0, |c| c.gain);
                    let new_settings = match (&settings, general_only) {
                        (Some(s), true) => match daw_align::merge_general(&r.settings, s) {
                            Ok(m) => m,
                            Err(e) => return Self::align_error(command, format!("réglages : {e}")),
                        },
                        (Some(s), false) => s.clone(),
                        (None, _) => r.settings.clone(),
                    };
                    // Un autre mode d'analyse, serrage ou écart maximal exige une nouvelle analyse : on ne le devine pas, on l'exige.
                    items.push(Item {
                        name: r.name.clone(),
                        guide_name: r.guide_name.clone(),
                        node,
                        gain,
                        source: r.source.clone(),
                        guide: r.guide.clone(),
                        window: r.window,
                        settings: new_settings,
                        analysis: (!reanalyse).then(|| r.analysis.clone()),
                        consonants: consonants.as_ref().map(|c| c.iter().map(|p| (p[0], p[1])).collect()),
                    });
                }
                (items, "Refaire le calage")
            }
            _ => return Vec::new(),
        };
        let mut events = self.end_preview(true, false);
        let rate = self.session.sample_rate();
        let total = items.len() as u32;
        let first = items[0].name.clone();
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        // Les éléments du travail (données audio partagées, sans verrou) partent sur le fil ; ceux de la pose restent ici.
        let work_items: Vec<Item> = items.iter().map(Item::clone_for_work).collect();
        let thread = {
            let cancel = cancel.clone();
            match std::thread::Builder::new().name("calage".into()).spawn(move || work(work_items, rate, tx, cancel)) {
                Ok(t) => t,
                Err(e) => return Self::align_error(command, format!("fil de calage impossible : {e}")),
            }
        };
        let job = AlignJob { rx, cancel, thread: Some(thread), total, done: 0, fraction: 0.0, current: first, items, results: Vec::new(), warnings: initial_warnings, label };
        events.push(job.event(true, false, None, Vec::new()));
        self.align = Some(job);
        events
    }

    pub(super) fn cancel_align(&mut self) -> Vec<Event> {
        if let Some(job) = &self.align {
            job.cancel.store(true, Ordering::Relaxed);
        }
        Vec::new()
    }

    /// Relève le fil de calage (appelé par `tick`) ; à la fin, pose toutes les prises d'un seul coup.
    pub(super) fn align_tick(&mut self) -> Vec<Event> {
        let Some(job) = self.align.as_mut() else { return Vec::new() };
        let mut changed = false;
        let mut end: Option<bool> = None; // Some(annulé)
        loop {
            match job.rx.try_recv() {
                Ok(Msg::Stage { fraction, name }) => {
                    job.fraction = fraction;
                    job.current = name;
                    changed = true;
                }
                Ok(Msg::Done(o)) => {
                    job.done += 1;
                    job.results.push(o);
                    changed = true;
                }
                Ok(Msg::Warning(w)) => {
                    job.warnings.push(w);
                    job.done += 1;
                    changed = true;
                }
                Ok(Msg::Finished) => end = Some(false),
                Ok(Msg::Cancelled) => end = Some(true),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    end.get_or_insert(false);
                    job.warnings.push("le calage s'est arrêté de façon inattendue".into());
                }
            }
            if end.is_some() {
                break;
            }
        }
        let Some(cancelled) = end else {
            return if changed { vec![job.event(true, false, None, Vec::new())] } else { Vec::new() };
        };
        let mut job = self.align.take().expect("calage en cours");
        if let Some(t) = job.thread.take() {
            let _ = t.join();
        }
        if cancelled {
            return vec![job.event(false, true, None, Vec::new())];
        }
        if job.results.is_empty() {
            let message = job.warnings.last().cloned().unwrap_or_else(|| "aucune prise calée".into());
            return vec![job.event(false, false, Some(message), Vec::new())];
        }
        self.finish_align(job)
    }

    /// Chemin `audio/<nom>-calage.wav` (ou `-calage-2`…) : ni un fichier du dossier du projet ni un chemin déjà pris.
    fn align_asset_path(&self, stem: &str) -> String {
        let base = project_io::sanitize(stem);
        let taken: Vec<&str> = self.session.assets().iter().map(|(_, p)| p.as_str()).collect();
        let free = |rel: &String| !taken.contains(&rel.as_str()) && self.project_dir.as_ref().is_none_or(|d| !d.join(rel).exists());
        let first = format!("audio/{base}-calage.wav");
        if free(&first) {
            return first;
        }
        (2..).map(|n| format!("audio/{base}-calage-{n}.wav")).find(free).expect("une suite infinie")
    }

    fn finish_align(&mut self, mut job: AlignJob) -> Vec<Event> {
        job.results.sort_by_key(|o| o.index);
        let label = job.label;
        // Pose : une prise de plus par double, dans le groupe de prises du double.
        let mut paths: Vec<String> = Vec::new();
        for o in &job.results {
            let it = &job.items[o.index];
            let mut p = self.align_asset_path(&format!("{} calé", it.name));
            while paths.contains(&p) {
                p = p.replace(".wav", "-b.wav");
            }
            paths.push(p);
        }
        let result = self.session.group(label, |s| {
            let mut placed = Vec::new();
            for (o, path) in job.results.iter().zip(&paths) {
                let it = &job.items[o.index];
                s.register_asset(path, Arc::clone(&o.rendered));
                let clip = Clip::new(Arc::clone(&o.rendered), it.window.0, 0, o.rendered.len().min(it.window.1), it.gain);
                let id = s.record_over(it.node, clip)?;
                placed.push((id, s.graph().nodes[it.node.0 as usize].name.clone()));
            }
            Ok(placed)
        });
        match result {
            Ok(placed) => {
                let mut clips = Vec::new();
                for (o, (id, track)) in job.results.iter().zip(placed) {
                    let it = &job.items[o.index];
                    let name = format!("{} · calé sur {}", it.name, it.guide_name);
                    self.meta.clip_names.insert(id.0, name.clone());
                    self.peaks.insert(Arc::as_ptr(&o.rendered) as usize, Arc::clone(&o.peaks));
                    self.aligns.push(AlignRecord {
                        rendered: Arc::clone(&o.rendered),
                        name: it.name.clone(),
                        guide_name: it.guide_name.clone(),
                        settings: it.settings.clone(),
                        analysis: o.analysis.clone(),
                        guide: it.guide.clone(),
                        source: it.source.clone(),
                        window: it.window,
                    });
                    clips.push(AlignedClip { clip: id.0, name, track, fell_back: o.fell_back });
                }
                let mut events = self.finish_edit("align", Ok(()));
                events.push(job.event(false, false, None, clips));
                events
            }
            Err(e) => vec![job.event(false, false, Some(format!("les prises n'ont pas pu être posées : {}", explain(&e))), Vec::new())],
        }
    }

    /// Détails du calage d'une prise calée (pour l'écran de réglage).
    pub(super) fn get_align(&self, clip: u64, settings: Option<String>, threshold_ms: Option<f64>) -> Vec<Event> {
        let Some(r) = self.align_record_of(clip) else { return Self::align_error("get_align", format!("le clip {clip} n'est pas une prise calée")) };
        let settings = settings.unwrap_or_else(|| r.settings.clone());
        let build = || -> Result<Event, AlignError> {
            let a = Analysis::from_bytes(&r.analysis)?;
            let pairs = |v: Vec<(f64, f64)>| v.into_iter().map(|p| [p.0, p.1]).collect::<Vec<[f64; 2]>>();
            Ok(Event::AlignDetail {
                clip,
                name: r.name.clone(),
                guide: r.guide_name.clone(),
                window_start: r.window.0,
                window_len: r.window.1,
                hop: a.hop_seconds(),
                path: a.path()?,
                shift: a.applied_shift(Some(&settings))?,
                consonants: pairs(a.consonants()?),
                suggest: pairs(a.suggest_strength(threshold_ms.unwrap_or(30.0))?),
                settings,
            })
        };
        match build() {
            Ok(e) => vec![e],
            Err(e) => Self::align_error("get_align", e.to_string()),
        }
    }

    pub(super) fn merge_align_settings(&self, dst: &str, src: &str) -> Vec<Event> {
        match daw_align::merge_general(dst, src) {
            Ok(settings) => vec![Event::AlignSettings { settings }],
            Err(e) => Self::align_error("merge_align_settings", e.to_string()),
        }
    }

    // ---- enregistrement dans le projet ----

    /// Les fiches telles qu'on les écrit : chaque fichier audio désigné par son chemin dans le projet (les données doivent être déclarées).
    pub(super) fn aligns_for_file(&self) -> Vec<UiAlign> {
        let piece = |p: &Piece| self.session.asset_path(&p.data).map(|a| UiAlignSrc { asset: a.to_owned(), start: p.start, offset: p.offset, len: p.len, more: Vec::new() });
        let src = |s: &Src| -> Option<UiAlignSrc> {
            let mut all = s.pieces.iter().map(&piece).collect::<Option<Vec<_>>>()?.into_iter();
            let mut first = all.next()?;
            first.more = all.collect();
            Some(first)
        };
        self.aligns
            .iter()
            .filter_map(|r| {
                Some(UiAlign {
                    asset: self.session.asset_path(&r.rendered)?.to_owned(),
                    name: r.name.clone(),
                    guide_name: r.guide_name.clone(),
                    settings: r.settings.clone(),
                    analysis: project_io::base64_encode(&r.analysis),
                    guide: src(&r.guide)?,
                    source: src(&r.source)?,
                    window_start: r.window.0,
                    window_len: r.window.1,
                })
            })
            .collect()
    }

    /// Relit les fiches d'un projet ouvert ; une fiche dont un fichier manque est ignorée (avertissement).
    pub(super) fn restore_aligns(&mut self, saved: &[UiAlign]) -> Vec<String> {
        let mut warnings = Vec::new();
        let find = |s: &Self, path: &str| s.session.assets().iter().find(|(_, p)| p == path).map(|(d, _)| Arc::clone(d));
        let rebuild = |s: &Self, u: &UiAlignSrc| -> Option<Src> {
            let mut pieces = Vec::new();
            for x in std::iter::once(u).chain(u.more.iter()) {
                pieces.push(Piece { data: find(s, &x.asset)?, start: x.start, offset: x.offset, len: x.len });
            }
            Some(Src { pieces })
        };
        for a in saved {
            let (Some(rendered), Some(guide), Some(source), Some(analysis)) = (find(self, &a.asset), rebuild(self, &a.guide), rebuild(self, &a.source), project_io::base64_decode(&a.analysis)) else {
                warnings.push(format!("calage de « {} » : fichier ou analyse introuvable, fiche ignorée", a.name));
                continue;
            };
            self.aligns.push(AlignRecord {
                rendered,
                name: a.name.clone(),
                guide_name: a.guide_name.clone(),
                settings: a.settings.clone(),
                analysis,
                guide,
                source,
                window: (a.window_start, a.window_len),
            });
        }
        warnings
    }
}

/// Écoute de calage en cours : ce qu'il faudra rendre à l'utilisateur (position, lecture, boucle, solos, états « muet » des clips).
pub(super) struct Listen {
    saved_pos: u64,
    saved_playing: bool,
    saved_loop: Option<super::LoopRange>,
    saved_solos: Vec<bool>,
    saved_muted: Vec<(ClipId, bool)>,
}

impl Host {
    /// Écoute le guide, le double d'avant, le double recalé, ou le guide avec l'un d'eux (voir `Command::ListenAlign`), en boucle sur la fenêtre du guide.
    pub(super) fn listen_align(&mut self, guide: u64, guide_more: Vec<u64>, node: u32, aligned: Option<u64>, what: String) -> Vec<Event> {
        let command = "listen_align";
        let Some(gc) = self.session.clip(ClipId(guide)).cloned() else { return Self::align_error(command, format!("guide inconnu : {guide}")) };
        let Some(guide_node) = self.owner_of_audio(ClipId(guide)) else { return Self::align_error(command, format!("guide inconnu : {guide}")) };
        if node as usize >= self.session.graph().nodes.len() || node == 0 {
            return Self::align_error(command, format!("piste inconnue : {node}"));
        }
        let mut guide_ids = vec![ClipId(guide)];
        let (mut ws, mut we) = (gc.start, gc.start + gc.len);
        for g in guide_more {
            let Some(c) = self.session.clip(ClipId(g)) else { return Self::align_error(command, format!("guide inconnu : {g}")) };
            ws = ws.min(c.start);
            we = we.max(c.end());
            guide_ids.push(ClipId(g));
        }
        let want_guide = what.starts_with("guide");
        let want_original = what.ends_with("original");
        let want_aligned = what.ends_with("aligned");
        if want_aligned && aligned.is_none_or(|a| self.align_record_of(a).is_none()) {
            return Self::align_error(command, "pas de prise calée à écouter".into());
        }
        // Les clips de la piste du double qui recouvrent la fenêtre : la prise calée choisie, les autres prises calées, les originaux.
        let ids: Vec<ClipId> = self.session.clip_ids(NodeId(node)).to_vec();
        let mut touched: Vec<(ClipId, bool)> = Vec::new();
        for id in ids {
            let Some(c) = self.session.clip(id) else { continue };
            if !(c.start < we && ws < c.end()) {
                continue;
            }
            let is_aligned = self.aligns.iter().any(|r| Arc::ptr_eq(&r.rendered, &c.data));
            let audible = if is_aligned { want_aligned && aligned == Some(id.0) } else { want_original };
            touched.push((id, c.muted));
            self.session.set_clip_muted_untracked(id, !audible);
        }
        // Le guide lui-même doit être audible s'il est voulu (c'est peut-être une prise inactive).
        if want_guide {
            for gid in &guide_ids {
                if let Some(c) = self.session.clip(*gid) {
                    touched.push((*gid, c.muted));
                }
                self.session.set_clip_muted_untracked(*gid, false);
            }
        }
        let first = self.listen.is_none();
        let mut state = match self.listen.take() {
            Some(l) => l,
            None => {
                // L'écoute remplace un aperçu de fondu éventuel.
                let _ = self.end_preview(true, true);
                Listen { saved_pos: self.handle.position(), saved_playing: self.playing, saved_loop: self.loop_range, saved_solos: self.nodes.iter().map(|n| n.solo).collect(), saved_muted: Vec::new() }
            }
        };
        // Mémoire des états d'avant : le premier relevé de chaque clip fait foi (les applications suivantes ne font que déplacer l'audible).
        for t in touched {
            if !state.saved_muted.iter().any(|(id, _)| *id == t.0) {
                state.saved_muted.push(t);
            }
        }
        for i in 0..self.nodes.len() {
            let solo = (want_guide && i == guide_node.0 as usize) || ((want_original || want_aligned) && i == node as usize);
            self.set_node_solo(i, solo);
        }
        if let Err(e) = self.handle.load_graph(self.session.graph()) {
            self.listen = Some(state);
            let mut events = self.end_listen(true, true);
            events.push(Event::Error { command: command.into(), message: format!("le moteur a refusé l'écoute : {e:?}") });
            return events;
        }
        self.loop_range = Some(super::LoopRange { start: ws, end: we });
        self.handle.send(EngineCommand::SetLoop(Some((ws, we))));
        let pos = self.handle.position();
        if first || pos < ws || pos >= we {
            self.handle.send(EngineCommand::Seek(ws));
        }
        self.handle.send(EngineCommand::Play);
        self.playing = true;
        self.listen = Some(state);
        vec![Event::Listen { active: true, what }, self.project_event(), self.position_event()]
    }

    /// Termine l'écoute (sans effet s'il n'y en a pas) : clips, solos, boucle ; la position et l'état de lecture de l'utilisateur si `restore_pos`.
    pub(super) fn end_listen(&mut self, restore_loop: bool, restore_pos: bool) -> Vec<Event> {
        let Some(l) = self.listen.take() else { return Vec::new() };
        for (id, muted) in l.saved_muted.iter().rev() {
            self.session.set_clip_muted_untracked(*id, *muted);
        }
        let _ = self.handle.load_graph(self.session.graph());
        for (i, s) in l.saved_solos.iter().enumerate() {
            self.set_node_solo(i, *s);
        }
        if restore_loop {
            self.loop_range = l.saved_loop;
            self.handle.send(EngineCommand::SetLoop(l.saved_loop.map(|r| (r.start, r.end))));
        }
        if restore_pos {
            self.handle.send(EngineCommand::Seek(l.saved_pos));
            self.playing = l.saved_playing;
            self.handle.send(if self.playing { EngineCommand::Play } else { EngineCommand::Stop });
        }
        vec![Event::Listen { active: false, what: String::new() }, self.project_event(), self.position_event()]
    }
}

impl Item {
    fn clone_for_work(&self) -> Item {
        Item {
            name: self.name.clone(),
            guide_name: self.guide_name.clone(),
            node: self.node,
            gain: self.gain,
            source: self.source.clone(),
            guide: self.guide.clone(),
            window: self.window,
            settings: self.settings.clone(),
            analysis: self.analysis.clone(),
            consonants: self.consonants.clone(),
        }
    }
}

impl AlignJob {
    fn event(&self, active: bool, cancelled: bool, error: Option<String>, clips: Vec<AlignedClip>) -> Event {
        let progress = if active { self.fraction.min(1.0) } else if error.is_none() && !cancelled { 1.0 } else { 0.0 };
        let done = if active { self.done.min(self.total) } else if error.is_none() && !cancelled { self.total } else { 0 };
        Event::Align { active, progress, done, total: self.total, current: self.current.clone(), clips, warnings: self.warnings.clone(), cancelled, error }
    }
}
