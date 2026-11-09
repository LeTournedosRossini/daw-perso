//! Moteur temps réel : transport, commandes et échange de graphe sans verrou.
//!
//! Trois rôles : [`Engine`] vit dans le thread audio ; [`EngineHandle`] vit côté contrôle/UI. Ils ne
//! partagent qu'une file SPSC de messages (vers l'audio), une file de rebut (retour des graphes remplacés,
//! libérés hors du thread audio) et la position de lecture (un atomique).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer, PushError, RingBuffer};

use crate::compiled::{AheadConfig, AheadParams, AheadStats, AheadSystem, CompiledGraph};
use std::collections::HashMap;

use crate::graph::{GraphDesc, GraphError};
use crate::insert::{InsertControl, InsertId};
use crate::midi::MidiMsg;
use crate::meters::MeterBank;
use crate::metronome::{ClickTrack, CountIn, Metronome};
use crate::midi_in::{new_midi_tap, MidiIn, MidiInputTx, MidiRecordReceiver, MidiTap};
use crate::parallel::Pool;
use crate::record::{new_tap_at, RecordReceiver, RecordSource, RecordTap, MAX_INPUTS};

const COMMAND_QUEUE: usize = 1024;
const TRASH_QUEUE: usize = 8;
/// Messages MIDI d'entrée en attente (par appel de `process`).
const MIDI_INPUT_QUEUE: usize = 1024;

/// Commande vers le thread audio. `Copy`, donc envoyable sans allocation. `node` est l'identifiant
/// du nœud (`NodeId.0`) ; un identifiant inconnu est ignoré.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Play,
    /// Arrête la lecture ; la position est conservée.
    Stop,
    Seek(u64),
    /// Boucle `[début, fin[` en échantillons ; `None` ou plage vide : pas de boucle.
    SetLoop(Option<(u64, u64)>),
    SetGain { node: u32, gain: f32 },
    SetPan { node: u32, pan: f32 },
    SetMute { node: u32, mute: bool },
    SetSolo { node: u32, solo: bool },
    SetSendGain { node: u32, send: u32, gain: f32 },
    /// Coupe / rétablit un envoi (seulement vers sa destination).
    SetSendMute { node: u32, send: u32, mute: bool },
    /// Solo d'un envoi dans sa destination.
    SetSendSolo { node: u32, send: u32, solo: bool },
    /// Paramètre d'un effet inséré (`insert` = identifiant renvoyé par `GraphDesc::add_insert`).
    SetInsertParam { insert: u32, param: u32, value: f64 },
    /// Note jouée en direct sur une piste (début du prochain bloc).
    Midi { node: u32, msg: MidiMsg },
    /// Entrée audio d'une piste : armement, écoute (monitoring, sans compensation de latence) et canal(aux) d'entrée.
    SetInput { node: u32, armed: bool, monitor: bool, source: RecordSource },
    /// Inverse (`true`) ou rétablit la polarité d'un nœud.
    SetInvert { node: u32, invert: bool },
    /// Allume ou éteint le clic du métronome, et règle son volume (gain linéaire).
    SetMetronome { enabled: bool, gain: f32 },
    /// Contourne (`true`) ou réactive un effet inséré, sans clic et sans changer la latence (contournement compensé).
    SetInsertBypass { insert: u32, bypass: bool },
    /// Panique : coupe toutes les notes de toutes les pistes (voir `CompiledGraph::panic`). Sans effet sur la lecture.
    Panic,
}

enum Msg {
    Cmd(Command),
    Graph(Box<CompiledGraph>),
    /// Installe (ou retire) la prise d'enregistrement d'une piste.
    Tap { node: u32, tap: Option<RecordTap> },
    /// Installe (ou retire) la prise MIDI d'une piste.
    MidiTap { node: u32, tap: Option<MidiTap> },
    /// Remplace les temps du métronome (`None` : les retire).
    Click(Option<Box<ClickTrack>>),
    /// Démarre (ou, avec `None`, annule) un pré-compte : la lecture est arrêtée, la tête immobile, et démarre à la fin.
    CountIn(Option<Box<CountIn>>),
}

/// Ce que le thread audio rend au contrôle pour qu'il le libère (le contenu n'est jamais lu : il est seulement détruit
/// côté contrôle).
#[allow(dead_code)]
enum Garbage {
    Graph(Box<CompiledGraph>),
    Tap(RecordTap),
    MidiTap(MidiTap),
    Click(Box<ClickTrack>),
    CountIn(Box<CountIn>),
}

