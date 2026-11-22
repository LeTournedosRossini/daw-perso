//! Télécommande d'une tablette de musicien : un retour dont le rôle est « transport » permet, depuis sa page, de lancer / arrêter la lecture, de
//! démarrer l'enregistrement, de revenir au début ou à une section de la forme, de régler le pré-roll en mesures et le métronome. Le serveur
//! (`crate::monitor`) ne fait que transmettre : chaque commande passe par les commandes ordinaires de l'hôte (mêmes garde-fous, mêmes refus,
//! mêmes événements pour la page principale). Les droits sont ceux du **jeton** (rôle choisi par l'utilisateur) ; le serveur refuse (403) toute
//! commande d'un jeton de simple musicien avant même de la lire.

use std::time::Instant;

use daw_core_types::NodeId;

use super::Host;
use crate::monitor::{MarkerKind, FxCmd, FxTarget, MixCmd, RemoteCmd, ROLE_ALL, ROLE_FINE, ROLE_MIXER, ROLE_TRANSPORT};
use crate::prefs::PreRollUnit;
use crate::protocol::{Command, Event, TransportAction};

/// Combien de temps le dernier refus reste affiché sur la tablette.
const MSG_SECONDS: u64 = 8;

/// Nom d'un accord tel qu'on l'affiche aux musiciens (`Dm7`, `G7/B`, `N.C.`).
fn chord_label(c: &daw_session::chords::ChordEv) -> String {
    if c.nc {
        "N.C.".to_owned()
    } else {
        format!("{}{}{}", c.root.name(), c.quality, c.bass.map(|b| format!("/{}", b.name())).unwrap_or_default())
    }
}

impl Host {
    /// Le bloc `remote` de l'état d'une page à rôle « transport » : ce qu'elle affiche (état, mesure, sections, pré-roll, métronome, dernier refus).
    pub(super) fn remote_state(&self) -> serde_json::Value {
        let rate = self.session.sample_rate();
        let pos = self.handle.position();
        let (tm, mm) = (self.session.tempo_map(), self.session.meter_map());
        let bar_of = |f| mm.locate(f).map(|(b, _)| b + 1).unwrap_or(1);
        let bar = tm.to_frac(pos, rate).map(bar_of).unwrap_or(1);
        let sections: Vec<serde_json::Value> = self.session.structure().iter().map(|s| serde_json::json!({ "name": s.name, "bar": bar_of(s.at), "color": s.color })).collect();
        let armed = self.nodes.iter().filter(|n| n.arm).count();
        let msg = self.remote_msg.as_ref().filter(|(t, _)| t.elapsed().as_secs() < MSG_SECONDS).map(|(_, m)| m.clone());
        serde_json::json!({
            "playing": self.playing,
            "recording": self.recording.is_some(),
            "armed": armed,
            "bar": bar,
            "sections": sections,
            "takes": self.takes_state(),
            "pre_roll_bars": if self.metro.pre_roll_unit == PreRollUnit::Bars { self.metro.pre_roll_bars } else { 0 },
            "click": self.metro.enabled,
            "play_pre_roll": self.prefs.transport.play_pre_roll,
            "msg": msg,
        })
    }

    /// Les accords et la section qui passent, pour **toutes** les tablettes de retour (lecture seule, aucun droit particulier) : position en secondes,
    /// tempo et chiffrage au point de lecture, la section en cours et les accords à partir de celui qui sonne (dix au plus). La tablette
    /// prolonge elle-même la position entre deux lectures de l'état. `None` quand le projet n'a ni accord ni section.
    pub(super) fn chart_state(&self) -> Option<serde_json::Value> {
        let track = self.session.chord_track();
        let structure = self.session.structure();
        if track.chords.is_empty() && structure.is_empty() {
            return None;
        }
        let rate = self.session.sample_rate();
        let (tm, mm) = (self.session.tempo_map(), self.session.meter_map());
        let pos = self.handle.position();
        let posf = tm.to_frac(pos, rate)?;
        let secs = |f: daw_core_types::time::Frac| tm.to_samples(f, rate).map(|s| s as f64 / f64::from(rate));
        let (bar0, within) = mm.locate(posf).unwrap_or((0, daw_core_types::time::Frac::ZERO));
        let meter = mm.meter_at_bar(bar0);
        let beat_pos = within.as_f64() / meter.beat_len().as_f64().max(1e-9);
        // L'accord qui sonne, sinon le premier à venir.
        let first = track.chords.iter().position(|c| c.start <= posf && posf < c.start.checked_add(c.len).unwrap_or(c.start)).or_else(|| track.chords.iter().position(|c| c.start > posf));
        let chords: Vec<serde_json::Value> = first
            .map(|i| &track.chords[i..])
            .unwrap_or(&[])
            .iter()
            .take(10)
            .filter_map(|c| {
                let end = c.start.checked_add(c.len)?;
                let name = chord_label(c);
                Some(serde_json::json!({ "name": name, "nc": c.nc, "at": (secs(c.start)? * 1000.0).round() / 1000.0, "end": (secs(end)? * 1000.0).round() / 1000.0 }))
            })
            .collect();
        let section = structure
            .iter()
            .find(|s| s.at <= posf && s.at.checked_add(s.dur).is_some_and(|e| posf < e))
            .and_then(|s| Some(serde_json::json!({ "name": s.name, "color": s.color, "at": secs(s.at)?, "end": secs(s.at.checked_add(s.dur)?)? })));
        let next_section = structure.iter().find(|s| s.at > posf).and_then(|s| Some(serde_json::json!({ "name": s.name, "color": s.color, "at": secs(s.at)? })));
        Some(serde_json::json!({
            "playing": self.playing,
            "pos": (pos as f64 / f64::from(rate) * 1000.0).round() / 1000.0,
            "bpm": tm.tempo_at(posf).as_f64(),
            "num": meter.num(),
            "den": meter.den(),
            "bar": bar0 + 1,
            "beat": (beat_pos * 1000.0).round() / 1000.0,
            "chords": chords,
            "section": section,
            "next_section": next_section,
        }))
    }

