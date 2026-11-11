//! Graphe prêt pour le thread audio : tampons pré-alloués, ordre fixé, paramètres lissés, effets insérés et
//! compensation de latence.
//!
//! Rien ici n'alloue, ne bloque ni ne panique une fois construit. Les accès par indice sont gardés
//! (`get_mut`) pour qu'une commande invalide soit ignorée au lieu de paniquer.

mod ahead;

use daw_core_types::{pan_gains, PanLaw};

use crate::audio::Clip;
use crate::automation::{AutoLane, AutoMode, AutoTarget, AUTO_GRID};
use crate::graph::{GraphDesc, GraphError, NodeKind, TrackFormat};
use std::sync::Arc;

use crate::insert::{Insert, InsertControl, InsertId};
use crate::midi::{expand, ActiveNotes, MidiEvent, MidiMsg, TimedMsg};
use crate::meters::MeterBank;
use crate::midi_in::MidiTap;
use crate::parallel::Pool;
use crate::time::TimeShared;
pub use ahead::{AheadConfig, AheadParams, AheadStats, AheadSystem};
use ahead::{AheadView, EpochRec, Twin, TwinCmd};
use crate::record::{sample, RecordSource, RecordTap};
use crate::smoother::Smoother;

/// Durée des rampes de paramètres : 5 ms (au moins 1 échantillon).
fn ramp_frames(sample_rate: u32) -> u32 {
    (sample_rate / 200).max(1)
}

/// Événements de notes gardés par bloc et par piste (au-delà : perdus et comptés).
const MAX_BLOCK_EVENTS: usize = 1024;
/// Messages en attente (notes jouées en direct, coupures de notes bloquées) par piste.
const MAX_PENDING: usize = 512;
/// Balayage de la panique : une fin de note par touche et par canal (16 × 128), `PANIC_PER_BLOCK` par bloc (sans dépasser la
/// capacité des listes d'événements).
const PANIC_SWEEP: u32 = 16 * 128;
const PANIC_PER_BLOCK: u32 = 256;

/// Ligne à retard stéréo de longueur fixe (0 = transparent), pour aligner les chemins de latence différente.
struct DelayLine {
    l: Vec<f32>,
    r: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    fn new(len: usize) -> Self {
        DelayLine { l: vec![0.0; len], r: vec![0.0; len], pos: 0 }
    }

    /// Insère (l, r), ressort l'échantillon d'il y a `len` échantillons.
    #[inline]
    fn tap(&mut self, l: f32, r: f32) -> (f32, f32) {
        let len = self.l.len();
        if len == 0 {
            return (l, r);
        }
        let out = (self.l[self.pos], self.r[self.pos]);
        self.l[self.pos] = l;
        self.r[self.pos] = r;
        self.pos += 1;
        if self.pos == len {
            self.pos = 0;
        }
        out
    }

    fn clear(&mut self) {
        self.l.fill(0.0);
        self.r.fill(0.0);
        self.pos = 0;
    }
}

struct SendC {
    to: usize,
    gain: Smoother,
    /// Courbe d'automation du niveau (absente : réglage manuel seul).
    auto: Option<AutoLane>,
    delay: DelayLine,
    /// Pris avant le fader (le signal vient de `pre_l`/`pre_r` du nœud).
    pre: bool,
    /// Envoi coupé / en solo dans sa destination.
    mute: bool,
    solo: bool,
    /// Audibilité lissée (1 ou 0) : muet, et solo d'un autre envoi vers la même destination.
    aud: Smoother,
    /// Envoi issu d'un port de sortie auxiliaire d'un plugin : indice dans `NodeC::aux` du tampon qui sert de source.
    aux: Option<usize>,
}

/// Ce qu'un port de sortie auxiliaire d'un effet a produit pendant le bloc en cours (rempli juste après l'effet, lu au routage).
struct AuxBuf {
    insert: usize,
    port: usize,
    l: Vec<f32>,
    r: Vec<f32>,
}

/// Les sorties auxiliaires branchées d'un nœud : `(indice de l'effet, indice du port auxiliaire, nœud destinataire)`.
fn aux_dests(d: &crate::graph::NodeDesc) -> Vec<(usize, usize, usize)> {
    d.inserts.iter().enumerate().flat_map(|(k, ins)| ins.aux.iter().enumerate().filter_map(move |(p, t)| t.map(|t| (k, p, t.0 as usize)))).collect()
}

struct InsertC {
    id: InsertId,
    latency: u32,
    /// `None` tant que l'instance n'a pas été adoptée du graphe précédent (passe-plat entre-temps).
    effect: Option<Box<dyn Insert>>,
    /// Contournement : cible, et position de la rampe entre l'effet (0) et le signal sec retardé (1).
    bypass: bool,
    mix: f32,
    /// Signal d'entrée retardé de `latency` échantillons (anneau pré-alloué), pour que le contournement reste aligné.
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    delay_pos: usize,
    /// Chaîne latérale de cet effet (second port d'entrée) : source, retard d'alignement et tampons du bloc.
    sc: Option<Sidechain>,
}

/// Chaîne latérale d'un effet : le signal d'un autre nœud (avant son fader), retardé pour rester aligné sur le signal principal de
/// l'effet, copié dans `buf_*` quand ce nœud a fini son bloc.
struct Sidechain {
    delay: DelayLine,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
}

/// Durée de la rampe entre l'effet et le signal sec quand on contourne ou réactive (en échantillons) : évite le clic.
const BYPASS_RAMP: f32 = 256.0;

impl InsertC {
    fn new(id: InsertId, latency: u32, effect: Option<Box<dyn Insert>>, bypass: bool) -> InsertC {
        InsertC { id, latency, effect, bypass, mix: if bypass { 1.0 } else { 0.0 }, delay_l: vec![0.0; latency as usize], delay_r: vec![0.0; latency as usize], delay_pos: 0, sc: None }
    }

    /// Mélange la sortie de l'effet (`out_*`, modifiée sur place) avec l'entrée retardée selon la rampe de contournement.
    fn apply_bypass(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        let lat = self.delay_l.len();
        let target = if self.bypass { 1.0 } else { 0.0 };
        if lat == 0 && self.mix == target && !self.bypass {
            return; // le cas courant : rien à faire
        }
        for j in 0..in_l.len() {
            let (dl, dr) = if lat > 0 {
                let d = (self.delay_l[self.delay_pos], self.delay_r[self.delay_pos]);
                self.delay_l[self.delay_pos] = in_l[j];
                self.delay_r[self.delay_pos] = in_r[j];
                self.delay_pos += 1;
                if self.delay_pos == lat {
                    self.delay_pos = 0;
                }
                d
            } else {
                (in_l[j], in_r[j])
            };
            if self.mix != target {
                let step = 1.0 / BYPASS_RAMP;
                self.mix = if self.mix < target { (self.mix + step).min(target) } else { (self.mix - step).max(target) };
            }
            let m = self.mix;
            out_l[j] = out_l[j] * (1.0 - m) + dl * m;
            out_r[j] = out_r[j] * (1.0 - m) + dr * m;
        }
    }
}

/// Paramètre d'effet automatisé : `insert` est l'indice dans la chaîne du nœud, `last` la dernière valeur envoyée.
struct ParamAuto {
    insert: usize,
    param: u32,
    lane: AutoLane,
    last: f64,
}