/// Partie temps réel.
pub struct Engine {
    rx: Consumer<Msg>,
    trash: Producer<Garbage>,
    graph: Box<CompiledGraph>,
    /// Entrée nulle pour `process` (sans entrée audio).
    silence: Vec<f32>,
    /// Entrée MIDI du pilote, et ce qui en a été lu pour l'appel de `process_io` en cours.
    midi_rx: Consumer<MidiIn>,
    midi_pending: Vec<MidiIn>,
    max_block: usize,
    playing: bool,
    pos: u64,
    /// Métronome et pré-compte.
    metro: Metronome,
    loop_range: Option<(u64, u64)>,
    shared_pos: Arc<AtomicU64>,
    shared_leaks: Arc<AtomicU32>,
    /// Fils de travail qui aident à calculer les pistes (absent : tout sur le fil audio).
    pool: Option<Pool>,
    /// Calcul en avance des pistes de lecture (absent : tout se calcule dans le callback), opération en attente du rendu des pistes
    /// (graphe, armement, prise...), et relance de l'avance à faire une fois les pistes rendues.
    ahead: Option<AheadSystem>,
    held: Option<Held>,
    restart_after: bool,
    ahead_stats: Option<Arc<AheadStats>>,
}

/// Ce qui attend que les pistes calculées en avance soient rendues au fil audio.
enum Held {
    Cmd(Command),
    Msg(Msg),
}

/// Partie contrôle.
pub struct EngineHandle {
    tx: Producer<Msg>,
    trash: Consumer<Garbage>,
    sample_rate: u32,
    max_block: usize,
    /// Réglage des jumelles pour les graphes compilés (calcul en avance activé).
    ahead: Option<AheadParams>,
    position: Arc<AtomicU64>,
    leaks: Arc<AtomicU32>,
    /// Effets vivants côté audio (après les messages déjà envoyés) et leur latence.
    live: HashMap<InsertId, u32>,
    /// Poignées de contrôle des effets vivants.
    controls: HashMap<InsertId, Arc<dyn InsertControl>>,
    latency: u32,
    midi_tx: Option<Producer<MidiIn>>,
    meters: Arc<MeterBank>,
}

#[derive(Debug, PartialEq)]
pub enum LoadError {
    Graph(GraphError),
    /// La file de messages est pleine : réessayer plus tard.
    QueueFull,
}

impl Engine {
    /// `max_block` : plus grand bloc traité d'un coup ; un tampon plus grand est découpé.
    pub fn new(sample_rate: u32, max_block: usize) -> (Engine, EngineHandle) {
        let max_block = max_block.max(1);
        let (tx, rx) = RingBuffer::new(COMMAND_QUEUE);
        let (trash_tx, trash_rx) = RingBuffer::new(TRASH_QUEUE);
        let (midi_tx, midi_rx) = RingBuffer::new(MIDI_INPUT_QUEUE);
        let position = Arc::new(AtomicU64::new(0));
        let leaks = Arc::new(AtomicU32::new(0));
        let meters = Arc::new(MeterBank::new());
        let mut graph = Box::new(GraphDesc::new().compile(sample_rate.max(1), max_block).expect("le graphe vide est toujours valide"));
        graph.set_meters(Arc::clone(&meters));
        (
            Engine {
                rx,
                trash: trash_tx,
                graph,
                silence: vec![0.0; max_block],
                midi_rx,
                midi_pending: Vec::with_capacity(MIDI_INPUT_QUEUE),
                max_block,
                playing: false,
                pos: 0,
                loop_range: None,
                metro: Metronome::new(),
                shared_pos: Arc::clone(&position),
                shared_leaks: Arc::clone(&leaks),
                pool: None,
                ahead: None,
                held: None,
                restart_after: false,
                ahead_stats: None,
            },
            EngineHandle { tx, trash: trash_rx, sample_rate: sample_rate.max(1), max_block, ahead: None, position, leaks, live: HashMap::new(), controls: HashMap::new(), latency: 0, midi_tx: Some(midi_tx), meters },
        )
    }

    /// Donne au moteur un groupe de fils de calcul (voir `Pool`). À faire **avant** de confier le moteur au fil audio : le groupe lance
    /// des fils. Le résultat du rendu ne change pas, seul le temps de calcul.
    pub fn set_pool(&mut self, pool: Pool) {
        self.pool = Some(pool);
    }