    /// La grille du morceau, pour la vue « Grille » des tablettes de retour (comme la page « Structure » du lecteur multipiste de l'école) : une carte
    /// par section de la forme, une case par mesure avec le ou les accords qui y sonnent. Sans structure, une seule carte « Morceau » couvre les
    /// accords. `None` sans accord ni section. `bar` : numéro de la mesure (à partir de 1), `chords` : noms dans l'ordre, sans répétition.
    pub(super) fn grid_state(&self) -> Option<serde_json::Value> {
        use daw_core_types::time::Frac;
        let track = self.session.chord_track();
        let structure = self.session.structure();
        if track.chords.is_empty() && structure.is_empty() {
            return None;
        }
        let rate = self.session.sample_rate();
        let (tm, mm) = (self.session.tempo_map(), self.session.meter_map());
        let secs = |f: Frac| tm.to_samples(f, rate).map(|s| s as f64 / f64::from(rate));
        let bar_of = |f: Frac| mm.locate(f).map(|(b, _)| b);
        // Les zones à afficher : (nom, couleur, catégorie, début, fin).
        let mut zones: Vec<(String, Option<String>, Frac, Frac)> = structure.iter().filter_map(|s| Some((s.name.clone(), s.color.clone(), s.at, s.at.checked_add(s.dur)?))).collect();
        if zones.is_empty() {
            let end = track.chords.iter().filter_map(|c| c.start.checked_add(c.len)).max()?;
            zones.push(("Morceau".into(), None, Frac::ZERO, end));
        }
        let mut sections = Vec::new();
        let mut total = 0usize;
        for (index, (name, color, at, end)) in zones.into_iter().enumerate() {
            let first = bar_of(at)?;
            // La dernière mesure est celle qui contient le dernier instant de la zone (la fin elle-même appartient à la suivante).
            let last_bar = mm.locate(end).map(|(b, w)| if w == Frac::ZERO && b > first { b - 1 } else { b })?.max(first);
            let count = (last_bar - first + 1) as usize;
            total += count;
            if total > 2_000 {
                break;
            }
            let mut cells = Vec::with_capacity(count);
            for b in first..=last_bar {
                let (bs, be) = (mm.bar_start(b)?, mm.bar_start(b + 1)?);
                let mut names: Vec<String> = Vec::new();
                for c in track.chords.iter().filter(|c| c.start < be && c.start.checked_add(c.len).is_some_and(|e| e > bs)) {
                    let n = chord_label(c);
                    if names.last() != Some(&n) {
                        names.push(n);
                    }
                }
                cells.push(names);
            }
            sections.push(serde_json::json!({ "index": index, "name": name, "color": color, "first": first + 1, "count": count, "at": secs(at)?, "cells": cells }));
        }
        Some(serde_json::json!({ "sections": sections }))
    }

