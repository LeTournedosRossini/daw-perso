//! Les paroles : édition par le protocole, annulation, refus sans trace, projet enregistré et rouvert.

use daw_engine::Engine;
use daw_ui::demo::demo_session;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

fn host() -> Host {
    let (_engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    std::mem::forget(_engine);
    Host::new(handle, session, meta).unwrap()
}

fn lyrics(ev: &[Event]) -> Option<Vec<(String, [u64; 2], bool, u8)>> {
    ev.iter().rev().find_map(|e| {
        if let Event::Chords { lyrics, .. } = e {
            Some(lyrics.iter().map(|l| (l.text.clone(), l.at, l.hyphen, l.verse)).collect())
        } else {
            None
        }
    })
}

fn errors(ev: &[Event]) -> Vec<String> {
    ev.iter().filter_map(|e| if let Event::Error { message, .. } = e { Some(message.clone()) } else { None }).collect()
}

const SONG: &str = r##"{"type":"set_lyrics","syllables":[
  {"at":[1,4],"text":"man","verse":1},
  {"at":[0,1],"text":"ma","hyphen":true},
  {"at":[1,4],"text":"mie","verse":2}]}"##;

#[test]
fn lyrics_are_set_sorted_undone_and_saved_with_the_project() {
    let dir = std::env::temp_dir().join(format!("daw-lyrics-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = host();
    assert_eq!(lyrics(&h.command_json(r#"{"type":"get_project"}"#)).unwrap().len(), 0);

    let ev = h.command_json(SONG);
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let list = lyrics(&ev).expect("événement chords avec les paroles");
    assert_eq!(
        list,
        vec![("ma".to_string(), [0, 1], true, 1), ("man".to_string(), [1, 4], false, 1), ("mie".to_string(), [1, 4], false, 2)],
        "triées par instant puis par couplet"
    );

    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(lyrics(&ev).unwrap().len(), 0);
    let ev = h.command_json(r#"{"type":"redo"}"#);
    assert_eq!(lyrics(&ev).unwrap().len(), 3);

    let path = dir.join("essai.daw");
    let p = serde_json::to_string(&path.to_string_lossy()).unwrap();
    let ev = h.command_json(&format!(r#"{{"type":"save_project","path":{p}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let mut h2 = host();
    let ev = h2.command_json(&format!(r#"{{"type":"open_project","path":{p}}}"#));
    assert!(errors(&ev).is_empty(), "{:?}", errors(&ev));
    let ev = h2.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(lyrics(&ev).unwrap().len(), 3, "relues à l'ouverture");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_lyrics_are_refused_without_a_trace() {
    let mut h = host();
    h.command_json(SONG);
    let bad = [
        r#"{"type":"set_lyrics","syllables":[{"at":[0,1],"text":""}]}"#,
        r#"{"type":"set_lyrics","syllables":[{"at":[0,1],"text":"deux mots"}]}"#,
        r#"{"type":"set_lyrics","syllables":[{"at":[0,1],"text":"ma","verse":0}]}"#,
        r#"{"type":"set_lyrics","syllables":[{"at":[0,1],"text":"ma","verse":9}]}"#,
        r#"{"type":"set_lyrics","syllables":[{"at":[1,0],"text":"ma"}]}"#,
        r#"{"type":"set_lyrics","syllables":[{"at":[0,1],"text":"ma"},{"at":[0,1],"text":"ma"}]}"#,
    ];
    for b in bad {
        let ev = h.command_json(b);
        assert!(!errors(&ev).is_empty(), "devrait être refusé : {b}");
    }
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert_eq!(lyrics(&ev).unwrap().len(), 3, "rien n'a changé");
    let ev = h.command_json(r#"{"type":"undo"}"#);
    assert_eq!(lyrics(&ev).unwrap().len(), 0, "une seule action dans l'historique");
}