    /// Active le calcul en avance des pistes de lecture (voir `AheadSystem`). À faire **avant** de charger le premier graphe et de
    /// confier le moteur au fil audio : lance les fils de rendu.
    pub fn enable_ahead(&mut self, handle: &mut EngineHandle, cfg: &AheadConfig) {
        let sys = AheadSystem::new(cfg, handle.sample_rate);
        handle.ahead = Some(sys.params());
        self.ahead_stats = Some(sys.stats());
        self.ahead = Some(sys);
    }

    /// Attend (au plus `timeout`) que les pistes calculées en avance aient `n` échantillons prêts ; faux si le délai est dépassé. Pour les
    /// tests et les mesures hors ligne : un vrai callback n'attend jamais.
    #[doc(hidden)]
    pub fn ahead_wait_ready(&mut self, n: usize, timeout: std::time::Duration) -> bool {
        let t0 = std::time::Instant::now();
        self.drain();
        while !self.graph.ahead_ready(n) {
            if t0.elapsed() > timeout {
                return false;
            }
            std::thread::yield_now();
            self.drain();
        }
        true
    }

    /// Compteurs de diagnostic du calcul en avance (`None` s'il n'est pas activé).
    pub fn ahead_stats(&self) -> Option<Arc<AheadStats>> {
        self.ahead_stats.clone()
    }

    /// La lecture démarre (commande ou fin de pré-compte) : les pistes de lecture passent aux fils de rendu.
    fn ahead_play_started(&mut self) {
        if let Some(a) = self.ahead.as_mut() {
            if a.is_releasing() {
                self.restart_after = true;
            } else {
                a.start(&mut self.graph, self.pos, self.loop_range);
            }
        }
    }

    /// La lecture s'arrête : les pistes de lecture sont rendues au fil audio (queues d'effets coupées).
    fn ahead_stopped(&mut self) {
        self.restart_after = false;
        if let Some(a) = self.ahead.as_mut() {
            if a.is_active() {
                a.begin_release(&mut self.graph, true);
            }
        }
    }

    /// La ligne de temps change (déplacement, boucle) : nouvelle époque, la production repart de la position courante.
    fn ahead_invalidate(&mut self) {
        if let Some(a) = self.ahead.as_mut() {
            a.invalidate(&mut self.graph, self.pos, self.loop_range, true);
        }
    }

    /// Met `h` en attente et demande le rendu des pistes calculées en avance : `h` est appliqué dès qu'elles sont revenues.
    fn hold(&mut self, h: Held) {
        if let Some(a) = self.ahead.as_mut() {
            a.begin_release(&mut self.graph, true);
        }
        self.held = Some(h);
        self.restart_after = true;
    }

    /// Les pistes sont-elles revenues ? Si oui, applique ce qui attendait et relance l'avance si la lecture tourne.
    fn ahead_poll(&mut self) -> bool {
        let done = match self.ahead.as_mut() {
            Some(a) => a.poll_release(&mut self.graph),
            None => true,
        };
        if done {
            if let Some(h) = self.held.take() {
                match h {
                    Held::Cmd(c) => self.apply(c),
                    Held::Msg(m) => self.apply_msg(m),
                }
            }
            if self.restart_after {
                self.restart_after = false;
                if self.playing {
                    self.ahead_play_started();
                }
            }
        }
        done
    }

