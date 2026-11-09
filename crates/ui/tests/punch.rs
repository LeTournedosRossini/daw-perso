//! Punch-in / punch-out : la zone est la bande bleue de la règle (la même que la boucle) ; avec Punch allumé `record` enregistre exactement
//! de son début à sa fin, avec pré-roll, la prise se ferme toute seule, la lecture continue et la boucle est rétablie ; en direct avant ou
//! dans la zone ; refus ; le réglage est enregistré avec le projet.

use daw_engine::{Engine, MidiMsg};
use daw_ui::demo::demo_session;
use daw_ui::protocol::{ClipInfo, NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;
const COMP: u64 = 100;

/// Dix canaux d'entrée : le canal `c` vaut `(c + 1) / 100 + j / 1_000_000` à l'échantillon absolu `j` (reconnaissable).
fn input(c: usize, j: usize) -> f32 {
    (c + 1) as f32 / 100.0 + j as f32 / 1_000_000.0
}

struct Rig {
    engine: Engine,
    host: Host,
    dir: std::path::PathBuf,
    fed: usize,
}

fn rig(name: &str) -> Rig {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut host = Host::new(handle, session, meta).unwrap();
    host.set_audio_inputs(10);
    host.set_record_compensation(COMP);
    let dir = std::env::temp_dir().join(format!("daw-rec-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Rig { engine, host, dir, fed: 0 }
}

impl Rig {
    fn cmd(&mut self, json: &str) -> Vec<Event> {
        self.host.command_json(json)
    }

    fn save(&mut self) {
        let ev = self.cmd(&format!(r#"{{"type":"save_project","path":{:?}}}"#, self.dir.join("P").to_string_lossy()));
        assert!(ev.iter().any(|e| matches!(e, Event::File { dirty: false, .. })), "{ev:?}");
    }

    /// Fait tourner le moteur sur `frames` échantillons d'entrée (blocs de 64), en continuant la numérotation des échantillons.
    fn feed(&mut self, frames: usize) {
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        let mut done = 0;
        while done < frames {
            let n = 64.min(frames - done);
            let data: Vec<Vec<f32>> = (0..10).map(|c| (0..n).map(|k| input(c, self.fed + k)).collect()).collect();
            let refs: Vec<&[f32]> = data.iter().map(Vec::as_slice).collect();
            self.engine.process_inputs(&refs, &mut l[..n], &mut r[..n]);
            self.fed += n;
            done += n;
        }
    }

    /// Fait avancer moteur et hôte jusqu'à la fin de l'enregistrement ; rend les événements reçus.
    fn settle(&mut self) -> Vec<Event> {
        let mut all = Vec::new();
        for _ in 0..400 {
            self.feed(64);
            let ev = self.host.tick();
            let done = ev.iter().any(|e| matches!(e, Event::Recording { active: false, .. }));
            all.extend(ev);
            if done {
                return all;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("l'enregistrement ne s'est pas terminé");
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(ev: &'a [Event], name: &str) -> &'a NodeInfo {
    project(ev).nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("pas de piste « {name} »"))
}

fn error(ev: &[Event]) -> String {
    ev.iter().find_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).expect("une erreur")
}

fn undo_label(ev: &[Event]) -> Option<String> {
    ev.iter().rev().find_map(|e| if let Event::History { undo_label, .. } = e { Some(undo_label.clone()) } else { None }).flatten()
}

fn audio_clip(c: &ClipInfo) -> (String, u64, u64, u64) {
    match c {
        ClipInfo::Audio { name, start, len, offset, .. } => (name.clone(), *start, *len, *offset),
        other => panic!("{other:?}"),
    }
}

fn read_f32_wav(path: &std::path::Path) -> (u16, Vec<f32>) {
    let b = std::fs::read(path).unwrap();
    let channels = u16::from_le_bytes([b[22], b[23]]);
    (channels, b[44..].chunks_exact(4).map(|x| f32::from_le_bytes([x[0], x[1], x[2], x[3]])).collect())
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

/// Projet enregistré avec « Piste 1 » (mono) et « Basse » (mono) ; retourne leurs numéros.
fn two_tracks(rig: &mut Rig) -> (u32, u32) {
    rig.cmd(r#"{"type":"new_project"}"#);
    rig.cmd(r#"{"type":"add_track","kind":"mono","name":"Basse"}"#);
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let ids = (node(&ev, "Piste 1").id, node(&ev, "Basse").id);
    rig.save();
    ids
}


fn punch_on(ev: &[Event]) -> bool {
    ev.iter().rev().find_map(|e| if let Event::Punch { enabled } = e { Some(*enabled) } else { None }).expect("événement punch")
}

fn finished(done: &[Event]) -> (Vec<String>, u32) {
    done.iter().find_map(|e| if let Event::Recording { active: false, warnings, overruns, .. } = e { Some((warnings.clone(), *overruns)) } else { None }).expect("fin d'enregistrement")
}

fn last_position(ev: &[Event]) -> Option<(u64, bool, bool)> {
    ev.iter().rev().find_map(|e| if let Event::Position { pos, playing, loop_range } = e { Some((*pos, *playing, loop_range.is_some())) } else { None })
}

/// Pose la zone (bande bleue = boucle) et allume le punch.
fn zone(rig: &mut Rig, start: u64, end: u64) {
    rig.cmd(&format!(r#"{{"type":"loop","start":{start},"end":{end}}}"#));
    assert!(punch_on(&rig.cmd(r#"{"type":"set_punch","enabled":true}"#)));
}

fn arm_audio(rig: &mut Rig, track: u32) {
    rig.cmd(&format!(r#"{{"type":"set_track_input","node":{track},"input":{{"kind":"audio","channels":[3]}}}}"#));
    rig.cmd(&format!(r#"{{"type":"set_node","node":{track},"arm":true}}"#));
}

#[test]
fn a_zone_with_punch_records_exactly_its_range_after_the_pre_roll_and_ends_by_itself_while_playback_continues() {
    let mut rig = rig("zone");
    let (t1, _t2) = two_tracks(&mut rig);
    arm_audio(&mut rig, t1);
    zone(&mut rig, 30_000, 34_000);
    rig.cmd(r#"{"type":"set_metronome","enabled":false,"only_recording":false,"volume":0.8,"count_in_bars":0,"pre_roll":0.1,"post_roll":0}"#);
    rig.feed(64);
    let j_play = rig.fed; // premier échantillon d'entrée traité une fois la lecture lancée
    let start = rig.cmd(r#"{"type":"record"}"#);
    let from = start.iter().find_map(|e| if let Event::Recording { active: true, start, .. } = e { Some(*start) } else { None }).expect("événement recording");
    assert_eq!(from, 30_000, "le conteneur de prise commence au début de la zone");
    let done = rig.settle(); // aucun ordre d'arrêt : la prise se ferme toute seule à 34 000
    assert_eq!(finished(&done), (Vec::new(), 0));
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let a = audio_clip(&node(&ev, "Piste 1").clips[0]);
    // La lecture a démarré 4 800 échantillons (0,1 s) avant la zone : l'échantillon de position 30 000 est le 4 800ᵉ après le départ.
    assert_eq!((a.1, a.2, a.3), (30_000 - COMP, 4_000, 0), "{a:?}");
    let (ch, wav) = read_f32_wav(&rig.dir.join("P.daw/audio/Piste-1-prise-001.wav"));
    assert_eq!((ch, wav.len()), (1, 4_000));
    for k in [0usize, 1, 63, 64, 1_999, 3_998, 3_999] {
        assert!(approx(wav[k], input(2, j_play + 4_800 + k)), "échantillon {k} : {} contre {}", wav[k], input(2, j_play + 4_800 + k));
    }
    assert_eq!(undo_label(&ev).as_deref(), Some("Enregistrer"));
    // La lecture continue après le punch-out et la zone est toujours là (la boucle est rétablie : elle ne joue qu'en revenant dans la zone).
    rig.feed(640);
    let (pos, playing, looped) = last_position(&rig.host.tick()).expect("position");
    assert!(playing && looped, "la lecture continue, la zone est toujours là");
    assert!(pos > 34_000, "la lecture a continué après la fin de la zone ({pos})");
}

#[test]
fn a_pre_roll_in_bars_starts_playback_at_the_bar_that_many_bars_before_the_zone() {
    // 120 par minute en 4/4 à 48 kHz : une mesure = 96 000 échantillons. Zone à la mesure 4 (début à 288 000) avec 2 mesures de pré-roll :
    // la lecture part du début de la mesure 2 (96 000), on le voit à la première prise : le premier échantillon d'entrée traité est à 96 000.
    let mut rig = rig("mesures");
    let (t1, _) = two_tracks(&mut rig);
    arm_audio(&mut rig, t1);
    zone(&mut rig, 288_000, 292_000);
    rig.cmd(r#"{"type":"set_metronome","enabled":false,"only_recording":false,"volume":0.8,"count_in_bars":0,"pre_roll":0,"post_roll":0,"pre_roll_bars":2,"pre_roll_unit":"bars"}"#);
    rig.feed(64);
    let j_play = rig.fed;
    rig.cmd(r#"{"type":"record"}"#);
    rig.feed(190_000); // les deux mesures de pré-roll, moins un peu
    let done = rig.settle();
    assert_eq!(finished(&done), (Vec::new(), 0));
    let (_, wav) = read_f32_wav(&rig.dir.join("P.daw/audio/Piste-1-prise-001.wav"));
    assert_eq!(wav.len(), 4_000);
    // L'échantillon de position 288 000 est le (288 000 − 96 000)ᵉ après le départ de la lecture.
    assert!(approx(wav[0], input(2, j_play + 192_000)), "{} contre {}", wav[0], input(2, j_play + 192_000));
}

#[test]
fn recording_while_playing_before_the_zone_waits_for_it_and_inside_the_zone_starts_now() {
    // Avant la zone : la prise attend le début de la zone.
    let mut rig = rig("direct-avant");
    let (t1, _) = two_tracks(&mut rig);
    arm_audio(&mut rig, t1);
    zone(&mut rig, 5_000, 7_000);
    rig.cmd(r#"{"type":"transport","action":"play"}"#);
    rig.feed(64);
    let j0 = rig.fed - 64; // l'échantillon de position 0 est l'indice j0 du flux
    rig.feed(936); // tête à 1 000
    rig.cmd(r#"{"type":"record"}"#);
    let done = rig.settle();
    assert_eq!(finished(&done), (Vec::new(), 0));
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let a = audio_clip(&node(&ev, "Piste 1").clips[0]);
    assert_eq!((a.1, a.2), (5_000 - COMP, 2_000), "{a:?}");
    let (_, wav) = read_f32_wav(&rig.dir.join("P.daw/audio/Piste-1-prise-001.wav"));
    assert!(approx(wav[0], input(2, j0 + 5_000)) && approx(wav[1_999], input(2, j0 + 6_999)), "premier et dernier échantillon de la zone");

    // Dans la zone : elle commence tout de suite et finit à la fin de la zone.
    let mut rig = self::rig("direct-dedans");
    let (t1, _) = two_tracks(&mut rig);
    arm_audio(&mut rig, t1);
    zone(&mut rig, 1_000, 3_000);
    rig.cmd(r#"{"type":"transport","action":"play"}"#);
    rig.feed(1_500);
    rig.cmd(r#"{"type":"record"}"#);
    let done = rig.settle();
    assert_eq!(finished(&done), (Vec::new(), 0));
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let a = audio_clip(&node(&ev, "Piste 1").clips[0]);
    assert_eq!(a.1 + COMP + a.2, 3_000, "la prise finit exactement à la fin de la zone : {a:?}");
    assert!(a.2 > 1_400 && a.2 <= 1_500, "elle a commencé à la tête de lecture : {a:?}");

    // Après la zone : refusé avec un message clair.
    let mut rig = self::rig("direct-apres");
    let (t1, _) = two_tracks(&mut rig);
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t1},"arm":true}}"#));
    zone(&mut rig, 1_000, 2_000);
    rig.cmd(r#"{"type":"seek","pos":3000}"#);
    rig.cmd(r#"{"type":"transport","action":"play"}"#);
    rig.feed(64);
    assert!(error(&rig.cmd(r#"{"type":"record"}"#)).contains("après la fin de la zone de punch"));
}

#[test]
fn a_midi_take_in_a_zone_keeps_only_what_is_inside_and_closes_held_notes_at_the_end() {
    let mut rig = rig("midi");
    let (_t1, t2) = two_tracks(&mut rig);
    rig.cmd(&format!(r#"{{"type":"set_track_input","node":{t2},"input":{{"kind":"midi"}}}}"#));
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t2},"arm":true}}"#));
    zone(&mut rig, 2_000, 4_000);
    let mut midi = rig.host.take_midi_input().expect("entrée MIDI");
    rig.feed(64);
    rig.cmd(r#"{"type":"record"}"#);
    rig.feed(512);
    midi.push(t2, MidiMsg::NoteOn { channel: 0, key: 60, velocity: 0.8 }, 0);
    rig.feed(2_560); // la fin (4 000) est dépassée
    midi.push(t2, MidiMsg::NoteOff { channel: 0, key: 60, velocity: 0.0 }, 0); // après la fin : ignorée
    midi.push(t2, MidiMsg::NoteOn { channel: 0, key: 62, velocity: 0.8 }, 0); // après la fin : ignorée
    let done = rig.settle();
    assert_eq!(finished(&done), (Vec::new(), 0));
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let clip = node(&ev, "Basse").clips.iter().find_map(|c| if let ClipInfo::Midi { notes, start, .. } = c { Some((notes.clone(), *start)) } else { None }).expect("clip de notes");
    assert_eq!(clip.0.len(), 1, "seule la note jouée dans la zone est gardée");
    assert_eq!(clip.0[0].key, 60);
    assert!(clip.1 >= 2_000 - COMP - 64 && clip.1 <= 2_000 - COMP + 64, "début du clip : {}", clip.1);
}

#[test]
fn the_punch_setting_is_saved_with_the_project_refused_while_recording_and_needs_a_zone() {
    let mut rig = rig("fichier");
    let (t1, _) = two_tracks(&mut rig);
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t1},"arm":true}}"#));
    assert!(rig.cmd(r#"{"type":"set_punch"}"#).iter().any(|e| matches!(e, Event::Error { .. })), "enabled est obligatoire");
    assert!(punch_on(&rig.cmd(r#"{"type":"set_punch","enabled":true}"#)));
    // Punch allumé sans zone : refus clair ; une boucle sans punch : refus qui propose le punch.
    assert!(error(&rig.cmd(r#"{"type":"record"}"#)).contains("pose d'abord une zone"));
    rig.cmd(r#"{"type":"set_punch","enabled":false}"#);
    rig.cmd(r#"{"type":"loop","start":12000,"end":24000}"#);
    assert!(error(&rig.cmd(r#"{"type":"record"}"#)).contains("allume Punch"));
    rig.cmd(r#"{"type":"set_punch","enabled":true}"#);
    // Enregistré avec le projet, retrouvé à l'ouverture ; « Nouveau » l'éteint.
    rig.save();
    rig.cmd(r#"{"type":"new_project"}"#);
    assert!(!punch_on(&rig.cmd(r#"{"type":"get_project"}"#)), "un nouveau projet n'a pas le punch allumé");
    let ev = rig.cmd(&format!(r#"{{"type":"open_project","path":{:?}}}"#, rig.dir.join("P.daw").to_string_lossy()));
    assert!(punch_on(&ev));
    // Refusé pendant une prise.
    rig.cmd(r#"{"type":"set_punch","enabled":false}"#);
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t1},"arm":true}}"#));
    rig.feed(64);
    rig.cmd(r#"{"type":"record"}"#);
    assert!(error(&rig.cmd(r#"{"type":"set_punch","enabled":true}"#)).contains("enregistrement"));
    rig.cmd(r#"{"type":"stop_record"}"#);
    rig.settle();
}

fn take_flags(ev: &[Event]) -> Vec<(u64, u64, bool, u32)> {
    node(ev, "Piste 1")
        .clips
        .iter()
        .map(|c| match c {
            ClipInfo::Audio { start, len, muted, take_group, .. } => (*start, *len, *muted, *take_group),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn punch_take(rig: &mut Rig, t: u32, start: u64, end: u64) {
    rig.cmd(&format!(r#"{{"type":"loop","start":{start},"end":{end}}}"#));
    rig.cmd(r#"{"type":"set_metronome","enabled":false,"only_recording":false,"volume":0.8,"count_in_bars":0,"pre_roll":0,"post_roll":0}"#);
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t},"arm":true}}"#));
    rig.feed(64);
    let ev = rig.cmd(r#"{"type":"record"}"#);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    rig.settle();
}

#[test]
fn a_punch_over_an_existing_take_keeps_the_old_passage_as_a_switchable_inactive_take() {
    let mut rig = rig("composite");
    let (t1, _) = two_tracks(&mut rig);
    rig.cmd(&format!(r#"{{"type":"set_track_input","node":{t1},"input":{{"kind":"audio","channels":[3]}}}}"#));
    assert!(punch_on(&rig.cmd(r#"{"type":"set_punch","enabled":true}"#)));
    punch_take(&mut rig, t1, 30_000, 34_000);
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    assert_eq!(take_flags(&ev), vec![(30_000 - COMP, 4_000, false, 0)], "rien dessous : pas de groupe");
    rig.cmd(r#"{"type":"stop"}"#);
    rig.cmd(r#"{"type":"seek","pos":30500}"#);
    punch_take(&mut rig, t1, 31_000, 32_000);
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let flags = take_flags(&ev);
    assert_eq!(flags.len(), 4, "{flags:?}");
    let g = flags.iter().find(|f| f.3 != 0).unwrap().3;
    assert_eq!(flags.iter().filter(|f| f.3 == g).count(), 2, "ancien passage (muet) + nouvelle prise");
    assert_eq!(flags.iter().filter(|f| f.3 == g && !f.2).count(), 1, "une seule active");
    assert_eq!(undo_label(&ev).as_deref(), Some("Enregistrer"));
    // Changer de prise : l'ancien passage devient actif, une action annulable.
    let old = match node(&ev, "Piste 1").clips.iter().find(|c| matches!(c, ClipInfo::Audio { muted: true, .. })).unwrap() {
        ClipInfo::Audio { id, .. } => *id,
        _ => unreachable!(),
    };
    let ev = rig.cmd(&format!(r#"{{"type":"set_active_take","clip":{old}}}"#));
    let after = take_flags(&ev);
    assert_eq!(after.iter().filter(|f| f.3 == g && f.2).count(), 1);
    assert_eq!(undo_label(&ev).as_deref(), Some("Changer de prise"));
    let ev = rig.cmd(r#"{"type":"undo"}"#);
    assert_eq!(take_flags(&ev), flags);
    assert!(error(&rig.cmd(r#"{"type":"set_active_take","clip":999999}"#)).contains("inconnu"));
}

#[test]
fn recording_in_a_loop_makes_one_take_per_pass_all_in_one_group_with_the_last_pass_active() {
    let mut rig = rig("loop-takes");
    let (t1, _) = two_tracks(&mut rig);
    rig.cmd(r#"{"type":"set_auto_crossfade","enabled":false}"#); // bornes exactes ; les raccords ont leur propre test
    arm_audio(&mut rig, t1);
    rig.cmd(r#"{"type":"loop","start":30000,"end":31000}"#);
    // Sans l'option, une boucle empêche d'enregistrer ; avec elle, la boucle continue de tourner.
    assert!(error(&rig.cmd(r#"{"type":"record"}"#)).contains("boucle"));
    let ev = rig.cmd(r#"{"type":"set_loop_record","enabled":true}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::LoopRecord { enabled: true })) && punch_on(&ev) == false);
    rig.cmd(r#"{"type":"set_metronome","enabled":false,"only_recording":false,"volume":0.8,"count_in_bars":0,"pre_roll":0,"post_roll":0}"#);
    rig.feed(64);
    let j0 = rig.fed;
    let ev = rig.cmd(r#"{"type":"record"}"#);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    rig.feed(3_700);
    // La boucle tourne toujours : la tête est restée dans la zone (sans boucle elle serait à 33 700).
    let pos = last_position(&rig.host.tick()).expect("une position").0;
    assert!((30_000..31_000).contains(&pos), "position {pos}");
    rig.cmd(r#"{"type":"stop_record"}"#);
    rig.settle();
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let group: Vec<(u64, u64, u64, bool)> = node(&ev, "Piste 1")
        .clips
        .iter()
        .filter_map(|c| match c {
            ClipInfo::Audio { start, len, offset, muted, take_group, .. } if *take_group != 0 => Some((*start, *len, *offset, *muted)),
            _ => None,
        })
        .collect();
    // Les passages sont rognés à la zone (le premier ne laisse pas dépasser avant elle la latence compensée : ce petit bout resterait
    // seul et actif à chaque changement de prise). Chaque passage recouvre le précédent : passages 0 et 1 muets, début du 2 muet
    // (recouvert par le dernier), fin du 2 restée active, dernier passage (interrompu après 600 échantillons) actif.
    assert_eq!(group.len(), 5, "{group:?}");
    let by_offset = |o: u64| group.iter().find(|g| g.2 == o).unwrap_or_else(|| panic!("pas de morceau à l'offset {o} : {group:?}"));
    assert_eq!((by_offset(100).0, by_offset(100).1, by_offset(100).3), (30_000, 1_000, true), "passage 0, muet, commence à la zone");
    assert_eq!((by_offset(1_100).0, by_offset(1_100).1, by_offset(1_100).3), (30_000, 1_000, true), "passage 1, muet");
    assert_eq!((by_offset(2_100).1, by_offset(2_100).3), (600, true), "début du passage 2, recouvert");
    assert_eq!((by_offset(2_700).0, by_offset(2_700).1, by_offset(2_700).3), (30_600, 400, false), "fin du passage 2, restée active");
    assert_eq!((by_offset(3_100).0, by_offset(3_100).1, by_offset(3_100).3), (30_000, 600, false), "dernier passage, actif");
    assert!(node(&ev, "Piste 1").clips.iter().all(|c| matches!(c, ClipInfo::Audio { take_group: 1, .. })), "aucun petit bout hors groupe");
    assert_eq!(undo_label(&ev).as_deref(), Some("Enregistrer"));
    // Le contenu du deuxième passage est bien ce qui est entré pendant ce passage.
    let (_, wav) = read_f32_wav(&rig.dir.join("P.daw/audio/Piste-1-prise-001.wav"));
    assert!(approx(wav[1_100], input(2, j0 + 1_100)) && approx(wav[2_100], input(2, j0 + 2_100)));
}

#[test]
fn composing_a_range_of_an_inactive_take_through_the_protocol_is_one_undo_step() {
    let mut rig = rig("compose");
    let (t1, _) = two_tracks(&mut rig);
    rig.cmd(&format!(r#"{{"type":"set_track_input","node":{t1},"input":{{"kind":"audio","channels":[3]}}}}"#));
    rig.cmd(r#"{"type":"set_auto_crossfade","enabled":false}"#); // bornes exactes ; les raccords ont leur propre test
    rig.cmd(r#"{"type":"set_punch","enabled":true}"#);
    punch_take(&mut rig, t1, 30_000, 34_000);
    rig.cmd(r#"{"type":"stop"}"#);
    rig.cmd(r#"{"type":"seek","pos":30500}"#);
    punch_take(&mut rig, t1, 31_000, 32_000);
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let (old, old_start) = node(&ev, "Piste 1")
        .clips
        .iter()
        .find_map(|c| match c {
            ClipInfo::Audio { id, start, muted: true, .. } => Some((*id, *start)),
            _ => None,
        })
        .unwrap();
    assert_eq!(old_start, 31_000 - COMP, "la prise est posée avec la compensation");
    let before = take_flags(&ev);
    let ev = rig.cmd(&format!(r#"{{"type":"activate_take_range","clip":{old},"start":31100,"end":31300}}"#));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    assert_eq!(undo_label(&ev).as_deref(), Some("Composer une prise"));
    let after = take_flags(&ev);
    // La plage 31 100-31 300 est maintenant l'ancien passage (actif) ; le reste de la nouvelle prise l'entoure.
    assert!(after.iter().any(|f| f.0 == 31_100 && f.1 == 200 && !f.2), "{after:?}");
    assert!(after.iter().any(|f| f.0 == 30_900 && f.1 == 200 && !f.2) && after.iter().any(|f| f.0 == 31_300 && f.1 == 600 && !f.2), "{after:?}");
    let ev = rig.cmd(r#"{"type":"undo"}"#);
    assert_eq!(take_flags(&ev), before);
    assert!(error(&rig.cmd(r#"{"type":"activate_take_range","clip":999999,"start":1,"end":2}"#)).contains("inconnu"));
}

#[test]
fn midi_loop_recording_makes_one_container_per_non_empty_pass_and_none_for_an_empty_pass() {
    let mut rig = rig("midi-loop");
    let (_t1, t2) = two_tracks(&mut rig);
    rig.cmd(&format!(r#"{{"type":"set_track_input","node":{t2},"input":{{"kind":"midi"}}}}"#));
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t2},"arm":true}}"#));
    rig.cmd(r#"{"type":"loop","start":30000,"end":31000}"#);
    rig.cmd(r#"{"type":"set_loop_record","enabled":true}"#);
    rig.cmd(r#"{"type":"set_metronome","enabled":false,"only_recording":false,"volume":0.8,"count_in_bars":0,"pre_roll":0,"post_roll":0}"#);
    let mut midi = rig.host.take_midi_input().expect("entrée MIDI");
    rig.feed(64);
    let ev = rig.cmd(r#"{"type":"record"}"#);
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    // Trois passages de 1 000 échantillons (blocs de 64) : une note au 1ᵉʳ, rien au 2ᵉ, une autre note au 3ᵉ.
    for step in 0..44 {
        let at = step * 64;
        let msg = match at {
            192 => Some(MidiMsg::NoteOn { channel: 0, key: 60, velocity: 0.8 }),
            320 => Some(MidiMsg::NoteOff { channel: 0, key: 60, velocity: 0.0 }),
            2_176 => Some(MidiMsg::NoteOn { channel: 0, key: 62, velocity: 0.8 }),
            2_304 => Some(MidiMsg::NoteOff { channel: 0, key: 62, velocity: 0.0 }),
            _ => None,
        };
        if let Some(m) = msg {
            midi.push(t2, m, 0);
        }
        rig.feed(64);
        rig.host.tick();
    }
    rig.cmd(r#"{"type":"stop_record"}"#);
    rig.settle();
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let clips: Vec<(Vec<u8>, bool, u32)> = node(&ev, "Basse")
        .clips
        .iter()
        .filter_map(|c| match c {
            ClipInfo::Midi { notes, muted, take_group, .. } => Some((notes.iter().map(|n| n.key).collect(), *muted, *take_group)),
            _ => None,
        })
        .collect();
    assert_eq!(clips, vec![(vec![60], true, 1), (vec![62], false, 1)], "le passage vide ne crée pas de conteneur : {clips:?}");
    assert_eq!(undo_label(&ev).as_deref(), Some("Enregistrer"));
    // Changer de conteneur actif : une action annulable.
    let other = node(&ev, "Basse").clips.iter().find_map(|c| if let ClipInfo::Midi { id, muted: true, .. } = c { Some(*id) } else { None }).unwrap();
    let ev = rig.cmd(&format!(r#"{{"type":"set_active_take","clip":{other}}}"#));
    assert_eq!(undo_label(&ev).as_deref(), Some("Changer de prise"));
    let muted: Vec<bool> = node(&ev, "Basse").clips.iter().filter_map(|c| if let ClipInfo::Midi { muted, .. } = c { Some(*muted) } else { None }).collect();
    assert_eq!(muted, vec![false, true]);
}
