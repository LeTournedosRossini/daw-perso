//! Calcul **en avance** des pistes de lecture (option B, voir `CLAUDE.md` tranche 42).
//!
//! Une piste de lecture (clips, notes, effets) ne dépend de rien qui arrive en direct : on peut la calculer **d'avance**, par gros
//! blocs, dans des fils dédiés, et ne laisser dans le callback au plus petit tampon que ce qui est vraiment en direct (pistes
//! armées, bus, sortie principale). Le callback lit alors le signal déjà calculé dans un anneau et applique lui-même le fader, le
//! panoramique, le muet, le solo, les envois et la compensation de latence : ces réglages réagissent donc **tout de suite**.
//!
//! Organisation :
//! - chaque piste éligible a une **jumelle** (`Twin`) qui reçoit, quand l'avance démarre, tout l'état de production de la piste (clips,
//!   notes, effets) par échange de pointeurs ; la jumelle appartient alors à un fil de rendu (`owner`), jusqu'à ce que le fil audio la
//!   reprenne (demande de **rendu**, acquittée par le fil de rendu à la fin de son bloc) ;
//! - le fil de rendu produit des blocs de `block` échantillons dans un anneau, tant que moins de `lookahead` échantillons sont en
//!   avance sur ce que le fil audio a lu ; il suit la **boucle** de lecture tout seul ;
//! - tout ce qui change la ligne de temps (déplacement, boucle, arrêt, nouveau graphe...) ouvre une nouvelle **époque** : les données
//!   de l'ancienne ne sont plus lues, la production repart de la nouvelle position ; en attendant, la piste est silencieuse (quelques
//!   ms) avec un petit fondu d'entrée.
//!
//! Conséquences voulues (à connaître) : au **Stop** les queues d'effets des pistes de lecture sont coupées (fondu court) ; un réglage de
//! **plugin** d'une piste de lecture se fait entendre avec le retard de l'avance ; éditer pendant la lecture remet à zéro la mémoire des
//! effets de ces pistes ; une piste armée pendant la lecture n'est « en direct » qu'après l'opération qui relance l'avance.

use std::cell::UnsafeCell;
use std::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering::*};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use super::{AheadState, CompiledGraph, NodeC, Phase, MAX_PENDING};
use crate::insert::InsertId;
use crate::midi::MidiMsg;

const TAG_SHIFT: u32 = 40;
const FRAMES_MASK: u64 = (1 << TAG_SHIFT) - 1;
const TAG_MASK: u64 = (1 << 24) - 1;
/// Fondu d'entrée (échantillons) à la première donnée d'une époque.
const FADE_IN: u32 = 64;

const OWNER_AUDIO: u8 = 0;
const OWNER_RENDER: u8 = 1;
const OWNER_RELEASE: u8 = 2;

/// Réglage du calcul en avance.
#[derive(Clone)]
pub struct AheadConfig {
    /// Avance, en millisecondes (50 par défaut côté application ; 100 en option).
    pub lookahead_ms: u32,
    /// Taille des blocs calculés en avance, en échantillons (256).
    pub block: usize,
    /// Nombre de fils de rendu (≥ 1).
    pub threads: usize,
    /// Appelé au début de chaque fil de rendu (priorité temps réel, par exemple).
    pub on_start: Option<Arc<dyn Fn(usize) + Send + Sync>>,
}

/// Ce dont la compilation d'un graphe a besoin pour fabriquer les jumelles.
#[derive(Clone, Copy, Debug)]
pub struct AheadParams {
    pub block: usize,
    pub ring_cap: usize,
}

/// Compteurs de diagnostic (lisibles de partout).
#[derive(Default)]
pub struct AheadStats {
    /// Blocs du fil audio rendus silencieux faute de données alors que la lecture avait déjà commencé (coupure audible).
    pub underruns: AtomicU64,
    /// Désaccords entre la position du moteur et celle de l'anneau (la relance est automatique).
    pub desyncs: AtomicU64,
    /// Blocs produits par les fils de rendu.
    pub produced_blocks: AtomicU64,
    /// Nombre d'époques ouvertes (déplacements, relances...).
    pub epochs: AtomicU64,
}

/// Époque : d'où et comment la lecture repart.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EpochRec {
    pub restart: u64,
    pub loop_s: u64,
    pub loop_e: u64,
    /// Remettre à zéro la mémoire des effets au début de l'époque (déplacement) ou continuer (démarrage de l'avance).
    pub reset: bool,
}