    fn apply(&mut self, c: Command) {
        match c {
            Command::Play => {
                if !self.playing {
                    self.playing = true;
                    self.ahead_play_started();
                }
            }
            Command::Stop => {
                self.playing = false;
                self.ahead_stopped();
                // Un pré-compte en cours est abandonné.
                if let Some(old) = self.metro.set_count_in(None) {
                    self.give_back(Garbage::CountIn(old));
                }
                // Le graphe continue de tourner à l'arrêt (écoute, queues d'effets) : les notes qui sonnent doivent finir.
                self.graph.notes_off_all();
            }
            Command::Seek(p) => {
                self.pos = p;
                self.graph.reset();
                self.ahead_invalidate();
            }
            Command::SetLoop(r) => {
                self.loop_range = r.filter(|&(s, e)| s < e);
                self.ahead_invalidate();
            }
            Command::SetGain { node, gain } => self.graph.set_gain(node as usize, gain),
            Command::SetPan { node, pan } => self.graph.set_pan(node as usize, pan),
            Command::SetMute { node, mute } => self.graph.set_mute(node as usize, mute),
            Command::SetSolo { node, solo } => self.graph.set_solo(node as usize, solo),
            Command::SetInvert { node, invert } => self.graph.set_invert(node as usize, invert),
            Command::SetMetronome { enabled, gain } => self.metro.set(enabled, gain),
            Command::SetSendGain { node, send, gain } => self.graph.set_send_gain(node as usize, send as usize, gain),
            Command::SetSendMute { node, send, mute } => self.graph.set_send_mute(node as usize, send as usize, mute),
            Command::SetSendSolo { node, send, solo } => self.graph.set_send_solo(node as usize, send as usize, solo),
            Command::SetInsertParam { insert, param, value } => self.graph.set_insert_param(insert, param, value),
            Command::Midi { node, msg } => self.graph.push_live(node as usize, msg),
            Command::SetInput { node, armed, monitor, source } => {
                if armed && self.graph.node_is_ahead(node as usize) {
                    // Une piste calculée en avance ne peut pas écouter l'entrée : on la reprend d'abord, puis l'avance repart sans elle.
                    self.hold(Held::Cmd(c));
                } else {
                    self.graph.set_input(node as usize, armed, monitor, source);
                }
            }
            Command::SetInsertBypass { insert, bypass } => self.graph.set_insert_bypass(insert, bypass),
            Command::Panic => self.graph.panic(),
        }
    }

    fn drain(&mut self) {
        // Une opération attend que les pistes calculées en avance soient rendues : tant qu'elles ne le sont pas, rien d'autre ne passe
        // (l'ordre des messages est conservé).
        if (self.held.is_some() || self.ahead.as_ref().is_some_and(AheadSystem::is_releasing)) && !self.ahead_poll() {
            return;
        }
        while let Ok(msg) = self.rx.pop() {
            let wait = match &msg {
                // Un nouveau graphe, une prise sur une piste calculée en avance : il faut d'abord reprendre les pistes.
                Msg::Graph(_) => self.ahead.as_ref().is_some_and(AheadSystem::is_active),
                Msg::Tap { node, .. } | Msg::MidiTap { node, .. } => self.graph.node_is_ahead(*node as usize),
                _ => false,
            };
            if wait {
                self.hold(Held::Msg(msg));
                if !self.ahead_poll() {
                    return;
                }
                continue;
            }
            self.apply_msg(msg);
            if self.held.is_some() {
                // La commande appliquée vient de demander le rendu des pistes (armement) : on attend.
                if !self.ahead_poll() {
                    return;
                }
            }
        }
    }

