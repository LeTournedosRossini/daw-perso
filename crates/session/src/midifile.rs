//! Fichiers MIDI standard (SMF) : lecture et écriture d'octets, sans accès disque.
//!
//! Géré : formats 0 et 1, division en ticks par noire, notes, contrôleurs, pressions, molette de hauteur, tempo
//! (méta 0x51), chiffrage (méta 0x58), nom de piste (méta 0x03), statut courant (« running status »). Ignoré et compté :
//! changements de programme, SysEx, autres méta-événements. Refusé : format 2, division en temps SMPTE.

use daw_core_types::time::{Frac, Meter, MeterMap, Tempo, TempoMap};
use daw_core_types::NodeId;
use daw_engine::{parse_midi, MidiMsg};

use crate::{music, MidiClipId, Session, SessionError, RECORD_PPQ};

#[derive(Debug, PartialEq, Eq)]
pub enum MidiFileError {
    /// Pas un fichier MIDI (en-tête `MThd` absent ou incohérent).
    NotMidi,
    /// Format 2 (séquences indépendantes) ou division SMPTE.
    Unsupported(&'static str),
    /// Fichier coupé ou longueur incohérente.
    Truncated,
    /// Octet de données invalide ou événement sans statut.
    Corrupt(&'static str),
}

/// Une piste : messages aux positions absolues en ticks, dans l'ordre du fichier (stable).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MidiTrack {
    pub name: Option<String>,
    pub events: Vec<(u64, MidiMsg)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MidiFile {
    /// Ticks par noire.
    pub ppq: u16,
    pub tracks: Vec<MidiTrack>,
    /// (tick, microsecondes par noire), toutes pistes confondues, triés.
    pub tempos: Vec<(u64, u32)>,
    /// (tick, numérateur, puissance de deux du dénominateur), triés.
    pub meters: Vec<(u64, u8, u8)>,
    /// Messages sans équivalent dans notre modèle (programmes, SysEx, autres méta) : ignorés.
    pub skipped: usize,
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, MidiFileError> {
        let v = *self.b.get(self.i).ok_or(MidiFileError::Truncated)?;
        self.i += 1;
        Ok(v)
    }
    fn take(&mut self, n: usize) -> Result<&[u8], MidiFileError> {
        let end = self.i.checked_add(n).filter(|&e| e <= self.b.len()).ok_or(MidiFileError::Truncated)?;
        let s = &self.b[self.i..end];
        self.i = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16, MidiFileError> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32, MidiFileError> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    /// Nombre à longueur variable (7 bits par octet, 4 octets au plus).
    fn vlq(&mut self) -> Result<u32, MidiFileError> {
        let mut v = 0u32;
        for _ in 0..4 {
            let b = self.u8()?;
            v = (v << 7) | (b & 0x7F) as u32;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(MidiFileError::Corrupt("nombre à longueur variable trop long"))
    }
}

pub fn parse(bytes: &[u8]) -> Result<MidiFile, MidiFileError> {
    let mut r = Reader { b: bytes, i: 0 };
    if r.take(4).map_err(|_| MidiFileError::NotMidi)? != b"MThd" {
        return Err(MidiFileError::NotMidi);
    }
    if r.u32()? < 6 {
        return Err(MidiFileError::NotMidi);
    }
    let format = r.u16()?;
    let ntracks = r.u16()?;
    let division = r.u16()?;
    if format > 1 {
        return Err(MidiFileError::Unsupported("format 2"));
    }
    if division & 0x8000 != 0 || division == 0 {
        return Err(MidiFileError::Unsupported("division en temps SMPTE"));
    }
    // L'en-tête peut être plus long que 6 octets : on saute le reste.
    let header_len = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    r.i = 8 + header_len;
    let mut file = MidiFile { ppq: division, tracks: Vec::new(), tempos: Vec::new(), meters: Vec::new(), skipped: 0 };
    while file.tracks.len() < ntracks as usize && r.i < bytes.len() {
        let id = r.take(4)?.to_vec();
        let len = r.u32()? as usize;
        let body = r.take(len)?;
        if id != b"MTrk" {
            continue; // bloc inconnu : ignoré
        }
        file.tracks.push(parse_track(body, &mut file.tempos, &mut file.meters, &mut file.skipped)?);
    }
    if file.tracks.len() != ntracks as usize {
        return Err(MidiFileError::Truncated);
    }
    file.tempos.sort_by_key(|t| t.0);
    file.meters.sort_by_key(|t| t.0);
    Ok(file)
}

fn parse_track(body: &[u8], tempos: &mut Vec<(u64, u32)>, meters: &mut Vec<(u64, u8, u8)>, skipped: &mut usize) -> Result<MidiTrack, MidiFileError> {
    let mut r = Reader { b: body, i: 0 };
    let mut track = MidiTrack::default();
    let (mut tick, mut running) = (0u64, 0u8);
    while r.i < body.len() {
        tick += r.vlq()? as u64;
        let mut status = r.u8()?;
        if status < 0x80 {
            // Statut courant : l'octet lu est déjà une donnée.
            if running < 0x80 {
                return Err(MidiFileError::Corrupt("événement sans statut"));
            }
            r.i -= 1;
            status = running;
        }
        match status {
            0xFF => {
                let kind = r.u8()?;
                let len = r.vlq()? as usize;
                let data = r.take(len)?;
                match (kind, data) {
                    (0x2F, _) => break,
                    (0x51, [a, b, c]) => tempos.push((tick, u32::from_be_bytes([0, *a, *b, *c]))),
                    (0x58, [n, d, ..]) => meters.push((tick, *n, *d)),
                    (0x03, name) => track.name = Some(String::from_utf8_lossy(name).into_owned()),
                    _ => *skipped += 1,
                }
            }
            0xF0 | 0xF7 => {
                let len = r.vlq()? as usize;
                r.take(len)?;
                *skipped += 1;
            }
            0xF1..=0xFE => return Err(MidiFileError::Corrupt("message système dans une piste")),
            s => {
                running = s;
                let n = if matches!(s & 0xF0, 0xC0 | 0xD0) { 1 } else { 2 };
                let d1 = r.u8()?;
                let d2 = if n == 2 { r.u8()? } else { 0 };
                if d1 > 127 || d2 > 127 {
                    return Err(MidiFileError::Corrupt("octet de données invalide"));
                }
                match parse_midi(&[s, d1, d2][..1 + n]) {
                    Some(m) => track.events.push((tick, m)),
                    None => *skipped += 1,
                }
            }
        }
    }
    Ok(track)
}

fn put_vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut bytes = [0u8; 5];
    let mut n = 0;
    loop {
        bytes[n] = (v & 0x7F) as u8;
        v >>= 7;
        n += 1;
        if v == 0 {
            break;
        }
    }
    for k in (0..n).rev() {
        out.push(bytes[k] | if k > 0 { 0x80 } else { 0 });
    }
}

fn raw(msg: &MidiMsg) -> ([u8; 3], usize) {
    let vel = |v: f32| (v * 127.0).round().clamp(0.0, 127.0) as u8;
    match *msg {
        MidiMsg::NoteOn { channel, key, velocity } => ([0x90 | channel, key, vel(velocity).max(1)], 3),
        MidiMsg::NoteOff { channel, key, velocity } => ([0x80 | channel, key, vel(velocity)], 3),
        MidiMsg::ControlChange { channel, controller, value } => ([0xB0 | channel, controller, value], 3),
        MidiMsg::PitchBend { channel, value } => {
            let raw = (value as i32 + 8192).clamp(0, 16383) as u16;
            ([0xE0 | channel, (raw & 127) as u8, (raw >> 7) as u8], 3)
        }
        MidiMsg::ChannelPressure { channel, value } => ([0xD0 | channel, value, 0], 2),
        MidiMsg::PolyPressure { channel, key, value } => ([0xA0 | channel, key, value], 3),
    }
}

fn track_chunk(out: &mut Vec<u8>, body: Vec<u8>) {
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
}

/// Écrit un fichier de format 1 : une piste « chef d'orchestre » (tempos et chiffrages), puis les pistes de `file`.
/// À instant égal : fins de notes, puis contrôleurs, puis débuts de notes. Pas de statut courant (fichier plus gros,
/// mais lisible partout).
pub fn write(file: &MidiFile) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(file.tracks.len() as u16 + 1).to_be_bytes());
    out.extend_from_slice(&file.ppq.to_be_bytes());

    let mut conductor = Vec::new();
    let mut meta: Vec<(u64, Vec<u8>)> = Vec::new();
    for &(t, us) in &file.tempos {
        meta.push((t, vec![0xFF, 0x51, 3, (us >> 16) as u8, (us >> 8) as u8, us as u8]));
    }
    for &(t, n, d) in &file.meters {
        meta.push((t, vec![0xFF, 0x58, 4, n, d, 24, 8]));
    }
    meta.sort_by_key(|m| m.0);
    let mut last = 0u64;
    for (t, bytes) in meta {
        put_vlq(&mut conductor, (t - last) as u32);
        conductor.extend_from_slice(&bytes);
        last = t;
    }
    put_vlq(&mut conductor, 0);
    conductor.extend_from_slice(&[0xFF, 0x2F, 0]);
    track_chunk(&mut out, conductor);

    for track in &file.tracks {
        let mut body = Vec::new();
        if let Some(name) = &track.name {
            put_vlq(&mut body, 0);
            body.extend_from_slice(&[0xFF, 0x03]);
            put_vlq(&mut body, name.len() as u32);
            body.extend_from_slice(name.as_bytes());
        }
        let class = |m: &MidiMsg| match m {
            MidiMsg::NoteOff { .. } => 0u8,
            MidiMsg::NoteOn { .. } => 2,
            _ => 1,
        };
        let mut events: Vec<&(u64, MidiMsg)> = track.events.iter().collect();
        events.sort_by_key(|(t, m)| (*t, class(m))); // tri stable
        let mut last = 0u64;
        for (t, msg) in events {
            put_vlq(&mut body, (*t - last) as u32);
            let (bytes, n) = raw(msg);
            body.extend_from_slice(&bytes[..n]);
            last = *t;
        }
        put_vlq(&mut body, 0);
        body.extend_from_slice(&[0xFF, 0x2F, 0]);
        track_chunk(&mut out, body);
    }
    out
}

// ---- import et export dans la session --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct ImportOptions {
    /// Remplace la carte des tempos et des chiffrages du projet par celles du fichier.
    pub import_tempo: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Un clip par piste du fichier qui contient des messages, dans l'ordre.
    pub clips: Vec<MidiClipId>,
    /// Messages sans équivalent (changements de programme, SysEx...) ignorés.
    pub skipped_messages: usize,
    /// Changements de chiffrage qui ne tombent pas en début de mesure (non gérés) ignorés.
    pub skipped_meters: usize,
}