    /// Les paroles du morceau en lignes de texte avec leurs accords, pour la vue « Paroles » des tablettes (comme la page Paroles du lecteur
    /// multipiste de l'école). Les syllabes n'ont pas de notion de ligne : une ligne commence à chaque section, toutes les deux mesures à partir du
    /// début de la section, et après une mesure vide au moins. Un mot réunit les syllabes liées par un tiret. Entre les lignes, un passage sans chant
    /// de deux mesures ou plus devient un bloc « instrumental » avec ses accords. Un jeu par couplet. `None` sans paroles.
    pub(super) fn lyrics_state(&self) -> Option<serde_json::Value> {
        use daw_core_types::time::Frac;
        let syl = self.session.lyrics();
        if syl.is_empty() {
            return None;
        }
        let track = self.session.chord_track();
        let structure = self.session.structure();
        let rate = self.session.sample_rate();
        let (tm, mm) = (self.session.tempo_map(), self.session.meter_map());
        let secs = |f: Frac| tm.to_samples(f, rate).map(|s| s as f64 / f64::from(rate)).unwrap_or(0.0);
        let bar_of = |f: Frac| mm.locate(f).map_or(0, |(b, _)| b);
        let chord_at = |f: Frac| track.chords.iter().find(|c| c.start <= f && c.start.checked_add(c.len).is_some_and(|e| f < e)).map(chord_label);
        let chords_in_bars = |from: u32, count: u32| -> Vec<String> {
            let (Some(bs), Some(be)) = (mm.bar_start(from), mm.bar_start(from + count)) else { return Vec::new() };
            let mut names: Vec<String> = Vec::new();
            for c in track.chords.iter().filter(|c| c.start < be && c.start.checked_add(c.len).is_some_and(|e| e > bs)) {
                let n = chord_label(c);
                if names.last() != Some(&n) {
                    names.push(n);
                }
            }
            names
        };
        // La section d'un instant : (indice, première mesure de la section).
        let section_of = |f: Frac| structure.iter().position(|s| s.at <= f && s.at.checked_add(s.dur).is_some_and(|e| f < e)).map(|i| (i, bar_of(structure[i].at)));
        // Dernière mesure qui contient un accord (la fin exacte d'un accord appartient à la mesure suivante).
        let song_end_bar = track.chords.iter().filter_map(|c| c.start.checked_add(c.len)).max().and_then(|e| mm.locate(e).map(|(b, w)| if w == Frac::ZERO { b.saturating_sub(1) } else { b }));
        let mut verses: Vec<u8> = syl.iter().map(|s| s.verse).collect();
        verses.sort_unstable();
        verses.dedup();
        let mut out = Vec::new();
        let mut total_words = 0usize;
        for verse in verses {
            // Les mots du couplet : (début, texte, première mesure).
            let mut words: Vec<(Frac, String)> = Vec::new();
            let mut open: Option<(Frac, String)> = None;
            for s in syl.iter().filter(|s| s.verse == verse) {
                let w = open.get_or_insert((s.at, String::new()));
                w.1.push_str(&s.text);
                if !s.hyphen {
                    words.push(open.take().unwrap());
                }
            }
            words.extend(open);
            total_words += words.len();
            if total_words > 6_000 {
                break;
            }
            // Les lignes.
            let mut lines: Vec<Vec<(Frac, String)>> = Vec::new();
            let mut prev: Option<(Frac, Option<(usize, u32)>)> = None;
            for (at, text) in words {
                let sec = section_of(at);
                let bar = bar_of(at);
                let brk = match prev {
                    None => true,
                    Some((pat, psec)) => {
                        let pbar = bar_of(pat);
                        let key = |sec: Option<(usize, u32)>, b: u32| sec.map_or(b / 2, |(_, first)| b.saturating_sub(first) / 2);
                        sec.map(|x| x.0) != psec.map(|x| x.0) || key(sec, bar) != key(psec, pbar) || bar.saturating_sub(pbar) >= 2
                    }
                };
                if brk {
                    lines.push(Vec::new());
                }
                lines.last_mut().unwrap().push((at, text));
                prev = Some((at, sec));
            }
            let mut items: Vec<serde_json::Value> = Vec::new();
            let mut next_free: u32 = 0; // première mesure pas encore couverte
            let gap = |items: &mut Vec<serde_json::Value>, from: u32, to: u32| {
                if to >= from + 2 {
                    let at = mm.bar_start(from).map_or(0.0, &secs);
                    items.push(serde_json::json!({ "k": "inst", "bar": from + 1, "count": to - from, "at": at, "chords": chords_in_bars(from, to - from) }));
                }
            };
            for line in &lines {
                let first_bar = bar_of(line[0].0);
                let last_bar = bar_of(line[line.len() - 1].0);
                gap(&mut items, next_free, first_bar);
                let section = section_of(line[0].0).map(|(i, _)| structure[i].name.clone());
                let mut last_chord: Option<String> = None;
                let w: Vec<serde_json::Value> = line
                    .iter()
                    .map(|(at, text)| {
                        let c = chord_at(*at);
                        let shown = c.clone().filter(|n| last_chord.as_ref() != Some(n));
                        if c.is_some() {
                            last_chord = c;
                        }
                        serde_json::json!({ "t": text, "at": secs(*at), "c": shown })
                    })
                    .collect();
                items.push(serde_json::json!({ "k": "line", "bar": first_bar + 1, "at": secs(line[0].0), "section": section, "words": w }));
                next_free = last_bar + 1;
            }
            if let Some(end) = song_end_bar {
                gap(&mut items, next_free, end + 1);
            }
            out.push(serde_json::json!({ "verse": verse, "items": items }));
        }
        Some(serde_json::json!({ "verses": out }))
    }

    /// Ce que les tablettes vont chercher à part de l'état, parce que c'est lourd et change rarement : la grille et les paroles.
    pub(super) fn song_state(&self) -> String {
        serde_json::json!({ "grid": self.grid_state(), "lyrics": self.lyrics_state() }).to_string()
    }

