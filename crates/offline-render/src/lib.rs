//! Rendu hors ligne : fait tourner le vrai moteur, sans pilote audio, aussi vite que le processeur le permet.
//!
//! Les événements (commandes) sont appliqués à l'échantillon près : le bloc est coupé à l'instant de
//! l'événement. Le résultat ne dépend donc ni de la taille de bloc demandée ni de la machine (même binaire
//! ou, pour les opérations exactes du moteur, même résultat entre plateformes).

use std::io::{self, Write};
use std::sync::Arc;

pub mod export;

use daw_engine::{Command, Engine, GraphDesc, GraphError, LoadError, MidiMsg, RecordReceiver, RecordSource, RecordedMidi};

#[derive(Debug, Clone)]
pub struct RenderSpec {
    pub sample_rate: u32,
    /// Durée à rendre, en échantillons.
    pub frames: u64,
    /// Taille des blocs passés au moteur (la sortie ne doit pas en dépendre).
    pub block: usize,
    /// Plus grand bloc traité d'un coup par le moteur.
    pub engine_block: usize,
    pub graph: GraphDesc,
    /// Commandes datées en échantillons (appliquées avant le calcul de cet échantillon). Pour jouer :
    /// `(0, Command::Play)`.
    pub events: Vec<(u64, Command)>,
    /// Compense la latence du graphe : on rend `latency` échantillons de plus et on retire les premiers, de sorte
    /// que le fichier obtenu est aligné sur la ligne de temps (comme un export). À désactiver pour voir la sortie
    /// brute, telle qu'on l'entendrait en lecture.
    pub compensate_latency: bool,
    /// Signal d'entrée simulé (gauche, droite), indexé par le numéro d'échantillon du rendu depuis 0 ; complété par
    /// du silence s'il est plus court. Alimente l'écoute et l'enregistrement des pistes armées.
    pub input: Option<(Vec<f32>, Vec<f32>)>,
    /// Prises d'enregistrement à faire pendant le rendu.
    pub records: Vec<RecordJob>,
    /// Messages MIDI joués en direct : (échantillon du rendu, piste, message). Livrés à l'échantillon près, sans couper
    /// les blocs (le décalage dans le bloc est celui que donnerait un pilote).
    pub midi_in: Vec<(u64, u32, MidiMsg)>,
    /// Prises MIDI à faire pendant le rendu.
    pub midi_records: Vec<MidiRecordJob>,
    /// Appelée après chaque bloc avec (échantillons faits, échantillons à faire) ; renvoyer `false` annule le rendu
    /// (`RenderError::Cancelled`). Ne change jamais le résultat d'un rendu qui va au bout.
    pub progress: Option<Progress>,
}

/// Fonction de progression / d'annulation d'un rendu (voir [`RenderSpec::progress`]).
#[derive(Clone)]
pub struct Progress(pub Arc<dyn Fn(u64, u64) -> bool + Send + Sync>);

impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Progress")
    }
}

/// Prise MIDI : les notes jouées en direct sur la piste `node` entre `start` et `stop` (échantillons du rendu).
#[derive(Debug, Clone)]
pub struct MidiRecordJob {
    pub node: u32,
    pub start: u64,
    pub stop: u64,
}

/// Prise MIDI enregistrée.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiTake {
    pub node: u32,
    pub start_pos: u64,
    pub stop_pos: u64,
    pub events: Vec<RecordedMidi>,
    pub overruns: u32,
}

/// Une prise : la piste `node` est armée de `start` à `stop` (échantillons du rendu) et son entrée est enregistrée.
#[derive(Debug, Clone)]
pub struct RecordJob {
    pub node: u32,
    pub source: RecordSource,
    pub start: u64,
    pub stop: u64,
    /// Écoute de l'entrée à travers la piste (sans compensation de latence) pendant la prise.
    pub monitor: bool,
}