impl EpochRec {
    fn looped(&self) -> bool {
        self.loop_e > self.loop_s
    }

    /// Position de la ligne de temps du `k`-ième échantillon de l'époque, boucle comprise (comme `Engine::play_block`).
    pub fn pos_of(&self, k: u64) -> u64 {
        if self.looped() && self.restart < self.loop_e {
            let first = self.loop_e - self.restart;
            if k < first {
                self.restart + k
            } else {
                self.loop_s + (k - first) % (self.loop_e - self.loop_s)
            }
        } else {
            self.restart + k
        }
    }
}

/// Ce que le fil audio sait de l'époque pendant un bloc.
#[derive(Clone, Copy, Debug, Default)]
pub struct AheadView {
    pub epoch: u64,
    pub rec: EpochRec,
}

/// Commande vers une jumelle (appliquée par le fil de rendu entre deux blocs).
#[derive(Clone, Copy, Debug)]
pub enum TwinCmd {
    Param { insert: InsertId, param: u32, value: f64 },
    Bypass { insert: InsertId, bypass: bool },
    Midi { msg: MidiMsg, offset: u32 },
    Panic,
}

// ---- anneau --------------------------------------------------------------------------------------------------------------------

/// Anneau d'échantillons à un producteur (fil de rendu) et un consommateur (fil audio). Les compteurs portent le numéro d'époque : ce
/// qui date d'une autre époque est ignoré. Échantillons en `AtomicU32` (bits des flottants) : aucun `unsafe`.
pub(super) struct Ring {
    cap: usize,
    l: Box<[AtomicU32]>,
    r: Box<[AtomicU32]>,
    /// (époque, échantillons écrits depuis le début de l'époque).
    written: AtomicU64,
    /// (époque, échantillons lus par le fil audio).
    consumed: AtomicU64,
}

fn tag(epoch: u64) -> u64 {
    (epoch & TAG_MASK) << TAG_SHIFT
}

impl Ring {
    pub(super) fn new(cap: usize) -> Ring {
        let cap = cap.next_power_of_two().max(1024);
        let mk = || (0..cap).map(|_| AtomicU32::new(0)).collect::<Vec<_>>().into_boxed_slice();
        Ring { cap, l: mk(), r: mk(), written: AtomicU64::new(0), consumed: AtomicU64::new(0) }
    }

    // côté rendu
    fn start_epoch(&self, e: u64) {
        self.written.store(tag(e), Release);
    }

    fn in_flight(&self, e: u64) -> u64 {
        let w = self.written.load(Acquire);
        let w = if w & !FRAMES_MASK == tag(e) { w & FRAMES_MASK } else { 0 };
        let c = self.consumed.load(Acquire);
        let c = if c & !FRAMES_MASK == tag(e) { c & FRAMES_MASK } else { 0 };
        w.saturating_sub(c)
    }

    fn push(&self, e: u64, l: &[f32], r: &[f32]) {
        let w = self.written.load(Relaxed) & FRAMES_MASK;
        let mask = self.cap - 1;
        for (i, (a, b)) in l.iter().zip(r).enumerate() {
            let k = (w as usize + i) & mask;
            self.l[k].store(a.to_bits(), Relaxed);
            self.r[k].store(b.to_bits(), Relaxed);
        }
        self.written.store(tag(e) | (w + l.len() as u64), Release);
    }

    // côté audio
    fn readable(&self, e: u64, cursor: u64) -> u64 {
        let w = self.written.load(Acquire);
        if w & !FRAMES_MASK != tag(e) {
            return 0;
        }
        (w & FRAMES_MASK).saturating_sub(cursor)
    }

    fn read(&self, cursor: u64, out_l: &mut [f32], out_r: &mut [f32]) {
        let mask = self.cap - 1;
        for (i, (a, b)) in out_l.iter_mut().zip(out_r.iter_mut()).enumerate() {
            let k = (cursor as usize + i) & mask;
            *a = f32::from_bits(self.l[k].load(Relaxed));
            *b = f32::from_bits(self.r[k].load(Relaxed));
        }
    }

    fn mark_consumed(&self, e: u64, cursor: u64) {
        self.consumed.store(tag(e) | (cursor & FRAMES_MASK), Release);
    }
}

// ---- jumelle -------------------------------------------------------------------------------------------------------------------