    /// Les prises entre lesquelles un musicien peut choisir, pour une tablette à droit de télécommande : par piste, les derniers groupes de
    /// prises (trois au plus), chacun avec les prises qui **recouvrent la plus récente** (la nouvelle prise et ce qu'elle a remplacé ou ses
    /// concurrentes d'un enregistrement en boucle) ; seulement s'il y en a deux ou plus. `None` quand il n'y a rien à choisir.
    pub(super) fn takes_state(&self) -> Option<serde_json::Value> {
        use super::MIDI_CLIP_BASE;
        let g = self.session.graph();
        let rate = f64::from(self.session.sample_rate());
        let (tm, mm) = (self.session.tempo_map(), self.session.meter_map());
        let bar_at = |sample: u64| tm.to_frac(sample, self.session.sample_rate()).and_then(|f| mm.locate(f)).map_or(1, |(b, _)| b + 1);
        let mut tracks = Vec::new();
        for &id in self.session.display_order() {
            let Some(n) = g.nodes.get(id.0 as usize) else { continue };
            if !matches!(n.kind, daw_engine::NodeKind::Track(_)) {
                continue;
            }
            // (identifiant du clip, groupe, début, longueur, muet) des clips en groupe de prises, audio puis notes.
            let mut members: Vec<(u64, u32, u64, u64, bool)> = Vec::new();
            for (k, c) in n.clips.iter().enumerate() {
                if c.take_group != 0 {
                    members.push((self.session.clip_ids(id)[k].0, c.take_group, c.start, c.len, c.muted));
                }
            }
            for (k, mid) in self.session.midi_clip_ids(id).iter().enumerate() {
                let (mc, Some(mclip)) = (&n.midi_clips[k], self.session.midi_clip(*mid)) else { continue };
                if mclip.take_group != 0 {
                    members.push((MIDI_CLIP_BASE + mid.0, mclip.take_group, mc.start, mc.len, mclip.muted));
                }
            }
            let mut group_ids: Vec<u32> = members.iter().map(|m| m.1).collect();
            group_ids.sort_unstable();
            group_ids.dedup();
            let mut groups = Vec::new();
            for gid in group_ids.into_iter().rev().take(3) {
                let all: Vec<_> = members.iter().filter(|m| m.1 == gid).collect();
                let Some(newest) = all.iter().max_by_key(|m| m.0) else { continue };
                let (s, e) = (newest.2, newest.2 + newest.3);
                let mut cands: Vec<_> = all.iter().filter(|m| m.2 < e && s < m.2 + m.3).collect();
                if cands.len() < 2 {
                    continue;
                }
                cands.sort_by_key(|m| m.0);
                let takes: Vec<serde_json::Value> = cands.iter().take(8).enumerate().map(|(rank, m)| serde_json::json!({ "clip": m.0, "label": format!("Prise {}", rank + 1), "active": !m.4, "at": (m.2 as f64 / rate * 1000.0).round() / 1000.0, "len": (m.3 as f64 / rate * 100.0).round() / 100.0 })).collect();
                groups.push(serde_json::json!({ "id": gid, "bar": bar_at(s), "takes": takes }));
            }
            if !groups.is_empty() {
                tracks.push(serde_json::json!({ "node": id.0, "name": n.name, "color": self.session.node_color(id), "groups": groups }));
            }
        }
        (!tracks.is_empty()).then(|| serde_json::Value::Array(tracks))
    }

    /// Choisit la prise (une action annulable, comme depuis la page) puis place la tête au début de cette prise.
    pub(super) fn remote_take(&mut self, clip: u64) -> Vec<Event> {
        let mut events = self.command(Command::SetActiveTake { clip });
        if !events.iter().any(|e| matches!(e, Event::Error { .. })) {
            let start = self.session.graph().nodes.iter().find_map(|n| {
                if clip >= super::MIDI_CLIP_BASE {
                    let mid = daw_session::MidiClipId(clip - super::MIDI_CLIP_BASE);
                    self.session.midi_clip_ids(NodeId(n.id.0)).iter().position(|m| *m == mid).map(|k| n.midi_clips[k].start)
                } else {
                    self.session.clip_ids(NodeId(n.id.0)).iter().position(|c| c.0 == clip).map(|k| n.clips[k].start)
                }
            });
            if let Some(pos) = start {
                events.extend(self.command(Command::Seek { pos }));
            }
        }
        events
    }