/// Prise enregistrée.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    pub node: u32,
    pub source: RecordSource,
    /// Position de la ligne de temps (échantillons) au premier échantillon enregistré.
    pub start_pos: u64,
    /// Échantillons entrelacés (1 ou 2 canaux selon la source).
    pub data: Vec<f32>,
    pub overruns: u32,
}

impl RenderSpec {
    pub fn new(sample_rate: u32, frames: u64, graph: GraphDesc) -> Self {
        RenderSpec { sample_rate, frames, block: 256, engine_block: 256, graph, events: vec![(0, Command::Play)], compensate_latency: true, input: None, records: Vec::new(), midi_in: Vec::new(), midi_records: Vec::new(), progress: None }
    }
}

#[derive(Debug, PartialEq)]
pub enum RenderError {
    Graph(GraphError),
    /// Trop de commandes au même échantillon pour la file du moteur.
    TooManyEvents(u64),
    InvalidBlock,
    /// La fonction de progression a demandé l'arrêt.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub sample_rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub takes: Vec<Take>,
    pub midi_takes: Vec<MidiTake>,
}

pub fn render(spec: &RenderSpec) -> Result<Rendered, RenderError> {
    if spec.block == 0 || spec.engine_block == 0 {
        return Err(RenderError::InvalidBlock);
    }
    let (mut engine, mut handle) = Engine::new(spec.sample_rate, spec.engine_block);
    handle.load_graph(&spec.graph).map_err(|e| match e {
        LoadError::Graph(g) => RenderError::Graph(g),
        LoadError::QueueFull => RenderError::TooManyEvents(0),
    })?;

    enum Act {
        Cmd(Command),
        Start(usize),
        Stop(usize),
        MStart(usize),
        MStop(usize),
    }
    let mut events: Vec<(u64, Act)> = spec.events.iter().map(|&(t, c)| (t, Act::Cmd(c))).collect();
    for (k, j) in spec.records.iter().enumerate() {
        events.push((j.start, Act::Start(k)));
        events.push((j.stop, Act::Stop(k)));
    }
    for (k, j) in spec.midi_records.iter().enumerate() {
        events.push((j.start, Act::MStart(k)));
        events.push((j.stop, Act::MStop(k)));
    }
    events.sort_by_key(|e| e.0); // tri stable : l'ordre des événements simultanés est conservé
    let latency = if spec.compensate_latency { handle.latency_samples() as u64 } else { 0 };
    let total = spec.frames + latency;
    let (mut left, mut right) = (vec![0.0f32; total as usize], vec![0.0f32; total as usize]);
    let (in_l, in_r) = match &spec.input {
        Some((l, r)) => {
            let pad = |v: &Vec<f32>| {
                let mut v = v.clone();
                v.resize(total as usize, 0.0);
                v
            };
            (pad(l), pad(r))
        }
        None => (vec![0.0f32; total as usize], vec![0.0f32; total as usize]),
    };

    let mut receivers: Vec<Option<RecordReceiver>> = spec.records.iter().map(|_| None).collect();
    let mut takes: Vec<Take> = spec.records.iter().map(|j| Take { node: j.node, source: j.source, start_pos: 0, data: Vec::new(), overruns: 0 }).collect();
    let drain = |receivers: &mut Vec<Option<RecordReceiver>>, takes: &mut Vec<Take>| {
        for (rx, take) in receivers.iter_mut().zip(takes.iter_mut()) {
            if let Some(rx) = rx {
                let n = rx.consumer.slots();
                if n > 0 {
                    let chunk = rx.consumer.read_chunk(n).expect("n échantillons sont lisibles");
                    let (a, b) = chunk.as_slices();
                    take.data.extend_from_slice(a);
                    take.data.extend_from_slice(b);
                    chunk.commit_all();
                }
            }
        }
    };

    let mut midi_tx = handle.take_midi_input().expect("première prise de l'entrée MIDI");
    let mut midi_in = spec.midi_in.clone();
    midi_in.sort_by_key(|e| e.0);
    let mut next_midi = 0usize;
    let mut midi_rx: Vec<Option<daw_engine::MidiRecordReceiver>> = spec.midi_records.iter().map(|_| None).collect();
    let mut midi_takes: Vec<MidiTake> = spec.midi_records.iter().map(|j| MidiTake { node: j.node, start_pos: 0, stop_pos: 0, events: Vec::new(), overruns: 0 }).collect();

    let (mut pos, mut next_event) = (0u64, 0usize);
    while pos < total {
        while next_event < events.len() && events[next_event].0 <= pos {
            let ok = match &events[next_event].1 {
                Act::Cmd(c) => handle.send(*c),
                Act::Start(k) => {
                    let j = &spec.records[*k];
                    let seconds = (j.stop.saturating_sub(j.start) as f32 / spec.sample_rate as f32) + 1.0;
                    match handle.start_record(j.node, j.source, seconds) {
                        Ok(rx) => {
                            receivers[*k] = Some(rx);
                            handle.send(Command::SetInput { node: j.node, armed: true, monitor: j.monitor, source: j.source })
                        }
                        Err(_) => false,
                    }
                }
                Act::MStart(k) => match handle.start_midi_record(spec.midi_records[*k].node, 4096) {
                    Ok(rx) => {
                        midi_rx[*k] = Some(rx);
                        true
                    }
                    Err(_) => false,
                },
                Act::MStop(k) => handle.stop_midi_record(spec.midi_records[*k].node),
                Act::Stop(k) => {
                    let j = &spec.records[*k];
                    handle.send(Command::SetInput { node: j.node, armed: false, monitor: false, source: j.source }) && handle.stop_record(j.node)
                }
            };
            if !ok {
                return Err(RenderError::TooManyEvents(pos));
            }
            next_event += 1;
        }
        let until_event = events.get(next_event).map_or(u64::MAX, |e| e.0) - pos;
        let n = (spec.block as u64).min(total - pos).min(until_event) as usize;
        let p = pos as usize;
        while next_midi < midi_in.len() && midi_in[next_midi].0 < pos + n as u64 {
            let (t, node, msg) = midi_in[next_midi];
            if t >= pos && !midi_tx.push(node, msg, (t - pos) as u32) {
                return Err(RenderError::TooManyEvents(t));
            }
            next_midi += 1;
        }
        engine.process_io(&in_l[p..p + n], &in_r[p..p + n], &mut left[p..p + n], &mut right[p..p + n]);
        drain(&mut receivers, &mut takes);
        for (rx, take) in midi_rx.iter_mut().zip(midi_takes.iter_mut()) {
            if let Some(rx) = rx {
                take.events.extend(rx.take_events());
            }
        }
        pos += n as u64;
        if let Some(p) = &spec.progress {
            if !(p.0)(pos, total) {
                return Err(RenderError::Cancelled);
            }
        }
    }
    // Prises encore ouvertes à la fin du rendu : on les ferme, et un appel à vide fait traiter les messages en attente.
    for (k, j) in spec.records.iter().enumerate() {
        if receivers[k].as_ref().is_some_and(|rx| !rx.status.closed.load(std::sync::atomic::Ordering::Acquire)) {
            handle.stop_record(j.node);
        }
    }
    for (k, j) in spec.midi_records.iter().enumerate() {
        if midi_rx[k].as_ref().is_some_and(|rx| !rx.is_closed()) {
            handle.stop_midi_record(j.node);
        }
    }
    engine.process_io(&[], &[], &mut [], &mut []);
    drain(&mut receivers, &mut takes);
    for (rx, take) in midi_rx.iter_mut().zip(midi_takes.iter_mut()) {
        if let Some(rx) = rx {
            take.events.extend(rx.take_events());
            take.start_pos = rx.start_pos();
            take.stop_pos = rx.stop_pos();
            take.overruns = rx.status.overruns.load(std::sync::atomic::Ordering::Relaxed);
        }
    }
    for (rx, take) in receivers.iter().zip(takes.iter_mut()) {
        if let Some(rx) = rx {
            take.start_pos = rx.status.start_pos.load(std::sync::atomic::Ordering::Acquire);
            take.overruns = rx.status.overruns.load(std::sync::atomic::Ordering::Relaxed);
        }
    }
    handle.collect_garbage();
    left.drain(..latency as usize);
    right.drain(..latency as usize);
    Ok(Rendered { sample_rate: spec.sample_rate, left, right, takes, midi_takes })
}