    /// Applique un message du contrôle (les pistes calculées en avance ont déjà été reprises si c'était nécessaire).
    fn apply_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Cmd(c) => self.apply(c),
            Msg::Graph(new) => {
                let mut old = std::mem::replace(&mut self.graph, new);
                self.graph.adopt_inserts_from(&mut old);
                // Une prise dont la piste a disparu ne recevra plus rien : on la déclare terminée.
                old.close_taps();
                // L'ancien graphe ne doit pas être libéré ici (désallocation interdite) : retour au contrôle.
                self.give_back(Garbage::Graph(old));
                // La lecture tourne : les pistes du nouveau graphe repartent en avance (si l'avance est activée).
                if self.playing {
                    self.ahead_play_started();
                }
            }
            Msg::MidiTap { node, tap } => {
                if let Some(old) = self.graph.set_midi_tap(node as usize, tap) {
                    old.close();
                    self.give_back(Garbage::MidiTap(old));
                }
            }
            Msg::Click(t) => {
                if let Some(old) = self.metro.set_track(t) {
                    self.give_back(Garbage::Click(old));
                }
            }
            Msg::CountIn(c) => {
                let start = c.is_some();
                if let Some(old) = self.metro.set_count_in(c) {
                    self.give_back(Garbage::CountIn(old));
                }
                if start {
                    self.playing = false;
                    self.ahead_stopped();
                }
            }
            Msg::Tap { node, tap } => {
                if let Some(old) = self.graph.set_tap(node as usize, tap) {
                    old.close();
                    self.give_back(Garbage::Tap(old));
                }
            }
        }
    }

    fn give_back(&mut self, g: Garbage) {
        if let Err(PushError::Full(g)) = self.trash.push(g) {
            // File de rebut pleine (le contrôle ne la vide pas) : on préfère fuir la mémoire plutôt que de libérer dans
            // le callback, et on le signale.
            std::mem::forget(g);
            self.shared_leaks.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Remplit `out_l` et `out_r` (même longueur utile), sans entrée audio. Sans allocation ni verrou.
    pub fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let silence = std::mem::take(&mut self.silence);
        let total = out_l.len().min(out_r.len());
        let mut done = 0;
        while done < total {
            let n = (total - done).min(self.max_block);
            self.process_io(&silence[..n], &silence[..n], &mut out_l[done..done + n], &mut out_r[done..done + n]);
            done += n;
        }
        self.silence = silence;
    }

    /// Comme `process`, avec l'entrée audio du pilote (`in_l`, `in_r`), qui alimente l'écoute et l'enregistrement
    /// des pistes armées. Les quatre tampons ont la même longueur utile (le plus court l'emporte).
    pub fn process_io(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        self.process_inputs(&[in_l, in_r], out_l, out_r);
    }

    /// Comme `process_io` avec autant de canaux d'entrée que le pilote en a ouvert (jusqu'à `MAX_INPUTS`) : chaque piste armée
    /// écoute et enregistre le ou les canaux qu'on lui a assignés. Tous les tampons ont la même longueur utile.
    pub fn process_inputs(&mut self, inputs: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32]) {
        let mut outs: [&mut [f32]; 2] = [out_l, out_r];
        self.process_outputs(inputs, &mut outs);
    }

    /// Comme `process_inputs`, avec **plusieurs canaux de sortie** : `outs[0]` et `outs[1]` sont la sortie principale (gauche, droite) ; les
    /// suivants (`outs[2]`...) reçoivent les bus qui ont une sortie physique (retours de monitoring) ; un canal que personne n'alimente est
    /// mis à zéro. Sans allocation (tableaux sur la pile).
    pub fn process_outputs(&mut self, inputs: &[&[f32]], outs: &mut [&mut [f32]]) {
        if outs.len() < 2 {
            return;
        }
        let (main, extra) = outs.split_at_mut(2);
        let (first, second) = main.split_at_mut(1);
        let (out_l, out_r): (&mut [f32], &mut [f32]) = (&mut *first[0], &mut *second[0]);
        for e in extra.iter_mut() {
            e.fill(0.0);
        }
        self.drain();
        if self.ahead.is_some() {
            // Désaccord de position avec un anneau : on rouvre une époque ; silences et désaccords sont comptés.
            if self.graph.take_resync() {
                self.ahead_invalidate();
            }
            let (underruns, desyncs) = self.graph.take_ahead_counts();
            if let Some(st) = &self.ahead_stats {
                if underruns > 0 {
                    st.underruns.fetch_add(underruns as u64, Ordering::Relaxed);
                }
                if desyncs > 0 {
                    st.desyncs.fetch_add(desyncs as u64, Ordering::Relaxed);
                }
            }
        }
        let inputs = &inputs[..inputs.len().min(MAX_INPUTS)];
        let total = inputs.iter().fold(out_l.len().min(out_r.len()), |m, s| m.min(s.len()));
        // Entrée MIDI : on la lit une fois, avec des décalages relatifs à cet appel (bornés à sa durée).
        self.midi_pending.clear();
        while self.midi_pending.len() < self.midi_pending.capacity() {
            match self.midi_rx.pop() {
                Ok(mut e) => {
                    e.offset = e.offset.min(total.saturating_sub(1) as u32);
                    self.midi_pending.push(e);
                }
                Err(_) => break,
            }
        }
        let mut done = 0;
        while done < total {
            let mut n = (total - done).min(self.max_block);
            // Un pré-compte coupe le bloc à sa fin : la lecture démarre à l'échantillon près.
            if let Some(left) = self.metro.count_in_remaining() {
                n = n.min(left.max(1) as usize);
            }
            let range = done..done + n;
            // Les tampons d'entrée de ce morceau (un tableau sur la pile : aucune allocation).
            let mut view: [&[f32]; MAX_INPUTS] = [&[]; MAX_INPUTS];
            for (c, slot) in view.iter_mut().enumerate().take(inputs.len()) {
                *slot = &inputs[c][range.clone()];
            }
            let view = &view[..inputs.len()];
            if self.playing {
                self.play_block(done, view, &mut out_l[range.clone()], &mut out_r[range.clone()], extra, range.start);
            } else {
                self.feed_midi(done, n);
                // À l'arrêt, le graphe tourne quand même (écoute en direct, queues d'effets, notes en direct) ;
                // les clips et les notes de clips se taisent et la tête de lecture ne bouge pas.
                self.graph.process_with(self.pool.as_ref(), self.pos, n, false, view, &mut out_l[range.clone()], &mut out_r[range.clone()]);
                Self::copy_extra(&self.graph, extra, range.start, n);
                // Pré-compte (la tête est immobile) et queues de clics ; à la fin du pré-compte, la lecture démarre.
                if self.metro.render(self.pos, n, false, &mut out_l[range.clone()], &mut out_r[range]) {
                    self.playing = true;
                    self.ahead_play_started();
                }
            }
            done += n;
        }
        self.shared_pos.store(self.pos, Ordering::Relaxed);
    }

    /// Livre à la piste les messages d'entrée dont l'instant tombe dans `[start, start + n[` de l'appel en cours.
    fn feed_midi(&mut self, start: usize, n: usize) {
        for k in 0..self.midi_pending.len() {
            let e = self.midi_pending[k];
            let o = e.offset as usize;
            if o >= start && o < start + n {
                self.graph.push_live_at(e.node as usize, e.msg, (o - start) as u32);
            }
        }
    }

    /// Copie les canaux supplémentaires (retours de monitoring) du dernier bloc du graphe vers les canaux de sortie demandés.
    fn copy_extra(graph: &CompiledGraph, extra: &mut [&mut [f32]], at: usize, n: usize) {
        for (k, out) in extra.iter_mut().enumerate().take(graph.extra_outputs()) {
            let n = n.min(out.len().saturating_sub(at));
            out[at..at + n].copy_from_slice(graph.extra_output(k, n));
        }
    }

    fn play_block(&mut self, base: usize, inputs: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32], extra: &mut [&mut [f32]], extra_at: usize) {
        let (mut off, mut left) = (0, out_l.len());
        while left > 0 {
            let seg = match self.loop_range {
                Some((_, end)) if self.pos < end => left.min((end - self.pos) as usize),
                _ => left,
            };
            let r = off..off + seg;
            self.feed_midi(base + off, seg);
            let mut view: [&[f32]; MAX_INPUTS] = [&[]; MAX_INPUTS];
            for (c, slot) in view.iter_mut().enumerate().take(inputs.len()) {
                *slot = &inputs[c][r.clone()];
            }
            self.graph.set_loop(self.loop_range);
            self.graph.process_with(self.pool.as_ref(), self.pos, seg, true, &view[..inputs.len()], &mut out_l[r.clone()], &mut out_r[r.clone()]);
            Self::copy_extra(&self.graph, extra, extra_at + off, seg);
            self.metro.render(self.pos, seg, true, &mut out_l[r.clone()], &mut out_r[r]);
            self.pos += seg as u64;
            off += seg;
            left -= seg;
            if let Some((start, end)) = self.loop_range {
                if self.pos == end {
                    self.pos = start;
                    // Les fins de notes situées après la boucle ne seront jamais atteintes : on coupe ce qui sonne.
                    self.graph.notes_off_all();
                }
            }
        }
    }
}