impl Session {
    /// Importe un fichier MIDI : un clip de notes par piste du fichier, sur le nœud `node`, à partir du début du projet,
    /// prolongé jusqu'à la fin de la mesure. Les positions sont **exactes** (`tick / (4 × ticks par noire)` de ronde,
    /// sans arrondi). Une seule action annulable.
    pub fn import_midi_file(&mut self, file: &MidiFile, node: NodeId, opts: ImportOptions) -> Result<ImportReport, SessionError> {
        let ppq = file.ppq as u32;
        let frac = |tick: u64| TempoMap::from_ticks(tick, ppq).ok_or(SessionError::TimeOverflow);
        let mut report = ImportReport { skipped_messages: file.skipped, ..Default::default() };
        self.group("Importer un fichier MIDI", |s| {
            if opts.import_tempo {
                let mut timing = s.timing.clone();
                if let Some(&(t0, us)) = file.tempos.first() {
                    let first = Tempo::new(60_000_000, us).ok_or(SessionError::BadMidi)?;
                    let mut map = TempoMap::new(if t0 == 0 { first } else { timing.tempo.tempo_at(Frac::ZERO) });
                    for &(tick, us) in &file.tempos {
                        map.set_tempo(frac(tick)?, Tempo::new(60_000_000, us).ok_or(SessionError::BadMidi)?);
                    }
                    timing.tempo = map;
                }
                if let Some(&(_, n, d)) = file.meters.first() {
                    let first = Meter::new(n as u32, 1u32 << d.min(31)).ok_or(SessionError::BadMidi)?;
                    let mut map = MeterMap::new(first);
                    for &(tick, n, d) in file.meters.iter().skip(1) {
                        let m = Meter::new(n as u32, 1u32 << d.min(31)).ok_or(SessionError::BadMidi)?;
                        match map.locate(frac(tick)?) {
                            Some((bar, within)) if within == Frac::ZERO => map.set_meter(bar, m),
                            _ => report.skipped_meters += 1,
                        }
                    }
                    timing.meters = map;
                }
                s.set_timing("Importer le tempo", |t| *t = timing)?;
            }
            for track in file.tracks.iter().filter(|t| !t.events.is_empty()) {
                let t_end = track.events.iter().map(|e| e.0).max().unwrap_or(0);
                let Some(mut clip) = music::build_clip(&track.events, 0, t_end, ppq).ok_or(SessionError::TimeOverflow)? else { continue };
                // Jusqu'à la fin de la mesure en cours.
                let end = clip.start.checked_add(clip.len).ok_or(SessionError::TimeOverflow)?;
                if let Some((bar, within)) = s.timing.meters.locate(end) {
                    if within != Frac::ZERO {
                        let next = s.timing.meters.bar_start(bar + 1).ok_or(SessionError::TimeOverflow)?;
                        clip.len = next.checked_sub(clip.start).ok_or(SessionError::TimeOverflow)?;
                    }
                }
                report.clips.push(s.add_midi_clip(node, clip)?);
            }
            Ok(())
        })?;
        Ok(report)
    }