struct NodeC {
    kind: NodeKind,
    law: PanLaw,
    gain: Smoother,
    pan: Smoother,
    user_gain: f32,
    /// Polarité inversée : le gain effectif est négatif.
    invert: bool,
    /// Courbes de gain et de panoramique : elles remplacent le réglage manuel.
    auto_gain: Option<AutoLane>,
    auto_pan: Option<AutoLane>,
    auto_params: Vec<ParamAuto>,
    /// Notes du bloc re-datées pour une tranche (effets automatisés seulement).
    chunk_events: Vec<MidiEvent>,
    mute: bool,
    solo: bool,
    /// Vrai si un autre nœud est en solo et pas celui-ci (pistes seulement).
    solo_excluded: bool,
    output: usize,
    /// Sortie physique du bus (voir `PhysOut`) : il n'alimente alors pas `output`.
    phys: Option<crate::graph::PhysOut>,
    /// Retour de monitoring : n'alimente jamais la sortie principale (muet sans `phys`).
    is_return: bool,
    out_delay: DelayLine,
    sends: Vec<SendC>,
    aux: Vec<AuxBuf>,
    /// Signal avant le fader, copié seulement si un envoi « avant fader » existe (sinon vides : rien d'alloué).
    pre_l: Vec<f32>,
    pre_r: Vec<f32>,
    clips: Vec<Clip>,
    inserts: Vec<InsertC>,
    /// Notes des clips MIDI de la piste, triées, en positions absolues.
    midi: Vec<TimedMsg>,
    /// Notes du bloc en cours (capacité fixe).
    events: Vec<MidiEvent>,
    /// Messages à livrer au début du prochain bloc (capacité fixe).
    live: Vec<(u32, MidiMsg)>,
    /// Balayage « panique » en cours : prochain indice (canal × 128 + touche) de fin de note à envoyer, `PANIC_SWEEP` = fini.
    panic_cursor: u32,
    /// Courbe de réponse de la vélocité des débuts de note (`None` : désactivée, les notes passent telles quelles).
    vel_table: Option<Box<[u8; 128]>>,
    /// Types de messages en direct reçus pendant le bloc (bits `meters::MIDI_*`), remis à zéro après publication.
    midi_seen: u8,
    /// Vélocité brute (1 à 127) de la dernière note jouée en direct pendant le bloc (0 : aucune).
    vel_in: u8,
    /// Prise des notes jouées en direct (enregistrement MIDI).
    midi_tap: Option<MidiTap>,
    active: ActiveNotes,
    /// Armement : entrée audio de la piste, écoute (monitoring) et canal(aux) d'entrée utilisé(s).
    armed: bool,
    monitor: bool,
    source: RecordSource,
    /// Dynamique d'entrée (porte + compresseur) : réglages, état (`None` : désactivée) et tampons du signal traité.
    dyn_cfg: crate::dynamics::InputDynamics,
    dyn_state: Option<crate::dynamics::DynState>,
    dyn_a: Vec<f32>,
    dyn_b: Vec<f32>,
    /// Crête de l'entrée écoutée sur le dernier bloc (pistes armées) : sert de vumètre quand on n'écoute pas l'entrée.
    in_peak: [f32; 2],
    /// Prise d'enregistrement en cours (pistes seulement).
    tap: Option<RecordTap>,
    l: Vec<f32>,
    r: Vec<f32>,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
    /// Notes perdues sur le dernier bloc (somme faite par le fil audio après le calcul du niveau).
    block_dropped: u32,
    /// Coût moyen (lissé) du calcul de ce nœud, en nanosecondes : sert à répartir les nœuds entre les fils.
    cost_ns: f32,
    /// Calcul **en avance** (voir `ahead.rs`) : la jumelle de ce nœud (qui reçoit l'état de production : clips, notes, effets), la file
    /// de commandes vers elle, les identifiants des effets qu'elle porte, et l'état de ce nœud vis-à-vis d'elle.
    twin: Option<Arc<Twin>>,
    twin_tx: Option<rtrb::Producer<TwinCmd>>,
    twin_inserts: Vec<InsertId>,
    ahead: AheadState,
    /// Transport donné aux plugins : carte des temps et boucle partagées par tous les nœuds, fréquence, et si on joue.
    time: Arc<TimeShared>,
    /// Piste d'accords du graphe (partagée), donnée aux plugins avant chaque traitement.
    chords: Arc<crate::chords::ChordSnapshot>,
    sr: u32,
    playing_now: bool,
}

/// Où en est un nœud qui a une jumelle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Phase {
    /// Le fil audio calcule le nœud lui-même (comme sans avance).
    #[default]
    Live,
    /// La jumelle calcule en avance ; le fil audio lit son anneau.
    Ahead,
    /// Rendu demandé au fil audio : la jumelle finit son bloc puis rend la main ; en attendant, le fil audio lit l'anneau tant qu'il
    /// est valide, sinon silence.
    Releasing,
}

/// État, côté fil audio, de la lecture de l'anneau d'un nœud.
#[derive(Clone, Copy, Default)]
struct AheadState {
    phase: Phase,
    /// Époque dans laquelle on lit, nombre d'échantillons déjà lus dans cette époque, des données sont-elles déjà arrivées, fondu
    /// d'entrée restant (échantillons), fondu de sortie déjà fait (arrêt).
    epoch: u64,
    cursor: u64,
    started: bool,
    fade_in: u32,
    faded_out: bool,
    /// Silences faute de données et désaccord de position constatés sur le dernier bloc (relevés par le graphe).
    underruns: u32,
    desync: bool,
}

// Un nœud est calculé par le fil audio ou par un fil de travail : tout ce qu'il contient doit pouvoir changer de fil.
const _: fn() = || {
    fn is_send<T: Send>() {}
    is_send::<NodeC>();
};

/// Passe le début d'une note par la courbe de vélocité de la piste (les autres messages et les fins de note ne changent pas).
#[inline]
fn shape_note(table: &Option<Box<[u8; 128]>>, m: MidiMsg) -> MidiMsg {
    match (table, m) {
        (Some(t), MidiMsg::NoteOn { channel, key, velocity }) => MidiMsg::NoteOn { channel, key, velocity: crate::velocity::shape(t, velocity) },
        _ => m,
    }
}

impl NodeC {
    fn effective_gain(&self) -> f32 {
        let g = if self.mute || self.solo_excluded {
            0.0
        } else if self.auto_gain.as_ref().is_some_and(|l| l.mode == AutoMode::Read) {
            1.0 // le gain vient de la courbe ; ce lisseur ne porte que mute et solo (en mode Trim il porte aussi le fader)
        } else {
            self.user_gain
        };
        // Polarité inversée : gain négatif (jamais −0,0 : un nœud muet reste en silence strict).
        if self.invert && g != 0.0 { -g } else { g }
    }

    fn is_track(&self) -> bool {
        matches!(self.kind, NodeKind::Track(_))
    }

    fn render_clips(&mut self, pos: u64, n: usize) {
        let mono = matches!(self.kind, NodeKind::Track(TrackFormat::Mono));
        for c in &self.clips {
            let a = pos.max(c.start);
            let b = (pos + n as u64).min(c.end());
            if a >= b {
                continue;
            }
            let src = (c.offset + (a - c.start)) as usize;
            let dst = (a - pos) as usize;
            let cnt = (b - a) as usize;
            let rel0 = a - c.start;
            let (dl, dr) = (&c.data.left()[src..src + cnt], &c.data.right()[src..src + cnt]);
            let (ol, or) = (&mut self.l[dst..dst + cnt], &mut self.r[dst..dst + cnt]);
            // Les fondus ne touchent que les bords : gain constant partout ailleurs.
            let faded = c.fade_in.len > 0 || c.fade_out.len > 0;
            for j in 0..cnt {
                let g = if faded { c.gain_at(rel0 + j as u64) } else { c.gain };
                if mono {
                    // Moyenne des canaux (identique à l'échantillon lui-même pour une source mono).
                    let s = ((dl[j] + dr[j]) * 0.5) * g;
                    ol[j] += s;
                    or[j] += s;
                } else {
                    ol[j] += dl[j] * g;
                    or[j] += dr[j] * g;
                }
            }
        }
    }

    /// Entrée audio de la piste : écoute (mélangée au signal de la piste, avant les effets) et copie vers la prise.
    fn input_stage(&mut self, pos: u64, inputs: &[&[f32]], n: usize, playing: bool) {
        // Dynamique d'entrée : le signal traité n'est calculé que si l'écoute ou la prise (option) s'en sert.
        let mut processed = false;
        if self.armed && (self.monitor || self.dyn_cfg.record) && n <= self.dyn_a.len() {
            if let Some(st) = self.dyn_state.as_mut() {
                let (ia, ib) = self.source.indices();
                let stereo = self.source.channels() == 2;
                for j in 0..n {
                    let a = sample(inputs, ia, j);
                    let b = if stereo { sample(inputs, ib, j) } else { a };
                    let (pa, pb) = st.process(a, b);
                    self.dyn_a[j] = pa;
                    self.dyn_b[j] = pb;
                }
                processed = true;
            }
        }
        if self.armed && self.monitor {
            let (ia, ib) = self.source.indices();
            let stereo = self.source.channels() == 2;
            for j in 0..n {
                let (a, b) = if processed {
                    (self.dyn_a[j], self.dyn_b[j])
                } else {
                    let a = sample(inputs, ia, j);
                    (a, if stereo { sample(inputs, ib, j) } else { a })
                };
                self.l[j] += a;
                self.r[j] += b;
            }
        }
        if self.armed {
            let (ia, ib) = self.source.indices();
            let stereo = self.source.channels() == 2;
            let (mut pl, mut pr) = (0.0f32, 0.0f32);
            for j in 0..n {
                let a = sample(inputs, ia, j).abs();
                pl = pl.max(a);
                pr = pr.max(if stereo { sample(inputs, ib, j).abs() } else { a });
            }
            self.in_peak = [pl, pr];
        }
        if let Some(tap) = self.tap.as_mut() {
            if processed && self.dyn_cfg.record {
                tap.push_processed(pos, &self.dyn_a[..n], &self.dyn_b[..n], n, playing);
            } else {
                tap.push(pos, inputs, n, playing);
            }
        }
    }

