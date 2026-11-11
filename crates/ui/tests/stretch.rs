//! Marqueurs d'étirement par le protocole : rendu sur un fil de travail, hauteur conservée, une action annulable, enregistrement et
//! réouverture, découpe (chaque moitié garde ses marqueurs), retrait, refus sans trace, annulation en cours de rendu.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daw_engine::Engine;
use daw_offline_render::export::{encode, BitDepth, Container, Format};
use daw_ui::demo::demo_session;
use daw_ui::protocol::{ClipInfo, NodeInfo, ProjectInfo, StretchInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> (Engine, Host) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    (engine, Host::new(handle, session, meta).unwrap())
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-stretch-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Sinus de 440 Hz (gauche) et 880 Hz (droite), `secs` secondes, à 48 kHz.
fn tone_file(dir: &Path, name: &str, secs: f32) -> PathBuf {
    let n = (SR as f32 * secs) as usize;
    let l: Vec<f32> = (0..n).map(|i| 0.5 * (i as f32 * std::f32::consts::TAU * 440.0 / SR as f32).sin()).collect();
    let r: Vec<f32> = (0..n).map(|i| 0.25 * (i as f32 * std::f32::consts::TAU * 880.0 / SR as f32).sin()).collect();
    let e = encode(&l, &r, SR, Format { container: Container::Wav, depth: BitDepth::Float32, dither: false }, None).unwrap();
    let p = dir.join(format!("{name}.wav"));
    std::fs::write(&p, e.bytes).unwrap();
    p
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn get(h: &mut Host) -> ProjectInfo {
    project(&h.command_json(r#"{"type":"get_project"}"#)).clone()
}

fn node<'a>(p: &'a ProjectInfo, name: &str) -> &'a NodeInfo {
    p.nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("piste « {name} » absente"))
}

struct Clip {
    id: u64,
    start: u64,
    len: u64,
    source_len: u64,
    data_id: u64,
    stretch: Option<StretchInfo>,
}

fn clips(n: &NodeInfo) -> Vec<Clip> {
    n.clips.iter().filter_map(|c| if let ClipInfo::Audio { id, start, len, source_len, stretch, data_id, .. } = c { Some(Clip { id: *id, start: *start, len: *len, source_len: *source_len, data_id: *data_id, stretch: stretch.clone() }) } else { None }).collect()
}

fn undo_label(ev: &[Event]) -> Option<String> {
    ev.iter().rev().find_map(|e| if let Event::History { undo_label, .. } = e { Some(undo_label.clone()) } else { None }).flatten()
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

/// Importe le fichier dans un projet enregistré (le dossier existe : les copies et les rendus s'y écrivent).
fn project_with_clip(name: &str, secs: f32) -> (Engine, Host, PathBuf, u64) {
    let (e, mut h) = host();
    let dir = temp(name);
    h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    let src = tone_file(&temp(&format!("{name}-src")), "boucle", secs);
    let list = format!(r#"{{"type":"import_audio","files":[{:?}]}}"#, src.to_string_lossy());
    let mut ev = h.command_json(&list);
    let t = Instant::now();
    loop {
        if ev.iter().any(|e| matches!(e, Event::Import { active: false, .. })) {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(60), "import trop long");
        std::thread::sleep(Duration::from_millis(5));
        ev = h.tick();
    }
    let p = get(&mut h);
    let id = clips(node(&p, "boucle"))[0].id;
    (e, h, dir.join("P.daw"), id)
}

/// Attend la fin d'un étirement : les événements (celui de la commande compris) et le dernier `stretch`.
fn run(h: &mut Host, json: &str) -> (Vec<Event>, Option<Event>) {
    let mut events = h.command_json(json);
    let mut all = Vec::new();
    let t = Instant::now();
    loop {
        for e in events.drain(..) {
            let done = matches!(&e, Event::Stretch { active: false, .. }) || matches!(&e, Event::Error { .. });
            all.push(e.clone());
            if done {
                return (all, Some(e));
            }
        }
        assert!(t.elapsed() < Duration::from_secs(120), "étirement trop long");
        std::thread::sleep(Duration::from_millis(5));
        events = h.tick();
    }
}

fn stretch(h: &mut Host, id: u64, markers: &str, engine: &str) -> Vec<Event> {
    let (ev, last) = run(h, &format!(r#"{{"type":"set_stretch","clip":{id},"markers":{markers}{engine}}}"#));
    match last {
        Some(Event::Stretch { error: None, cancelled: false, .. }) => ev,
        other => panic!("étirement échoué : {other:?}"),
    }
}

/// Fréquence (Hz) d'une voie du fichier étiré, par passages à zéro montants entre deux instants.
fn hz(file: &Path, channel: usize, from: f64, to: f64) -> f64 {
    let d = daw_recorder::decode::decode_file(file).unwrap().audio;
    let ch = d.channels as usize;
    let x: Vec<f32> = d.samples.iter().skip(channel).step_by(ch).copied().collect();
    let (a, b) = ((from * SR as f64) as usize, (to * SR as f64) as usize);
    let (mut count, mut first, mut last) = (0usize, 0usize, 0usize);
    for i in a + 1..b {
        if x[i - 1] < 0.0 && x[i] >= 0.0 {
            if count == 0 {
                first = i;
            }
            last = i;
            count += 1;
        }
    }
    (count - 1) as f64 * SR as f64 / (last - first) as f64
}

fn stretched_file(proj: &Path) -> PathBuf {
    std::fs::read_dir(proj.join("audio")).unwrap().map(|f| f.unwrap().path()).find(|p| p.file_name().unwrap().to_string_lossy().contains("etire")).expect("un fichier étiré dans audio/")
}

#[test]
fn markers_stretch_the_audio_without_changing_the_pitch_in_one_undoable_step() {
    let (_e, mut h, proj, id) = project_with_clip("pitch", 3.0);
    let before = clips(node(&get(&mut h), "boucle")).remove(0);
    assert!(before.stretch.is_none(), "un clip ordinaire n'a pas de marqueurs");
    let (len, start) = (before.len, before.start);
    assert_eq!(len, 144_000);
    // Un marqueur : 1,0 s de la source est tiré à 1,5 s du clip. Avant : ×1,5 ; après : ×0,75. La longueur ne change pas.
    let ev = stretch(&mut h, id, "[[72000,48000]]", "");
    assert_eq!(undo_label(&ev).as_deref(), Some("Étirer l'audio"));
    let c = clips(node(&get(&mut h), "boucle")).remove(0);
    assert_eq!((c.start, c.len, c.source_len), (start, len, len), "même place, même longueur, fichier de même durée");
    let info = c.stretch.expect("le clip est étiré");
    assert_eq!((info.engine.as_str(), info.src_len, info.markers.clone()), ("signalsmith", 144_000, vec![[72_000, 48_000]]));
    // Le fichier rendu est écrit avec le projet : la hauteur est conservée de chaque côté du marqueur, sur les deux voies.
    h.command_json(r#"{"type":"save_project"}"#);
    let f = stretched_file(&proj);
    for (ch, want) in [(0, 440.0), (1, 880.0)] {
        for (a, b) in [(0.3, 1.3), (1.8, 2.8)] {
            let got = hz(&f, ch, a, b);
            assert!((got - want).abs() < want * 0.01, "voie {ch} entre {a} et {b} s : {got:.1} Hz au lieu de {want}");
        }
    }
    // Le clip joue un autre fichier sans changer d'identifiant ni de géométrie : la page doit le voir (cache des formes d'onde).
    assert_ne!(c.data_id, before.data_id, "l'identité des données change avec le rendu");
    // Annuler rend le clip ordinaire sur sa source ; rétablir redonne l'étirement (la fiche est retrouvée par les données).
    h.command_json(r#"{"type":"undo"}"#);
    let undone = clips(node(&get(&mut h), "boucle")).remove(0);
    assert!(undone.stretch.is_none());
    assert_eq!(undone.data_id, before.data_id, "annulé : les données d'origine");
    h.command_json(r#"{"type":"redo"}"#);
    assert_eq!(clips(node(&get(&mut h), "boucle"))[0].data_id, c.data_id, "rétabli : les données rendues");
    assert_eq!(clips(node(&get(&mut h), "boucle"))[0].stretch.as_ref().unwrap().markers, vec![[72_000, 48_000]]);
}

#[test]
fn the_markers_are_saved_with_the_project_and_survive_reopening() {
    let (_e, mut h, proj, id) = project_with_clip("save", 3.0);
    stretch(&mut h, id, "[[60000,48000],[110000,100000]]", r#","engine":"rubberband_r2""#);
    h.command_json(r#"{"type":"save_project"}"#);
    let (_e2, mut h2) = host();
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    let c = clips(node(&get(&mut h2), "boucle")).remove(0);
    let info = c.stretch.expect("fiche relue");
    assert_eq!((info.engine.as_str(), info.markers), ("rubberband_r2", vec![[60_000, 48_000], [110_000, 100_000]]));
    // Et on peut continuer à l'éditer après réouverture : le rendu repart de la source.
    stretch(&mut h2, c.id, "[[60000,48000]]", "");
    let again = clips(node(&get(&mut h2), "boucle")).remove(0).stretch.unwrap();
    assert_eq!((again.engine.as_str(), again.markers), ("rubberband_r2", vec![[60_000, 48_000]]), "le moteur du clip est gardé quand la commande n'en donne pas");
}

#[test]
fn moving_a_marker_renders_again_from_the_source_and_removing_them_all_gives_the_source_back() {
    let (_e, mut h, _proj, id) = project_with_clip("move", 3.0);
    stretch(&mut h, id, "[[72000,48000]]", "");
    // Déplacer le marqueur : nouveau rendu depuis la source (la source du clip n'a pas changé), un seul pas d'annulation.
    let ev = stretch(&mut h, id, "[[96000,48000]]", r#","engine":"rubberband_r3""#);
    assert_eq!(undo_label(&ev).as_deref(), Some("Étirer l'audio"));
    let info = clips(node(&get(&mut h), "boucle"))[0].stretch.clone().unwrap();
    assert_eq!((info.engine.as_str(), info.src_len, info.markers), ("rubberband_r3", 144_000, vec![[96_000, 48_000]]));
    // Plus aucun marqueur : retour au clip ordinaire, sans rendu (réponse immédiate).
    let ev = h.command_json(&format!(r#"{{"type":"set_stretch","clip":{id},"markers":[]}}"#));
    assert!(errors(&ev).is_empty(), "{ev:?}");
    assert_eq!(undo_label(&ev).as_deref(), Some("Retirer l'étirement"));
    let c = clips(node(&get(&mut h), "boucle")).remove(0);
    assert!(c.stretch.is_none() && c.len == 144_000 && c.source_len == 144_000);
    // Annuler ce retrait remet l'étirement.
    h.command_json(r#"{"type":"undo"}"#);
    assert!(clips(node(&get(&mut h), "boucle"))[0].stretch.is_some());
}

#[test]
fn each_half_of_a_split_clip_keeps_its_markers_and_can_be_stretched_on_its_own() {
    let (_e, mut h, _proj, id) = project_with_clip("split", 3.0);
    stretch(&mut h, id, "[[72000,48000]]", "");
    // Découpe à 100 000 : la gauche garde le marqueur, la droite n'en a pas mais reste étirée (même fichier, autre fenêtre).
    h.command_json(&format!(r#"{{"type":"split_clip","clip":{id},"at":100000}}"#));
    let p = get(&mut h);
    let mut cs = clips(node(&p, "boucle"));
    cs.sort_by_key(|c| c.start);
    assert_eq!(cs.len(), 2);
    let (left, right) = (&cs[0], &cs[1]);
    assert_eq!((left.len, right.len), (100_000, 44_000));
    assert_eq!(left.stretch.as_ref().unwrap().markers, vec![[72_000, 48_000]]);
    let r = right.stretch.as_ref().unwrap();
    assert!(r.markers.is_empty(), "le marqueur est dans l'autre moitié");
    // La droite couvre la source de 1,7778 s à 3 s (rapport 0,75 depuis 1,5 s de sortie) : ≈ 58 667 échantillons.
    assert!((r.src_len as i64 - 58_667).abs() <= 2, "source couverte par la moitié droite : {}", r.src_len);
    // On pose un marqueur dans la moitié droite : elle seule est rendue de nouveau, depuis la source.
    let rid = right.id;
    stretch(&mut h, rid, "[[22000,30000]]", "");
    let after = clips(node(&get(&mut h), "boucle"));
    let right_after = after.iter().find(|c| c.id == rid).unwrap();
    assert_eq!(right_after.len, 44_000, "même longueur sur la ligne de temps");
    assert_eq!(right_after.stretch.as_ref().unwrap().markers, vec![[22_000, 30_000]]);
    let left_after = after.iter().find(|c| c.id == left.id).unwrap();
    assert_eq!(left_after.stretch.as_ref().unwrap().markers, vec![[72_000, 48_000]], "la moitié gauche n'a pas bougé");
}

#[test]
fn bad_requests_are_refused_without_a_trace() {
    let (_e, mut h, _proj, id) = project_with_clip("refus", 3.0);
    let cases = [
        (format!(r#"{{"type":"set_stretch","clip":{id},"markers":[]}}"#), "sans marqueur sur un clip ordinaire"),
        (format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[1000,48000]]}}"#), "rapport bien trop petit"),
        (format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[48000,1000]]}}"#), "rapport bien trop grand"),
        (format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[0,100]]}}"#), "marqueur au bord"),
        (format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[144000,100]]}}"#), "marqueur à la fin du clip"),
        (format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[50000,50000],[40000,60000]]}}"#), "marqueurs dans le désordre"),
        (format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[50000,50000],[50100,50100]]}}"#), "tronçon de moins de 10 ms"),
        (format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[50000,50000]],"engine":"zplane"}}"#), "moteur inconnu"),
        (r#"{"type":"set_stretch","clip":987654,"markers":[[50000,50000]]}"#.to_owned(), "clip inconnu"),
    ];
    for (json, what) in cases {
        let ev = h.command_json(&json);
        assert_eq!(errors(&ev).len(), 1, "{what} : {ev:?}");
    }
    // Rien n'a été posé : le clip est resté ordinaire et l'historique ne contient pas d'étirement.
    assert!(clips(node(&get(&mut h), "boucle"))[0].stretch.is_none());
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert!(undo_label(&ev).is_none_or(|l| !l.contains("tirer") && !l.contains("tirement")), "{:?}", undo_label(&ev));
    // Un second étirement pendant un premier est refusé (un seul rendu à la fois).
    let first = h.command_json(&format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[72000,48000]],"engine":"rubberband_r3"}}"#));
    assert!(errors(&first).is_empty());
    let second = h.command_json(&format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[70000,48000]]}}"#));
    assert_eq!(errors(&second).len(), 1, "un étirement est déjà en cours");
    // Nouveau projet pendant le rendu : refusé aussi.
    assert_eq!(errors(&h.command_json(r#"{"type":"new_project"}"#)).len(), 1);
    let _ = run(&mut h, r#"{"type":"get_project"}"#); // laisse finir
    let t = Instant::now();
    while clips(node(&get(&mut h), "boucle"))[0].stretch.is_none() {
        assert!(t.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(10));
        h.tick();
    }
}

#[test]
fn cancelling_a_render_leaves_nothing_behind() {
    let (_e, mut h, _proj, id) = project_with_clip("cancel", 12.0);
    let mut ev = h.command_json(&format!(r#"{{"type":"set_stretch","clip":{id},"markers":[[200000,100000]],"engine":"rubberband_r3"}}"#));
    assert!(errors(&ev).is_empty());
    std::thread::sleep(Duration::from_millis(100));
    h.command_json(r#"{"type":"cancel_stretch"}"#);
    let t = Instant::now();
    let mut cancelled = false;
    while !cancelled {
        assert!(t.elapsed() < Duration::from_secs(60), "annulation trop lente");
        for e in ev.drain(..) {
            if let Event::Stretch { active: false, cancelled: c, error, .. } = e {
                assert!(error.is_none());
                cancelled = c;
                assert!(c, "annulé, pas terminé");
            }
        }
        std::thread::sleep(Duration::from_millis(10));
        ev = h.tick();
    }
    assert!(clips(node(&get(&mut h), "boucle"))[0].stretch.is_none(), "rien n'a été posé");
    // Un nouvel étirement fonctionne ensuite.
    stretch(&mut h, id, "[[200000,300000]]", r#","engine":"signalsmith""#);
    assert!(clips(node(&get(&mut h), "boucle"))[0].stretch.is_some());
}

#[test]
fn a_smoothed_marker_changes_the_speed_gradually_keeps_the_pitch_and_survives_reopening() {
    let (_e, mut h, proj, id) = project_with_clip("smooth", 3.0);
    // Même marqueur que le premier test (×1,5 puis ×0,75), mais lissé.
    let ev = stretch(&mut h, id, "[[72000,48000]],\"smooth\":[true]", "");
    assert!(errors(&ev).is_empty());
    let info = clips(node(&get(&mut h), "boucle")).remove(0).stretch.expect("étiré");
    assert_eq!((info.markers.clone(), info.smooth.clone()), (vec![[72_000, 48_000]], vec![true]));
    h.command_json(r#"{"type":"save_project"}"#);
    let f = stretched_file(&proj);
    for (ch, want) in [(0, 440.0), (1, 880.0)] {
        for (a, b) in [(0.3, 1.0), (2.0, 2.8)] {
            let got = hz(&f, ch, a, b);
            assert!((got - want).abs() < want * 0.01, "voie {ch} entre {a} et {b} s : {got:.1} Hz au lieu de {want}");
        }
    }
    // Le fichier a la longueur du clip.
    let d = daw_recorder::decode::decode_file(&f).unwrap().audio;
    assert_eq!(d.samples.len() / d.channels as usize, 144_000);
    // Rouvert : l'indicateur est retrouvé ; sans `smooth` dans la commande suivante, plus de lissage.
    let (_e2, mut h2) = host();
    h2.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    let c = clips(node(&get(&mut h2), "boucle")).remove(0);
    assert_eq!(c.stretch.as_ref().unwrap().smooth, vec![true]);
    stretch(&mut h2, c.id, "[[72000,48000]]", "");
    assert_eq!(clips(node(&get(&mut h2), "boucle")).remove(0).stretch.unwrap().smooth, vec![false]);
    // Un indicateur de trop est refusé.
    let ev = h2.command_json(&format!(r#"{{"type":"set_stretch","clip":{},"markers":[[72000,48000]],"smooth":[true,false]}}"#, c.id));
    assert!(!errors(&ev).is_empty());
}

#[test]
fn segments_with_ratio_one_are_copied_from_the_source_untouched() {
    let (_e, mut h, proj, id) = project_with_clip("unity", 3.0);
    // 0-1 s inchangé (rapport 1), puis 1 s de source étiré à 150 %, puis le reste comprimé à 50 %.
    stretch(&mut h, id, "[[48000,48000],[120000,96000]]", "");
    h.command_json(r#"{"type":"save_project"}"#);
    let f = stretched_file(&proj);
    let rendered = daw_recorder::decode::decode_file(&f).unwrap().audio;
    let source = daw_recorder::decode::decode_file(&proj.join("audio").join("boucle.wav")).unwrap().audio;
    let ch = rendered.channels as usize;
    // Hors des 10 ms de fondu avant 1 s, le rendu est la source, échantillon pour échantillon.
    for i in 0..(48_000 - 600) * ch {
        assert_eq!(rendered.samples[i], source.samples[i], "échantillon {i}");
    }
}

#[test]
fn detect_onsets_finds_the_hits_of_a_clip_and_refuses_bad_requests() {
    let (_e, mut h) = host();
    let dir = temp("onsets");
    h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    // 3 s de quasi-silence avec des coups de bruit décroissants à 0,5 s, 1,2 s et 2,1 s.
    let mut seed = 5u64;
    let mut noise = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
    };
    let mut x: Vec<f32> = (0..3 * SR as usize).map(|_| noise() * 0.0005).collect();
    let times = [0.5, 1.2, 2.1];
    for t in times {
        for k in 0..(0.08 * SR as f64) as usize {
            x[(t * SR as f64) as usize + k] += noise() * 0.8 * (-(k as f32) / 800.0).exp();
        }
    }
    let e = encode(&x, &x, SR, Format { container: Container::Wav, depth: BitDepth::Float32, dither: false }, None).unwrap();
    let src = temp("onsets-src").join("coups.wav");
    std::fs::write(&src, e.bytes).unwrap();
    let mut ev = h.command_json(&format!(r#"{{"type":"import_audio","files":[{:?}]}}"#, src.to_string_lossy()));
    let t = Instant::now();
    while !ev.iter().any(|e| matches!(e, Event::Import { active: false, .. })) {
        assert!(t.elapsed() < Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(5));
        ev = h.tick();
    }
    let id = clips(node(&get(&mut h), "coups"))[0].id;
    let ev = h.command_json(&format!(r#"{{"type":"detect_onsets","clip":{id}}}"#));
    let Some(Event::Onsets { clip, positions }) = ev.iter().find(|e| matches!(e, Event::Onsets { .. })).cloned() else { panic!("{ev:?}") };
    assert_eq!((clip, positions.len()), (id, 3), "{positions:?}");
    for (p, t) in positions.iter().zip(times) {
        assert!((*p as f64 / SR as f64 - t).abs() < 0.008, "attaque à {t} s trouvée à {} s", *p as f64 / SR as f64);
    }
    assert!(!errors(&h.command_json(r#"{"type":"detect_onsets","clip":99999}"#)).is_empty());
    assert!(!errors(&h.command_json(&format!(r#"{{"type":"detect_onsets","clip":{id},"sensitivity":2}}"#))).is_empty());
}