    /// Une commande d'une tablette autorisée. Les refus de l'hôte (aucune piste armée, enregistrement en cours…) sont gardés pour la tablette.
    pub(super) fn remote_request(&mut self, ret: u32, cmd: RemoteCmd) -> Vec<Event> {
        // Le rôle est relu ici aussi : il a pu changer entre la requête et ce tick.
        if self.effective_role(ret) & ROLE_TRANSPORT == 0 {
            return Vec::new();
        }
        let events = match cmd {
            RemoteCmd::Toggle => self.command(Command::Transport { action: TransportAction::Toggle }),
            RemoteCmd::Stop => self.command(Command::Transport { action: TransportAction::Stop }),
            RemoteCmd::Start => self.command(Command::Seek { pos: 0 }),
            RemoteCmd::Record => self.command(Command::Record),
            RemoteCmd::Section(i) => {
                let Some(sec) = self.session.structure().get(i as usize) else {
                    self.remote_msg = Some((Instant::now(), "section inconnue".into()));
                    return Vec::new();
                };
                let rate = self.session.sample_rate();
                match self.session.tempo_map().to_samples(sec.at, rate) {
                    Some(pos) => self.command(Command::Seek { pos }),
                    None => Vec::new(),
                }
            }
            RemoteCmd::Bar(n) => {
                let rate = self.session.sample_rate();
                let target = n.checked_sub(1).and_then(|b| self.session.meter_map().bar_start(b)).and_then(|f| self.session.tempo_map().to_samples(f, rate));
                match target {
                    Some(pos) => self.command(Command::Seek { pos }),
                    None => {
                        self.remote_msg = Some((Instant::now(), "mesure inconnue".into()));
                        Vec::new()
                    }
                }
            }
            RemoteCmd::PreRoll(bars) => {
                let m = &self.metro;
                self.command(Command::SetMetronome { enabled: m.enabled, only_recording: m.only_recording, volume: m.volume, count_in_bars: m.count_in_bars, pre_roll: m.pre_roll, post_roll: m.post_roll, pre_roll_bars: bars, pre_roll_unit: PreRollUnit::Bars })
            }
            RemoteCmd::PlayPreRoll(on) => {
                let mut prefs = self.prefs.clone();
                prefs.transport.play_pre_roll = on;
                self.command(Command::SetPrefs { prefs })
            }
            RemoteCmd::Take(clip) => self.remote_take(clip),
            RemoteCmd::Marker(kind) => {
                let (name, color) = match kind {
                    MarkerKind::Good => ("Bonne prise", Some("#2fb457")),
                    MarkerKind::Retake => ("À refaire", Some("#e5484d")),
                    MarkerKind::Plain => ("Marqueur", None),
                };
                self.add_marker(None, Some(name.to_owned()), color.map(str::to_owned))
            }
            RemoteCmd::Click(on) => {
                let m = &self.metro;
                self.command(Command::SetMetronome { enabled: on, only_recording: m.only_recording, volume: m.volume, count_in_bars: m.count_in_bars, pre_roll: m.pre_roll, post_roll: m.post_roll, pre_roll_bars: m.pre_roll_bars, pre_roll_unit: m.pre_roll_unit })
            }
        };
        if let Some(Event::Error { message, .. }) = events.iter().find(|e| matches!(e, Event::Error { .. })) {
            self.remote_msg = Some((Instant::now(), message.clone()));
        } else {
            self.remote_msg = None;
        }
        events
    }

    /// La régie d'une tablette : état de chaque nœud dans l'ordre d'affichage (niveau, panoramique, muet, solo, armement, crête récente).
    pub(super) fn mix_state(&self, fine: bool) -> serde_json::Value {
        let g = self.session.graph();
        // L'ordre d'affichage du projet ; la sortie principale n'en fait pas partie : elle vient en dernier.
        let mut order: Vec<NodeId> = self.session.display_order().to_vec();
        if !order.contains(&NodeId(0)) {
            order.push(NodeId(0));
        }
        let nodes: Vec<serde_json::Value> = order
            .iter()
            .filter_map(|id| {
                let n = g.nodes.get(id.0 as usize)?;
                let st = self.nodes.get(id.0 as usize)?;
                let kind = match n.kind {
                    daw_engine::NodeKind::Master => "master",
                    daw_engine::NodeKind::Bus if n.monitor => "return",
                    daw_engine::NodeKind::Bus => "bus",
                    daw_engine::NodeKind::Track(_) => "track",
                };
                let peak = self.peak_hold.get(&id.0).map_or(0.0, |p| p.0);
                let peak_db = if peak <= 1e-6 { -120.0 } else { (20.0 * f64::from(peak).log10()).max(-120.0) };
                let color = match self.session.node_color(*id) {
                    "" => "#9aa3b2",
                    c => c,
                };
                let group = self.session.node_group(*id);
                Some(serde_json::json!({
                    "id": id.0, "name": n.name, "kind": kind, "color": color, "group": group,
                    "db": (f64::from(daw_core_types::gain_to_db(st.gain)).clamp(-60.0, 6.0) * 10.0).round() / 10.0,
                    "pan": st.pan, "mute": st.mute, "solo": st.solo, "arm": st.arm,
                    "peak": (peak_db * 10.0).round() / 10.0,
                    // Réglages fins : la chaîne de plugins de la piste (un plugin a une page de contrôle s'il en a une dans la banque).
                    "auto": if fine { serde_json::json!({ "gain": self.auto_mode_name(id.0, daw_engine::AutoTarget::Gain), "pan": self.auto_mode_name(id.0, daw_engine::AutoTarget::Pan) }) } else { serde_json::Value::Null },
                    "inserts": if fine { n.inserts.iter().map(|i| serde_json::json!({ "id": i.id, "name": i.name, "bypass": i.bypass, "page": self.insert_has_page(i.id) })).collect::<Vec<_>>() } else { Vec::new() },
                }))
            })
            .collect();
        // Groupes du projet (ceux de la page, pas ceux d'un retour) : état muet / solo des membres et décalage de niveau en cours.
        let groups: Vec<serde_json::Value> = self
            .session
            .effective_group_list(None)
            .iter()
            .map(|gr| {
                let members = self.session.effective_members(gr.id, None);
                let all = |f: &dyn Fn(&super::NodeState) -> bool| !members.is_empty() && members.iter().all(|m| self.nodes.get(m.0 as usize).is_some_and(f));
                let any_solo = members.iter().any(|m| self.nodes.get(m.0 as usize).is_some_and(|s| s.solo));
                serde_json::json!({ "id": gr.id, "name": gr.name, "color": gr.color, "count": members.len(), "mute": all(&|s| s.mute), "solo": any_solo, "offset_db": self.group_offset(gr.id, None) })
            })
            .collect();
        // Talkback : la tablette de régie voit l'état, les retours qu'elle peut viser et si un micro est choisi (il se choisit sur l'ordinateur).
        let returns: Vec<serde_json::Value> = order.iter().filter_map(|id| g.nodes.get(id.0 as usize).filter(|n| n.monitor).map(|n| serde_json::json!({ "id": id.0, "name": n.name }))).collect();
        let talk = serde_json::json!({ "ready": self.talk.channel.is_some(), "active": self.talk.until.is_some(), "targets": self.talk.targets, "returns": returns });
        serde_json::json!({ "nodes": nodes, "groups": groups, "talk": talk })
    }

