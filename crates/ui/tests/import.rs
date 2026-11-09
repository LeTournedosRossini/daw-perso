//! Import audio par le protocole : décodage, conversion de fréquence, copie ou référence en place, multicanal, annulation.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use daw_engine::Engine;
use daw_offline_render::export::{encode, BitDepth, Container, Format};
use daw_ui::demo::demo_session;
use daw_ui::protocol::{ClipInfo, ImportedClip, NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> (Engine, Host) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    (engine, Host::new(handle, session, meta).unwrap())
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("daw-import-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn saved(h: &mut Host, dir: &Path) -> PathBuf {
    h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, dir.join("P").to_string_lossy()));
    dir.join("P.daw")
}

/// Fichier de test : sinus de `freq` Hz (gauche) et `2 × freq` (droite), `secs` secondes, à `rate` Hz.
fn tone_file(dir: &Path, name: &str, rate: u32, secs: f32, container: Container, depth: BitDepth) -> PathBuf {
    let n = (rate as f32 * secs) as usize;
    let l: Vec<f32> = (0..n).map(|i| 0.5 * (i as f32 * std::f32::consts::TAU * 440.0 / rate as f32).sin()).collect();
    let r: Vec<f32> = (0..n).map(|i| 0.25 * (i as f32 * std::f32::consts::TAU * 880.0 / rate as f32).sin()).collect();
    let e = encode(&l, &r, rate, Format { container, depth, dither: false }, None).unwrap();
    let ext = if container == Container::Wav { "wav" } else { "flac" };
    let p = dir.join(format!("{name}.{ext}"));
    std::fs::write(&p, e.bytes).unwrap();
    p
}

/// WAV flottant à `channels` canaux dont le canal `c` vaut la constante `(c + 1) × 0,01`.
fn multichannel_file(dir: &Path, name: &str, channels: u16) -> PathBuf {
    let frames = 4_800u32;
    let data: Vec<u8> = (0..frames).flat_map(|_| (0..channels).flat_map(|c| ((c + 1) as f32 * 0.01).to_le_bytes())).collect();
    let mut v = b"RIFF".to_vec();
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&3u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&SR.to_le_bytes());
    v.extend_from_slice(&(SR * channels as u32 * 4).to_le_bytes());
    v.extend_from_slice(&(channels * 4).to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    let p = dir.join(format!("{name}.wav"));
    std::fs::write(&p, v).unwrap();
    p
}

struct Done {
    clips: Vec<ImportedClip>,
    warnings: Vec<String>,
    cancelled: bool,
    error: Option<String>,
    events: Vec<Event>,
    progress: Vec<f64>,
}

fn run(h: &mut Host, json: &str) -> Done {
    let mut events = h.command_json(json);
    let mut all = Vec::new();
    let mut progress = Vec::new();
    let t = Instant::now();
    loop {
        for e in events.drain(..) {
            match &e {
                Event::Import { active: true, progress: p, .. } => progress.push(*p),
                Event::Import { active: false, clips, warnings, cancelled, error, .. } => {
                    let d = Done { clips: clips.clone(), warnings: warnings.clone(), cancelled: *cancelled, error: error.clone(), events: Vec::new(), progress };
                    all.push(e);
                    return Done { events: all, ..d };
                }
                Event::Error { message, .. } => return Done { clips: vec![], warnings: vec![], cancelled: false, error: Some(message.clone()), events: vec![e.clone()], progress },
                _ => {}
            }
            all.push(e);
        }
        assert!(t.elapsed() < Duration::from_secs(60), "import trop long");
        std::thread::sleep(Duration::from_millis(5));
        events = h.tick();
    }
}

fn import_json(files: &[&Path], extra: &str) -> String {
    let list: Vec<String> = files.iter().map(|f| format!("{:?}", f.to_string_lossy())).collect();
    format!(r#"{{"type":"import_audio","files":[{}]{extra}}}"#, list.join(","))
}

fn project(h: &mut Host) -> ProjectInfo {
    h.command_json(r#"{"type":"get_project"}"#).into_iter().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).unwrap()
}

fn node<'a>(p: &'a ProjectInfo, name: &str) -> &'a NodeInfo {
    p.nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("piste « {name} » absente : {:?}", p.nodes.iter().map(|n| &n.name).collect::<Vec<_>>()))
}