    /// Exporte les clips de notes en fichier MIDI de format 1 (960 ticks par noire) : une piste de tempo et de chiffrage,
    /// puis une piste par nœud qui a des clips de notes. Les positions sont arrondies au tick ; le tempo est arrondi à la
    /// microsecode par noire (≤ 0,5 µs d'erreur).
    pub fn export_midi_file(&self) -> Result<Vec<u8>, SessionError> {
        let ppq = RECORD_PPQ;
        let tick = |f: Frac| TempoMap::to_ticks(f, ppq).ok_or(SessionError::TimeOverflow);
        let mut file = MidiFile { ppq: ppq as u16, tracks: Vec::new(), tempos: Vec::new(), meters: Vec::new(), skipped: 0 };
        for (pos, t) in self.timing.tempo.changes() {
            let (num, den) = (t.num() as u64, t.den() as u64);
            let us = (60_000_000u64 * den + num / 2) / num;
            if us == 0 || us > 0x00FF_FFFF {
                return Err(SessionError::TimeOverflow);
            }
            file.tempos.push((tick(*pos)?, us as u32));
        }
        for (bar, m) in self.timing.meters.changes() {
            let at = self.timing.meters.bar_start(*bar).ok_or(SessionError::TimeOverflow)?;
            file.meters.push((tick(at)?, u8::try_from(m.num()).map_err(|_| SessionError::TimeOverflow)?, m.den().trailing_zeros() as u8));
        }
        for (n, clips) in self.midi.iter().enumerate() {
            if clips.is_empty() {
                continue;
            }
            let mut track = MidiTrack { name: Some(self.graph.nodes[n].name.clone()), events: Vec::new() };
            for (_, c) in clips {
                for note in &c.notes {
                    let on = c.start.checked_add(note.start).ok_or(SessionError::TimeOverflow)?;
                    let off = on.checked_add(note.len).ok_or(SessionError::TimeOverflow)?;
                    let (t_on, t_off) = (tick(on)?, tick(off)?.max(tick(on)? + 1));
                    track.events.push((t_on, MidiMsg::NoteOn { channel: note.channel, key: note.key, velocity: note.velocity }));
                    track.events.push((t_off, MidiMsg::NoteOff { channel: note.channel, key: note.key, velocity: 0.0 }));
                }
                for e in &c.events {
                    track.events.push((tick(c.start.checked_add(e.start).ok_or(SessionError::TimeOverflow)?)?, e.msg()));
                }
            }
            file.tracks.push(track);
        }
        Ok(write(&file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_length_numbers_round_trip() {
        for v in [0u32, 1, 127, 128, 8192, 16383, 16384, 0x1FFFFF, 0x0FFFFFFF] {
            let mut b = Vec::new();
            put_vlq(&mut b, v);
            assert_eq!(Reader { b: &b, i: 0 }.vlq().unwrap(), v);
        }
        // Exemples de la spécification.
        let mut b = Vec::new();
        put_vlq(&mut b, 0x80);
        assert_eq!(b, [0x81, 0x00]);
        assert!(Reader { b: &[0x80, 0x80, 0x80, 0x80, 0x01], i: 0 }.vlq().is_err());
    }
}