impl EngineHandle {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Envoie une commande ; `false` si la file est pleine (la commande est perdue).
    pub fn send(&mut self, c: Command) -> bool {
        self.tx.push(Msg::Cmd(c)).is_ok()
    }

    /// Valide, compile et envoie un graphe. Les paramètres du graphe repartent de ceux de `desc`. Les effets dont
    /// l'identifiant est déjà vivant sont conservés (adoptés), les autres sont créés par leur usine.
    pub fn load_graph(&mut self, desc: &GraphDesc) -> Result<(), LoadError> {
        self.load_inner(desc, None)
    }

    /// Compile puis envoie. `renew` : un effet vivant à recréer quand même, avec l'état à lui donner avant son activation.
    fn load_inner(&mut self, desc: &GraphDesc, renew: Option<(InsertId, Option<Vec<u8>>)>) -> Result<(), LoadError> {
        let live = &self.live;
        let renew_id = renew.as_ref().map(|r| r.0);
        let g = desc
            .compile_with_ahead(
                self.sample_rate,
                self.max_block,
                &|id| if Some(id) == renew_id { None } else { live.get(&id).copied() },
                &|id| renew.as_ref().filter(|r| r.0 == id).and_then(|r| r.1.clone()),
                self.ahead,
            )
            .map_err(LoadError::Graph)?;
        let mut g = g;
        g.set_meters(Arc::clone(&self.meters));
        let latency = g.latency();
        // Nouvelles tables : les effets de ce graphe (les autres partent avec l'ancien graphe).
        let (mut next_live, mut next_controls) = (HashMap::new(), HashMap::new());
        for n in &desc.nodes {
            for i in &n.inserts {
                let created = g.new_controls().iter().find(|(id, _)| *id == i.id).map(|(_, c)| c.clone());
                let lat = if self.live.contains_key(&i.id) && Some(i.id) != renew_id {
                    self.live[&i.id]
                } else {
                    g.insert_latency(i.id).unwrap_or(0)
                };
                next_live.insert(i.id, lat);
                if let Some(c) = created.or_else(|| self.controls.get(&i.id).cloned()) {
                    next_controls.insert(i.id, c);
                }
            }
        }
        self.tx.push(Msg::Graph(Box::new(g))).map_err(|_| LoadError::QueueFull)?;
        self.live = next_live;
        self.controls = next_controls;
        self.latency = latency;
        Ok(())
    }