pub(super) struct TwinInner {
    pub(super) node: NodeC,
    cmds: Consumer<TwinCmd>,
    epoch: u64,
    rec: EpochRec,
    pos: u64,
}

/// La jumelle d'une piste : porte l'état de production pendant que le fil de rendu calcule en avance.
pub(super) struct Twin {
    pub(super) owner: AtomicU8,
    pub(super) ring: Ring,
    inner: UnsafeCell<TwinInner>,
}

// SAFETY: `inner` n'est touché que par le propriétaire courant (`owner`) : le fil de rendu quand `owner == RENDER` ou `RELEASE`
// (jusqu'à son acquittement), le fil audio quand `owner == AUDIO`. Le passage de l'un à l'autre se fait par un échange atomique
// `Release` / `Acquire` sur `owner`, qui rend visibles toutes les écritures précédentes. `ring` ne contient que des atomiques.
unsafe impl Sync for Twin {}
unsafe impl Send for Twin {}

impl Twin {
    pub(super) fn new(node: NodeC, ring: Ring, cmds: Consumer<TwinCmd>) -> Arc<Twin> {
        Arc::new(Twin { owner: AtomicU8::new(OWNER_AUDIO), ring, inner: UnsafeCell::new(TwinInner { node, cmds, epoch: 0, rec: EpochRec::default(), pos: 0 }) })
    }

    /// # Safety
    /// L'appelant doit être le propriétaire courant de la jumelle (voir `owner`) et ne garder aucune autre référence à l'intérieur.
    pub(super) unsafe fn inner(&self) -> &mut TwinInner {
        &mut *self.inner.get()
    }
}

// ---- état partagé avec les fils de rendu ---------------------------------------------------------------------------------------

struct RecSlot {
    seq: AtomicU64,
    restart: AtomicU64,
    loop_s: AtomicU64,
    loop_e: AtomicU64,
    reset: AtomicBool,
}

impl RecSlot {
    fn new() -> RecSlot {
        RecSlot { seq: AtomicU64::new(0), restart: AtomicU64::new(0), loop_s: AtomicU64::new(0), loop_e: AtomicU64::new(0), reset: AtomicBool::new(false) }
    }
}

struct Shared {
    epoch: AtomicU64,
    recs: [RecSlot; 2],
    stop: AtomicBool,
    release_reset: AtomicBool,
    /// L'avance est en marche : les fils de rendu restent en attente active (sinon ils dorment par courtes pauses).
    active: AtomicBool,
    lookahead: u64,
    block: u64,
    stats: Arc<AheadStats>,
}

impl Shared {
    fn publish(&self, e: u64, rec: EpochRec) {
        let slot = &self.recs[(e & 1) as usize];
        slot.seq.store(u64::MAX, Relaxed);
        fence(Release);
        slot.restart.store(rec.restart, Relaxed);
        slot.loop_s.store(rec.loop_s, Relaxed);
        slot.loop_e.store(rec.loop_e, Relaxed);
        slot.reset.store(rec.reset, Relaxed);
        slot.seq.store(e, Release);
        self.epoch.store(e, Release);
        self.stats.epochs.fetch_add(1, Relaxed);
    }

    fn read_rec(&self, e: u64) -> Option<EpochRec> {
        let slot = &self.recs[(e & 1) as usize];
        if slot.seq.load(Acquire) != e {
            return None;
        }
        let rec = EpochRec { restart: slot.restart.load(Relaxed), loop_s: slot.loop_s.load(Relaxed), loop_e: slot.loop_e.load(Relaxed), reset: slot.reset.load(Relaxed) };
        fence(Acquire);
        (slot.seq.load(Relaxed) == e).then_some(rec)
    }
}

enum ToRender {
    Add(Arc<Twin>),
    Clear,
}

/// Applique les commandes en attente de la jumelle (entre deux blocs).
fn apply_cmds(inner: &mut TwinInner) {
    while let Ok(c) = inner.cmds.pop() {
        match c {
            TwinCmd::Param { insert, param, value } => {
                if let Some(e) = inner.node.inserts.iter_mut().find(|i| i.id == insert).and_then(|i| i.effect.as_mut()) {
                    e.set_param(param, value);
                }
            }
            TwinCmd::Bypass { insert, bypass } => {
                if let Some(i) = inner.node.inserts.iter_mut().find(|i| i.id == insert) {
                    i.bypass = bypass;
                }
            }
            TwinCmd::Midi { msg, offset } => {
                if inner.node.live.len() < MAX_PENDING {
                    inner.node.live.push((offset, msg));
                }
            }
            TwinCmd::Panic => {
                inner.node.panic();
            }
        }
    }
}