    /// Le mode d'automation d'une cible : l'armement d'écriture (`touch`, `latch`, `write`) s'il y en a un, sinon le mode de sa courbe (`read`, `trim`, `off`), sinon `none`.
    pub(super) fn auto_mode_name(&self, node: u32, target: daw_engine::AutoTarget) -> &'static str {
        if let Some(a) = self.write_arms.iter().find(|a| a.node == node && a.target == target) {
            return a.mode.name();
        }
        match self.session.lane(NodeId(node), target) {
            Some(l) => super::lane_mode_name(l.mode),
            None => "none",
        }
    }

    fn insert_has_page(&self, insert: u32) -> bool {
        self.meta.plugins.get(&insert).is_some_and(|p| self.bank.get(&p.clap_id).is_some_and(|e| !e.params.is_empty()))
    }

    /// La page de contrôle ouverte par la tablette du retour `ret` : les paramètres choisis du plugin avec leurs valeurs et leurs textes.
    pub(super) fn fine_state(&self, ret: u32) -> serde_json::Value {
        let Some(&insert) = self.remote_open.get(&ret) else { return serde_json::json!({ "page": null }) };
        let Some(Event::InsertPage { configured, params, missing, .. }) = self.insert_page_event(insert).into_iter().next() else { return serde_json::json!({ "page": null }) };
        let owner = self.session.graph().nodes.iter().find(|n| n.inserts.iter().any(|i| i.id == insert));
        let name = owner.and_then(|n| n.inserts.iter().find(|i| i.id == insert)).map(|i| i.name.clone()).unwrap_or_default();
        serde_json::json!({ "page": { "insert": insert, "name": name, "track": owner.map(|n| n.name.clone()), "configured": configured, "missing": missing,
            "node": owner.map(|n| n.id.0),
            "params": params.iter().map(|p| serde_json::json!({ "id": p.id, "name": p.name, "min": p.min, "max": p.max, "default": p.default, "value": p.value, "text": p.text, "auto": owner.map_or("none", |n| self.auto_mode_name(n.id.0, daw_engine::AutoTarget::InsertParam { insert, param: p.id })) })).collect::<Vec<_>>() } })
    }

    /// Un réglage fin d'une tablette : la page d'un plugin s'ouvre, ses paramètres **choisis** se règlent (aucun autre), il se contourne. Le droit est relu ici.
    pub(super) fn fx_request(&mut self, ret: u32, cmd: FxCmd) -> Vec<Event> {
        if self.effective_role(ret) & ROLE_FINE == 0 {
            return Vec::new();
        }
        let refuse = |this: &mut Self, m: &str| {
            this.remote_msg = Some((Instant::now(), m.into()));
            Vec::new()
        };
        let in_page = |this: &Self, insert: u32, param: u32| matches!(this.insert_page_event(insert).into_iter().next(), Some(Event::InsertPage { params, .. }) if params.iter().any(|p| p.id == param));
        match cmd {
            FxCmd::Open(None) => {
                self.remote_open.remove(&ret);
                Vec::new()
            }
            FxCmd::Open(Some(insert)) => {
                if self.plugin_handle(insert).is_none() {
                    return refuse(self, "plugin inconnu");
                }
                self.remote_open.insert(ret, insert);
                self.remote_msg = None;
                Vec::new()
            }
            FxCmd::Param { insert, param, value } => {
                if !in_page(self, insert, param) {
                    return refuse(self, "ce paramètre n'est pas dans la page de contrôle du plugin");
                }
                // Borne à la plage du paramètre.
                let ph = self.plugin_handle(insert).expect("plugin déjà vérifié par la page");
                let value = ph.params().iter().find(|p| p.id == param).map_or(value, |p| value.clamp(p.min.min(p.max), p.max.max(p.min)));
                self.remote_msg = None;
                self.fine_hot = true;
                self.command(Command::SetPluginParam { insert, param, value })
            }
            FxCmd::Touch { insert, param, active } => {
                if !in_page(self, insert, param) {
                    return refuse(self, "ce paramètre n'est pas dans la page de contrôle du plugin");
                }
                let Some(node) = self.session.graph().nodes.iter().position(|n| n.inserts.iter().any(|i| i.id == insert)) else { return refuse(self, "plugin inconnu") };
                self.gesture_auto(node as u32, daw_engine::AutoTarget::InsertParam { insert, param }, active)
            }
            FxCmd::TouchTrack { node, target, active } => {
                if self.session.graph().nodes.get(node as usize).is_none() {
                    return refuse(self, "piste inconnue");
                }
                let t = match target {
                    FxTarget::Gain => daw_engine::AutoTarget::Gain,
                    _ => daw_engine::AutoTarget::Pan,
                };
                self.gesture_auto(node, t, active)
            }
            FxCmd::Auto { node, target, mode } => {
                if self.session.graph().nodes.get(node as usize).is_none() {
                    return refuse(self, "piste inconnue");
                }
                let lane = match target {
                    FxTarget::Gain => crate::protocol::LaneTarget::Gain,
                    FxTarget::Pan => crate::protocol::LaneTarget::Pan,
                    FxTarget::Insert { insert, param } => {
                        // Seulement un paramètre de la page de contrôle, d'un plugin de cette piste.
                        let on_node = self.session.graph().nodes[node as usize].inserts.iter().any(|i| i.id == insert);
                        if !on_node || !in_page(self, insert, param) {
                            return refuse(self, "ce paramètre n'est pas dans la page de contrôle d'un plugin de cette piste");
                        }
                        crate::protocol::LaneTarget::Insert { insert, param }
                    }
                };
                self.remote_msg = None;
                let mut events = match mode.as_str() {
                    "touch" | "latch" | "write" | "none" => return self.command(Command::SetWriteMode { node, target: lane, mode }),
                    // Les modes de la courbe : on désarme d'abord l'écriture (sinon elle remet la courbe en lecture).
                    _ => self.command(Command::SetWriteMode { node, target: lane, mode: "none".into() }),
                };
                let more = self.command(Command::SetLaneMode { node, target: lane, mode });
                if let Some(Event::Error { message, .. }) = more.iter().find(|e| matches!(e, Event::Error { .. })) {
                    self.remote_msg = Some((Instant::now(), message.clone()));
                }
                events.extend(more);
                events
            }
            FxCmd::Bypass { insert, on } => {
                if self.plugin_handle(insert).is_none() {
                    return refuse(self, "plugin inconnu");
                }
                self.remote_msg = None;
                self.command(Command::SetPluginBypass { insert, bypass: on })
            }
        }
    }

    /// Un réglage de groupe de la régie : passe par les mêmes fonctions que la page (écarts de niveau conservés). Le droit est relu ici aussi.
    pub(super) fn mix_group_request(&mut self, ret: u32, group: u32, mute: Option<bool>, solo: Option<bool>, offset_db: Option<f64>) -> Vec<Event> {
        if self.effective_role(ret) & ROLE_MIXER == 0 {
            return Vec::new();
        }
        let mut events = Vec::new();
        if mute.is_some() || solo.is_some() {
            events.extend(self.set_group(group, mute, solo, None, None));
        }
        if let Some(db) = offset_db {
            events.extend(self.set_group_offset(group, db, None));
        }
        match events.iter().find(|e| matches!(e, Event::Error { .. })) {
            Some(Event::Error { message, .. }) => self.remote_msg = Some((Instant::now(), message.clone())),
            _ => self.remote_msg = None,
        }
        events
    }

    /// Un réglage de régie, avec le droit relu à l'exécution. Passe par `set_node` (mêmes refus que la page : armement d'un bus, pendant une prise…).
    pub(super) fn mix_request(&mut self, ret: u32, cmd: MixCmd) -> Vec<Event> {
        if self.effective_role(ret) & ROLE_MIXER == 0 {
            return Vec::new();
        }
        let Some(n) = self.session.graph().nodes.get(cmd.node as usize) else {
            self.remote_msg = Some((Instant::now(), "piste inconnue".into()));
            return Vec::new();
        };
        let master = cmd.node == 0;
        let is_track = matches!(n.kind, daw_engine::NodeKind::Track(_));
        if (master && (cmd.pan.is_some() || cmd.solo.is_some() || cmd.arm.is_some())) || (cmd.arm.is_some() && !is_track) {
            self.remote_msg = Some((Instant::now(), "ce réglage n'existe pas pour ce nœud".into()));
            return Vec::new();
        }
        let gain = cmd.db.map(|db| if db <= -60.0 { 0.0 } else { daw_core_types::db_to_gain(db.clamp(-60.0, 6.0) as f32) });
        let pan = cmd.pan.map(|p| p.clamp(-1.0, 1.0) as f32);
        let events = self.command(Command::SetNode { node: cmd.node, gain, pan, mute: cmd.mute, solo: cmd.solo, arm: cmd.arm, monitor: None, invert: None });
        match events.iter().find(|e| matches!(e, Event::Error { .. })) {
            Some(Event::Error { message, .. }) => self.remote_msg = Some((Instant::now(), message.clone())),
            _ => self.remote_msg = None,
        }
        events
    }

    /// Change le rôle de la télécommande d'un retour (0 : son retour seulement ; 1 : aussi le transport). Sans historique, compte comme modification.
    pub(super) fn set_monitor_role(&mut self, node: u32, role: u8) -> Vec<Event> {
        if !self.session.graph().nodes.get(node as usize).is_some_and(|n| n.monitor) {
            return vec![Event::Error { command: "set_monitor_role".into(), message: "seul un retour de monitoring a un rôle de télécommande".into() }];
        }
        if !self.session.set_monitor_role_untracked(NodeId(node), role) {
            return vec![Event::Error { command: "set_monitor_role".into(), message: format!("rôle inconnu (0 à {ROLE_ALL} : 1 = transport, 2 = régie, 4 = réglages fins, additionnés)") }];
        }
        self.dirty = true;
        self.monitor_grants.insert(node, (role, Instant::now())); // redonner les mêmes droits les renouvelle
        if self.monitor.is_some() {
            self.monitor_publish();
        }
        vec![self.monitor_event(None)]
    }

    /// Arrêt automatique (option des réglages) : arrête le transport quand la lecture ou la prise a dépassé la fin du projet de trop longtemps.
    /// Une prise s'arrête comme à un arrêt normal (fichier fermé, clips posés). Ne s'applique ni en boucle, ni pendant un aperçu ou une écoute.
    pub(super) fn auto_stop_tick(&mut self) -> Vec<Event> {
        let a = self.prefs.auto_stop;
        if !a.enabled || !self.playing || self.loop_range.is_some() || self.preview.is_some() || self.listen.is_some() {
            return Vec::new();
        }
        let rate = u64::from(self.session.sample_rate());
        let end = daw_offline_render::export::project_end(self.session.graph());
        // Pendant une prise, la référence est la fin du dernier élément ou, s'il n'y a rien derrière, le début de la prise.
        let (base, secs) = match &self.recording {
            Some(r) => (end.max(r.start), a.recording_secs),
            None => (end, a.after_end_secs),
        };
        if self.handle.position() < base + u64::from(secs) * rate {
            return Vec::new();
        }
        let message = if self.recording.is_some() { format!("Arrêt automatique : {} min sans rien derrière pendant l'enregistrement.", secs.div_ceil(60)) } else { format!("Arrêt automatique : {} s après la fin du projet.", secs) };
        self.remote_msg = Some((Instant::now(), message.clone()));
        let mut events = self.command(Command::Transport { action: TransportAction::Stop });
        events.push(Event::Error { command: "auto_stop".into(), message });
        events
    }
}