    /// Prépare la liste de notes du bloc : messages en attente à l'instant 0, puis notes des clips dans `[pos, pos + n[`.
    /// Retourne le nombre de notes perdues faute de place.
    fn gather_midi(&mut self, pos: u64, n: usize, playing: bool) -> u32 {
        self.events.clear();
        let mut dropped = 0;
        if let Some(t) = self.midi_tap.as_mut() {
            t.begin(pos, n, playing);
        }
        let mut offset_live = false;
        // Balayage de la panique : une tranche de fins de notes au début du bloc (voix que le suivi ne connaît pas).
        if self.panic_cursor < PANIC_SWEEP {
            let end = (self.panic_cursor + PANIC_PER_BLOCK).min(PANIC_SWEEP);
            for i in self.panic_cursor..end {
                if self.events.len() < MAX_BLOCK_EVENTS {
                    self.events.push(MidiEvent { time: 0, msg: MidiMsg::NoteOff { channel: (i / 128) as u8, key: (i % 128) as u8, velocity: 0.0 } });
                } else {
                    dropped += 1;
                }
            }
            self.panic_cursor = end;
        }
        for k in 0..self.live.len() {
            let (off, m) = self.live[k];
            let time = (off as usize).min(n.saturating_sub(1)) as u32;
            if self.events.len() < MAX_BLOCK_EVENTS {
                self.events.push(MidiEvent { time, msg: shape_note(&self.vel_table, m) });
                self.active.apply(m);
                self.midi_seen |= match m {
                    MidiMsg::NoteOn { .. } | MidiMsg::NoteOff { .. } => {
                        if let MidiMsg::NoteOn { velocity, .. } = m {
                            self.vel_in = ((velocity * 127.0 + 0.5).floor() as i32).clamp(1, 127) as u8;
                        }
                        crate::meters::MIDI_NOTE
                    }
                    MidiMsg::ControlChange { .. } => crate::meters::MIDI_CONTROL,
                    _ => crate::meters::MIDI_BEND,
                };
                if let Some(t) = self.midi_tap.as_mut() {
                    t.push(pos + time as u64, m);
                }
                offset_live |= time > 0;
            } else {
                dropped += 1;
            }
        }
        self.live.clear();
        if !playing {
            self.sort_events(offset_live);
            return dropped;
        }
        let first = self.midi.partition_point(|e| e.time < pos);
        for e in &self.midi[first..] {
            if e.time >= pos + n as u64 {
                break;
            }
            if self.events.len() < MAX_BLOCK_EVENTS {
                self.events.push(MidiEvent { time: (e.time - pos) as u32, msg: shape_note(&self.vel_table, e.msg) });
                self.active.apply(e.msg);
            } else {
                dropped += 1;
            }
        }
        self.sort_events(offset_live);
        dropped
    }