/// Un tour de service d'une jumelle par son fil de rendu ; vrai s'il y a eu du travail.
fn service(shared: &Shared, twin: &Twin) -> bool {
    match twin.owner.load(Acquire) {
        OWNER_RENDER => {}
        OWNER_RELEASE => {
            // SAFETY: tant qu'on n'a pas acquitté, la jumelle est encore à nous.
            let inner = unsafe { twin.inner() };
            apply_cmds(inner);
            if shared.release_reset.load(Relaxed) {
                inner.node.reset_production();
            }
            inner.epoch = 0;
            twin.owner.store(OWNER_AUDIO, Release);
            return true;
        }
        _ => return false,
    }
    // SAFETY: `owner == RENDER` : la jumelle est à ce fil.
    let inner = unsafe { twin.inner() };
    let e = shared.epoch.load(Acquire);
    if inner.epoch != e {
        let Some(rec) = shared.read_rec(e) else { return false };
        inner.epoch = e;
        inner.rec = rec;
        inner.pos = rec.restart;
        if rec.reset {
            inner.node.reset_production();
        }
        twin.ring.start_epoch(e);
    }
    apply_cmds(inner);
    let mut did = false;
    while twin.ring.in_flight(e) < shared.lookahead {
        let rec = inner.rec;
        let to_end = if rec.looped() && inner.pos < rec.loop_e { rec.loop_e - inner.pos } else { u64::MAX };
        let n = shared.block.min(to_end) as usize;
        inner.node.produce(inner.pos, n);
        twin.ring.push(e, &inner.node.l[..n], &inner.node.r[..n]);
        inner.pos += n as u64;
        if rec.looped() && inner.pos == rec.loop_e {
            // Comme le moteur au retour de boucle : ce qui sonne reçoit sa fin de note.
            inner.pos = rec.loop_s;
            inner.node.release_all();
        }
        shared.stats.produced_blocks.fetch_add(1, Relaxed);
        did = true;
        if shared.epoch.load(Relaxed) != e || twin.owner.load(Relaxed) != OWNER_RENDER {
            break;
        }
        apply_cmds(inner);
    }
    did
}

fn render_main(shared: Arc<Shared>, mut rx: Consumer<ToRender>, me: usize, on_start: Option<Arc<dyn Fn(usize) + Send + Sync>>) {
    if let Some(f) = on_start {
        f(me);
    }
    let mut mine: Vec<Arc<Twin>> = Vec::new();
    while !shared.stop.load(Relaxed) {
        while let Ok(m) = rx.pop() {
            match m {
                ToRender::Add(t) => mine.push(t),
                ToRender::Clear => mine.clear(),
            }
        }
        let mut did = false;
        for t in &mine {
            did |= service(&shared, t);
        }
        if !did {
            if shared.active.load(Relaxed) {
                std::hint::spin_loop();
            } else {
                std::thread::sleep(Duration::from_micros(500));
            }
        }
    }
}

// ---- système (côté moteur) -----------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SysPhase {
    Idle,
    Active,
    Releasing,
}

/// Le calcul en avance, vu du fil audio : lance les fils de rendu et pilote les époques et le passage des pistes de l'un à l'autre.
pub struct AheadSystem {
    shared: Arc<Shared>,
    txs: Vec<Producer<ToRender>>,
    threads: Vec<JoinHandle<()>>,
    epoch: u64,
    phase: SysPhase,
    params: AheadParams,
}

impl AheadSystem {
    /// Lance les fils de rendu. À faire hors du fil audio.
    pub fn new(cfg: &AheadConfig, sample_rate: u32) -> AheadSystem {
        let block = cfg.block.clamp(32, 4096);
        let lookahead = (cfg.lookahead_ms.clamp(5, 1000) as u64 * sample_rate as u64 / 1000).max(block as u64);
        let ring_cap = (lookahead as usize + 3 * block).next_power_of_two();
        let shared = Arc::new(Shared {
            epoch: AtomicU64::new(0),
            recs: [RecSlot::new(), RecSlot::new()],
            stop: AtomicBool::new(false),
            release_reset: AtomicBool::new(true),
            active: AtomicBool::new(false),
            lookahead,
            block: block as u64,
            stats: Arc::new(AheadStats::default()),
        });
        let (mut txs, mut threads) = (Vec::new(), Vec::new());
        for me in 0..cfg.threads.clamp(1, 16) {
            let (tx, rx) = RingBuffer::<ToRender>::new(1024);
            let (s, hook) = (Arc::clone(&shared), cfg.on_start.clone());
            threads.push(std::thread::Builder::new().name(format!("daw-avance-{me}")).spawn(move || render_main(s, rx, me, hook)).expect("fil de rendu"));
            txs.push(tx);
        }
        AheadSystem { shared, txs, threads, epoch: 0, phase: SysPhase::Idle, params: AheadParams { block, ring_cap } }
    }