    /// Oublie les effets vivants : le prochain `load_graph` crée **toutes** les instances à neuf au lieu de reprendre celles
    /// de même identifiant. À appeler avant de charger un autre projet (ses effets reçoivent les mêmes petits identifiants).
    pub fn forget_inserts(&mut self) {
        self.live.clear();
        self.controls.clear();
    }

    /// Niveaux de crête par nœud (voir `MeterBank::take`), pour l'interface.
    pub fn meters(&self) -> Arc<MeterBank> {
        Arc::clone(&self.meters)
    }

    /// Poignée de contrôle d'un effet vivant (sauvegarde d'état, etc.).
    pub fn insert_control(&self, id: InsertId) -> Option<Arc<dyn InsertControl>> {
        self.controls.get(&id).cloned()
    }

    /// Recrée l'effet `id` : instance neuve (latence et ports relus), créée avec l'état de l'ancienne instance appliqué
    /// avant son activation, puis échange du graphe. Les voix en cours sont coupées.
    pub fn restart_insert(&mut self, desc: &GraphDesc, id: InsertId) -> Result<(), LoadError> {
        let state = self.controls.get(&id).and_then(|c| c.save_state());
        self.load_inner(desc, Some((id, state)))
    }

    /// Relance les effets qui l'ont demandé (changement de latence, de ports). Retourne leur nombre ; la latence totale
    /// et la compensation sont recalculées par le nouveau graphe.
    pub fn poll_restarts(&mut self, desc: &GraphDesc) -> Result<usize, LoadError> {
        let mut wanted: Vec<InsertId> = self.controls.iter().filter(|(_, c)| c.restart_requested()).map(|(id, _)| *id).collect();
        wanted.sort_unstable();
        for id in &wanted {
            self.restart_insert(desc, *id)?;
        }
        Ok(wanted.len())
    }

    /// Latence totale du dernier graphe envoyé, en échantillons (compensation de latence comprise).
    pub fn latency_samples(&self) -> u32 {
        self.latency
    }

    /// Libère ce que le thread audio a rendu : graphes remplacés (leur nombre est retourné) et prises terminées.
    pub fn collect_garbage(&mut self) -> usize {
        let mut n = 0;
        while let Ok(g) = self.trash.pop() {
            if matches!(g, Garbage::Graph(_)) {
                n += 1;
            }
        }
        n
    }

    /// Côté producteur de l'entrée MIDI du moteur, à confier au fil du pilote (une seule fois : ensuite `None`).
    pub fn take_midi_input(&mut self) -> Option<MidiInputTx> {
        self.midi_tx.take().map(|producer| MidiInputTx { producer })
    }

    /// Commence à enregistrer les notes jouées en direct sur la piste `node` (entrée MIDI ou `Command::Midi`), avec leur
    /// position sur la ligne de temps. `capacity` : messages que la file peut contenir avant d'être lue.
    pub fn start_midi_record(&mut self, node: u32, capacity: usize) -> Result<MidiRecordReceiver, LoadError> {
        self.start_midi_record_window(node, capacity, None, None)
    }

