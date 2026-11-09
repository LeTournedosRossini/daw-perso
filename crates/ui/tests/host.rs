use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::host::MIDI_CLIP_BASE;
use daw_ui::{Command, Event, Host};

const SR: u32 = 48_000;

fn setup() -> (Engine, Host) {
    let (engine, handle) = Engine::new(SR, 256);
    let (session, meta) = demo_session(SR);
    (engine, Host::new(handle, session, meta).unwrap())
}

fn run(engine: &mut Engine, blocks: usize) {
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    for _ in 0..blocks {
        engine.process(&mut l, &mut r);
    }
}

fn json(host: &mut Host, s: &str) -> Vec<Event> {
    host.command_json(s)
}

fn project(events: &[Event]) -> &daw_ui::protocol::ProjectInfo {
    events.iter().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

#[test]
fn project_describes_nodes_clips_and_timing() {
    let (_, mut host) = setup();
    let ev = json(&mut host, r#"{"type":"get_project"}"#);
    let p = project(&ev);
    assert_eq!(p.sample_rate, SR);
    assert_eq!(p.nodes.len(), 6);
    assert_eq!(p.nodes[0].kind, "master");
    assert_eq!(p.nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["Master", "Batterie", "Basse", "Claviers", "Guitare", "Voix"]);
    assert_eq!(p.tempo_map.len(), 1);
    assert_eq!((p.tempo_map[0].num, p.tempo_map[0].den, p.meters[0].num, p.meters[0].den), (120, 1, 4, 4));
    let json = serde_json::to_value(&ev[0]).unwrap();
    assert_eq!(json["type"], "project");
    let clips = &json["nodes"][1]["clips"];
    assert_eq!(clips[0]["kind"], "audio");
    assert_eq!(clips[0]["name"], "Beat A");
    assert_eq!(clips[1]["fade_out"], 24_000);
    let midi = &json["nodes"][3]["clips"][0];
    assert_eq!((midi["kind"].as_str(), midi["id"].as_u64(), midi["name"].as_str()), (Some("midi"), Some(MIDI_CLIP_BASE), Some("Arpège")));
    assert_eq!(midi["notes"].as_array().unwrap().len(), 64);
    // Identifiants de clips uniques dans tout le projet.
    let mut ids: Vec<u64> = json["nodes"].as_array().unwrap().iter().flat_map(|n| n["clips"].as_array().unwrap().iter().map(|c| c["id"].as_u64().unwrap())).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 7);
}

#[test]
fn transport_position_and_loop_follow_the_engine() {
    let (mut engine, mut host) = setup();
    // Ce test exerce la pause : sans retour de la tête à l'arrêt (le comportement par défaut est de revenir, voir `play_preroll.rs`).
    json(&mut host, r#"{"type":"set_prefs","prefs":{"transport":{"play_pre_roll":false,"return_on_stop":false}}}"#);
    let pos = |ev: &[Event]| ev.iter().find_map(|e| if let Event::Position { pos, playing, loop_range } = e { Some((*pos, *playing, *loop_range)) } else { None }).unwrap();
    json(&mut host, r#"{"type":"transport","action":"play"}"#);
    run(&mut engine, 10); // 2 560 échantillons
    let (p, playing, _) = pos(&host.tick());
    assert_eq!((p, playing), (2_560, true));
    json(&mut host, r#"{"type":"transport","action":"stop"}"#);
    run(&mut engine, 5);
    let (p, playing, _) = pos(&host.tick());
    assert_eq!((p, playing), (2_560, false), "la position est conservée à l'arrêt");
    let ev = json(&mut host, r#"{"type":"seek","pos":96000}"#);
    assert_eq!(pos(&ev).0, 96_000);
    run(&mut engine, 1);
    assert_eq!(pos(&host.tick()).0, 96_000);
    let ev = json(&mut host, r#"{"type":"loop","start":1000,"end":5000}"#);
    assert_eq!(pos(&ev).2.map(|l| (l.start, l.end)), Some((1_000, 5_000)));
    let ev = json(&mut host, r#"{"type":"loop","start":null,"end":null}"#);
    assert_eq!(pos(&ev).2, None);
    json(&mut host, r#"{"type":"transport","action":"toggle"}"#);
    assert!(host.is_playing());
    let json_pos = serde_json::to_value(&host.tick()[0]).unwrap();
    assert_eq!(json_pos["type"], "position");
    assert!(json_pos.get("loop").is_some(), "le champ s'appelle `loop` dans le JSON");
}

#[test]
fn meters_show_the_playing_tracks_and_follow_mute_and_gain() {
    let (mut engine, mut host) = setup();
    let level = |host: &mut Host, node: &str| -> [f32; 2] {
        match &host.tick()[1] {
            Event::Meters { levels } => levels[node],
            e => panic!("{e:?}"),
        }
    };
    json(&mut host, r#"{"type":"seek","pos":96000}"#);
    json(&mut host, r#"{"type":"transport","action":"play"}"#);
    run(&mut engine, 40);
    // Une seule lecture : `tick` remet les crêtes à zéro, donc on lit tous les nœuds du même coup.
    let Event::Meters { levels } = host.tick().remove(1) else { panic!() };
    let drums = levels["1"];
    assert!(drums[0] > 0.05 && drums[1] > 0.05, "la batterie sonne : {drums:?}");
    assert!(levels["0"][0] > 0.0 && levels["0"][1] > 0.0, "la sortie principale sonne : {:?}", levels["0"]);
    assert_eq!(levels["3"], [0.0, 0.0], "aucun son sur Claviers (clip de notes sans instrument)");
    // Muet : la crête retombe à zéro après la rampe de 5 ms.
    json(&mut host, r#"{"type":"set_node","node":1,"mute":true}"#);
    run(&mut engine, 40);
    level(&mut host, "1");
    run(&mut engine, 40);
    assert_eq!(level(&mut host, "1"), [0.0, 0.0]);
    // Un gain divisé par deux divise la crête par deux.
    json(&mut host, r#"{"type":"set_node","node":1,"mute":false,"gain":0.8}"#);
    run(&mut engine, 60);
    level(&mut host, "1");
    run(&mut engine, 120);
    let a = level(&mut host, "1")[0];
    json(&mut host, r#"{"type":"set_node","node":1,"gain":0.4}"#);
    run(&mut engine, 60);
    level(&mut host, "1");
    run(&mut engine, 120);
    let b = level(&mut host, "1")[0];
    assert!(a > 0.0 && (b / a - 0.5).abs() < 0.25, "a = {a}, b = {b}");
}

#[test]
fn set_node_is_confirmed_by_a_project_event_and_refused_when_wrong() {
    let (_, mut host) = setup();
    let ev = json(&mut host, r#"{"type":"set_node","node":2,"gain":0.25,"pan":-0.5,"mute":true,"solo":true,"arm":true}"#);
    let n = &project(&ev).nodes[2];
    assert_eq!((n.gain, n.pan, n.mute, n.solo, n.arm), (0.25, -0.5, true, true, true));
    let err = |ev: &[Event]| match &ev[0] {
        Event::Error { command, message } => (command.clone(), message.clone()),
        e => panic!("pas une erreur : {e:?}"),
    };
    assert_eq!(err(&json(&mut host, r#"{"type":"set_node","node":99,"gain":1}"#)).0, "set_node");
    assert!(err(&json(&mut host, r#"{"type":"set_node","node":1,"gain":-1}"#)).1.contains("gain"));
    assert!(err(&json(&mut host, r#"{"type":"set_node","node":1,"pan":2}"#)).1.contains("pan"));
    assert!(err(&json(&mut host, r#"{"type":"set_node","node":1}"#)).1.contains("rien"));
    assert!(err(&json(&mut host, r#"{"type":"loop","start":10,"end":10}"#)).1.contains("loop"));
    assert!(err(&json(&mut host, r#"{"type":"zigzag"}"#)).0 == "zigzag");
    assert!(err(&json(&mut host, "pas du json")).1.contains("JSON invalide"));
    assert!(err(&json(&mut host, r#"{"type":"seek","pos":-5}"#)).0 == "seek");
    // Une commande refusée ne change rien.
    let ev = json(&mut host, r#"{"type":"get_project"}"#);
    assert_eq!(project(&ev).nodes[1].gain, 0.8);
}

#[test]
fn peaks_cover_the_requested_range_and_match_the_signal() {
    let (_, mut host) = setup();
    let get = |host: &mut Host, clip: u64, from: u64, to: u64, columns: usize| match host.command(Command::GetPeaks { clip, from, to, columns }).remove(0) {
        Event::Peaks { columns, .. } => Ok(columns),
        Event::Error { message, .. } => Err(message),
        e => panic!("{e:?}"),
    };
    // Beat A : id 0, à partir de 0 sur 8 mesures.
    let cols = get(&mut host, 0, 0, 96_000, 200).unwrap();
    assert_eq!(cols.len(), 200);
    assert!(cols.iter().all(|[lo, hi]| lo <= hi && *lo >= -1.0 && *hi <= 1.0));
    assert!(cols.iter().any(|[lo, hi]| hi - lo > 0.2), "il y a du signal");
    assert_eq!(get(&mut host, 0, 0, 96_000, 200).unwrap(), cols, "déterministe");
    // Hors du clip : colonnes vides ; à cheval : vide d'un côté.
    let outside = get(&mut host, 0, 5_000_000, 5_100_000, 10).unwrap();
    assert!(outside.iter().all(|c| *c == [0.0, 0.0]));
    let straddle = get(&mut host, 2, 96_000 * 14, 96_000 * 18, 8).unwrap(); // « Basse » finit à 16 mesures
    assert!(straddle[0][1] > 0.0 && straddle[7] == [0.0, 0.0]);
    // Clip inconnu ou clip de notes : refusé.
    assert!(get(&mut host, 4242, 0, 100, 2).unwrap_err().contains("inconnu"));
    assert!(get(&mut host, MIDI_CLIP_BASE, 0, 100, 2).is_err());
    assert!(host.command(Command::GetPeaks { clip: 0, from: 5, to: 5, columns: 4 })[0] != Event::Peaks { clip: 0, from: 5, to: 5, columns: vec![] });
}

#[test]
fn analogsynth_makes_the_arpeggio_audible_when_installed() {
    let (session, meta, note) = daw_ui::demo::demo_session_with_instruments(SR);
    if !note.contains("chargé") {
        eprintln!("AnalogSynth absent de cette machine : essai ignoré ({note})");
        return;
    }
    let (mut engine, handle) = Engine::new(SR, 256);
    let mut host = Host::new(handle, session, meta).unwrap();
    // Le clip de notes commence à la mesure 5 (4 × 96 000 échantillons).
    host.command(Command::Seek { pos: 4 * 96_000 });
    host.command(Command::Transport { action: daw_ui::protocol::TransportAction::Play });
    let mut peak = 0.0f32;
    for _ in 0..40 {
        run(&mut engine, 25);
        let Event::Meters { levels } = host.tick().remove(1) else { panic!() };
        peak = peak.max(levels["3"][0]).max(levels["3"][1]);
    }
    assert!(peak > 0.01, "l'arpège sonne sur Claviers : crête {peak}");
}

// ---- édition de clips ----------------------------------------------------------------------------------------------

fn clip_of<'a>(p: &'a daw_ui::protocol::ProjectInfo, id: u64) -> Option<&'a daw_ui::protocol::ClipInfo> {
    p.nodes.iter().flat_map(|n| n.clips.iter()).find(|c| match c {
        daw_ui::protocol::ClipInfo::Audio { id: i, .. } | daw_ui::protocol::ClipInfo::Midi { id: i, .. } => *i == id,
    })
}
fn audio(c: &daw_ui::protocol::ClipInfo) -> (u64, u64, u64, u64, u64) {
    match c {
        daw_ui::protocol::ClipInfo::Audio { start, len, offset, fade_in, fade_out, .. } => (*start, *len, *offset, *fade_in, *fade_out),
        _ => panic!("pas un clip audio"),
    }
}
fn history(ev: &[Event]) -> (bool, bool, Option<String>, Option<String>) {
    ev.iter().find_map(|e| if let Event::History { can_undo, can_redo, undo_label, redo_label } = e { Some((*can_undo, *can_redo, undo_label.clone(), redo_label.clone())) } else { None }).expect("événement history")
}

#[test]
fn move_trim_split_fades_delete_and_undo_through_the_protocol() {
    let (_, mut host) = setup();
    // Ce test suit les éditions une à une : sans fondus enchaînés automatiques (testés à part).
    host.command_json(r#"{"type":"set_auto_crossfade","enabled":false}"#);
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(history(&ev), (false, false, None, None));
    // Beat A (id 0) : début 0, 8 mesures = 768 000.
    assert_eq!(audio(clip_of(project(&ev), 0).unwrap()), (0, 768_000, 0, 0, 0));
    // Déplacement.
    let ev = host.command_json(r#"{"type":"move_clip","clip":0,"start":48000}"#);
    assert_eq!(audio(clip_of(project(&ev), 0).unwrap()).0, 48_000);
    assert_eq!(history(&ev), (true, false, Some("Déplacer un clip".into()), None));
    // Rognage début et fin en une seule action.
    let ev = host.command_json(r#"{"type":"trim_clip","clip":0,"start":100000,"end":500000}"#);
    assert_eq!(audio(clip_of(project(&ev), 0).unwrap()), (100_000, 400_000, 52_000, 0, 0));
    assert_eq!(history(&ev).2.as_deref(), Some("Rogner un clip"));
    // Fondus (bornés à la longueur du clip).
    let ev = host.command_json(r#"{"type":"set_clip_fades","clip":0,"fade_in":2000,"fade_out":99999999}"#);
    assert_eq!(audio(clip_of(project(&ev), 0).unwrap()), (100_000, 400_000, 52_000, 2_000, 400_000));
    // Découpe : le clip d'origine garde son identifiant (partie gauche), la droite en reçoit un nouveau.
    let before: usize = project(&ev).nodes.iter().map(|n| n.clips.len()).sum();
    let ev = host.command_json(r#"{"type":"split_clip","clip":0,"at":300000}"#);
    let p = project(&ev);
    assert_eq!(p.nodes.iter().map(|n| n.clips.len()).sum::<usize>(), before + 1);
    let (s, l, o, fi, fo) = audio(clip_of(p, 0).unwrap());
    assert_eq!((s, l, o, fi, fo), (100_000, 200_000, 52_000, 2_000, 0));
    let right = p.nodes[1].clips.iter().filter_map(|c| if let daw_ui::protocol::ClipInfo::Audio { id, name, .. } = c { Some((*id, name.clone())) } else { None }).find(|(id, _)| *id > 5 && *id != 1).expect("nouvelle moitié");
    assert_eq!(right.1, "Beat A", "la moitié droite garde le nom");
    // Suppression, puis tout défaire en ordre inverse jusqu'à l'état d'origine.
    let ev = host.command_json(&format!(r#"{{"type":"delete_clip","clip":{}}}"#, right.0));
    assert!(clip_of(project(&ev), right.0).is_none());
    let mut labels = Vec::new();
    loop {
        let ev = host.command_json(r#"{"type":"undo"}"#);
        match ev.first() {
            Some(Event::Error { message, .. }) => {
                assert!(message.contains("rien à annuler"));
                break;
            }
            _ => labels.push(history(&ev).3.unwrap()),
        }
    }
    assert_eq!(labels, ["Supprimer un clip", "Découper", "Modifier un clip", "Rogner un clip", "Déplacer un clip"]);
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(audio(clip_of(project(&ev), 0).unwrap()), (0, 768_000, 0, 0, 0));
    assert_eq!(history(&ev).1, true, "on peut rétablir");
    let ev = host.command_json(r#"{"type":"redo"}"#);
    assert_eq!(audio(clip_of(project(&ev), 0).unwrap()).0, 48_000);
}

#[test]
fn edits_that_make_no_sense_are_refused_and_change_nothing() {
    let (_, mut host) = setup();
    let err = |ev: Vec<Event>| match ev.into_iter().next() {
        Some(Event::Error { command, message }) => (command, message),
        e => panic!("{e:?}"),
    };
    assert_eq!(err(host.command_json(r#"{"type":"move_clip","clip":4242,"start":0}"#)).0, "move_clip");
    assert!(err(host.command_json(r#"{"type":"split_clip","clip":0,"at":0}"#)).1.contains("hors du clip"));
    assert!(err(host.command_json(r#"{"type":"split_clip","clip":0,"at":99999999}"#)).1.contains("hors du clip"));
    assert!(err(host.command_json(r#"{"type":"trim_clip","clip":0,"start":900000}"#)).1.contains("hors du clip"));
    assert!(err(host.command_json(r#"{"type":"trim_clip","clip":0}"#)).1.contains("start et/ou end"));
    assert!(err(host.command_json(r#"{"type":"trim_clip","clip":0,"start":10,"end":10}"#)).1.contains("start < end"));
    assert!(err(host.command_json(r#"{"type":"set_clip_fades","clip":0}"#)).1.contains("fade_in"));
    assert!(err(host.command_json(r#"{"type":"trim_clip","clip":4242,"start":5}"#)).1.contains("inconnu"));
    assert!(err(host.command_json(r#"{"type":"trim_clip","clip":1000000,"start":5}"#)).1.contains("hors du clip"), "avant le début de la matière d'un clip de notes");
    assert!(err(host.command_json(r#"{"type":"undo"}"#)).1.contains("rien à annuler"));
    assert!(err(host.command_json(r#"{"type":"redo"}"#)).1.contains("rien à rétablir"));
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(history(&ev), (false, false, None, None), "rien n'a été inscrit");
}

#[test]
fn notes_clips_move_on_the_musical_grid_and_can_be_deleted() {
    let (_, mut host) = setup();
    // Arpège : id 1 000 000, début à la mesure 5 (384 000).
    let ev = host.command_json(r#"{"type":"move_clip","clip":1000000,"start":480000}"#);
    let n = &project(&ev).nodes[3];
    match &n.clips[0] {
        daw_ui::protocol::ClipInfo::Midi { start, notes, .. } => {
            assert_eq!(*start, 480_000);
            assert_eq!(notes.len(), 64);
        }
        c => panic!("{c:?}"),
    }
    let ev = host.command_json(r#"{"type":"delete_clip","clip":1000000}"#);
    assert!(project(&ev).nodes[3].clips.is_empty());
    let ev = host.command_json(r#"{"type":"undo"}"#);
    assert_eq!(project(&ev).nodes[3].clips.len(), 1);
}

#[test]
fn mixer_state_and_sound_survive_a_clip_edit() {
    let (mut engine, mut host) = setup();
    host.command_json(r#"{"type":"set_node","node":1,"gain":0.2,"pan":0.5,"mute":false}"#);
    host.command_json(r#"{"type":"set_node","node":2,"mute":true}"#);
    // Une édition recharge le graphe : les réglages tirés avant doivent y être.
    let ev = host.command_json(r#"{"type":"move_clip","clip":2,"start":0}"#);
    let p = project(&ev);
    assert_eq!((p.nodes[1].gain, p.nodes[1].pan, p.nodes[2].mute), (0.2, 0.5, true));
    host.command_json(r#"{"type":"seek","pos":96000}"#);
    host.command_json(r#"{"type":"transport","action":"play"}"#);
    run(&mut engine, 40);
    let Event::Meters { levels } = host.tick().remove(1) else { panic!() };
    assert!(levels["1"][0] > 0.0 && levels["1"][0] < 0.5, "batterie à 0,2 : {:?}", levels["1"]);
    assert_eq!(levels["2"], [0.0, 0.0], "basse toujours muette après l'édition");
    // Les crêtes d'un clip déplacé suivent son contenu.
    host.command_json(r#"{"type":"move_clip","clip":0,"start":96000}"#);
    let peaks = |host: &mut Host| match host.command(Command::GetPeaks { clip: 0, from: 96_000, to: 192_000, columns: 50 }).remove(0) {
        Event::Peaks { columns, .. } => columns,
        e => panic!("{e:?}"),
    };
    let moved = peaks(&mut host);
    host.command_json(r#"{"type":"undo"}"#);
    let original = peaks(&mut host);
    assert_ne!(moved, original, "le clip n'est plus au même endroit : les crêtes de cette fenêtre diffèrent (ou sont décalées)");
}

// ---- clips entre pistes, clips de notes, fondus enchaînés automatiques -----------------------------------------------

fn node_of(p: &daw_ui::protocol::ProjectInfo, clip: u64) -> Option<u32> {
    p.nodes.iter().find(|n| n.clips.iter().any(|c| matches!(c, daw_ui::protocol::ClipInfo::Audio { id, .. } | daw_ui::protocol::ClipInfo::Midi { id, .. } if *id == clip))).map(|n| n.id)
}

#[test]
fn clips_move_between_tracks_and_back_in_one_step() {
    let (_, mut host) = setup();
    // Beat A (id 0) de Batterie (nœud 1) vers Basse (nœud 2), puis l'arpège (clip de notes) de Claviers (3) vers Voix (5).
    let ev = host.command_json(r#"{"type":"move_clip","clip":0,"start":10000,"node":2}"#);
    assert_eq!((node_of(project(&ev), 0), audio(clip_of(project(&ev), 0).unwrap()).0), (Some(2), 10_000));
    assert_eq!(history(&ev).2.as_deref(), Some("Déplacer un clip"));
    let ev = host.command_json(r#"{"type":"move_clip","clip":1000000,"start":384000,"node":5}"#);
    assert_eq!(node_of(project(&ev), 1_000_000), Some(5));
    // Un clip de notes déplacé garde ses 64 notes ; ses notes sont désormais jouées par la piste Voix.
    match clip_of(project(&ev), 1_000_000).unwrap() {
        daw_ui::protocol::ClipInfo::Midi { notes, .. } => assert_eq!(notes.len(), 64),
        c => panic!("{c:?}"),
    }
    let ev = host.command_json(r#"{"type":"undo"}"#);
    assert_eq!(node_of(project(&ev), 1_000_000), Some(3));
    let ev = host.command_json(r#"{"type":"undo"}"#);
    assert_eq!((node_of(project(&ev), 0), audio(clip_of(project(&ev), 0).unwrap()).0), (Some(1), 0));
    // Destination invalide : la sortie principale (0), un nœud inconnu.
    let err = |ev: Vec<Event>| match ev.into_iter().next() {
        Some(Event::Error { message, .. }) => message,
        e => panic!("{e:?}"),
    };
    assert!(err(host.command_json(r#"{"type":"move_clip","clip":0,"start":0,"node":0}"#)).contains("pas une piste"));
    assert!(err(host.command_json(r#"{"type":"move_clip","clip":0,"start":0,"node":99}"#)).contains("inconnu"));
    assert_eq!(history(&host.command_json(r#"{"type":"get_project"}"#)).0, false, "rien n'est inscrit");
}

#[test]
fn overlapping_clips_get_an_automatic_crossfade_in_the_same_undo_step() {
    let (_, mut host) = setup();
    // Beat A : 0 → 768 000 ; Beat B : 768 000 → 1 536 000 (fondu de sortie préexistant de 24 000). On remonte Beat B de 96 000.
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(project(&ev).auto_crossfade, true);
    let b_before = audio(clip_of(project(&ev), 1).unwrap());
    let ev = host.command_json(r#"{"type":"move_clip","clip":1,"start":672000}"#);
    let a = audio(clip_of(project(&ev), 0).unwrap());
    let b = audio(clip_of(project(&ev), 1).unwrap());
    // Recouvrement : de 672 000 à 768 000 = 96 000 échantillons.
    assert_eq!((a.4, b.3), (96_000, 96_000), "fondu de sortie de A et d'entrée de B = la zone commune");
    assert_eq!(b.0, 672_000);
    assert_eq!(history(&ev).2.as_deref(), Some("Déplacer un clip"));
    // Rogner A pour changer le recouvrement : les fondus suivent.
    let ev = host.command_json(r#"{"type":"trim_clip","clip":0,"end":720000}"#);
    let (a, b) = (audio(clip_of(project(&ev), 0).unwrap()), audio(clip_of(project(&ev), 1).unwrap()));
    assert_eq!((a.4, b.3), (48_000, 48_000));
    // Un seul pas pour chaque édition : deux annulations ramènent à l'état d'origine, fondus compris.
    host.command_json(r#"{"type":"undo"}"#);
    let ev = host.command_json(r#"{"type":"undo"}"#);
    assert_eq!(audio(clip_of(project(&ev), 1).unwrap()), b_before);
    assert_eq!(audio(clip_of(project(&ev), 0).unwrap()), (0, 768_000, 0, 0, 0));
    // Désactivé : aucun fondu automatique.
    let ev = host.command_json(r#"{"type":"set_auto_crossfade","enabled":false}"#);
    assert_eq!(project(&ev).auto_crossfade, false);
    let ev = host.command_json(r#"{"type":"move_clip","clip":1,"start":672000}"#);
    assert_eq!(audio(clip_of(project(&ev), 0).unwrap()).4, 0);
    assert_eq!(audio(clip_of(project(&ev), 1).unwrap()).3, 0);
}

#[test]
fn riff_crossfade_of_the_demo_is_left_alone_and_unrelated_clips_are_not_touched() {
    let (_, mut host) = setup();
    // Riff 1 (id 3) et Riff 2 (id 4) se chevauchent déjà avec des fondus de 48 000 qui correspondent : déplacer un autre clip
    // de la même piste ne doit rien changer.
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    let (r1, r2) = (audio(clip_of(project(&ev), 3).unwrap()), audio(clip_of(project(&ev), 4).unwrap()));
    assert_eq!((r1.4, r2.3), (48_000, 48_000));
    let ev = host.command_json(r#"{"type":"move_clip","clip":5,"start":1100000,"node":4}"#); // Couplet sur Guitare, après Riff 2 (fin à 960 000) : sans recouvrement
    assert_eq!(audio(clip_of(project(&ev), 3).unwrap()), r1);
    assert_eq!(audio(clip_of(project(&ev), 4).unwrap()), r2);
}

#[test]
fn notes_clips_can_be_trimmed_split_and_faded() {
    let (_, mut host) = setup();
    let notes_of = |ev: &[Event], id: u64| match clip_of(project(ev), id).unwrap() {
        daw_ui::protocol::ClipInfo::Midi { notes, start, len, offset, fade_in, fade_out, .. } => (notes.len(), *start, *len, *offset, *fade_in, *fade_out),
        c => panic!("{c:?}"),
    };
    // L'arpège : 64 croches, début à 384 000, 8 mesures = 768 000. Une croche = 12 000.
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(notes_of(&ev, 1_000_000), (64, 384_000, 768_000, 0, 0, 0));
    // Rogner la fin à 480 000 (un huitième de mesure... 96 000 = 8 croches) : 8 notes restent visibles.
    let ev = host.command_json(r#"{"type":"trim_clip","clip":1000000,"end":480000}"#);
    assert_eq!(notes_of(&ev, 1_000_000), (8, 384_000, 96_000, 0, 0, 0));
    // Rogner le début à 408 000 (2 croches plus tard) : 6 notes, le décalage de la fenêtre est de 24 000.
    let ev = host.command_json(r#"{"type":"trim_clip","clip":1000000,"start":408000}"#);
    let (n, start, len, offset, _, _) = notes_of(&ev, 1_000_000);
    assert_eq!((n, start, len), (6, 408_000, 72_000));
    assert!((offset as i64 - 24_000).abs() <= 30, "décalage ≈ 24 000 : {offset}");
    // Étendre à droite au-delà : tout est de nouveau visible (rien n'a été détruit).
    let ev = host.command_json(r#"{"type":"trim_clip","clip":1000000,"start":384000,"end":1152000}"#);
    assert_eq!(notes_of(&ev, 1_000_000).0, 64);
    // Découpe à 480 000 : deux clips, 8 notes à gauche, 56 à droite, nom conservé.
    let ev = host.command_json(r#"{"type":"split_clip","clip":1000000,"at":480000}"#);
    let p = project(&ev);
    let midi: Vec<_> = p.nodes[3].clips.iter().filter_map(|c| if let daw_ui::protocol::ClipInfo::Midi { id, name, notes, .. } = c { Some((*id, name.clone(), notes.len())) } else { None }).collect();
    assert_eq!(midi.len(), 2);
    assert_eq!((midi[0].0, midi[0].2), (1_000_000, 8));
    assert!(midi[1].0 > 1_000_000 && midi[1].2 == 56 && midi[1].1 == "Arpège");
    assert_eq!(history(&ev).2.as_deref(), Some("Découper"));
    host.command_json(r#"{"type":"undo"}"#);
    // Fondus de vélocité : 48 000 échantillons d'entrée (4 croches) puis annulation.
    let ev = host.command_json(r#"{"type":"set_clip_fades","clip":1000000,"fade_in":48000,"fade_out":24000}"#);
    let (_, _, _, _, fi, fo) = notes_of(&ev, 1_000_000);
    assert!((fi as i64 - 48_000).abs() <= 30 && (fo as i64 - 24_000).abs() <= 30, "{fi} {fo}");
    // Le moteur reçoit des vélocités atténuées pour les premières notes : on le voit dans les notes du projet.
    match clip_of(project(&ev), 1_000_000).unwrap() {
        daw_ui::protocol::ClipInfo::Midi { notes, .. } => {
            assert!(notes[0].velocity < notes[8].velocity, "la première note est plus douce que celle du milieu : {} < {}", notes[0].velocity, notes[8].velocity);
        }
        c => panic!("{c:?}"),
    }
    // Refus : rogner en dehors, découper hors du clip.
    let err = |ev: Vec<Event>| match ev.into_iter().next() {
        Some(Event::Error { message, .. }) => message,
        e => panic!("{e:?}"),
    };
    assert!(err(host.command_json(r#"{"type":"split_clip","clip":1000000,"at":10}"#)).contains("hors du clip"));
    assert!(err(host.command_json(r#"{"type":"trim_clip","clip":1000000,"start":99999999}"#)).contains("hors du clip"));
    // Rogner avant le début de la matière : refusé (rien avant la matière).
    assert!(err(host.command_json(r#"{"type":"trim_clip","clip":1000000,"start":100}"#)).contains("hors du clip"));
}

// ---- courbes de fondu et éditeur de fondu enchaîné -------------------------------------------------------------------

fn curves(p: &daw_ui::protocol::ProjectInfo, id: u64) -> (daw_ui::protocol::CurveSpec, daw_ui::protocol::CurveSpec) {
    match clip_of(p, id).unwrap() {
        daw_ui::protocol::ClipInfo::Audio { fade_in_curve, fade_out_curve, .. } => (fade_in_curve.clone(), fade_out_curve.clone()),
        c => panic!("{c:?}"),
    }
}
fn curve(shape: &str, bend: i16, smooth: u16) -> daw_ui::protocol::CurveSpec {
    daw_ui::protocol::CurveSpec { shape: shape.into(), bend, smooth }
}

#[test]
fn fade_curves_are_editable_for_one_fade_and_for_a_crossfade_in_one_step() {
    let (_, mut host) = setup();
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(curves(project(&ev), 0), (curve("linear", 0, 0), curve("linear", 0, 0)));
    // Un seul fondu : courbe seule (la longueur reste), puis longueur et courbe ensemble.
    let ev = host.command_json(r#"{"type":"set_clip_fades","clip":0,"curve_in":{"shape":"linear","bend":0,"smooth":1000}}"#);
    assert_eq!(curves(project(&ev), 0).0, curve("linear", 0, 1000));
    assert_eq!(audio(clip_of(project(&ev), 0).unwrap()).3, 0, "la longueur du fondu n'a pas changé");
    let ev = host.command_json(r#"{"type":"set_clip_fades","clip":0,"fade_out":5000,"curve_out":{"shape":"equal_power","bend":-350}}"#);
    assert_eq!((audio(clip_of(project(&ev), 0).unwrap()).4, curves(project(&ev), 0).1), (5_000, curve("equal_power", -350, 0)), "smooth absent = 0");
    // Une longueur seule garde la courbe.
    let ev = host.command_json(r#"{"type":"set_clip_fades","clip":0,"fade_out":6000}"#);
    assert_eq!((audio(clip_of(project(&ev), 0).unwrap()).4, curves(project(&ev), 0).1), (6_000, curve("equal_power", -350, 0)));
    host.command_json(r#"{"type":"undo"}"#);
    host.command_json(r#"{"type":"undo"}"#);
    host.command_json(r#"{"type":"undo"}"#);
    // Fondu enchaîné automatique (puissance constante par défaut), puis édition des deux courbes d'un coup.
    let ev = host.command_json(r#"{"type":"move_clip","clip":1,"start":672000}"#);
    assert_eq!((curves(project(&ev), 0).1, curves(project(&ev), 1).0), (curve("equal_power", 0, 0), curve("equal_power", 0, 0)));
    let ev = host.command_json(r#"{"type":"set_crossfade","clip_out":0,"clip_in":1,"curve_out":{"shape":"linear","bend":800,"smooth":200},"curve_in":{"shape":"linear","bend":-800,"smooth":200}}"#);
    assert_eq!((curves(project(&ev), 0).1, curves(project(&ev), 1).0), (curve("linear", 800, 200), curve("linear", -800, 200)));
    assert_eq!(history(&ev).2.as_deref(), Some("Courbe du fondu enchaîné"));
    assert_eq!((audio(clip_of(project(&ev), 0).unwrap()).4, audio(clip_of(project(&ev), 1).unwrap()).3), (96_000, 96_000), "longueurs inchangées");
    // Les courbes choisies survivent à un nouveau déplacement (seules les longueurs sont recalculées).
    let ev = host.command_json(r#"{"type":"move_clip","clip":1,"start":720000}"#);
    assert_eq!((curves(project(&ev), 0).1, curves(project(&ev), 1).0), (curve("linear", 800, 200), curve("linear", -800, 200)));
    assert_eq!((audio(clip_of(project(&ev), 0).unwrap()).4, audio(clip_of(project(&ev), 1).unwrap()).3), (48_000, 48_000));
    // Une seule annulation défait le choix de courbes... ici celle du déplacement, puis celle des courbes.
    host.command_json(r#"{"type":"undo"}"#);
    let ev = host.command_json(r#"{"type":"undo"}"#);
    assert_eq!((curves(project(&ev), 0).1, curves(project(&ev), 1).0), (curve("equal_power", 0, 0), curve("equal_power", 0, 0)));
}

#[test]
fn bad_curves_and_crossfade_requests_are_refused_without_trace() {
    let (_, mut host) = setup();
    let err = |ev: Vec<Event>| match ev.into_iter().next() {
        Some(Event::Error { message, .. }) => message,
        e => panic!("{e:?}"),
    };
    assert!(err(host.command_json(r#"{"type":"set_clip_fades","clip":0,"curve_in":{"shape":"zigzag"}}"#)).contains("forme de fondu inconnue"));
    assert!(err(host.command_json(r#"{"type":"set_clip_fades","clip":0,"curve_in":{"shape":"linear","bend":1500}}"#)).contains("hors plage"));
    assert!(err(host.command_json(r#"{"type":"set_clip_fades","clip":0,"curve_in":{"shape":"linear","smooth":1001}}"#)).contains("hors plage"));
    assert!(err(host.command_json(r#"{"type":"set_crossfade","clip_out":0,"clip_in":1,"curve_out":{"shape":"zigzag"}}"#)).contains("forme de fondu inconnue"));
    assert!(err(host.command_json(r#"{"type":"set_crossfade","clip_out":0,"clip_in":0,"curve_out":{"shape":"linear"}}"#)).contains("deux clips différents"));
    assert!(err(host.command_json(r#"{"type":"set_crossfade","clip_out":0,"clip_in":1}"#)).contains("curve_out"));
    assert!(err(host.command_json(r#"{"type":"set_crossfade","clip_out":0,"clip_in":1000000,"curve_out":{"shape":"linear"}}"#)).contains("deux clips audio"));
    assert!(err(host.command_json(r#"{"type":"set_clip_fades","clip":1000000,"curve_in":{"shape":"linear"}}"#)).contains("linéaires"), "pas de courbe pour un clip de notes");
    assert_eq!(history(&host.command_json(r#"{"type":"get_project"}"#)).0, false);
}

#[test]
fn crossfade_length_is_set_around_an_anchor_in_one_undo_step() {
    let (_, mut host) = setup();
    let ev = host.command_json(r#"{"type":"get_project"}"#);
    let (a0, b0) = (audio(clip_of(project(&ev), 3).unwrap()), audio(clip_of(project(&ev), 4).unwrap()));
    let overlap = a0.4;
    assert!(overlap > 0 && a0.0 + a0.1 - b0.0 == overlap, "Riff 1 / Riff 2 forment un fondu enchaîné");
    let new_len = overlap / 2;
    let ev = host.command_json(&format!(r#"{{"type":"set_crossfade_length","clip_out":3,"clip_in":4,"length":{new_len},"anchor":"center"}}"#));
    let (a, b) = (audio(clip_of(project(&ev), 3).unwrap()), audio(clip_of(project(&ev), 4).unwrap()));
    assert_eq!((a.4, b.3), (new_len, new_len));
    assert_eq!(a.0 + a.1 - b.0, new_len, "zone commune = nouvelle longueur");
    assert_eq!((a.0, b.0 + b.1), (a0.0, b0.0 + b0.1), "début de a et fin de b ne bougent pas");
    // Le contenu reste en place : début − décalage ne change pas.
    assert_eq!(b.0 as i64 - b.2 as i64, b0.0 as i64 - b0.2 as i64);
    assert_eq!(history(&ev).2.as_deref(), Some("Longueur du fondu enchaîné"));
    // Début fixe : seule la fin de a bouge (il lui reste 12 000 échantillons de matière : 48 000 serait refusé).
    let longer = new_len + 12_000;
    let ev = host.command_json(&format!(r#"{{"type":"set_crossfade_length","clip_out":3,"clip_in":4,"length":{longer},"anchor":"start"}}"#));
    let (a2, b2) = (audio(clip_of(project(&ev), 3).unwrap()), audio(clip_of(project(&ev), 4).unwrap()));
    assert_eq!(b2.0, b.0);
    assert_eq!(a2.0 + a2.1 - b2.0, longer);
    // Deux annulations : état d'origine.
    host.command_json(r#"{"type":"undo"}"#);
    let ev = host.command_json(r#"{"type":"undo"}"#);
    assert_eq!((audio(clip_of(project(&ev), 3).unwrap()), audio(clip_of(project(&ev), 4).unwrap())), (a0, b0));
}

#[test]
fn bad_crossfade_length_requests_are_refused_without_trace() {
    let (_, mut host) = setup();
    let err = |ev: Vec<Event>| match ev.into_iter().next() {
        Some(Event::Error { message, .. }) => message,
        e => panic!("{e:?}"),
    };
    assert!(err(host.command_json(r#"{"type":"set_crossfade_length","clip_out":3,"clip_in":3,"length":10,"anchor":"center"}"#)).contains("deux clips différents"));
    assert!(err(host.command_json(r#"{"type":"set_crossfade_length","clip_out":3,"clip_in":4,"length":0,"anchor":"center"}"#)).contains("length"));
    assert!(err(host.command_json(r#"{"type":"set_crossfade_length","clip_out":3,"clip_in":4,"length":10,"anchor":"left"}"#)).contains("anchor"));
    assert!(err(host.command_json(&format!(r#"{{"type":"set_crossfade_length","clip_out":3,"clip_in":4,"length":{},"anchor":"start"}}"#, 48_000 + 12_001))).contains("hors du clip"), "plus que la matière disponible");
    assert!(err(host.command_json(r#"{"type":"set_crossfade_length","clip_out":3,"clip_in":1000000,"length":10,"anchor":"end"}"#)).contains("deux clips audio"));
    assert!(err(host.command_json(r#"{"type":"set_crossfade_length","clip_out":3,"clip_in":4,"length":999999999,"anchor":"center"}"#)).contains("hors du clip"));
    assert!(err(host.command_json(r#"{"type":"set_crossfade_length","clip_out":0,"clip_in":2,"length":10,"anchor":"center"}"#)).contains("chevauchent"));
    assert_eq!(history(&host.command_json(r#"{"type":"get_project"}"#)).0, false);
}

fn position(ev: &[Event]) -> (u64, bool, Option<(u64, u64)>) {
    ev.iter().rev().find_map(|e| if let Event::Position { pos, playing, loop_range } = e { Some((*pos, *playing, loop_range.map(|l| (l.start, l.end)))) } else { None }).expect("événement position")
}
fn soloed(ev: &[Event]) -> Vec<u32> {
    // Le dernier événement `project` fait foi : une édition qui met fin à l'aperçu en envoie un second, après la restauration.
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project").nodes.iter().filter(|n| n.solo).map(|n| n.id).collect()
}
fn preview_flags(ev: &[Event]) -> Vec<bool> {
    ev.iter().filter_map(|e| if let Event::Preview { active } = e { Some(*active) } else { None }).collect()
}

const PREVIEW: &str = r#"{"type":"preview_fade","clip_out":3,"clip_in":4,"margin_ms":1500,"solo":true}"#;

#[test]
fn preview_loops_the_fade_zone_with_margins_and_solos_its_track() {
    let (mut engine, mut host) = setup();
    // Riff 1 / Riff 2 : zone commune [576 000 ; 624 000) ; marge 1,5 s = 72 000 échantillons.
    let ev = host.command_json(PREVIEW);
    assert_eq!(preview_flags(&ev), vec![true]);
    let (_, playing, lp) = position(&ev);
    assert!(playing);
    assert_eq!(lp, Some((504_000, 696_000)));
    assert_eq!(soloed(&ev), vec![4], "la piste du fondu (Guitare) est seule");
    // Le moteur joue en boucle : la position reste dans la zone, même après plusieurs tours.
    run(&mut engine, 3_000);
    let (pos, playing, _) = position(&host.tick());
    assert!(playing && (504_000..696_000).contains(&pos), "position {pos}");
}

#[test]
fn preview_follows_edits_and_stop_gives_everything_back() {
    let (mut engine, mut host) = setup();
    // État de l'utilisateur : boucle à lui, solo sur la basse, position 1 000, arrêté.
    host.command_json(r#"{"type":"loop","start":0,"end":96000}"#);
    host.command_json(r#"{"type":"set_node","node":2,"solo":true}"#);
    host.command_json(r#"{"type":"seek","pos":1000}"#);
    run(&mut engine, 1);
    host.command_json(PREVIEW);
    // Une édition du fondu pendant l'aperçu : la boucle suit, le solo de l'aperçu est conservé.
    let ev = host.command_json(r#"{"type":"set_crossfade_length","clip_out":3,"clip_in":4,"length":12000,"anchor":"center"}"#);
    assert_eq!(soloed(&ev), vec![4]);
    let (_, playing, lp) = position(&ev);
    assert!(playing);
    // Centré sur 600 000 : [594 000 ; 606 000) ± 72 000.
    assert_eq!(lp, Some((522_000, 678_000)));
    // Arrêt : tout est rendu.
    let ev = host.command_json(r#"{"type":"stop_preview"}"#);
    assert_eq!(preview_flags(&ev), vec![false]);
    assert_eq!(soloed(&ev), vec![2], "le solo de l'utilisateur est revenu");
    let (pos, playing, lp) = position(&ev);
    assert_eq!((playing, lp), (false, Some((0, 96_000))));
    assert_eq!(pos, 1_000);
    // Un second arrêt est sans effet.
    assert_eq!(preview_flags(&host.command_json(r#"{"type":"stop_preview"}"#)), vec![false]);
}

#[test]
fn taking_the_transport_ends_the_preview_without_taking_the_position_back() {
    let (_, mut host) = setup();
    host.command_json(r#"{"type":"set_node","node":2,"solo":true}"#);
    host.command_json(PREVIEW);
    // Espace pendant l'aperçu : il s'arrête, la boucle et les solos de l'utilisateur reviennent, la position non.
    let ev = host.command_json(r#"{"type":"transport","action":"toggle"}"#);
    assert_eq!(preview_flags(&ev), vec![false]);
    assert_eq!(soloed(&ev), vec![2]);
    let (_, playing, lp) = position(&ev);
    assert_eq!((playing, lp), (false, None));
    // Une boucle posée par l'utilisateur pendant l'aperçu est la sienne : elle n'est pas écrasée.
    host.command_json(PREVIEW);
    let ev = host.command_json(r#"{"type":"loop","start":1000,"end":5000}"#);
    assert_eq!(preview_flags(&ev), vec![false]);
    assert_eq!(position(&ev).2, Some((1_000, 5_000)));
    // Déplacer la tête de lecture : même principe.
    host.command_json(PREVIEW);
    let ev = host.command_json(r#"{"type":"seek","pos":777}"#);
    assert_eq!(preview_flags(&ev), vec![false]);
    assert_eq!(position(&ev).0, 777);
}

#[test]
fn single_fade_preview_and_refusals_leave_no_trace() {
    let (_, mut host) = setup();
    // Fondu de sortie seul de Beat B (24 000 échantillons, de 1 512 000 à 1 536 000).
    let ev = host.command_json(r#"{"type":"preview_fade","clip":1,"side":"out","margin_ms":500,"solo":false}"#);
    assert_eq!(position(&ev).2, Some((1_512_000 - 24_000, 1_536_000 + 24_000)));
    assert!(soloed(&ev).is_empty(), "sans « piste seule », aucun solo");
    host.command_json(r#"{"type":"stop_preview"}"#);
    let err = |ev: Vec<Event>| ev.iter().find_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).expect("une erreur");
    // Pas de fondu d'entrée sur Beat A : refus, et plus rien ne reste réglé.
    let ev = host.command_json(r#"{"type":"preview_fade","clip":0,"side":"in","margin_ms":500,"solo":true}"#);
    assert!(err(ev.clone()).contains("pas de fondu"));
    assert!(soloed(&ev).is_empty());
    assert_eq!(position(&ev).2, None);
    assert!(err(host.command_json(r#"{"type":"preview_fade","margin_ms":0,"solo":true}"#)).contains("clip_out"));
    assert!(err(host.command_json(r#"{"type":"preview_fade","clip_out":3,"clip_in":3,"margin_ms":0,"solo":true}"#)).contains("deux clips différents"));
    assert!(err(host.command_json(r#"{"type":"preview_fade","clip":1,"side":"middle","margin_ms":0,"solo":true}"#)).contains("side"));
    assert!(err(host.command_json(r#"{"type":"preview_fade","clip":1,"side":"in","margin_ms":10001,"solo":true}"#)).contains("margin_ms"));
    assert!(err(host.command_json(r#"{"type":"preview_fade","clip_out":3,"clip_in":1000000,"margin_ms":0,"solo":true}"#)).contains("deux clips audio"));
}

#[test]
fn preview_ends_by_itself_when_the_fade_disappears() {
    let (_, mut host) = setup();
    // Fondu enchaîné créé par un déplacement, écouté, puis annulé : la zone n'existe plus.
    host.command_json(r#"{"type":"move_clip","clip":1,"start":672000}"#);
    let ev = host.command_json(r#"{"type":"preview_fade","clip_out":0,"clip_in":1,"margin_ms":0,"solo":true}"#);
    assert_eq!(preview_flags(&ev), vec![true]);
    let ev = host.command_json(r#"{"type":"undo"}"#);
    assert_eq!(preview_flags(&ev), vec![false]);
    assert!(soloed(&ev).is_empty());
    assert_eq!(position(&ev).2, None);
}

#[test]
fn panic_is_accepted_and_changes_neither_transport_nor_position() {
    let (mut engine, mut host) = setup();
    host.command_json(r#"{"type":"seek","pos":5000}"#);
    host.command_json(r#"{"type":"transport","action":"play"}"#);
    run(&mut engine, 4);
    let ev = host.command_json(r#"{"type":"panic"}"#);
    assert!(ev.iter().all(|e| !matches!(e, Event::Error { .. })));
    let (_, playing, _) = position(&ev);
    assert!(playing, "la lecture continue");
    run(&mut engine, 40);
    let (pos, playing, _) = position(&host.tick());
    assert!(playing && pos > 5_000, "la position continue d'avancer ({pos})");
}

#[test]
fn peaks_also_cover_the_material_hidden_by_a_trim() {
    let (_, mut host) = setup();
    let get = |host: &mut Host, from: u64, to: u64| match host.command(Command::GetPeaks { clip: 0, from, to, columns: 50 }).remove(0) {
        Event::Peaks { columns, .. } => columns,
        e => panic!("{e:?}"),
    };
    let visible = get(&mut host, 600_000, 700_000);
    // Beat A : données de 0 à 768 000 ; on rogne la fin à 600 000 : la matière de 600 000 à 768 000 est cachée mais existe.
    host.command_json(r#"{"type":"trim_clip","clip":0,"end":600000}"#);
    let hidden = get(&mut host, 600_000, 700_000);
    assert_eq!(visible, hidden, "mêmes crêtes avant et après le rognage : c'est la matière du fichier");
    assert!(hidden.iter().any(|[lo, hi]| hi - lo > 0.2));
    // Au-delà des données du fichier : silence.
    assert!(get(&mut host, 800_000, 900_000).iter().all(|c| *c == [0.0, 0.0]));
}