#[cfg(test)]
mod take_tests {

    use daw_core_types::NodeId;
    use daw_engine::{AudioData, Clip, Engine};

    use crate::demo::demo_session;
    use crate::{Event, Host};

    const SR: u32 = 48_000;

    fn take(frames: usize, start: u64) -> Clip {
        let data = AudioData::new(SR, vec![vec![0.1; frames]]).unwrap();
        Clip::new(data, start, 0, frames as u64, 1.0)
    }

    #[test]
    fn the_tablet_lists_the_takes_over_the_newest_one_and_choosing_one_activates_it_and_moves_the_head_there() {
        let (engine, handle) = Engine::new(SR, 64);
        let (session, meta) = demo_session(SR);
        std::mem::forget(engine);
        let mut h = Host::new(handle, session, meta).unwrap();
        assert!(h.takes_state().is_none(), "rien à choisir au départ");

        // Une piste de la démonstration : trois prises successives sur la même zone (chacune recouvre la précédente).
        let node = h.session.display_order().iter().copied().find(|id| h.session.graph().nodes[id.0 as usize].name == "Voix").expect("piste Voix");
        let zone = 480_000u64;
        let a = h.session.record_over(node, take(20_000, zone)).unwrap();
        let b = h.session.record_over(node, take(20_000, zone)).unwrap();
        let c = h.session.record_over(node, take(20_000, zone)).unwrap();
        let takes = h.takes_state().expect("trois prises sur la même zone");
        let t = &takes.as_array().unwrap()[0];
        assert_eq!(t["name"], "Voix");
        let g = &t["groups"].as_array().unwrap()[0];
        let list = g["takes"].as_array().unwrap();
        assert_eq!(list.len(), 3, "{takes}");
        assert_eq!(list.iter().map(|x| x["label"].as_str().unwrap()).collect::<Vec<_>>(), ["Prise 1", "Prise 2", "Prise 3"]);
        let active: Vec<bool> = list.iter().map(|x| x["active"].as_bool().unwrap()).collect();
        assert_eq!(active, [false, false, true], "la dernière prise est la prise active");
        assert_eq!(list[0]["clip"], a.0);
        assert_eq!(list[2]["clip"], c.0);
        assert_eq!(list[0]["at"], 10.0, "480 000 échantillons à 48 kHz = 10 s");
        assert_eq!(g["bar"], 6, "10 s à 120 bpm = la mesure 6");

        // Choisir la première : elle devient active, la tête se place à son début, un seul pas d'annulation.
        let ev = h.remote_take(a.0);
        assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
        assert!(ev.iter().any(|e| matches!(e, Event::Position { pos: 480_000, .. })), "la tête est au début de la prise (10 s) : {ev:?}");
        let list = h.takes_state().unwrap()[0]["groups"][0]["takes"].clone();
        let active: Vec<bool> = list.as_array().unwrap().iter().map(|x| x["active"].as_bool().unwrap()).collect();
        assert_eq!(active, [true, false, false]);
        let _ = b;
        assert!(h.session.can_undo());
        // Un clip qui n'est pas une prise : refusé, sans déplacement.
        let ev = h.remote_take(987_654);
        assert!(ev.iter().any(|e| matches!(e, Event::Error { .. })));
    }
}