impl Rendered {
    /// Empreinte FNV-1a 64 bits des échantillons (bits exacts, gauche puis droite, entrelacés).
    /// Sert aux « golden files » : deux rendus sont identiques au bit près si et seulement si leurs empreintes le sont.
    pub fn hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for (l, r) in self.left.iter().zip(&self.right) {
            for b in l.to_bits().to_le_bytes().into_iter().chain(r.to_bits().to_le_bytes()) {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        h
    }

    pub fn peak(&self) -> f32 {
        self.left.iter().chain(&self.right).fold(0.0, |m, s| m.max(s.abs()))
    }

    /// WAV stéréo flottant 32 bits (format IEEE, tag 3).
    pub fn write_wav<W: Write>(&self, mut w: W) -> io::Result<()> {
        let data_len = (self.left.len() * 8) as u32;
        w.write_all(b"RIFF")?;
        w.write_all(&(36 + data_len).to_le_bytes())?;
        w.write_all(b"WAVEfmt ")?;
        w.write_all(&16u32.to_le_bytes())?;
        w.write_all(&3u16.to_le_bytes())?; // IEEE float
        w.write_all(&2u16.to_le_bytes())?; // canaux
        w.write_all(&self.sample_rate.to_le_bytes())?;
        w.write_all(&(self.sample_rate * 8).to_le_bytes())?; // octets par seconde
        w.write_all(&8u16.to_le_bytes())?; // octets par trame
        w.write_all(&32u16.to_le_bytes())?; // bits par échantillon
        w.write_all(b"data")?;
        w.write_all(&data_len.to_le_bytes())?;
        for (l, r) in self.left.iter().zip(&self.right) {
            w.write_all(&l.to_le_bytes())?;
            w.write_all(&r.to_le_bytes())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_and_payload() {
        let r = Rendered { sample_rate: 48_000, left: vec![0.5, -0.25], right: vec![1.0, 0.0], takes: vec![], midi_takes: vec![] };
        let mut buf = Vec::new();
        r.write_wav(&mut buf).unwrap();
        assert_eq!(&buf[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), 36 + 16);
        assert_eq!(&buf[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(buf[20..22].try_into().unwrap()), 3);
        assert_eq!(u16::from_le_bytes(buf[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(buf[24..28].try_into().unwrap()), 48_000);
        assert_eq!(&buf[36..40], b"data");
        assert_eq!(u32::from_le_bytes(buf[40..44].try_into().unwrap()), 16);
        assert_eq!(buf.len(), 44 + 16);
        assert_eq!(f32::from_le_bytes(buf[44..48].try_into().unwrap()), 0.5);
        assert_eq!(f32::from_le_bytes(buf[48..52].try_into().unwrap()), 1.0);
    }

    #[test]
    fn hash_changes_with_one_bit() {
        let a = Rendered { sample_rate: 48_000, left: vec![1.0; 4], right: vec![0.0; 4], takes: vec![], midi_takes: vec![] };
        let mut b = a.clone();
        assert_eq!(a.hash(), b.hash());
        b.right[3] = f32::from_bits(1);
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn rejects_zero_block() {
        let mut s = RenderSpec::new(48_000, 10, GraphDesc::new());
        s.block = 0;
        assert_eq!(render(&s), Err(RenderError::InvalidBlock));
    }
}