    /// Messages d'entrée datés dans le bloc : tri stable (insertion, sans allocation) par instant.
    fn sort_events(&mut self, needed: bool) {
        if !needed {
            return;
        }
        for i in 1..self.events.len() {
            let mut j = i;
            while j > 0 && self.events[j - 1].time > self.events[j].time {
                self.events.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    /// Panique sur cette piste (voir `CompiledGraph::panic`). Rend le nombre de messages perdus faute de place.
    fn panic(&mut self) -> u32 {
        let mut dropped = 0;
        self.live.clear();
        dropped += self.release_all();
        for channel in 0..16u8 {
            for controller in [123u8, 120] {
                if self.live.len() < MAX_PENDING {
                    self.live.push((0, MidiMsg::ControlChange { channel, controller, value: 0 }));
                } else {
                    dropped += 1;
                }
            }
        }
        self.panic_cursor = 0;
        dropped
    }

    /// Programme la coupure de tout ce qui sonne sur cette piste : fins de notes, pédale relâchée, molette recentrée
    /// (livrées au début du prochain bloc). Retourne le nombre de messages perdus faute de place.
    fn release_all(&mut self) -> u32 {
        let mut dropped = 0;
        let NodeC { active, live, .. } = self;
        active.for_each(|channel, key| {
            if live.len() < MAX_PENDING {
                live.push((0, MidiMsg::NoteOff { channel, key, velocity: 0.0 }));
            } else {
                dropped += 1;
            }
        });
        active.for_each_sustain(|channel| {
            if live.len() < MAX_PENDING {
                live.push((0, MidiMsg::ControlChange { channel, controller: 64, value: 0 }));
            } else {
                dropped += 1;
            }
        });
        active.for_each_bend(|channel| {
            if live.len() < MAX_PENDING {
                live.push((0, MidiMsg::PitchBend { channel, value: 0 }));
            } else {
                dropped += 1;
            }
        });
        dropped
    }

    /// Chaîne d'effets insérés, avant le fader. Chaque effet lit `l/r` et écrit `scratch_*`, puis on échange.
    /// Si des paramètres d'effets sont automatisés, le bloc est découpé sur la grille absolue `AUTO_GRID`.
    fn run_inserts(&mut self, pos: u64, n: usize) {
        if !self.auto_params.is_empty() {
            return self.run_inserts_automated(pos, n);
        }
        let transport = self.time.info_at(pos, self.sr, self.playing_now);
        for k in 0..self.inserts.len() {
            let ins = &mut self.inserts[k];
            if let Some(effect) = ins.effect.as_mut() {
                effect.set_transport(&transport, self.sr);
                effect.set_chords(&self.chords);
                if let Some(sc) = ins.sc.as_ref().filter(|_| effect.has_sidechain()) {
                    effect.set_sidechain(&sc.buf_l[..n], &sc.buf_r[..n]);
                }
                if effect.accepts_notes() {
                    effect.process_with_notes(&self.l[..n], &self.r[..n], &mut self.scratch_l[..n], &mut self.scratch_r[..n], &self.events);
                } else {
                    effect.process(&self.l[..n], &self.r[..n], &mut self.scratch_l[..n], &mut self.scratch_r[..n]);
                }
                for buf in self.aux.iter_mut().filter(|x| x.insert == k) {
                    match effect.aux_output(buf.port, n) {
                        Some((l, r)) => {
                            buf.l[..n].copy_from_slice(l);
                            buf.r[..n].copy_from_slice(r);
                        }
                        None => {
                            buf.l[..n].fill(0.0);
                            buf.r[..n].fill(0.0);
                        }
                    }
                }
                self.inserts[k].apply_bypass(&self.l[..n], &self.r[..n], &mut self.scratch_l[..n], &mut self.scratch_r[..n]);
                std::mem::swap(&mut self.l, &mut self.scratch_l);
                std::mem::swap(&mut self.r, &mut self.scratch_r);
            }
        }
    }

    /// Variante avec automation de paramètres : tranches alignées sur `AUTO_GRID` échantillons absolus, valeur envoyée
    /// au début de chaque tranche. Le résultat ne dépend donc pas de la taille du bloc.
    fn run_inserts_automated(&mut self, pos: u64, n: usize) {
        let mut a = 0usize;
        while a < n {
            let abs = pos + a as u64;
            let b = (a as u64 + (AUTO_GRID - abs % AUTO_GRID)).min(n as u64) as usize;
            for pa in &mut self.auto_params {
                // Valeur au début de la case de la grille (et non au début de la tranche) : indépendant du bloc.
                let v = pa.lane.value_at(abs - abs % AUTO_GRID);
                if v != pa.last {
                    if let Some(e) = self.inserts.get_mut(pa.insert).and_then(|i| i.effect.as_mut()) {
                        e.set_param(pa.param, v);
                    }
                    pa.last = v;
                }
            }
            let transport = self.time.info_at(abs, self.sr, self.playing_now);
            for k in 0..self.inserts.len() {
                let ins = &mut self.inserts[k];
                let Some(effect) = ins.effect.as_mut() else { continue };
                effect.set_transport(&transport, self.sr);
                effect.set_chords(&self.chords);
                if let Some(sc) = ins.sc.as_ref().filter(|_| effect.has_sidechain()) {
                    effect.set_sidechain(&sc.buf_l[a..b], &sc.buf_r[a..b]);
                }
                if effect.accepts_notes() {
                    self.chunk_events.clear();
                    for e in &self.events {
                        if (e.time as usize) >= a && (e.time as usize) < b && self.chunk_events.len() < MAX_BLOCK_EVENTS {
                            self.chunk_events.push(MidiEvent { time: e.time - a as u32, msg: e.msg });
                        }
                    }
                    effect.process_with_notes(&self.l[a..b], &self.r[a..b], &mut self.scratch_l[a..b], &mut self.scratch_r[a..b], &self.chunk_events);
                } else {
                    effect.process(&self.l[a..b], &self.r[a..b], &mut self.scratch_l[a..b], &mut self.scratch_r[a..b]);
                }
                for buf in self.aux.iter_mut().filter(|x| x.insert == k) {
                    match effect.aux_output(buf.port, b - a) {
                        Some((l, r)) => {
                            buf.l[a..b].copy_from_slice(l);
                            buf.r[a..b].copy_from_slice(r);
                        }
                        None => {
                            buf.l[a..b].fill(0.0);
                            buf.r[a..b].fill(0.0);
                        }
                    }
                }
                self.inserts[k].apply_bypass(&self.l[a..b], &self.r[a..b], &mut self.scratch_l[a..b], &mut self.scratch_r[a..b]);
                self.l[a..b].copy_from_slice(&self.scratch_l[a..b]);
                self.r[a..b].copy_from_slice(&self.scratch_r[a..b]);
            }
            a = b;
        }
    }

    /// Tout le travail de ce nœud sur un bloc, dans l'ordre : clips, notes, entrée audio, effets, fader, niveaux. Ne lit que son
    /// propre état, l'entrée du pilote (lecture seule) et la banque de niveaux (atomique) : deux nœuds d'un même niveau du graphe
    /// peuvent donc être calculés en même temps sur deux fils.
    ///
    /// Si la production de ce nœud est confiée à sa jumelle (calcul en avance), le signal vient de l'anneau de la jumelle au lieu d'être
    /// calculé ici ; le fader, les envois et les niveaux restent ceux du fil audio.
    #[allow(clippy::too_many_arguments)]
    fn compute(&mut self, index: usize, pos: u64, n: usize, playing: bool, inputs: &[&[f32]], meters: Option<&MeterBank>, view: AheadView) {
        self.block_dropped = 0;
        self.playing_now = playing;
        if self.ahead.phase != Phase::Live {
            self.read_ahead(pos, n, playing, view);
            return self.finish(index, pos, n, meters);
        }
        if self.is_track() {
            if playing {
                self.render_clips(pos, n);
            }
            self.block_dropped = self.gather_midi(pos, n, playing);
            if self.midi_seen != 0 {
                if let Some(m) = meters {
                    m.note_midi(index, self.midi_seen);
                }
                self.midi_seen = 0;
            }
            if self.vel_in != 0 {
                if let Some(m) = meters {
                    m.note_velocity(index, self.vel_in);
                }
                self.vel_in = 0;
            }
            self.input_stage(pos, inputs, n, playing);
        }
        self.run_inserts(pos, n);
        self.finish(index, pos, n, meters);
    }

    /// Fin du travail d'un nœud : copie « avant fader » pour les envois, fader, niveaux de crête.
    fn finish(&mut self, index: usize, pos: u64, n: usize, meters: Option<&MeterBank>) {
        if !self.pre_l.is_empty() {
            // Un envoi « avant fader » existe : on garde le signal tel qu'il sort des effets.
            self.pre_l[..n].copy_from_slice(&self.l[..n]);
            self.pre_r[..n].copy_from_slice(&self.r[..n]);
        }
        self.fader(pos, n);
        if let Some(m) = meters {
            let (mut pl, mut pr) = (0.0f32, 0.0f32);
            if self.armed && !self.monitor {
                // Piste armée sans écoute : le vumètre montre l'entrée (avant le fader), pour régler le niveau de prise.
                [pl, pr] = self.in_peak;
            } else {
                for j in 0..n {
                    pl = pl.max(self.l[j].abs());
                    pr = pr.max(self.r[j].abs());
                }
            }
            m.update(index, pl, pr);
        }
    }

    /// Production seule (clips, notes, effets), sans entrée audio ni fader : ce que fait la jumelle, à la position `pos`, pour `n`
    /// échantillons, avec la lecture en marche. Rend les notes perdues faute de place.
    fn produce(&mut self, pos: u64, n: usize) -> u32 {
        self.l[..n].fill(0.0);
        self.r[..n].fill(0.0);
        self.playing_now = true;
        self.render_clips(pos, n);
        let dropped = self.gather_midi(pos, n, true);
        self.run_inserts(pos, n);
        dropped
    }

    /// Remet à zéro ce qui dépend de la position de lecture, pour la production (déplacement de la tête de lecture, arrêt) : mémoire
    /// des effets, notes en attente, et envoi des fins de notes qu'on savait ouvertes (voir `CompiledGraph::reset`).
    fn reset_production(&mut self) -> u32 {
        for i in &mut self.inserts {
            if let Some(e) = i.effect.as_mut() {
                e.reset();
            }
        }
        for pa in &mut self.auto_params {
            pa.last = f64::NAN;
        }
        self.live.clear();
        self.events.clear();
        self.release_all()
    }

    /// Échange avec la jumelle tout ce qui sert à la production (clips, notes, effets, automation de paramètres, notes en cours) : un
    /// déplacement de pointeurs, sans allocation. Appelé deux fois de suite, il remet tout en place.
    fn swap_production(&mut self, twin: &mut NodeC) {
        std::mem::swap(&mut self.clips, &mut twin.clips);
        std::mem::swap(&mut self.midi, &mut twin.midi);
        std::mem::swap(&mut self.events, &mut twin.events);
        std::mem::swap(&mut self.live, &mut twin.live);
        std::mem::swap(&mut self.active, &mut twin.active);
        std::mem::swap(&mut self.inserts, &mut twin.inserts);
        std::mem::swap(&mut self.auto_params, &mut twin.auto_params);
        std::mem::swap(&mut self.chunk_events, &mut twin.chunk_events);
        std::mem::swap(&mut self.panic_cursor, &mut twin.panic_cursor);
    }

    /// Ce nœud peut-il être calculé en avance ? Une piste sans armement ni prise, qui a de quoi faire (effets ou notes).
    fn ahead_eligible(&self) -> bool {
        self.twin.is_some() && !self.armed && self.tap.is_none() && self.midi_tap.is_none()
    }

    /// Gain, panoramique et mute appliqués sur place.
    fn fader(&mut self, pos: u64, n: usize) {
        let (l, r) = (&mut self.l[..n], &mut self.r[..n]);
        if self.auto_gain.is_some() || self.auto_pan.is_some() {
            for j in 0..n {
                let t = pos + j as u64;
                // Le lisseur de gain ne porte que mute/solo quand le gain est automatisé.
                let mut g = self.gain.next();
                if let Some(lane) = &self.auto_gain {
                    g *= lane.value_at(t) as f32;
                }
                let p = match &self.auto_pan {
                    Some(lane) if lane.mode == AutoMode::Trim => (lane.value_at(t) as f32 + self.pan.next()).clamp(-1.0, 1.0),
                    Some(lane) => (lane.value_at(t) as f32).clamp(-1.0, 1.0),
                    None => self.pan.next(),
                };
                let (gl, gr) = pan_gains(self.law, p);
                l[j] *= g * gl;
                r[j] *= g * gr;
            }
        } else if self.gain.is_settled() && self.pan.is_settled() {
            let (gl, gr) = pan_gains(self.law, self.pan.target());
            let (a, b) = (self.gain.target() * gl, self.gain.target() * gr);
            if a == 1.0 && b == 1.0 {
                return;
            }
            for j in 0..n {
                l[j] *= a;
                r[j] *= b;
            }
        } else {
            for j in 0..n {
                let g = self.gain.next();
                let (gl, gr) = pan_gains(self.law, self.pan.next());
                l[j] *= g * gl;
                r[j] *= g * gr;
            }
        }
    }
}

fn two_mut<T>(s: &mut [T], i: usize, j: usize) -> (&mut T, &mut T) {
    debug_assert!(i != j);
    if i < j {
        let (a, b) = s.split_at_mut(j);
        (&mut a[i], &mut b[0])
    } else {
        let (a, b) = s.split_at_mut(i);
        (&mut b[0], &mut a[j])
    }
}

/// Ajoute le signal de `src` à `dst` (après la ligne à retard de compensation de l'arête), tel quel
/// (`send = None`) ou multiplié par le gain lissé de l'envoi `k`.
fn route(nodes: &mut [NodeC], src: usize, dst: usize, n: usize, send: Option<usize>, pos: u64) {
    let (s, d) = two_mut(nodes, src, dst);
    let live_monitor = s.armed && s.monitor;
    let NodeC { l, r, pre_l, pre_r, sends, out_delay, aux, .. } = s;
    match send {
        None if live_monitor => {
            // Piste écoutée en direct : pas de compensation de latence, pour garder l'écoute aussi courte que possible (y compris vers les
            // envois : un musicien qui s'entend dans son retour ne doit pas être retardé par le plugin lent d'une autre piste).
            for j in 0..n {
                d.l[j] += l[j];
                d.r[j] += r[j];
            }
        }
        None => {
            for j in 0..n {
                let (a, b) = out_delay.tap(l[j], r[j]);
                d.l[j] += a;
                d.r[j] += b;
            }
        }
        Some(k) => {
            let SendC { gain, delay, pre, auto, aud, aux: aux_idx, .. } = &mut sends[k];
            let aux_k: Option<usize> = *aux_idx;
            let source = |pre: bool| -> (&[f32], &[f32]) {
                match aux_k {
                    Some(a) => (&aux[a].l, &aux[a].r),
                    None if pre => (pre_l, pre_r),
                    None => (l, r),
                }
            };
            // Audibilité : 1 ou 0, lissée ; hors rampe c'est une constante (cas courant : aucun surcoût par échantillon).
            if aud.is_settled() && aud.target() == 0.0 {
                // Envoi coupé : on fait quand même avancer la ligne à retard pour rester aligné au rétablissement.
                if !live_monitor {
                    let (sl, sr) = source(*pre);
                    for j in 0..n {
                        delay.tap(sl[j], sr[j]);
                    }
                }
                return;
            }
            let (sl, sr) = source(*pre);
            if let Some(lane) = auto {
                // Niveau automatisé : lu à l'échantillon près ; en mode Trim le réglage manuel (lissé) multiplie la courbe.
                let trim = lane.mode == AutoMode::Trim;
                for j in 0..n {
                    let manual = if trim { gain.next() } else { 1.0 };
                    let g = manual * lane.value_at(pos + j as u64) as f32 * aud.next();
                    let (a, b) = if live_monitor { (sl[j], sr[j]) } else { delay.tap(sl[j], sr[j]) };
                    d.l[j] += a * g;
                    d.r[j] += b * g;
                }
            } else if gain.is_settled() && aud.is_settled() {
                let g = gain.target() * aud.target();
                for j in 0..n {
                    let (a, b) = if live_monitor { (sl[j], sr[j]) } else { delay.tap(sl[j], sr[j]) };
                    d.l[j] += a * g;
                    d.r[j] += b * g;
                }
            } else {
                for j in 0..n {
                    let g = gain.next() * aud.next();
                    let (a, b) = if live_monitor { (sl[j], sr[j]) } else { delay.tap(sl[j], sr[j]) };
                    d.l[j] += a * g;
                    d.r[j] += b * g;
                }
            }
        }
    }
}

/// Un niveau n'est calculé en parallèle que s'il coûte au moins 30 µs : en dessous, l'attente de synchronisation ne rapporte rien.
const PAR_MIN_NS: f32 = 30_000.0;
/// Répartition des nœuds entre fils recalculée tous les 128 blocs.
const REBALANCE_EVERY: u32 = 128;
/// Lissage du coût mesuré d'un nœud (part conservée à chaque bloc).
const COST_KEEP: f32 = 0.95;

/// Ce que les fils de travail ont besoin de connaître pour calculer un nœud d'un niveau (pointeurs bruts : voir `process_with`).
struct LevelCtx {
    nodes: *mut NodeC,
    order: *const usize,
    pos: u64,
    n: usize,
    playing: bool,
    inputs: *const &'static [f32],
    n_inputs: usize,
    meters: *const MeterBank,
    view: AheadView,
}

/// Calcule le `k`-ième nœud du niveau décrit par `ctx` (appelée par le groupe de fils, un seul appel par `k`).
///
/// # Safety
/// `ctx` désigne un `LevelCtx` valide ; `k` est dans le niveau ; aucun autre fil ne touche ce nœud pendant l'appel.
unsafe fn run_node(ctx: *mut (), k: usize) {
    let c = &*(ctx as *const LevelCtx);
    let i = *c.order.add(k);
    let node = &mut *c.nodes.add(i);
    let inputs = std::slice::from_raw_parts(c.inputs, c.n_inputs);
    let meters = if c.meters.is_null() { None } else { Some(&*c.meters) };
    let t0 = std::time::Instant::now();
    node.compute(i, c.pos, c.n, c.playing, inputs, meters, c.view);
    node.cost_ns = node.cost_ns * COST_KEEP + t0.elapsed().as_nanos() as f32 * (1.0 - COST_KEEP);
}

pub struct CompiledGraph {
    /// Canaux de sortie au-delà de la paire principale : un tampon par canal (le canal physique `k + 2`), rempli à chaque bloc par les bus
    /// qui ont une sortie physique ; la longueur est le nombre de canaux supplémentaires dont ce graphe a besoin.
    phys_bufs: Vec<Vec<f32>>,
    /// Transport donné aux plugins (voir `NodeC::time`) ; la boucle y est mise à jour par le moteur avant chaque bloc.
    time: Arc<TimeShared>,
    /// Niveaux de crête rendus à l'interface (après chaque fader) ; `None` : non mesuré.
    meters: Option<Arc<MeterBank>>,
    nodes: Vec<NodeC>,
    /// Ordre de calcul : **par niveau** (un nœud après tous ceux qui l'alimentent), à l'intérieur d'un niveau par ordre topologique
    /// d'origine. C'est aussi l'ordre des sommes dans les destinations : le même quel que soit le nombre de fils.
    order: Vec<usize>,
    /// Fin (exclue, dans `order`) de chaque niveau.
    level_ends: Vec<usize>,
    /// Fil prévu pour chaque position de `order` (0 = fil audio), recalculé d'après les coûts mesurés.
    assign: Vec<u8>,
    /// Positions de `order` triées par coût (réserve pour la répartition, sans allocation dans le callback).
    scratch: Vec<usize>,
    /// Blocs traités avec un groupe de fils (cadence de la répartition) et nombre de fils de la dernière répartition.
    par_blocks: u32,
    assigned_for: usize,
    ramp: u32,
    max_block: usize,
    /// Latence totale entre l'entrée (position de la ligne de temps) et la sortie, en échantillons.
    latency: u32,
    /// Notes perdues faute de place (doit rester 0).
    midi_dropped: u32,
    /// Poignées de contrôle des effets **créés** pour ce graphe (pas des effets adoptés).
    controls: Vec<(InsertId, Arc<dyn InsertControl>)>,
    /// Calcul en avance : époque courante et description de son déroulement (voir `ahead.rs`) ; vrai si un nœud a lu un anneau qui ne
    /// correspond plus à la position (le moteur relance alors l'avance) ; compteur de silences faute de données.
    ahead_epoch: u64,
    ahead_rec: EpochRec,
    resync: bool,
    ahead_underruns: u32,
    ahead_desyncs: u32,
    /// Chaînes latérales : (nœud source, nœud destination, rang de l'effet dans la destination).
    sc_edges: Vec<(usize, usize, usize)>,
}

impl CompiledGraph {
    pub(crate) fn build(
        desc: &GraphDesc,
        order: Vec<usize>,
        sample_rate: u32,
        max_block: usize,
        live: &dyn Fn(InsertId) -> Option<u32>,
        restore: &dyn Fn(InsertId) -> Option<Vec<u8>>,
        ahead: Option<AheadParams>,
    ) -> Result<Self, GraphError> {
        // 1. Effets : instance neuve, ou « à adopter » si l'identifiant est déjà vivant côté audio.
        let mut controls = Vec::new();
        let mut inserts: Vec<Vec<InsertC>> = Vec::with_capacity(desc.nodes.len());
        for d in &desc.nodes {
            let mut v = Vec::with_capacity(d.inserts.len());
            for i in &d.inserts {
                match live(i.id) {
                    Some(latency) => v.push(InsertC::new(i.id, latency, None, i.bypass)),
                    None => {
                        let state = restore(i.id);
                        let effect = (i.factory)(sample_rate, max_block, state.as_deref())
                            .map_err(|reason| GraphError::InsertFailed { node: d.id, insert: i.id, reason })?;
                        if let Some(c) = effect.control() {
                            controls.push((i.id, c));
                        }
                        v.push(InsertC::new(i.id, effect.latency(), Some(effect), i.bypass));
                    }
                }
            }
            inserts.push(v);
        }

        // 2. Compensation de latence : latence de sortie de chaque nœud = max des latences des sources + la sienne ;
        //    chaque arête reçoit un retard égal à l'écart avec la source la plus lente de sa destination.
        let n = desc.nodes.len();
        let own: Vec<u32> = inserts.iter().map(|v| v.iter().map(|i| i.latency).sum()).collect();
        let (mut lat_in, mut lat_out) = (vec![0u32; n], vec![0u32; n]);
        // Les chaînes latérales n'entrent PAS dans cette compensation : le signal d'un nœud qui a ses propres clips naît sans retard, la
        // latence d'entrée n'est que celle de ses arêtes. La chaîne latérale est alignée plus bas, par un retard sur le signal de la source
        // quand celle-ci est en avance sur le signal principal de l'effet ; en retard, elle ne peut pas être avancée.
        let out_edges = desc.out_edges();
        for &i in &order {
            lat_out[i] = lat_in[i] + own[i];
            let node = &desc.nodes[i];
            for t in node.main_output().into_iter().chain(node.sends.iter().map(|s| s.to)).chain(node.inserts.iter().flat_map(|x| x.aux.iter().flatten().copied())) {
                lat_in[t.0 as usize] = lat_in[t.0 as usize].max(lat_out[i]);
            }
        }
        let edge_delay = |src: usize, dst: usize| -> usize {
            if desc.pdc {
                (lat_in[dst] - lat_out[src]) as usize
            } else {
                0
            }
        };

        // 3. Nœuds.
        let time = Arc::new(TimeShared::new(Arc::clone(&desc.time_map)));
        let mut nodes: Vec<NodeC> = Vec::with_capacity(n);
        let mut sc_edges: Vec<(usize, usize, usize)> = Vec::new();
        let is_sc_source: Vec<bool> = (0..n).map(|i| desc.nodes.iter().any(|o| o.inserts.iter().any(|x| x.sidechain.is_some_and(|s| s.0 as usize == i)))).collect();
        for (i, (d, mut ins)) in desc.nodes.iter().zip(inserts).enumerate() {
            let output = d.output.map_or(0, |o| o.0 as usize);
            // Chaîne latérale de chaque effet : le signal de sa source est retardé pour arriver aligné sur le signal principal **à cet
            // effet** (entrée du nœud + latence des effets qui le précèdent).
            let mut before = 0u32;
            for (k, desc_ins) in d.inserts.iter().enumerate() {
                if let Some(src) = desc_ins.sidechain {
                    let s = src.0 as usize;
                    let delay = if desc.pdc { (lat_in[i] + before).saturating_sub(lat_out[s]) as usize } else { 0 };
                    ins[k].sc = Some(Sidechain { delay: DelayLine::new(delay), buf_l: vec![0.0; max_block], buf_r: vec![0.0; max_block] });
                    sc_edges.push((s, i, k));
                }
                before += ins[k].latency;
            }
            nodes.push(NodeC {
                kind: d.kind,
                law: if matches!(d.kind, NodeKind::Track(TrackFormat::Mono)) { PanLaw::EqualPower } else { PanLaw::Balance },
                gain: Smoother::new(0.0),
                pan: Smoother::new(d.params.pan),
                user_gain: d.params.gain,
                invert: d.params.invert,
                auto_gain: d.automation.iter().find(|l| l.target == AutoTarget::Gain && l.mode != AutoMode::Off).cloned(),
                auto_pan: d.automation.iter().find(|l| l.target == AutoTarget::Pan && l.mode != AutoMode::Off).cloned(),
                auto_params: d
                    .automation
                    .iter()
                    .filter_map(|l| match l.target {
                        AutoTarget::InsertParam { insert, param } if l.mode != AutoMode::Off => {
                            let idx = d.inserts.iter().position(|i| i.id == insert)?;
                            Some(ParamAuto { insert: idx, param, lane: l.clone(), last: f64::NAN })
                        }
                        _ => None,
                    })
                    .collect(),
                chunk_events: Vec::with_capacity(if d.automation.iter().any(|l| matches!(l.target, AutoTarget::InsertParam { .. })) { MAX_BLOCK_EVENTS } else { 0 }),
                mute: d.params.mute,
                solo: d.params.solo,
                solo_excluded: false,
                output,
                phys: d.phys,
                is_return: d.monitor,
                out_delay: DelayLine::new(if d.output.is_some() && d.phys.is_none() && !d.monitor { edge_delay(i, output) } else { 0 }),
                sends: d
                    .sends
                    .iter()
                    .map(|s| SendC { to: s.to.0 as usize, gain: Smoother::new(s.gain), auto: d.automation.iter().find(|l| l.target == (AutoTarget::SendGain { to: s.to }) && l.mode != AutoMode::Off).cloned(), delay: DelayLine::new(edge_delay(i, s.to.0 as usize)), pre: s.pre, mute: s.mute, solo: s.solo, aud: Smoother::new(1.0), aux: None })
                    .chain(aux_dests(d).into_iter().enumerate().map(|(a, (_, _, to))| SendC { to, gain: Smoother::new(1.0), auto: None, delay: DelayLine::new(edge_delay(i, to)), pre: true, mute: false, solo: false, aud: Smoother::new(1.0), aux: Some(a) }))
                    .collect(),
                aux: aux_dests(d).into_iter().map(|(insert, port, _)| AuxBuf { insert, port, l: vec![0.0; max_block], r: vec![0.0; max_block] }).collect(),
                pre_l: vec![0.0; if d.sends.iter().any(|s| s.pre) || is_sc_source[i] { max_block } else { 0 }],
                pre_r: vec![0.0; if d.sends.iter().any(|s| s.pre) || is_sc_source[i] { max_block } else { 0 }],
                clips: d.clips.iter().filter(|c| !c.muted).cloned().collect(),
                inserts: ins,
                midi: if matches!(d.kind, NodeKind::Track(_)) { expand(&d.midi_clips) } else { Vec::new() },
                events: Vec::with_capacity(if matches!(d.kind, NodeKind::Track(_)) { MAX_BLOCK_EVENTS } else { 0 }),
                midi_tap: None,
                live: Vec::with_capacity(if matches!(d.kind, NodeKind::Track(_)) { MAX_PENDING } else { 0 }),
                panic_cursor: PANIC_SWEEP,
                active: ActiveNotes::new(),
                midi_seen: 0,
                vel_in: 0,
                vel_table: d.vel_curve.table().filter(|_| matches!(d.kind, NodeKind::Track(_))),
                dyn_cfg: d.input_dyn,
                dyn_state: (d.input_dyn.enabled && matches!(d.kind, NodeKind::Track(_))).then(|| crate::dynamics::DynState::new(d.input_dyn, sample_rate)),
                dyn_a: if d.input_dyn.enabled && matches!(d.kind, NodeKind::Track(_)) { vec![0.0; max_block] } else { Vec::new() },
                dyn_b: if d.input_dyn.enabled && matches!(d.kind, NodeKind::Track(_)) { vec![0.0; max_block] } else { Vec::new() },
                armed: false,
                monitor: false,
                source: RecordSource::Left,
                in_peak: [0.0; 2],
                tap: None,
                l: vec![0.0; max_block],
                r: vec![0.0; max_block],
                scratch_l: vec![0.0; max_block],
                scratch_r: vec![0.0; max_block],
                block_dropped: 0,
                cost_ns: 0.0,
                twin: None,
                twin_tx: None,
                twin_inserts: Vec::new(),
                ahead: AheadState::default(),
                time: Arc::clone(&time),
                chords: Arc::clone(&desc.chords),
                sr: sample_rate,
                playing_now: false,
            });
        }
        let any_solo = nodes.iter().any(|n| n.is_track() && n.solo);
        for n in &mut nodes {
            n.solo_excluded = any_solo && n.is_track() && !n.solo;
            n.gain = Smoother::new(n.effective_gain());
        }
        // Niveaux : un nœud est au niveau 1 + le plus haut de ses sources. L'ordre de calcul range les nœuds par niveau (tri stable : à
        // l'intérieur d'un niveau, l'ordre topologique d'origine), pour pouvoir calculer un niveau entier en parallèle.
        let mut level = vec![0usize; n];
        for &i in &order {
            for &t in &out_edges[i] {
                level[t] = level[t].max(level[i] + 1);
            }
        }
        let mut order = order;
        order.sort_by_key(|&i| level[i]);
        let mut level_ends = Vec::new();
        for k in 1..=order.len() {
            if k == order.len() || level[order[k]] != level[order[k - 1]] {
                level_ends.push(k);
            }
        }
        let extra = desc.nodes.iter().filter_map(|d| d.phys).map(|p| usize::from(p.first) + usize::from(p.channels())).max().unwrap_or(2) - 2;
        let mut g = CompiledGraph { phys_bufs: vec![vec![0.0; max_block]; extra], time, meters: None, nodes, assign: vec![0; order.len()], scratch: Vec::with_capacity(order.len()), order, level_ends, par_blocks: 0, assigned_for: 0, ramp: ramp_frames(sample_rate), max_block, latency: lat_out[0], midi_dropped: 0, controls, ahead_epoch: 0, ahead_rec: EpochRec::default(), resync: false, ahead_underruns: 0, ahead_desyncs: 0, sc_edges };
        g.refresh_sends(0);
        if let Some(p) = ahead {
            g.make_twins(p);
        }
        Ok(g)
    }

    /// Branche la banque de niveaux de crête (avant de confier le graphe au thread audio).
    pub fn set_meters(&mut self, bank: Arc<MeterBank>) {
        self.meters = Some(bank);
    }

    /// Poignées de contrôle des effets créés pour ce graphe.
    pub(crate) fn new_controls(&self) -> &[(InsertId, Arc<dyn InsertControl>)] {
        &self.controls
    }

    /// Latence d'un effet de ce graphe (`None` s'il n'y en a pas avec cet identifiant).
    pub(crate) fn insert_latency(&self, id: InsertId) -> Option<u32> {
        self.nodes.iter().flat_map(|n| &n.inserts).find(|i| i.id == id).map(|i| i.latency)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Latence totale du graphe, en échantillons : l'audio de la position `p` sort en position `p + latency()`.
    pub fn latency(&self) -> u32 {
        self.latency
    }

    // ---- échange de graphe et réinitialisation (thread audio, sans allocation) ----

    /// Reprend dans `old` les instances des effets que ce graphe attend (même identifiant). Les effets de `old` qui
    /// n'existent plus ici y restent : ils sont libérés avec lui, hors du thread audio.
    pub fn adopt_inserts_from(&mut self, old: &mut CompiledGraph) {
        // Armement et prise d'enregistrement suivent leur piste (même indice) : modifier le routage en pleine prise ne
        // l'interrompt pas.
        let mut dropped = 0;
        for (new, o) in self.nodes.iter_mut().zip(old.nodes.iter_mut()) {
            if new.is_track() && o.is_track() {
                new.armed = o.armed;
                // L'enveloppe de la dynamique d'entrée suit la piste : modifier un réglage ne la remet pas à zéro (pas de coup de gain).
                if let (Some(n), Some(old)) = (new.dyn_state.as_mut(), o.dyn_state.as_ref()) {
                    n.env = old.env;
                    n.gate = old.gate;
                    n.open = old.open;
                }
                new.monitor = o.monitor;
                new.source = o.source;
                new.tap = o.tap.take();
                new.midi_tap = o.midi_tap.take();
                // Notes en cours et messages en attente suivent leur piste. Si les notes de la piste ont changé (clip déplacé,
                // rogné, supprimé...), les fins de notes attendues dans l'ancien graphe n'arriveront peut-être jamais :
                // on coupe ce qui sonne. Pour une modification étrangère à cette piste, rien n'est interrompu.
                new.active = o.active.clone();
                // `drain` et non `take` : le tampon de l'ancien graphe reste dans l'ancien graphe, libéré hors du thread audio.
                for p in o.live.drain(..) {
                    if new.live.len() < MAX_PENDING {
                        new.live.push(p);
                    }
                }
                if new.midi != o.midi {
                    dropped += new.release_all();
                }
            }
        }
        self.midi_dropped += dropped;
        for node in &mut self.nodes {
            for ins in node.inserts.iter_mut().filter(|i| i.effect.is_none()) {
                'search: for o in &mut old.nodes {
                    for oi in &mut o.inserts {
                        if oi.id == ins.id && oi.effect.is_some() {
                            ins.effect = oi.effect.take();
                            break 'search;
                        }
                    }
                }
            }
        }
    }

    /// Déclare terminées les prises restées dans ce graphe (leur piste n'existe plus).
    pub(crate) fn close_taps(&mut self) {
        for n in &mut self.nodes {
            if let Some(t) = n.tap.as_ref() {
                t.close();
            }
            if let Some(t) = n.midi_tap.as_ref() {
                t.close();
            }
        }
    }

    /// Vide les lignes à retard et les mémoires des effets (déplacement de la tête de lecture).
    pub fn reset(&mut self) {
        let mut dropped = 0;
        for node in &mut self.nodes {
            node.out_delay.clear();
            for s in &mut node.sends {
                s.delay.clear();
            }
            for i in &mut node.inserts {
                if let Some(e) = i.effect.as_mut() {
                    e.reset();
                }
            }
            // Les paramètres automatisés seront renvoyés à la prochaine tranche.
            for pa in &mut node.auto_params {
                pa.last = f64::NAN;
            }
            // Un plugin n'oublie pas forcément ses voix en `reset` (AnalogSynth les garde : la note serait restée bloquée
            // pour toujours) : on lui envoie la fin des notes que l'on sait ouvertes, ainsi que pédale et molette.
            node.live.clear();
            node.events.clear();
            dropped += node.release_all();
        }
        self.midi_dropped += dropped;
    }

    /// Notes perdues faute de place dans les listes d'événements (doit rester 0).
    pub fn midi_dropped(&self) -> u32 {
        self.midi_dropped
    }

    /// Joue une note en direct sur une piste, au début du prochain bloc (inconnu ou file pleine : ignoré).
    pub fn push_live(&mut self, node: usize, msg: MidiMsg) {
        self.push_live_at(node, msg, 0);
    }

    /// Comme `push_live`, à `offset` échantillons après le début du prochain bloc (borné à ce bloc).
    pub fn push_live_at(&mut self, node: usize, msg: MidiMsg, offset: u32) {
        if let Some(n) = self.nodes.get_mut(node).filter(|n| n.is_track()) {
            if n.ahead.phase != Phase::Live {
                // Production confiée à la jumelle : la note s'entend avec le retard de l'avance.
                if let Some(tx) = n.twin_tx.as_mut() {
                    if tx.push(TwinCmd::Midi { msg, offset }).is_err() {
                        self.midi_dropped += 1;
                    }
                }
                return;
            }
            if n.live.len() < MAX_PENDING {
                n.live.push((offset, msg));
            } else {
                self.midi_dropped += 1;
            }
        }
    }

    /// Programme la coupure (note-off) de toutes les notes en cours, livrée au début du prochain bloc.
    pub fn notes_off_all(&mut self) {
        let mut dropped = 0;
        // Les pistes calculées en avance suivent la boucle toutes seules : elles coupent leurs notes elles-mêmes.
        for n in self.nodes.iter_mut().filter(|n| n.ahead.phase == Phase::Live) {
            dropped += n.release_all();
        }
        self.midi_dropped += dropped;
    }

    /// Panique : coupe tout ce qui sonne sur toutes les pistes, même ce que le suivi ne connaît pas. Fins des notes suivies,
    /// pédale et molette, « toutes les notes » / « tout le son » coupés sur les 16 canaux, puis une fin de note pour chacune
    /// des 16 × 128 touches, étalée sur quelques blocs. Ne touche ni à la position ni aux effets.
    pub fn panic(&mut self) {
        let mut dropped = 0;
        for n in self.nodes.iter_mut().filter(|n| n.is_track()) {
            match (&mut n.twin_tx, n.ahead.phase) {
                // Production confiée à la jumelle : elle reçoit l'ordre et le fait elle-même.
                (Some(tx), Phase::Ahead | Phase::Releasing) => {
                    if tx.push(TwinCmd::Panic).is_err() {
                        dropped += 1;
                    }
                }
                _ => dropped += n.panic(),
            }
        }
        self.midi_dropped += dropped;
    }

    // ---- paramètres (appelés par le thread audio, sans allocation) ----

    pub fn set_gain(&mut self, node: usize, gain: f32) {
        let ramp = self.ramp;
        if let (Some(n), true) = (self.nodes.get_mut(node), gain.is_finite()) {
            n.user_gain = gain;
            let g = n.effective_gain();
            n.gain.set_target(g, ramp);
        }
    }

    pub fn set_pan(&mut self, node: usize, pan: f32) {
        let ramp = self.ramp;
        if let (Some(n), true) = (self.nodes.get_mut(node), pan.is_finite()) {
            n.pan.set_target(pan.clamp(-1.0, 1.0), ramp);
        }
    }

    pub fn set_mute(&mut self, node: usize, mute: bool) {
        let ramp = self.ramp;
        if let Some(n) = self.nodes.get_mut(node) {
            n.mute = mute;
            let g = n.effective_gain();
            n.gain.set_target(g, ramp);
        }
    }

    /// Inverse (ou rétablit) la polarité d'un nœud : le gain passe de + à − (rampe courte, sans clic).
    pub fn set_invert(&mut self, node: usize, invert: bool) {
        let ramp = self.ramp;
        if let Some(n) = self.nodes.get_mut(node) {
            n.invert = invert;
            let g = n.effective_gain();
            n.gain.set_target(g, ramp);
        }
    }

    pub fn set_solo(&mut self, node: usize, solo: bool) {
        let ramp = self.ramp;
        if let Some(n) = self.nodes.get_mut(node) {
            n.solo = solo;
        }
        let any_solo = self.nodes.iter().any(|n| n.is_track() && n.solo);
        for n in &mut self.nodes {
            if n.is_track() {
                n.solo_excluded = any_solo && !n.solo;
                let g = n.effective_gain();
                n.gain.set_target(g, ramp);
            }
        }
    }

    pub fn set_send_mute(&mut self, node: usize, send: usize, mute: bool) {
        if let Some(s) = self.nodes.get_mut(node).and_then(|n| n.sends.get_mut(send)) {
            s.mute = mute;
            self.refresh_sends(self.ramp);
        }
    }

    pub fn set_send_solo(&mut self, node: usize, send: usize, solo: bool) {
        if let Some(s) = self.nodes.get_mut(node).and_then(|n| n.sends.get_mut(send)) {
            s.solo = solo;
            self.refresh_sends(self.ramp);
        }
    }

    /// Recalcule l'audibilité de chaque envoi : coupé, ou muet parce qu'un autre envoi vers la même destination est en solo.
    /// **Seulement vers un retour de monitoring** (un bus de type retour) : vers un bus ordinaire ou la sortie principale, la
    /// coupe et le solo des envois n'ont aucun effet (ils sont conservés : ils reviennent si le bus redevient un retour).
    /// Sans allocation (balayage des nœuds : rare, seulement sur commande).
    fn refresh_sends(&mut self, ramp: u32) {
        for i in 0..self.nodes.len() {
            for s in 0..self.nodes[i].sends.len() {
                let (to, mute, solo) = {
                    let x = &self.nodes[i].sends[s];
                    (x.to, x.mute, x.solo)
                };
                if !self.nodes.get(to).is_some_and(|d| d.is_return) {
                    self.nodes[i].sends[s].aud.set_target(1.0, ramp);
                    continue;
                }
                let others = self.nodes.iter().any(|n| n.sends.iter().any(|y| y.to == to && y.solo));
                let audible = !mute && (solo || !others);
                self.nodes[i].sends[s].aud.set_target(if audible { 1.0 } else { 0.0 }, ramp);
            }
        }
    }

    pub fn set_send_gain(&mut self, node: usize, send: usize, gain: f32) {
        let ramp = self.ramp;
        if let (Some(s), true) = (self.nodes.get_mut(node).and_then(|n| n.sends.get_mut(send)), gain.is_finite()) {
            s.gain.set_target(gain, ramp);
        }
    }

    /// Armement d'une piste : entrée audio, écoute et canal(aux). Inconnu ou pas une piste : ignoré.
    pub fn set_input(&mut self, node: usize, armed: bool, monitor: bool, source: RecordSource) {
        if let Some(n) = self.nodes.get_mut(node).filter(|n| n.is_track()) {
            let was = n.armed && n.monitor;
            n.armed = armed;
            n.monitor = monitor;
            n.source = source;
            if was != (armed && monitor) {
                // La ligne à retard de compensation change de rôle : on évite de rejouer d'anciens échantillons.
                n.out_delay.clear();
            }
        }
    }

    /// Installe (ou retire, avec `None`) la prise d'enregistrement d'une piste ; retourne l'ancienne.
    pub(crate) fn set_tap(&mut self, node: usize, tap: Option<RecordTap>) -> Option<RecordTap> {
        match self.nodes.get_mut(node).filter(|n| n.is_track()) {
            Some(n) => std::mem::replace(&mut n.tap, tap),
            None => tap, // piste inconnue : la prise revient telle quelle (jamais installée)
        }
    }

    /// Installe (ou retire) la prise des notes jouées en direct sur une piste ; retourne l'ancienne.
    pub(crate) fn set_midi_tap(&mut self, node: usize, tap: Option<MidiTap>) -> Option<MidiTap> {
        match self.nodes.get_mut(node).filter(|n| n.is_track()) {
            Some(n) => std::mem::replace(&mut n.midi_tap, tap),
            None => tap,
        }
    }

    /// Paramètre d'un effet, désigné par son identifiant (inconnu : ignoré).
    /// Contourne ou réactive un effet (rampe courte, sans clic). Identifiant inconnu : ignoré.
    pub fn set_insert_bypass(&mut self, insert: InsertId, bypass: bool) {
        for node in &mut self.nodes {
            if node.ahead.phase != Phase::Live && node.twin_inserts.contains(&insert) {
                // Production confiée à la jumelle : elle applique l'ordre entre deux blocs.
                if let Some(tx) = node.twin_tx.as_mut() {
                    let _ = tx.push(TwinCmd::Bypass { insert, bypass });
                }
                return;
            }
            for i in &mut node.inserts {
                if i.id == insert {
                    i.bypass = bypass;
                    return;
                }
            }
        }
    }

    pub fn set_insert_param(&mut self, insert: InsertId, param: u32, value: f64) {
        for node in &mut self.nodes {
            if node.ahead.phase != Phase::Live && node.twin_inserts.contains(&insert) {
                if let Some(tx) = node.twin_tx.as_mut() {
                    let _ = tx.push(TwinCmd::Param { insert, param, value });
                }
                return;
            }
            for i in &mut node.inserts {
                if i.id == insert {
                    if let Some(e) = i.effect.as_mut() {
                        e.set_param(param, value);
                    }
                    return;
                }
            }
        }
    }

    /// Calcule `n` échantillons (`n <= max_block`) de la position `pos` et les écrit au début de `out_l`/`out_r`, sur le seul fil appelant.
    ///
    /// `inputs` : un tampon par canal d'entrée du pilote (peut être vide : pas d'entrée).
    pub fn process(&mut self, pos: u64, n: usize, playing: bool, inputs: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32]) {
        self.process_with(None, pos, n, playing, inputs, out_l, out_r)
    }

    /// Comme `process`, avec un groupe de fils qui aide à calculer les nœuds d'un même niveau (voir `parallel.rs`). Le résultat est
    /// **identique au bit près** avec ou sans groupe : chaque nœud ne dépend que de lui-même, et le routage reste séquentiel, dans l'ordre
    /// de `order`.
    #[allow(clippy::too_many_arguments)]
    /// Nombre de canaux de sortie **au-delà de la paire principale** dont ce graphe a besoin (0 : aucun retour de monitoring).
    pub fn extra_outputs(&self) -> usize {
        self.phys_bufs.len()
    }

    /// Les `n` premiers échantillons du canal supplémentaire `k` (canal physique `k + 2`) pour le dernier bloc traité.
    pub fn extra_output(&self, k: usize, n: usize) -> &[f32] {
        &self.phys_bufs[k][..n]
    }

    /// Dit au graphe quelle boucle le transport joue (pour l'information donnée aux plugins).
    pub fn set_loop(&self, range: Option<(u64, u64)>) {
        self.time.set_loop(range);
    }

    pub fn process_with(&mut self, pool: Option<&Pool>, pos: u64, n: usize, playing: bool, inputs: &[&[f32]], out_l: &mut [f32], out_r: &mut [f32]) {
        let n = inputs.iter().fold(n.min(self.max_block).min(out_l.len()).min(out_r.len()), |m, s| m.min(s.len()));
        for node in &mut self.nodes {
            node.l[..n].fill(0.0);
            node.r[..n].fill(0.0);
        }
        for b in &mut self.phys_bufs {
            b[..n].fill(0.0);
        }
        let view = AheadView { epoch: self.ahead_epoch, rec: self.ahead_rec };
        let timing = pool.is_some();
        self.par_blocks = self.par_blocks.wrapping_add(1);
        let mut start = 0;
        for lv in 0..self.level_ends.len() {
            let end = self.level_ends[lv];
            let count = end - start;
            let mut parallel = false;
            if let Some(p) = pool {
                if count >= 2 && p.threads() >= 2 {
                    let est: f32 = self.order[start..end].iter().map(|&i| self.nodes[i].cost_ns).sum();
                    parallel = est >= PAR_MIN_NS;
                    if parallel && (self.assigned_for != p.threads() || self.par_blocks % REBALANCE_EVERY == 0) {
                        self.rebalance(p.threads());
                    }
                }
            }
            if parallel {
                let mut ctx = LevelCtx {
                    nodes: self.nodes.as_mut_ptr(),
                    order: self.order[start..end].as_ptr(),
                    pos,
                    n,
                    playing,
                    inputs: inputs.as_ptr() as *const &'static [f32],
                    n_inputs: inputs.len(),
                    meters: self.meters.as_deref().map_or(std::ptr::null(), |m| m as *const MeterBank),
                    view,
                };
                // SAFETY: `run_node` n'accède, pour le nœud `order[start + k]`, qu'à ce nœud (chaque `k` est pris une seule fois par
                // tâche, et les nœuds d'un même niveau sont distincts) ; `inputs` et `meters` sont en lecture seule / atomiques ; `ctx`
                // et ce qu'il désigne restent valides jusqu'au retour de `run`, qui n'a lieu qu'une fois tous les nœuds finis.
                unsafe { pool.expect("groupe présent").run(count, &self.assign[start..end], &mut ctx as *mut LevelCtx as *mut (), run_node) };
            } else {
                let meters = self.meters.as_deref();
                for k in start..end {
                    let i = self.order[k];
                    let t0 = timing.then(std::time::Instant::now);
                    self.nodes[i].compute(i, pos, n, playing, inputs, meters, view);
                    if let Some(t0) = t0 {
                        let c = &mut self.nodes[i].cost_ns;
                        *c = *c * COST_KEEP + t0.elapsed().as_nanos() as f32 * (1.0 - COST_KEEP);
                    }
                }
            }
            self.collect_ahead_flags(start, end);
            for k in start..end {
                let i = self.order[k];
                self.midi_dropped += self.nodes[i].block_dropped;
                if self.nodes[i].kind == NodeKind::Master {
                    out_l[..n].copy_from_slice(&self.nodes[i].l[..n]);
                    out_r[..n].copy_from_slice(&self.nodes[i].r[..n]);
                    continue;
                }
                self.fill_sidechains(i, n);
                if let Some(p) = self.nodes[i].phys {
                    // Retour de monitoring : le bus sort sur la carte son et n'alimente pas la sortie principale.
                    let ch = usize::from(p.first) - 2;
                    let (l, r) = (&self.nodes[i].l, &self.nodes[i].r);
                    if p.stereo && !p.mono {
                        self.phys_bufs[ch][..n].copy_from_slice(&l[..n]);
                        self.phys_bufs[ch + 1][..n].copy_from_slice(&r[..n]);
                    } else {
                        for k in 0..n {
                            self.phys_bufs[ch][k] = (l[k] + r[k]) * 0.5;
                        }
                        if p.stereo {
                            let (a, b) = self.phys_bufs.split_at_mut(ch + 1);
                            b[0][..n].copy_from_slice(&a[ch][..n]);
                        }
                    }
                } else if !self.nodes[i].is_return {
                    let out = self.nodes[i].output;
                    route(&mut self.nodes, i, out, n, None, pos);
                }
                for s in 0..self.nodes[i].sends.len() {
                    let to = self.nodes[i].sends[s].to;
                    route(&mut self.nodes, i, to, n, Some(s), pos);
                }
            }
            start = end;
        }
    }

    /// Le nœud `src` a fini son bloc : son signal (avant le fader) part, retardé, vers la chaîne latérale des effets qui l'ont pour source.
    /// Les destinations sont d'un niveau plus haut : elles n'ont pas encore été calculées.
    fn fill_sidechains(&mut self, src: usize, n: usize) {
        for e in 0..self.sc_edges.len() {
            let (s, d, k) = self.sc_edges[e];
            if s != src {
                continue;
            }
            let (from, to) = two_mut(&mut self.nodes, s, d);
            if let Some(sc) = to.inserts.get_mut(k).and_then(|i| i.sc.as_mut()) {
                for j in 0..n.min(from.pre_l.len()) {
                    let (a, b) = sc.delay.tap(from.pre_l[j], from.pre_r[j]);
                    sc.buf_l[j] = a;
                    sc.buf_r[j] = b;
                }
            }
        }
    }

    /// Répartit les nœuds de chaque niveau entre `threads` fils d'après leur coût mesuré : on donne le plus coûteux au fil le moins
    /// chargé (algorithme glouton). Sans allocation. Les égalités se départagent par position : répartition déterministe.
    fn rebalance(&mut self, threads: usize) {
        let threads = threads.clamp(1, 64);
        let mut start = 0;
        for lv in 0..self.level_ends.len() {
            let end = self.level_ends[lv];
            self.scratch.clear();
            self.scratch.extend(start..end);
            let (nodes, order) = (&self.nodes, &self.order);
            self.scratch.sort_unstable_by(|&a, &b| nodes[order[b]].cost_ns.total_cmp(&nodes[order[a]].cost_ns).then(a.cmp(&b)));
            let mut load = [0.0f32; 64];
            for &k in &self.scratch {
                let t = (0..threads).fold(0, |best, t| if load[t] < load[best] { t } else { best });
                self.assign[k] = t as u8;
                load[t] += self.nodes[self.order[k]].cost_ns.max(1.0);
            }
            start = end;
        }
        self.assigned_for = threads;
    }
}