    /// Comme `start_midi_record`, mais la prise ne commence qu'à la position `start_at` de la ligne de temps, et seulement pendant
    /// la lecture (pré-roll et pré-compte ne sont pas enregistrés).
    pub fn start_midi_record_at(&mut self, node: u32, capacity: usize, start_at: Option<u64>) -> Result<MidiRecordReceiver, LoadError> {
        self.start_midi_record_window(node, capacity, start_at, None)
    }

    /// Comme `start_midi_record_at`, avec une fin : la prise s'arrête **toute seule** à la position `end_at` (exclue) et se déclare
    /// terminée (`is_closed`) : punch-out. Les messages datés à partir de `end_at` ne sont pas enregistrés.
    pub fn start_midi_record_window(&mut self, node: u32, capacity: usize, start_at: Option<u64>, end_at: Option<u64>) -> Result<MidiRecordReceiver, LoadError> {
        let (tap, receiver) = new_midi_tap(capacity, start_at, end_at);
        self.tx.push(Msg::MidiTap { node, tap: Some(tap) }).map_err(|_| LoadError::QueueFull)?;
        Ok(receiver)
    }

    pub fn stop_midi_record(&mut self, node: u32) -> bool {
        self.tx.push(Msg::MidiTap { node, tap: None }).is_ok()
    }

    /// Commence à copier l'entrée audio de la piste `node` (sa source) dans une file lock-free de
    /// `buffer_seconds` secondes, à lire par un fil d'écriture (voir le crate `recorder`). L'armement de la piste
    /// (`Command::SetInput`) est indépendant : la prise enregistre même une piste non armée.
    pub fn start_record(&mut self, node: u32, source: RecordSource, buffer_seconds: f32) -> Result<RecordReceiver, LoadError> {
        self.start_record_window(node, source, buffer_seconds, None, None)
    }

    /// Comme `start_record`, mais la prise ne commence qu'à la position `start_at` de la ligne de temps (au premier échantillon
    /// près), et seulement pendant la lecture : le pré-roll et le pré-compte ne sont pas enregistrés.
    pub fn start_record_at(&mut self, node: u32, source: RecordSource, buffer_seconds: f32, start_at: Option<u64>) -> Result<RecordReceiver, LoadError> {
        self.start_record_window(node, source, buffer_seconds, start_at, None)
    }

    /// Comme `start_record_at`, avec une fin : la prise s'arrête **toute seule** à la position `end_at` (exclue, au dernier échantillon
    /// près) et se déclare terminée (`RecordStatus::closed`) : punch-out sans attendre un ordre du contrôle.
    pub fn start_record_window(&mut self, node: u32, source: RecordSource, buffer_seconds: f32, start_at: Option<u64>, end_at: Option<u64>) -> Result<RecordReceiver, LoadError> {
        let frames = (self.sample_rate as f32 * buffer_seconds.max(0.1)) as usize;
        let (tap, receiver) = new_tap_at(source, frames, self.sample_rate, start_at, end_at);
        self.tx.push(Msg::Tap { node, tap: Some(tap) }).map_err(|_| LoadError::QueueFull)?;
        Ok(receiver)
    }

    /// Arrête la prise de `node` : le moteur la retire, la déclare terminée (`RecordStatus::closed`) et la rend.
    pub fn stop_record(&mut self, node: u32) -> bool {
        self.tx.push(Msg::Tap { node, tap: None }).is_ok()
    }

    /// Remplace les temps du métronome (voir `ClickTrack`) ; les anciens sont libérés par `collect_garbage`.
    pub fn set_click_track(&mut self, track: ClickTrack) -> Result<(), LoadError> {
        self.tx.push(Msg::Click(Some(Box::new(track)))).map_err(|_| LoadError::QueueFull)
    }

    /// Démarre un pré-compte : le moteur s'arrête, la tête reste où elle est, les clics du pré-compte jouent, puis la lecture
    /// démarre toute seule à la même position. Un `Stop` pendant le pré-compte l'annule.
    pub fn start_count_in(&mut self, count_in: CountIn) -> Result<(), LoadError> {
        self.tx.push(Msg::CountIn(Some(Box::new(count_in)))).map_err(|_| LoadError::QueueFull)
    }

    /// Position de lecture (échantillons), mise à jour à chaque bloc.
    pub fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    /// Graphes perdus faute de place dans la file de rebut (doit rester 0).
    pub fn leaked_graphs(&self) -> u32 {
        self.leaks.load(Ordering::Relaxed)
    }
}