fn audio(n: &NodeInfo) -> Vec<(String, u64, u64, bool)> {
    n.clips.iter().filter_map(|c| if let ClipInfo::Audio { name, start, len, missing, .. } = c { Some((name.clone(), *start, *len, *missing)) } else { None }).collect()
}

fn clip_id(n: &NodeInfo) -> u64 {
    n.clips.iter().find_map(|c| if let ClipInfo::Audio { id, .. } = c { Some(*id) } else { None }).unwrap()
}

fn peaks(h: &mut Host, id: u64, from: u64, to: u64) -> Vec<[f32; 2]> {
    h.command_json(&format!(r#"{{"type":"get_peaks","clip":{id},"from":{from},"to":{to},"columns":64}}"#)).into_iter().find_map(|e| if let Event::Peaks { columns, .. } = e { Some(columns) } else { None }).unwrap()
}

fn undo_label(ev: &[Event]) -> Option<String> {
    ev.iter().rev().find_map(|e| if let Event::History { undo_label, .. } = e { Some(undo_label.clone()) } else { None }).flatten()
}

fn audio_dir_files(proj: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(proj.join("audio")).map(|d| d.map(|f| f.unwrap().file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

#[test]
fn probing_reads_headers_and_flags_a_rate_conversion() {
    let (_e, mut h) = host();
    let d = temp("probe");
    let a = tone_file(&d, "a", 44_100, 0.5, Container::Wav, BitDepth::Pcm16);
    let b = tone_file(&d, "b", 48_000, 0.25, Container::Flac, BitDepth::Pcm24);
    let six = multichannel_file(&d, "six", 6);
    let junk = d.join("junk.wav");
    std::fs::write(&junk, b"ce n'est pas de l'audio").unwrap();
    let ev = h.command_json(&format!(r#"{{"type":"probe_audio","files":[{:?},{:?},{:?},{:?}]}}"#, a.to_string_lossy(), b.to_string_lossy(), six.to_string_lossy(), junk.to_string_lossy()));
    let Event::AudioInfo { files } = &ev[0] else { panic!("{ev:?}") };
    assert_eq!((files[0].sample_rate, files[0].channels, files[0].frames, files[0].needs_conversion), (44_100, 2, Some(22_050), true));
    assert_eq!((files[0].format.as_deref(), files[0].name.as_str()), (Some("wav"), "a"));
    assert_eq!((files[1].sample_rate, files[1].needs_conversion, files[1].format.as_deref()), (48_000, false, Some("flac")));
    assert_eq!((files[2].channels, files[2].frames), (6, Some(4_800)));
    assert!(files[3].error.is_some() && files[3].format.is_none());
}

#[test]
fn a_file_is_converted_copied_into_audio_and_placed_on_a_new_track_in_one_undo_step() {
    let (_e, mut h) = host();
    let proj = saved(&mut h, &temp("copy"));
    let src = tone_file(&temp("copy-src"), "Ma boucle", 44_100, 1.0, Container::Wav, BitDepth::Pcm24);
    let before = project(&mut h).nodes.len();
    let d = run(&mut h, &import_json(&[&src], r#","pos":96000"#));
    assert!(d.error.is_none(), "{:?}", d.error);
    assert_eq!(d.clips, vec![ImportedClip { name: "Ma boucle".into(), track: "Ma boucle".into(), converted: true, referenced: false }]);
    assert!(d.progress.windows(2).all(|p| p[1] >= p[0]));
    let p = project(&mut h);
    assert_eq!(p.nodes.len(), before + 1);
    let n = node(&p, "Ma boucle");
    let clips = audio(n);
    assert_eq!(clips.len(), 1);
    assert_eq!((clips[0].0.as_str(), clips[0].1, clips[0].3), ("Ma boucle", 96_000, false));
    assert_eq!(clips[0].2, 48_000, "1 s à 44,1 kHz devient 1 s à 48 kHz");
    // La copie est dans `audio/`, l'original n'a pas bougé.
    assert_eq!(audio_dir_files(&proj).iter().filter(|f| f.starts_with("Ma-boucle")).count(), 1);
    assert!(src.exists());
    // Crêtes : le sinus de 0,5 est bien là, sur toute la durée.
    let id = clip_id(n);
    let pk = peaks(&mut h, id, 96_000, 144_000);
    assert!(pk.iter().all(|c| c[1] > 0.3 && c[0] < -0.3), "{pk:?}");
    // Un seul pas d'annulation : la piste et le clip disparaissent ensemble, puis reviennent.
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(project(&mut h).nodes.len(), before);
    assert!(undo_label(&ev).is_none() || !undo_label(&ev).unwrap().contains("Importer"));
    h.command_json(r#"{"type":"redo"}"#);
    assert_eq!(audio(node(&project(&mut h), "Ma boucle")).len(), 1);
    // Le projet reste ouvrable avec ce clip.
    h.command_json(r#"{"type":"save_project"}"#);
    h.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    let reopened = project(&mut h);
    assert_eq!(audio(node(&reopened, "Ma boucle"))[0].1, 96_000);
    let again = peaks(&mut h, clip_id(node(&reopened, "Ma boucle")), 96_000, 144_000);
    assert!(again.iter().all(|c| c[1] > 0.3));
}

#[test]
fn the_import_label_is_in_the_history() {
    let (_e, mut h) = host();
    saved(&mut h, &temp("label"));
    let src = tone_file(&temp("label-src"), "x", 48_000, 0.2, Container::Wav, BitDepth::Float32);
    let d = run(&mut h, &import_json(&[&src], ""));
    assert_eq!(undo_label(&d.events).as_deref(), Some("Importer de l'audio"));
}

#[test]
fn the_first_file_goes_on_the_chosen_track_and_the_others_on_new_tracks() {
    let (mut engine, mut h) = host();
    saved(&mut h, &temp("target"));
    let s = temp("target-src");
    let a = tone_file(&s, "un", 48_000, 0.3, Container::Wav, BitDepth::Pcm16);
    let b = tone_file(&s, "deux", 48_000, 0.3, Container::Flac, BitDepth::Pcm16);
    let before = project(&mut h);
    let on_bass = audio(node(&before, "Basse")).len();
    let d = run(&mut h, &import_json(&[&a, &b], r#","node":2,"pos":12345"#));
    assert!(d.error.is_none(), "{:?}", d.error);
    assert_eq!(d.clips.iter().map(|c| c.track.as_str()).collect::<Vec<_>>(), ["Basse", "deux"]);
    assert!(d.clips.iter().all(|c| !c.converted));
    let p = project(&mut h);
    assert_eq!(p.nodes.len(), before.nodes.len() + 1);
    let bass = audio(node(&p, "Basse"));
    assert_eq!(bass.len(), on_bass + 1);
    assert!(bass.iter().any(|c| c.0 == "un" && c.1 == 12_345));
    assert_eq!(audio(node(&p, "deux"))[0].1, 12_345);
    // Sans position : à la tête de lecture.
    h.command_json(r#"{"type":"seek","pos":777}"#);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    engine.process(&mut l, &mut r); // le moteur applique le déplacement
    run(&mut h, &import_json(&[&a], ""));
    assert!(audio(node(&project(&mut h), "un")).iter().any(|c| c.1 == 777));
}

#[test]
fn referencing_in_place_copies_nothing_and_survives_a_reopen_and_a_missing_file() {
    let (_e, mut h) = host();
    let proj = saved(&mut h, &temp("ref"));
    let src_dir = temp("ref-src");
    let src = tone_file(&src_dir, "Externe", 48_000, 0.5, Container::Flac, BitDepth::Pcm16);
    let d = run(&mut h, &import_json(&[&src], r#","reference":true,"pos":0"#));
    assert!(d.error.is_none(), "{:?}", d.error);
    assert_eq!(d.clips.len(), 1);
    assert!(d.clips[0].referenced && !d.clips[0].converted);
    assert!(audio_dir_files(&proj).iter().all(|f| !f.contains("Externe")), "rien n'est copié : {:?}", audio_dir_files(&proj));
    h.command_json(r#"{"type":"save_project"}"#);
    assert!(audio_dir_files(&proj).iter().all(|f| !f.contains("Externe")), "l'enregistrement non plus");
    let json = std::fs::read_to_string(proj.join("projet.json")).unwrap();
    assert!(json.contains("Externe.flac") && json.contains("\"frames\": 24000"), "chemin absolu et longueur enregistrés");
    // Rouvert : le fichier est relu depuis sa place (FLAC, aucune copie WAV).
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    let id = clip_id(node(&project(&mut h), "Externe"));
    assert!(peaks(&mut h, id, 0, 24_000).iter().all(|c| c[1] > 0.3));
    // Le fichier disparaît : le projet s'ouvre quand même, le clip est là, signalé manquant, silencieux ; le chemin est conservé.
    std::fs::remove_file(&src).unwrap();
    let ev = h.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    let warnings: Vec<String> = ev.iter().filter_map(|e| if let Event::File { warnings, .. } = e { Some(warnings.clone()) } else { None }).flatten().collect();
    assert!(warnings.iter().any(|w| w.contains("manquant") && w.contains("Externe.flac")), "{warnings:?}");
    let p = project(&mut h);
    let c = &audio(node(&p, "Externe"))[0];
    assert_eq!((c.1, c.2, c.3), (0, 24_000, true));
    assert!(peaks(&mut h, clip_id(node(&p, "Externe")), 0, 24_000).iter().all(|c| c[1] == 0.0 && c[0] == 0.0));
    h.command_json(r#"{"type":"save_project"}"#);
    assert!(std::fs::read_to_string(proj.join("projet.json")).unwrap().contains("Externe.flac"), "le chemin survit à un enregistrement");
    // Il revient : tout redevient normal.
    tone_file(&src_dir, "Externe", 48_000, 0.5, Container::Flac, BitDepth::Pcm16);
    h.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, proj.to_string_lossy()));
    let back = project(&mut h);
    assert!(!audio(node(&back, "Externe"))[0].3);
}

#[test]
fn referencing_falls_back_to_a_copy_when_the_rate_or_the_channel_count_forbids_it() {
    let (_e, mut h) = host();
    let proj = saved(&mut h, &temp("fallback"));
    let src = tone_file(&temp("fallback-src"), "Autre-taux", 44_100, 0.3, Container::Wav, BitDepth::Pcm16);
    let d = run(&mut h, &import_json(&[&src], r#","reference":true"#));
    assert!(d.error.is_none(), "{:?}", d.error);
    assert!(!d.clips[0].referenced && d.clips[0].converted);
    assert!(d.warnings.iter().any(|w| w.contains("copié") && w.contains("44100")), "{:?}", d.warnings);
    assert!(audio_dir_files(&proj).iter().any(|f| f.starts_with("Autre-taux")));
}

#[test]
fn more_than_two_channels_need_a_choice_split_or_a_stereo_mixdown() {
    let (_e, mut h) = host();
    saved(&mut h, &temp("multi"));
    let six = multichannel_file(&temp("multi-src"), "Six", 6);
    // Sans choix : refusé, rien n'est ajouté.
    let n0 = project(&mut h).nodes.len();
    let none = run(&mut h, &import_json(&[&six], ""));
    assert!(none.error.as_deref().is_some_and(|e| e.contains("canaux")), "{:?}", none.error);
    assert_eq!(project(&mut h).nodes.len(), n0);
    // Une piste mono par canal.
    let split = run(&mut h, &import_json(&[&six], r#","multichannel":"split","pos":0"#));
    assert_eq!(split.clips.iter().map(|c| c.name.clone()).collect::<Vec<_>>(), (1..=6).map(|c| format!("Six {c}")).collect::<Vec<_>>());
    let p = project(&mut h);
    assert_eq!(p.nodes.len(), n0 + 6);
    for c in 1..=6 {
        let n = node(&p, &format!("Six {c}"));
        let pk = peaks(&mut h, clip_id(n), 0, 4_800);
        let expected = c as f32 * 0.01;
        assert!(pk.iter().all(|x| (x[1] - expected).abs() < 1e-5), "canal {c} : {:?}", &pk[..2]);
    }
    h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(project(&mut h).nodes.len(), n0, "les six pistes partent en un seul pas");
    // Mixage stéréo : canaux pairs à gauche (0,01 0,03 0,05 → 0,03), impairs à droite (0,02 0,04 0,06 → 0,04).
    let mix = run(&mut h, &import_json(&[&six], r#","multichannel":"stereo","pos":0"#));
    assert_eq!(mix.clips.len(), 1);
    let id = clip_id(node(&project(&mut h), "Six"));
    let pk = peaks(&mut h, id, 0, 4_800);
    assert!(pk.iter().all(|x| x[1] >= 0.03 - 1e-5 && x[1] <= 0.04 + 1e-5), "{:?}", &pk[..2]);
}

#[test]
fn an_mp3_is_imported_and_converted() {
    let (_e, mut h) = host();
    saved(&mut h, &temp("mp3"));
    let mp3 = Path::new(env!("CARGO_MANIFEST_DIR")).join("../recorder/tests/fixtures/st.mp3");
    let d = run(&mut h, &import_json(&[&mp3], r#","pos":0"#));
    assert!(d.error.is_none(), "{:?}", d.error);
    assert!(d.clips[0].converted);
    let p = project(&mut h);
    let c = &audio(node(&p, "st"))[0];
    // 0,5 s à 44,1 kHz (plus le délai de l'encodeur) → environ 24 000 trames à 48 kHz.
    assert!((23_000..28_000).contains(&c.2), "{} trames", c.2);
}

#[test]
fn refusals_and_bad_files_leave_the_project_untouched() {
    let (_e, mut h) = host();
    let src = tone_file(&temp("refus-src"), "x", 48_000, 0.2, Container::Wav, BitDepth::Pcm16);
    let n0 = project(&mut h).nodes.len();
    // Projet sans dossier : copier est impossible, référencer reste possible.
    let e = run(&mut h, &import_json(&[&src], "")).error.unwrap();
    assert!(e.contains("dossier") && e.contains("référencer"), "{e}");
    assert_eq!(project(&mut h).nodes.len(), n0);
    let ok = run(&mut h, &import_json(&[&src], r#","reference":true"#));
    assert!(ok.error.is_none() && ok.clips[0].referenced, "{:?}", ok.error);
    saved(&mut h, &temp("refus"));
    // Fichier illisible : dit clairement, rien d'ajouté.
    let junk = temp("refus-junk").join("faux.wav");
    std::fs::write(&junk, b"RIFFxxxx pas du son").unwrap();
    let n1 = project(&mut h).nodes.len();
    let bad = run(&mut h, &import_json(&[&junk], ""));
    assert!(bad.error.as_deref().is_some_and(|e| e.contains("faux")), "{:?}", bad.error);
    assert_eq!(project(&mut h).nodes.len(), n1);
    // Un bon et un mauvais : le bon est importé, le mauvais est signalé.
    let mixed = run(&mut h, &import_json(&[&junk, &src], r#","pos":0"#));
    assert!(mixed.error.is_none() && mixed.clips.len() == 1, "{:?}", mixed.error);
    assert!(mixed.warnings.iter().any(|w| w.contains("faux")));
    // Validation du protocole.
    for bad in [r#"{"type":"import_audio","files":[]}"#, r#"{"type":"import_audio","files":["a"],"node":0}"#, r#"{"type":"import_audio","files":["a"],"multichannel":"mono"}"#, r#"{"type":"probe_audio","files":[" "]}"#] {
        assert!(matches!(&h.command_json(bad)[0], Event::Error { .. }), "{bad}");
    }
    let not_track = h.command_json(&import_json(&[&src], r#","node":99"#));
    assert!(matches!(&not_track[0], Event::Error { message, .. } if message.contains("piste")));
}

#[test]
fn an_import_can_be_cancelled_without_leaving_anything() {
    let (_e, mut h) = host();
    let proj = saved(&mut h, &temp("cancel"));
    let big = tone_file(&temp("cancel-src"), "gros", 44_100, 40.0, Container::Wav, BitDepth::Float32);
    let n0 = project(&mut h).nodes.len();
    let start = h.command_json(&import_json(&[&big, &big], r#","pos":0"#));
    assert!(start.iter().any(|e| matches!(e, Event::Import { active: true, .. })));
    // Pendant l'import : un second import, et « Nouveau », sont refusés.
    assert!(matches!(&h.command_json(&import_json(&[&big], ""))[0], Event::Error { message, .. } if message.contains("déjà")));
    assert!(matches!(&h.command_json(r#"{"type":"new_project"}"#)[0], Event::Error { message, .. } if message.contains("import")));
    h.command_json(r#"{"type":"cancel_import"}"#);
    let t = Instant::now();
    let end = loop {
        if let Some(e) = h.tick().into_iter().find(|e| matches!(e, Event::Import { active: false, .. })) {
            break e;
        }
        assert!(t.elapsed() < Duration::from_secs(60), "annulation trop lente");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(end, Event::Import { cancelled: true, .. }), "{end:?}");
    assert_eq!(project(&mut h).nodes.len(), n0);
    assert!(audio_dir_files(&proj).iter().all(|f| !f.starts_with("gros")), "{:?}", audio_dir_files(&proj));
    // L'hôte est libre de nouveau.
    assert!(run(&mut h, &import_json(&[&big], r#","pos":0,"reference":true"#)).error.is_none());
}