    /// De quoi fabriquer les jumelles à la compilation d'un graphe.
    pub fn params(&self) -> AheadParams {
        self.params
    }

    pub fn stats(&self) -> Arc<AheadStats> {
        Arc::clone(&self.shared.stats)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.phase != SysPhase::Idle
    }

    pub(crate) fn is_releasing(&self) -> bool {
        self.phase == SysPhase::Releasing
    }

    fn open_epoch(&mut self, g: &mut CompiledGraph, pos: u64, lp: Option<(u64, u64)>, reset: bool) {
        self.epoch += 1;
        let (loop_s, loop_e) = lp.unwrap_or((0, 0));
        let rec = EpochRec { restart: pos, loop_s, loop_e, reset };
        self.shared.publish(self.epoch, rec);
        g.ahead_epoch = self.epoch;
        g.ahead_rec = rec;
    }

    /// Démarre l'avance : les pistes éligibles de `g` passent aux fils de rendu, qui calculent à partir de `pos`. Sans effet si déjà en
    /// marche, ou s'il n'y a aucune piste éligible.
    pub(crate) fn start(&mut self, g: &mut CompiledGraph, pos: u64, lp: Option<(u64, u64)>) {
        if self.phase != SysPhase::Idle {
            return;
        }
        let eligible: usize = g.nodes.iter().filter(|n| n.ahead.phase == Phase::Live && n.ahead_eligible()).count();
        if eligible == 0 {
            return;
        }
        self.open_epoch(g, pos, lp, false);
        let threads = self.txs.len();
        let mut k = 0;
        for node in g.nodes.iter_mut() {
            if node.ahead.phase != Phase::Live || !node.ahead_eligible() {
                continue;
            }
            let Some(twin) = node.twin.clone() else { continue };
            if twin.owner.load(Acquire) != OWNER_AUDIO {
                continue;
            }
            // SAFETY: `owner == AUDIO` : la jumelle est à nous jusqu'au `store(RENDER)` ci-dessous.
            let inner = unsafe { twin.inner() };
            node.swap_production(&mut inner.node);
            node.ahead = AheadState { phase: Phase::Ahead, epoch: 0, cursor: 0, started: false, fade_in: FADE_IN, faded_out: false, underruns: 0, desync: false };
            if self.txs[k % threads].push(ToRender::Add(Arc::clone(&twin))).is_err() {
                // File pleine (ne devrait pas arriver) : on reprend la piste telle quelle.
                node.swap_production(&mut inner.node);
                node.ahead = AheadState::default();
                continue;
            }
            twin.owner.store(OWNER_RENDER, Release);
            k += 1;
        }
        self.shared.active.store(true, Relaxed);
        self.phase = SysPhase::Active;
    }

    /// Nouvelle époque (déplacement, boucle modifiée...) : la production repart de `pos`, mémoire des effets remise à zéro si `reset`.
    pub(crate) fn invalidate(&mut self, g: &mut CompiledGraph, pos: u64, lp: Option<(u64, u64)>, reset: bool) {
        if self.phase == SysPhase::Active {
            self.open_epoch(g, pos, lp, reset);
        }
    }

    /// Demande aux fils de rendu de rendre les jumelles (arrêt, nouveau graphe, armement...). `reset` : remettre à zéro la mémoire des
    /// effets avant de les rendre. À appeler jusqu'à ce que `poll_release` rende vrai.
    pub(crate) fn begin_release(&mut self, g: &mut CompiledGraph, reset: bool) {
        if self.phase != SysPhase::Active {
            return;
        }
        self.shared.release_reset.store(reset, Relaxed);
        for node in g.nodes.iter_mut() {
            if node.ahead.phase == Phase::Ahead {
                if let Some(t) = &node.twin {
                    t.owner.store(OWNER_RELEASE, Release);
                }
                node.ahead.phase = Phase::Releasing;
            }
        }
        self.phase = SysPhase::Releasing;
    }

