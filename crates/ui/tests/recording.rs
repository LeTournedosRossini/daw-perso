//! Enregistrement : entrées par piste, armement, prises audio et MIDI devenues des clips, refus, enregistrement du projet.

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

#[test]
fn armed_tracks_record_their_own_inputs_and_become_clips_in_one_undo_step() {
    let mut rig = rig("prises");
    let (t1, t2) = two_tracks(&mut rig);
    // Piste 1 : l'entrée 3 en mono ; Basse : la paire 5-6 en stéréo.
    rig.cmd(&format!(r#"{{"type":"set_track_input","node":{t1},"input":{{"kind":"audio","channels":[3]}}}}"#));
    let ev = rig.cmd(&format!(r#"{{"type":"set_track_input","node":{t2},"input":{{"kind":"audio","channels":[5,6]}}}}"#));
    assert_eq!(node(&ev, "Basse").input.channels, [5, 6]);
    assert_eq!(project(&ev).audio_inputs, 10);
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t1},"arm":true}}"#));
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t2},"arm":true}}"#));
    rig.cmd(r#"{"type":"seek","pos":1000}"#);
    rig.feed(64); // le moteur prend le déplacement en compte ; il est arrêté, la position reste 1000
    let first = rig.fed; // indice, dans le flux d'entrée du test, du premier échantillon qui sera enregistré
    let start = rig.cmd(r#"{"type":"record"}"#);
    assert!(start.iter().any(|e| matches!(e, Event::Recording { active: true, stopping: false, .. })));
    // La page dessine un conteneur « en cours d'enregistrement » : elle sait où la prise commence et sur quelles pistes.
    let (from, nodes) = start.iter().find_map(|e| if let Event::Recording { active: true, start, nodes, .. } = e { Some((*start, nodes.clone())) } else { None }).unwrap();
    assert_eq!(from, 1000);
    assert!(nodes.contains(&t1) && nodes.contains(&t2) && nodes.len() == 2, "{nodes:?}");
    rig.feed(4_864);
    let ev = rig.cmd(r#"{"type":"stop_record"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Recording { stopping: true, .. })));
    let done = rig.settle();
    let fin = done.iter().find_map(|e| if let Event::Recording { active: false, warnings, overruns, .. } = e { Some((warnings.clone(), *overruns)) } else { None }).unwrap();
    assert_eq!(fin, (Vec::new(), 0), "aucun avertissement, aucun bloc perdu");
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    assert_eq!(undo_label(&ev).as_deref(), Some("Enregistrer"));
    let (a, b) = (audio_clip(&node(&ev, "Piste 1").clips[0]), audio_clip(&node(&ev, "Basse").clips[0]));
    assert_eq!((a.0.as_str(), b.0.as_str()), ("Piste 1 prise 1", "Basse prise 1"));
    // La prise a commencé à la position 1000 ; le clip est posé 100 échantillons plus tôt (latence aller-retour).
    assert_eq!((a.1, a.2, a.3), (1000 - COMP, 4_864, 0), "{a:?}");
    assert_eq!((b.1, b.2, b.3), (1000 - COMP, 4_864, 0));
    // Contenu : canal 3 (indice 2) en mono ; canaux 5 et 6 (indices 4 et 5) entrelacés.
    let (ca, wav_a) = read_f32_wav(&rig.dir.join("P.daw/audio/Piste-1-prise-001.wav"));
    let (cb, wav_b) = read_f32_wav(&rig.dir.join("P.daw/audio/Basse-prise-001.wav"));
    assert_eq!((ca, cb), (1, 2));
    assert_eq!((wav_a.len(), wav_b.len()), (4_864, 2 * 4_864));
    for j in [0usize, 1, 100, 4_863] {
        assert!(approx(wav_a[j], input(2, first + j)), "piste 1, échantillon {j}");
        assert!(approx(wav_b[2 * j], input(4, first + j)) && approx(wav_b[2 * j + 1], input(5, first + j)), "basse, trame {j}");
    }
    // Une seule annulation retire les deux clips ; rétablir les remet.
    let ev = rig.cmd(r#"{"type":"undo"}"#);
    assert!(node(&ev, "Piste 1").clips.is_empty() && node(&ev, "Basse").clips.is_empty());
    let ev = rig.cmd(r#"{"type":"redo"}"#);
    assert_eq!((node(&ev, "Piste 1").clips.len(), node(&ev, "Basse").clips.len()), (1, 1));
    // Une deuxième prise ne remplace pas la première : nouveaux fichiers, nouveaux noms.
    rig.cmd(r#"{"type":"seek","pos":20000}"#);
    rig.feed(64);
    rig.cmd(r#"{"type":"record"}"#);
    rig.feed(640);
    rig.cmd(r#"{"type":"transport","action":"stop"}"#); // arrêter la lecture arrête aussi l'enregistrement
    rig.settle();
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let names: Vec<String> = node(&ev, "Piste 1").clips.iter().map(|c| audio_clip(c).0).collect();
    assert_eq!(names, ["Piste 1 prise 1", "Piste 1 prise 2"]);
    assert!(rig.dir.join("P.daw/audio/Piste-1-prise-002.wav").exists());
}

#[test]
fn a_take_that_starts_before_the_latency_is_trimmed_not_moved_before_zero() {
    let mut rig = rig("tete");
    let (t1, _) = two_tracks(&mut rig);
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t1},"arm":true}}"#));
    rig.feed(64);
    rig.cmd(r#"{"type":"record"}"#); // départ à la position 0 : 100 échantillons de latence à rogner
    rig.feed(1_000);
    rig.cmd(r#"{"type":"stop_record"}"#);
    rig.settle();
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let c = audio_clip(&node(&ev, "Piste 1").clips[0]);
    assert_eq!((c.1, c.3, c.2), (0, COMP, 1_000 - COMP), "début à 0, tête rognée de la latence, durée en conséquence : {c:?}");
}

#[test]
fn a_midi_track_records_the_notes_played_while_armed() {
    let mut rig = rig("midi");
    let (t1, t2) = two_tracks(&mut rig);
    rig.cmd(&format!(r#"{{"type":"set_track_input","node":{t2},"input":{{"kind":"midi"}}}}"#));
    let ev = rig.cmd(&format!(r#"{{"type":"set_node","node":{t2},"arm":true}}"#));
    assert_eq!(node(&ev, "Basse").input.kind, "midi");
    assert_eq!(rig.host.armed_midi_tracks(), vec![t2]);
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t1},"arm":true}}"#)); // piste audio armée aussi
    assert_eq!(rig.host.armed_midi_tracks(), vec![t2], "seules les pistes MIDI reçoivent le MIDI");
    let mut midi = rig.host.take_midi_input().expect("entrée MIDI");
    rig.feed(64);
    rig.cmd(r#"{"type":"record"}"#);
    midi.push(t2, MidiMsg::NoteOn { channel: 0, key: 60, velocity: 0.8 }, 0);
    rig.feed(2_400);
    midi.push(t2, MidiMsg::NoteOff { channel: 0, key: 60, velocity: 0.0 }, 0);
    rig.feed(2_400);
    rig.cmd(r#"{"type":"stop_record"}"#);
    rig.settle();
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let clip = node(&ev, "Basse").clips.iter().find_map(|c| if let ClipInfo::Midi { name, notes, .. } = c { Some((name.clone(), notes.clone())) } else { None }).expect("clip de notes");
    assert_eq!(clip.0, "Basse prise 1");
    assert_eq!(clip.1.len(), 1);
    assert_eq!(clip.1[0].key, 60);
    assert!(clip.1[0].len > 2_000 && clip.1[0].len < 2_700, "durée d'environ 2 400 échantillons : {}", clip.1[0].len);
    // La piste audio a aussi fait sa prise.
    assert_eq!(node(&ev, "Piste 1").clips.len(), 1);
    assert_eq!(undo_label(&ev).as_deref(), Some("Enregistrer"));
}

#[test]
fn recording_is_refused_with_a_clear_message_when_it_cannot_work() {
    let mut rig = rig("refus");
    rig.cmd(r#"{"type":"new_project"}"#);
    rig.cmd(r#"{"type":"set_node","node":1,"arm":true}"#);
    assert!(error(&rig.cmd(r#"{"type":"record"}"#)).contains("Enregistrer sous"), "projet pas encore enregistré");
    rig.save();
    rig.cmd(r#"{"type":"set_node","node":1,"arm":false}"#);
    assert!(error(&rig.cmd(r#"{"type":"record"}"#)).contains("aucune piste armée"));
    rig.cmd(r#"{"type":"set_node","node":1,"arm":true}"#);
    rig.cmd(r#"{"type":"loop","start":0,"end":48000}"#);
    assert!(error(&rig.cmd(r#"{"type":"record"}"#)).contains("boucle"));
    rig.cmd(r#"{"type":"loop","start":null,"end":null}"#);
    // Pendant une prise : on ne déplace pas la tête, on ne change pas la structure, on n'ouvre pas d'autre projet.
    rig.feed(64);
    assert!(rig.cmd(r#"{"type":"record"}"#).iter().any(|e| matches!(e, Event::Recording { active: true, .. })));
    assert!(error(&rig.cmd(r#"{"type":"record"}"#)).contains("déjà"));
    for bad in [
        r#"{"type":"seek","pos":5}"#,
        r#"{"type":"loop","start":0,"end":100}"#,
        r#"{"type":"add_track","kind":"mono"}"#,
        r#"{"type":"delete_track","node":1}"#,
        r#"{"type":"new_project"}"#,
        r#"{"type":"open_project","path":"/nulle/part.daw"}"#,
        r#"{"type":"undo"}"#,
        r#"{"type":"set_node","node":1,"arm":false}"#,
        r#"{"type":"set_track_input","node":1,"input":{"kind":"midi"}}"#,
    ] {
        assert!(error(&rig.cmd(bad)).contains("arrête d'abord l'enregistrement"), "{bad}");
    }
    rig.cmd(r#"{"type":"stop_record"}"#);
    rig.settle();
    // Une prise vide (rien enregistré) ne crée rien.
    rig.cmd(r#"{"type":"record"}"#);
    rig.cmd(r#"{"type":"stop_record"}"#);
    rig.settle();
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    assert!(node(&ev, "Piste 1").clips.len() <= 1);
    // Entrées : numéros hors du pilote, nœuds qui ne sont pas des pistes.
    assert!(error(&rig.cmd(r#"{"type":"set_track_input","node":1,"input":{"kind":"audio","channels":[11]}}"#)).contains("inexistante"));
    assert!(error(&rig.cmd(r#"{"type":"set_track_input","node":0,"input":{"kind":"audio","channels":[1]}}"#)).contains("sortie principale"));
    rig.cmd(r#"{"type":"add_track","kind":"bus","name":"B"}"#);
    let bus = node(&rig.cmd(r#"{"type":"get_project"}"#), "B").id;
    assert!(error(&rig.cmd(&format!(r#"{{"type":"set_track_input","node":{bus},"input":{{"kind":"audio","channels":[1]}}}}"#))).contains("pas une piste"));
    assert!(error(&rig.cmd(r#"{"type":"set_track_input","node":1,"input":{"kind":"audio","channels":[]}}"#)).contains("1 ou 2 canaux"));
    assert!(error(&rig.cmd(r#"{"type":"set_track_input","node":1,"input":{"kind":"video"}}"#)).contains("inconnu"));
}

#[test]
fn inputs_and_monitoring_are_saved_with_the_project() {
    let mut a = rig("persistance");
    let (t1, t2) = two_tracks(&mut a);
    a.cmd(&format!(r#"{{"type":"set_track_input","node":{t1},"input":{{"kind":"audio","channels":[7,8]}}}}"#));
    a.cmd(&format!(r#"{{"type":"set_track_input","node":{t2},"input":{{"kind":"midi"}}}}"#));
    let ev = a.cmd(&format!(r#"{{"type":"set_node","node":{t1},"monitor":false}}"#));
    assert!(!node(&ev, "Piste 1").monitor);
    a.cmd(&format!(r#"{{"type":"save_project","path":{:?}}}"#, a.dir.join("P").to_string_lossy()));
    let mut b = rig("persistance-b");
    b.host.set_audio_inputs(10);
    let ev = b.cmd(&format!(r#"{{"type":"open_project","path":{:?}}}"#, a.dir.join("P.daw").to_string_lossy()));
    let (p1, p2) = (node(&ev, "Piste 1"), node(&ev, "Basse"));
    assert_eq!((p1.input.kind.as_str(), p1.input.channels.clone(), p1.monitor), ("audio", vec![7, 8], false));
    assert_eq!((p2.input.kind.as_str(), p2.monitor), ("midi", true));
    // Une piste qui n'a jamais été réglée garde l'entrée par défaut : audio, entrée 1, écoute directe.
    let ev = b.cmd(r#"{"type":"add_track","kind":"stereo"}"#);
    let fresh = project(&ev).nodes.last().unwrap();
    assert_eq!((fresh.input.kind.as_str(), fresh.input.channels.clone(), fresh.monitor), ("audio", vec![1], true));
}

#[test]
fn count_in_and_pre_roll_play_before_the_take_which_starts_exactly_at_the_chosen_position() {
    let mut rig = rig("precompte");
    let (t1, _) = two_tracks(&mut rig);
    rig.cmd(r#"{"type":"set_tempo","bpm":400,"at_bar":1}"#); // une mesure à 4/4 = 28 800 échantillons
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t1},"arm":true}}"#));
    let ev = rig.cmd(r#"{"type":"set_metronome","enabled":false,"only_recording":true,"volume":0.8,"count_in_bars":1,"pre_roll":0.01,"post_roll":0}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Metronome { count_in_bars: 1, only_recording: true, .. })));
    rig.cmd(r#"{"type":"seek","pos":2000}"#);
    rig.feed(64);
    let first = rig.fed;
    rig.cmd(r#"{"type":"record"}"#);
    // Pré-compte (28 800) + pré-roll (480) puis 1 000 échantillons de prise.
    rig.feed(28_800 + 480 + 1_000);
    rig.cmd(r#"{"type":"stop_record"}"#);
    rig.settle();
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let c = audio_clip(&node(&ev, "Piste 1").clips[0]);
    assert_eq!((c.1, c.2, c.3), (2000 - COMP, 1_000, 0), "la prise commence à la position 2000, ni avant ni après : {c:?}");
    let (_, wav) = read_f32_wav(&rig.dir.join("P.daw/audio/Piste-1-prise-001.wav"));
    assert!(approx(wav[0], input(0, first + 28_800 + 480)), "premier échantillon de la prise");
}

#[test]
fn the_post_roll_keeps_recording_after_the_stop_request_then_stops_by_itself() {
    let mut rig = rig("postroll");
    let (t1, _) = two_tracks(&mut rig);
    rig.cmd(&format!(r#"{{"type":"set_node","node":{t1},"arm":true}}"#));
    rig.cmd(r#"{"type":"set_metronome","enabled":false,"only_recording":false,"volume":0.8,"count_in_bars":0,"pre_roll":0,"post_roll":0.01}"#); // 480 échantillons
    rig.feed(64);
    rig.cmd(r#"{"type":"record"}"#);
    rig.feed(1_000);
    let ev = rig.cmd(r#"{"type":"stop_record"}"#);
    assert!(!ev.iter().any(|e| matches!(e, Event::Recording { stopping: true, .. })), "le post-roll commence : on enregistre encore");
    let mut all = Vec::new();
    for _ in 0..40 {
        rig.feed(64);
        all.extend(rig.host.tick());
        if all.iter().any(|e| matches!(e, Event::Recording { active: false, .. })) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(all.iter().any(|e| matches!(e, Event::Recording { active: false, .. })), "terminé tout seul");
    let ev = rig.cmd(r#"{"type":"get_project"}"#);
    let c = audio_clip(&node(&ev, "Piste 1").clips[0]);
    assert!((1_480..=1_480 + 128).contains(&(c.2 + c.3)), "1 000 + 480 échantillons, à un bloc près (tête rognée de la latence comprise) : {c:?}");
}

#[test]
fn metronome_settings_are_validated() {
    let mut rig = rig("valid");
    for bad in [
        r#"{"type":"set_metronome","enabled":true,"only_recording":false,"volume":2,"count_in_bars":0,"pre_roll":0,"post_roll":0}"#,
        r#"{"type":"set_metronome","enabled":true,"only_recording":false,"volume":1,"count_in_bars":3,"pre_roll":0,"post_roll":0}"#,
        r#"{"type":"set_metronome","enabled":true,"only_recording":false,"volume":1,"count_in_bars":0,"pre_roll":11,"post_roll":0}"#,
    ] {
        assert!(!error(&rig.cmd(bad)).is_empty());
    }
}
