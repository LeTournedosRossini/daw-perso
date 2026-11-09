//! Calage d'une prise sur une autre par le protocole (bibliothèque `daw-align`, voix de référence de Neptune) : une prise de plus dans le
//! groupe de prises du double, une seule action annulable, fiche de réglages par prise, refaire le rendu, enregistrement / réouverture, annulation.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::protocol::{ClipInfo, NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 44_100;

fn reference(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../align/tests/ref").join(name)
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-align-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn state(h: &mut Host) -> ProjectInfo {
    project(&h.command_json(r#"{"type":"get_project"}"#)).clone()
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

/// Fait tourner l'hôte jusqu'à ce que `done` reconnaisse un événement (délai : 3 minutes), rend tous les événements vus.
fn settle(h: &mut Host, done: impl Fn(&Event) -> bool) -> Vec<Event> {
    let mut all = Vec::new();
    let t0 = Instant::now();
    loop {
        let ev = h.tick();
        let finished = ev.iter().any(&done);
        all.extend(ev);
        if finished {
            return all;
        }
        assert!(t0.elapsed() < Duration::from_secs(180), "délai dépassé : {all:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn audio_clips(n: &NodeInfo) -> Vec<(u64, String, u64, u64, bool, u32, bool)> {
    n.clips.iter().filter_map(|c| if let ClipInfo::Audio { id, name, start, len, muted, take_group, aligned, .. } = c { Some((*id, name.clone(), *start, *len, *muted, *take_group, *aligned)) } else { None }).collect()
}

/// Projet enregistré où `guide.wav` et `double.wav` sont importés sur deux pistes ; rend (moteur, hôte, dossier, id du guide, id du double).
fn setup(name: &str) -> (Engine, Host, PathBuf, u64, u64) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    let dir = temp(name);
    h.command_json(r#"{"type":"new_project"}"#);
    h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    for f in ["guide.wav", "double.wav"] {
        h.command_json(&format!(r#"{{"type":"import_audio","files":[{:?}],"pos":0}}"#, reference(f).to_string_lossy()));
        settle(&mut h, |e| matches!(e, Event::Import { active: false, .. }));
    }
    let p = state(&mut h);
    let id_of = |track: &str| {
        let n = p.nodes.iter().find(|n| n.name == track).unwrap_or_else(|| panic!("piste {track} : {:?}", p.nodes.iter().map(|n| &n.name).collect::<Vec<_>>()));
        audio_clips(n)[0].0
    };
    let (g, d) = (id_of("guide"), id_of("double"));
    (engine, h, dir, g, d)
}

fn onsets() -> Vec<(f64, f64)> {
    std::fs::read_to_string(reference("onsets.txt"))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut it = l.split_whitespace().map(|x| x.parse::<f64>().unwrap());
            (it.next().unwrap(), it.next().unwrap())
        })
        .collect()
}

fn aligned_clip(ev: &[Event]) -> u64 {
    ev.iter().find_map(|e| if let Event::Align { clips, .. } = e { clips.first().map(|c| c.clip) } else { None }).expect("prise calée")
}

#[test]
fn aligning_adds_one_take_to_the_group_in_one_undo_step_and_the_path_matches_the_known_onsets() {
    let (_engine, mut h, _dir, guide, double) = setup("full");
    let ev = h.command_json(&format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{double}]}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    assert!(ev.iter().any(|e| matches!(e, Event::Align { active: true, total: 1, .. })));
    let ev = settle(&mut h, |e| matches!(e, Event::Align { active: false, .. }));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let Some(Event::Align { progress, done, clips, warnings, cancelled, .. }) = ev.iter().rev().find(|e| matches!(e, Event::Align { active: false, .. })) else { unreachable!() };
    assert!(!*cancelled && warnings.is_empty() && *done == 1 && (*progress - 1.0).abs() < 1e-9, "{warnings:?}");
    assert_eq!(clips.len(), 1);
    assert!(clips[0].name.contains("calé sur"), "{:?}", clips[0].name);

    // L'original est dessous (muet, même groupe) ; la prise calée est active et couvre la fenêtre du guide.
    let p = state(&mut h);
    let track = p.nodes.iter().find(|n| n.name == "double").unwrap();
    let list = audio_clips(track);
    let new = list.iter().find(|c| c.6).expect("prise calée");
    let old = list.iter().find(|c| !c.6).expect("original");
    assert!(!new.4 && old.4, "la nouvelle est active, l'original muet : {list:?}");
    assert!(new.5 != 0 && new.5 == old.5, "même groupe de prises : {list:?}");
    let g = audio_clips(p.nodes.iter().find(|n| n.name == "guide").unwrap())[0].clone();
    assert_eq!((new.2, new.3), (g.2, g.3), "la prise calée a la durée et la place du guide");

    // Une seule action annulable.
    let hist = h.command_json(r#"{"type":"get_project"}"#);
    assert!(hist.iter().any(|e| matches!(e, Event::History { undo_label: Some(l), .. } if l == "Caler des prises")), "{hist:?}");
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(audio_clips(project(&ev).nodes.iter().find(|n| n.name == "double").unwrap()).len(), 1, "annulé en un pas");
    let ev = h.command_json(r#"{"type":"redo"}"#);
    assert_eq!(audio_clips(project(&ev).nodes.iter().find(|n| n.name == "double").unwrap()).len(), 2);

    // Détails : le chemin retrouve les débuts de notes connus.
    let clip = new.0;
    let ev = h.command_json(&format!(r#"{{"type":"get_align","clip":{clip}}}"#));
    let Some(Event::AlignDetail { hop, path, shift, consonants, settings, window_start, .. }) = ev.iter().find(|e| matches!(e, Event::AlignDetail { .. })) else { panic!("{ev:?}") };
    assert_eq!(*window_start, g.2);
    assert_eq!(path.len(), shift.len());
    assert!(settings.contains("flibuste-align-settings") && !consonants.is_empty());
    let errs: Vec<f64> = onsets().iter().map(|&(tg, td)| (path[((tg / hop).round() as usize).min(path.len() - 1)] - td).abs() * 1000.0).collect();
    let (mean, worst) = (errs.iter().sum::<f64>() / errs.len() as f64, errs.iter().fold(0.0f64, |m, &e| m.max(e)));
    assert!(mean < 15.0 && worst < 45.0, "moyenne {mean:.1} ms, pire {worst:.1} ms");
    // Un clip qui n'est pas une prise calée n'a pas de fiche.
    assert!(!errors(&h.command_json(&format!(r#"{{"type":"get_align","clip":{}}}"#, old.0))).is_empty());
}

#[test]
fn redoing_with_other_settings_adds_another_take_and_the_files_survive_save_and_reopen() {
    let (_engine, mut h, dir, guide, double) = setup("redo");
    h.command_json(&format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{double}]}}"#));
    let ev = settle(&mut h, |e| matches!(e, Event::Align { active: false, .. }));
    let first = aligned_clip(&ev);

    // Réglages : force à 0 (le double reste tel quel) ; seul le rendu est refait, l'analyse enregistrée sert.
    let detail = h.command_json(&format!(r#"{{"type":"get_align","clip":{first}}}"#));
    let Some(Event::AlignDetail { settings, .. }) = detail.iter().find(|e| matches!(e, Event::AlignDetail { .. })) else { panic!() };
    let mut v: serde_json::Value = serde_json::from_str(settings).unwrap();
    v["general"]["strength"] = serde_json::json!(0.0);
    let tweaked = serde_json::to_string(&v).unwrap();
    let cmd = serde_json::json!({"type": "align_redo", "clips": [first], "settings": tweaked});
    let t0 = Instant::now();
    let ev = h.command_json(&cmd.to_string());
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let ev = settle(&mut h, |e| matches!(e, Event::Align { active: false, .. }));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    println!("rendu refait en {:?}", t0.elapsed());
    let second = aligned_clip(&ev);
    assert_ne!(first, second);
    let p = state(&mut h);
    let list = audio_clips(p.nodes.iter().find(|n| n.name == "double").unwrap());
    assert_eq!(list.iter().filter(|c| c.6).count(), 2, "deux prises calées maintenant : {list:?}");
    assert_eq!(list.iter().filter(|c| !c.4).count(), 1, "une seule prise active");

    // Enregistrer puis rouvrir : les fiches reviennent et servent encore.
    let ev = h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    h.command_json(r#"{"type":"new_project"}"#);
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let p = state(&mut h);
    let list = audio_clips(p.nodes.iter().find(|n| n.name == "double").unwrap());
    assert_eq!(list.iter().filter(|c| c.6).count(), 2, "les prises calées sont reconnues après la réouverture : {list:?}");
    let any = list.iter().find(|c| c.6).unwrap().0;
    let ev = h.command_json(&format!(r#"{{"type":"get_align","clip":{any}}}"#));
    assert!(ev.iter().any(|e| matches!(e, Event::AlignDetail { path, .. } if !path.is_empty())), "{ev:?}");
    // Et on peut encore refaire le rendu d'une prise rouverte.
    h.command_json(&format!(r#"{{"type":"align_redo","clips":[{any}]}}"#));
    let ev = settle(&mut h, |e| matches!(e, Event::Align { active: false, .. }));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
}

#[test]
fn cancelling_leaves_nothing_and_bad_requests_are_refused_without_trace() {
    let (_engine, mut h, _dir, guide, double) = setup("cancel");
    let before = state(&mut h);
    h.command_json(&format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{double}]}}"#));
    assert!(!errors(&h.command_json(&format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{double}]}}"#))).is_empty(), "un seul calage à la fois");
    h.command_json(r#"{"type":"cancel_align"}"#);
    let ev = settle(&mut h, |e| matches!(e, Event::Align { active: false, .. }));
    assert!(ev.iter().any(|e| matches!(e, Event::Align { cancelled: true, .. })), "{ev:?}");
    assert_eq!(state(&mut h).nodes.iter().map(|n| n.clips.len()).collect::<Vec<_>>(), before.nodes.iter().map(|n| n.clips.len()).collect::<Vec<_>>(), "aucun clip ajouté");
    // Refus : guide ou double inconnu, double = guide, réglages invalides, clip qui n'est pas une prise calée.
    for cmd in [
        format!(r#"{{"type":"align_clips","guide":999999,"doubles":[{double}]}}"#),
        format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[999999]}}"#),
        format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{guide}]}}"#),
        format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[]}}"#),
        format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{double}],"settings":"pas du json"}}"#),
        format!(r#"{{"type":"align_redo","clips":[{double}]}}"#),
        r#"{"type":"merge_align_settings","dst":"{","src":"{"}"#.to_string(),
    ] {
        assert!(!errors(&h.command_json(&cmd)).is_empty(), "{cmd}");
    }
    assert!(h.tick().iter().all(|e| !matches!(e, Event::Align { .. })), "aucun travail lancé par un refus");
}

#[test]
fn copying_general_settings_keeps_the_destination_local_block() {
    let (_engine, mut h, _dir, _g, _d) = setup("merge");
    let default = daw_align::default_settings_json();
    let mut src: serde_json::Value = serde_json::from_str(&default).unwrap();
    src["general"]["tightness"] = serde_json::json!(0.5);
    src["local"]["strength_curve"] = serde_json::json!([[1.0, 0.0]]);
    let cmd = serde_json::json!({"type": "merge_align_settings", "dst": default, "src": serde_json::to_string(&src).unwrap()});
    let ev = h.command_json(&cmd.to_string());
    let Some(Event::AlignSettings { settings }) = ev.iter().find(|e| matches!(e, Event::AlignSettings { .. })) else { panic!("{ev:?}") };
    let v: serde_json::Value = serde_json::from_str(settings).unwrap();
    assert_eq!(v["general"]["tightness"], serde_json::json!(0.5), "le bloc général est copié");
    assert_eq!(v["local"]["strength_curve"], serde_json::json!([]), "la courbe de force ne l'est jamais");
}

#[test]
fn general_only_copies_the_general_block_into_each_clip_keeping_its_own_curve() {
    let (_engine, mut h, _dir, guide, double) = setup("general");
    h.command_json(&format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{double}]}}"#));
    let first = aligned_clip(&settle(&mut h, |e| matches!(e, Event::Align { active: false, .. })));
    // Une courbe de force propre à la prise, puis un « copier » d'une fiche qui a un autre moteur de force et sa propre courbe.
    let own = serde_json::json!({"format":"flibuste-align-settings","version":1,"general":{"strength":0.8},"local":{"strength_curve":[[1.0, 0.2]]}});
    h.command_json(&serde_json::json!({"type":"align_redo","clips":[first],"settings":own.to_string()}).to_string());
    let second = aligned_clip(&settle(&mut h, |e| matches!(e, Event::Align { active: false, .. })));
    let other = serde_json::json!({"format":"flibuste-align-settings","version":1,"general":{"strength":0.3},"local":{"strength_curve":[[2.0, 0.9]]}});
    h.command_json(&serde_json::json!({"type":"align_redo","clips":[second],"settings":other.to_string(),"general_only":true}).to_string());
    let third = aligned_clip(&settle(&mut h, |e| matches!(e, Event::Align { active: false, .. })));
    let ev = h.command_json(&format!(r#"{{"type":"get_align","clip":{third}}}"#));
    let Some(Event::AlignDetail { settings, .. }) = ev.iter().find(|e| matches!(e, Event::AlignDetail { .. })) else { panic!("{ev:?}") };
    let v: serde_json::Value = serde_json::from_str(settings).unwrap();
    assert_eq!(v["general"]["strength"], serde_json::json!(0.3), "le bloc général vient de la fiche copiée");
    let curve: Vec<(f64, f64)> = v["local"]["strength_curve"].as_array().unwrap().iter().map(|p| (p[0].as_f64().unwrap(), p[1].as_f64().unwrap())).collect();
    assert_eq!(curve, vec![(1.0, 0.2)], "la courbe de force reste celle de la prise");
}

#[test]
fn a_double_on_the_guides_track_is_refused_because_the_aligned_take_would_cover_the_guide() {
    let (_engine, mut h, _dir, guide, double) = setup("same-track");
    let p = state(&mut h);
    let guide_track = p.nodes.iter().find(|n| audio_clips(n).iter().any(|c| c.0 == guide)).unwrap().id;
    // On ramène le double sur la piste du guide : il le recouvre (deux clips de la même durée au même endroit).
    let ev = h.command_json(&format!(r#"{{"type":"move_clip","clip":{double},"start":0,"node":{guide_track}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let before = state(&mut h);
    let ev = h.command_json(&format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{double}]}}"#));
    assert!(errors(&ev).iter().any(|m| m.contains("même piste")), "{:?}", errors(&ev));
    assert!(h.tick().iter().all(|e| !matches!(e, Event::Align { .. })), "aucun travail lancé");
    assert_eq!(state(&mut h).nodes.iter().map(|n| n.clips.len()).collect::<Vec<_>>(), before.nodes.iter().map(|n| n.clips.len()).collect::<Vec<_>>());
}

/// État vu par la page : pour la piste du double, (muet, prise calée) de chaque clip audio ; pour chaque piste, le solo.
fn view(h: &mut Host) -> (Vec<(bool, bool)>, Vec<(String, bool)>) {
    let p = state(h);
    let double = p.nodes.iter().find(|n| n.name == "double").unwrap();
    (audio_clips(double).iter().map(|c| (c.4, c.6)).collect(), p.nodes.iter().map(|n| (n.name.clone(), n.solo)).collect())
}

fn loop_of(ev: &[Event]) -> Option<(u64, u64)> {
    ev.iter().rev().find_map(|e| if let Event::Position { loop_range, .. } = e { Some(loop_range.as_ref().map(|l| (l.start, l.end))) } else { None }).flatten()
}

#[test]
fn listening_switches_between_guide_original_and_aligned_without_touching_the_project_and_restores_everything() {
    let (_engine, mut h, _dir, guide, double) = setup("listen");
    h.command_json(&format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{double}]}}"#));
    let aligned = aligned_clip(&settle(&mut h, |e| matches!(e, Event::Align { active: false, .. })));
    let p = state(&mut h);
    let node = p.nodes.iter().find(|n| n.name == "double").unwrap().id;
    let g = audio_clips(p.nodes.iter().find(|n| n.name == "guide").unwrap())[0].clone();
    let before = view(&mut h);
    assert_eq!(before.0.iter().filter(|c| !c.0).count(), 1, "avant : une seule prise audible (la calée)");
    let undo_before = h.command_json(r#"{"type":"get_project"}"#).iter().find_map(|e| if let Event::History { undo_label, .. } = e { undo_label.clone() } else { None });

    let listen = |h: &mut Host, what: &str| h.command_json(&format!(r#"{{"type":"listen_align","guide":{guide},"node":{node},"aligned":{aligned},"what":"{what}"}}"#));
    // Original : la prise calée se tait, l'original joue, seule la piste du double est en solo ; boucle sur la fenêtre du guide.
    let ev = listen(&mut h, "original");
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    assert!(ev.iter().any(|e| matches!(e, Event::Listen { active: true, what } if what == "original")));
    assert_eq!(loop_of(&ev), Some((g.2, g.2 + g.3)));
    let (clips, solos) = view(&mut h);
    assert!(clips.iter().filter(|c| c.1).all(|c| c.0), "toute prise calée muette");
    assert!(clips.iter().filter(|c| !c.1).any(|c| !c.0), "l'original joue");
    assert_eq!(solos.iter().filter(|s| s.1).map(|s| s.0.as_str()).collect::<Vec<_>>(), vec!["double"]);
    // Recalé.
    listen(&mut h, "aligned");
    let (clips, _) = view(&mut h);
    assert!(clips.iter().filter(|c| c.1).any(|c| !c.0) && clips.iter().filter(|c| !c.1).all(|c| c.0));
    // Guide seul : la piste du guide en solo, tout le double muet.
    listen(&mut h, "guide");
    let (clips, solos) = view(&mut h);
    assert!(clips.iter().all(|c| c.0));
    assert_eq!(solos.iter().filter(|s| s.1).map(|s| s.0.as_str()).collect::<Vec<_>>(), vec!["guide"]);
    // Guide + recalé : les deux pistes en solo.
    listen(&mut h, "guide_aligned");
    let (_, solos) = view(&mut h);
    let mut on: Vec<&str> = solos.iter().filter(|s| s.1).map(|s| s.0.as_str()).collect();
    on.sort();
    assert_eq!(on, vec!["double", "guide"]);
    // Arrêt : tout est rendu, rien n'est entré dans l'historique ni marqué modifié par l'écoute.
    let ev = h.command_json(r#"{"type":"stop_listen"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Listen { active: false, .. })));
    assert_eq!(view(&mut h), before, "clips et solos comme avant");
    assert_eq!(loop_of(&ev), None, "boucle d'avant (aucune)");
    let undo_after = h.command_json(r#"{"type":"get_project"}"#).iter().find_map(|e| if let Event::History { undo_label, .. } = e { undo_label.clone() } else { None });
    assert_eq!(undo_after, undo_before);

    // Une commande de modification termine l'écoute d'abord (l'état provisoire n'entre jamais dans l'historique).
    listen(&mut h, "original");
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::Listen { active: false, .. })), "{ev:?}");
    // Refus : prise calée absente pour « aligned », mode inconnu, piste inconnue.
    for cmd in [
        format!(r#"{{"type":"listen_align","guide":{guide},"node":{node},"what":"aligned"}}"#),
        format!(r#"{{"type":"listen_align","guide":{guide},"node":{node},"what":"autre"}}"#),
        format!(r#"{{"type":"listen_align","guide":{guide},"node":999,"what":"guide"}}"#),
        format!(r#"{{"type":"listen_align","guide":999999,"node":{node},"what":"guide"}}"#),
    ] {
        assert!(!errors(&h.command_json(&cmd)).is_empty(), "{cmd}");
    }
}

/// Coupe le clip `clip` à `at` ; rend l'identifiant de la moitié droite (la gauche garde le sien).
fn split_at(h: &mut Host, clip: u64, at: u64) -> u64 {
    let before: Vec<u64> = state(h).nodes.iter().flat_map(audio_clips).map(|c| c.0).collect();
    let ev = h.command_json(&format!(r#"{{"type":"split_clip","clip":{clip},"at":{at}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    state(h).nodes.iter().flat_map(audio_clips).map(|c| c.0).find(|id| !before.contains(id)).expect("moitié droite")
}

#[test]
fn a_double_made_of_several_clips_is_aligned_as_one_band_and_a_gap_is_reported_as_silence() {
    // Deux moitiés jointives : même résultat qu'un clip entier (chemin aux débuts de notes connus), une seule prise calée.
    let (_engine, mut h, dir, guide, double) = setup("bands");
    let p = state(&mut h);
    let d0 = audio_clips(p.nodes.iter().find(|n| n.name == "double").unwrap())[0].clone();
    let g0 = audio_clips(p.nodes.iter().find(|n| n.name == "guide").unwrap())[0].clone();
    let mid = d0.2 + d0.3 / 2;
    let right = split_at(&mut h, double, mid);
    let ev = h.command_json(&format!(r#"{{"type":"align_clips","guide":{guide},"doubles":[{double},{right}]}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let ev = settle(&mut h, |e| matches!(e, Event::Align { active: false, .. }));
    let Some(Event::Align { clips, warnings, error, total, .. }) = ev.iter().rev().find(|e| matches!(e, Event::Align { active: false, .. })) else { unreachable!() };
    assert!(error.is_none() && warnings.is_empty() && *total == 1 && clips.len() == 1, "{error:?} {warnings:?}");
    let new = clips[0].clip;
    let ev = h.command_json(&format!(r#"{{"type":"get_align","clip":{new}}}"#));
    let Some(Event::AlignDetail { hop, path, .. }) = ev.iter().find(|e| matches!(e, Event::AlignDetail { .. })) else { panic!("{ev:?}") };
    let errs: Vec<f64> = onsets().iter().map(|&(tg, td)| (path[((tg / hop).round() as usize).min(path.len() - 1)] - td).abs() * 1000.0).collect();
    let mean = errs.iter().sum::<f64>() / errs.len() as f64;
    assert!(mean < 15.0, "moyenne {mean:.1} ms");
    let track = audio_clips(state(&mut h).nodes.iter().find(|n| n.name == "double").unwrap());
    let calee = track.iter().find(|c| c.6).unwrap();
    assert_eq!((calee.2, calee.3), (g0.2, g0.3));

    // Enregistrée puis rouverte, la fiche garde ses deux morceaux : refaire le rendu marche encore.
    h.command_json(r#"{"type":"save_project"}"#);
    h.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, dir.join("P.daw").to_string_lossy()));
    let reopened = audio_clips(state(&mut h).nodes.iter().find(|n| n.name == "double").unwrap());
    let again = reopened.iter().find(|c| c.6).expect("prise calée rouverte").0;
    let ev = h.command_json(&format!(r#"{{"type":"align_redo","clips":[{again}]}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let ev = settle(&mut h, |e| matches!(e, Event::Align { active: false, .. }));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));

    // Un trou d'une seconde au milieu : l'avertissement le dit, le calage a lieu quand même.
    let (_engine2, mut h2, _dir2, guide2, double2) = setup("gap");
    let sr = SR as u64;
    let a = split_at(&mut h2, double2, mid);
    let b = split_at(&mut h2, a, mid + sr);
    assert!(errors(&h2.command_json(&format!(r#"{{"type":"delete_clip","clip":{a}}}"#))).is_empty());
    let ev = h2.command_json(&format!(r#"{{"type":"align_clips","guide":{guide2},"doubles":[{double2},{b}]}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let ev = settle(&mut h2, |e| matches!(e, Event::Align { active: false, .. }));
    let Some(Event::Align { warnings, clips, error, .. }) = ev.iter().rev().find(|e| matches!(e, Event::Align { active: false, .. })) else { unreachable!() };
    assert!(error.is_none() && clips.len() == 1, "{error:?}");
    assert!(warnings.iter().any(|w| w.contains("sans matière")), "{warnings:?}");

    // Deux clips du guide sur sa piste : la fenêtre va du premier au dernier.
    let (_engine3, mut h3, _dir3, guide3, double3) = setup("gguide");
    let gright = split_at(&mut h3, guide3, g0.2 + g0.3 / 2);
    let ev = h3.command_json(&format!(r#"{{"type":"align_clips","guide":{guide3},"guide_more":[{gright}],"doubles":[{double3}]}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let ev = settle(&mut h3, |e| matches!(e, Event::Align { active: false, .. }));
    assert!(errors(&ev).is_empty());
    let track = audio_clips(state(&mut h3).nodes.iter().find(|n| n.name == "double").unwrap());
    let calee = track.iter().find(|c| c.6).unwrap();
    assert_eq!((calee.2, calee.3), (g0.2, g0.3), "fenêtre = union des clips du guide");
    // Un clip « guide supplémentaire » d'une autre piste est refusé.
    assert!(!errors(&h3.command_json(&format!(r#"{{"type":"align_clips","guide":{guide3},"guide_more":[{double3}],"doubles":[{double3}]}}"#))).is_empty());
}