    /// Reprend les jumelles que les fils de rendu ont rendues ; vrai quand toutes l'ont été (le système est alors au repos).
    pub(crate) fn poll_release(&mut self, g: &mut CompiledGraph) -> bool {
        if self.phase != SysPhase::Releasing {
            return true;
        }
        let mut pending = 0;
        for node in g.nodes.iter_mut() {
            if node.ahead.phase != Phase::Releasing {
                continue;
            }
            let Some(twin) = node.twin.clone() else { continue };
            if twin.owner.load(Acquire) == OWNER_AUDIO {
                // SAFETY: `owner == AUDIO` : acquitté par le fil de rendu, la jumelle est de nouveau à nous.
                let inner = unsafe { twin.inner() };
                node.swap_production(&mut inner.node);
                node.ahead = AheadState::default();
            } else {
                pending += 1;
            }
        }
        if pending > 0 {
            return false;
        }
        for tx in &mut self.txs {
            let _ = tx.push(ToRender::Clear);
        }
        self.phase = SysPhase::Idle;
        self.shared.active.store(false, Relaxed);
        true
    }
}

impl Drop for AheadSystem {
    fn drop(&mut self) {
        self.shared.stop.store(true, Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

// ---- côté nœud / graphe --------------------------------------------------------------------------------------------------------

impl NodeC {
    /// Une jumelle vide pour ce nœud (mêmes réserves de notes que lui ; les vecteurs s'échangent avec les siens).
    pub(super) fn twin_shell(&self, size: usize) -> NodeC {
        NodeC {
            kind: self.kind,
            law: self.law,
            gain: super::Smoother::new(0.0),
            pan: super::Smoother::new(0.0),
            user_gain: 1.0,
            invert: false,
            auto_gain: None,
            auto_pan: None,
            auto_params: Vec::new(),
            chunk_events: Vec::with_capacity(self.chunk_events.capacity()),
            mute: false,
            solo: false,
            solo_excluded: false,
            output: 0,
            phys: None,
            is_return: false,
            out_delay: super::DelayLine::new(0),
            sends: Vec::new(),
            pre_l: Vec::new(),
            pre_r: Vec::new(),
            clips: Vec::new(),
            inserts: Vec::new(),
            midi: Vec::new(),
            events: Vec::with_capacity(self.events.capacity()),
            live: Vec::with_capacity(self.live.capacity()),
            panic_cursor: super::PANIC_SWEEP,
            midi_tap: None,
            midi_seen: 0,
            vel_in: 0,
            vel_table: self.vel_table.clone(),
            active: crate::midi::ActiveNotes::new(),
            dyn_cfg: Default::default(),
            dyn_state: None,
            dyn_a: Vec::new(),
            dyn_b: Vec::new(),
            armed: false,
            monitor: false,
            source: super::RecordSource::Left,
            in_peak: [0.0; 2],
            tap: None,
            l: vec![0.0; size],
            r: vec![0.0; size],
            scratch_l: vec![0.0; size],
            scratch_r: vec![0.0; size],
            block_dropped: 0,
            cost_ns: 0.0,
            twin: None,
            twin_tx: None,
            twin_inserts: Vec::new(),
            time: Arc::clone(&self.time),
            chords: Arc::clone(&self.chords),
            sr: self.sr,
            playing_now: true,
            ahead: AheadState::default(),
        }
    }

    /// Lecture de l'anneau de la jumelle à la place du calcul (voir `compute`).
    pub(super) fn read_ahead(&mut self, pos: u64, n: usize, playing: bool, view: AheadView) {
        let Some(twin) = self.twin.as_ref() else { return };
        if self.ahead.epoch != view.epoch {
            self.ahead.epoch = view.epoch;
            self.ahead.cursor = 0;
            self.ahead.started = false;
            self.ahead.fade_in = FADE_IN;
            self.ahead.faded_out = false;
        }
        let st = &mut self.ahead;
        if !playing {
            // Arrêt : un seul fondu de sortie sur ce bloc (si des données sont là), puis silence.
            if st.started && !st.faded_out && twin.ring.readable(view.epoch, st.cursor) >= n as u64 {
                twin.ring.read(st.cursor, &mut self.l[..n], &mut self.r[..n]);
                for j in 0..n {
                    let g = 1.0 - (j + 1) as f32 / n as f32;
                    self.l[j] *= g;
                    self.r[j] *= g;
                }
                st.cursor += n as u64;
                twin.ring.mark_consumed(view.epoch, st.cursor);
            }
            st.faded_out = true;
            return;
        }
        let expected = view.rec.pos_of(st.cursor);
        if expected != pos {
            // La position du moteur et celle de l'anneau ne coïncident plus : silence, et le moteur relance l'avance.
            st.desync = true;
            st.cursor += n as u64;
            return;
        }
        let avail = twin.ring.readable(view.epoch, st.cursor);
        if avail >= n as u64 {
            twin.ring.read(st.cursor, &mut self.l[..n], &mut self.r[..n]);
            if st.fade_in > 0 {
                let total = FADE_IN as f32;
                for j in 0..n.min(st.fade_in as usize) {
                    let g = (FADE_IN - st.fade_in + j as u32 + 1) as f32 / total;
                    self.l[j] *= g;
                    self.r[j] *= g;
                }
                st.fade_in = st.fade_in.saturating_sub(n as u32);
            }
            st.started = true;
        } else if st.started {
            st.underruns += 1; // coupure : les fils de rendu n'ont pas suivi
        }
        // Le curseur avance de toute façon : il suit la position du moteur ; les données produites en retard sont simplement ignorées.
        st.cursor += n as u64;
        twin.ring.mark_consumed(view.epoch, st.cursor);
    }
}

impl CompiledGraph {
    /// Le nœud `node` est-il (ou est-il en train de cesser d'être) calculé en avance ?
    pub(crate) fn node_is_ahead(&self, node: usize) -> bool {
        self.nodes.get(node).is_some_and(|n| n.ahead.phase != Phase::Live)
    }

    /// Le moteur a-t-il perdu l'accord avec un anneau (une seule fois) ?
    pub(crate) fn take_resync(&mut self) -> bool {
        std::mem::take(&mut self.resync)
    }

    /// Silences et désaccords constatés depuis le dernier appel (pour les compteurs du moteur).
    pub(crate) fn take_ahead_counts(&mut self) -> (u32, u32) {
        (std::mem::take(&mut self.ahead_underruns), std::mem::take(&mut self.ahead_desyncs))
    }

    /// Toutes les pistes calculées en avance ont-elles au moins `n` échantillons prêts à lire ? (vrai s'il n'y en a pas) — pour les tests
    /// et les rendus qui veulent attendre les fils de rendu au lieu de laisser des silences.
    pub(crate) fn ahead_ready(&self, n: usize) -> bool {
        self.nodes.iter().filter(|nd| nd.ahead.phase == Phase::Ahead).all(|nd| {
            let cursor = if nd.ahead.epoch == self.ahead_epoch { nd.ahead.cursor } else { 0 };
            nd.twin.as_ref().is_some_and(|t| t.ring.readable(self.ahead_epoch, cursor) >= n as u64)
        })
    }

    /// Ajoute les compteurs des nœuds lus en avance pendant le dernier niveau calculé.
    pub(super) fn collect_ahead_flags(&mut self, start: usize, end: usize) {
        for k in start..end {
            let i = self.order[k];
            let a = &mut self.nodes[i].ahead;
            self.ahead_underruns += std::mem::take(&mut a.underruns);
            if std::mem::take(&mut a.desync) {
                self.resync = true;
                self.ahead_desyncs += 1;
            }
        }
    }

    /// Fabrique la jumelle de chaque piste qui a de quoi faire (effets ou notes) ; à la compilation, hors du fil audio.
    pub(super) fn make_twins(&mut self, params: AheadParams) {
        for node in &mut self.nodes {
            // Une piste dont un effet reçoit une chaîne latérale se calcule dans le callback : le signal de sa source n'existe que là.
            if !node.is_track() || (node.inserts.is_empty() && node.midi.is_empty()) || node.inserts.iter().any(|i| i.sc.is_some()) {
                continue;
            }
            let size = node.l.len().max(params.block);
            let (tx, rx) = RingBuffer::<TwinCmd>::new(512);
            node.twin_inserts = node.inserts.iter().map(|i| i.id).collect();
            node.twin = Some(Twin::new(node.twin_shell(size), Ring::new(params.ring_cap), rx));
            node.twin_tx = Some(tx);
        }
    }
}
