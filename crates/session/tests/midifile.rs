use daw_core_types::time::{Frac, Meter, Tempo};
use daw_core_types::NodeId;
use daw_engine::{GraphDesc, MidiMsg, TrackFormat};
use daw_session::midifile::{parse, write, ImportOptions, MidiFile, MidiFileError, MidiTrack};
use daw_session::{MClip, MEvent, MKind, MNote, Session};

fn f(n: u64, d: u64) -> Frac {
    Frac::new(n, d).unwrap()
}

fn session() -> (Session, NodeId) {
    let mut g = GraphDesc::new();
    let t = g.add_track("Piano", TrackFormat::Stereo);
    (Session::new(g), t)
}

/// Fichier de format 1, 480 ticks par noire, écrit à la main : statut courant, tempo, chiffrage, nom, SysEx,
/// changement de programme, contrôleur, molette, pression.
fn handmade() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"MThd");
    b.extend_from_slice(&[0, 0, 0, 6, 0, 1, 0, 2, 0x01, 0xE0]); // format 1, 2 pistes, 480
    let conductor: Vec<u8> = vec![
        0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, // tempo 500 000 µs = 120
        0x00, 0xFF, 0x58, 0x04, 0x03, 0x02, 0x18, 0x08, // 3/4
        0x00, 0xFF, 0x2F, 0x00,
    ];
    b.extend_from_slice(b"MTrk");
    b.extend_from_slice(&(conductor.len() as u32).to_be_bytes());
    b.extend_from_slice(&conductor);
    let track: Vec<u8> = vec![
        0x00, 0xFF, 0x03, 0x05, b'B', b'a', b's', b's', b'!', // nom
        0x00, 0xC0, 0x05, // changement de programme : ignoré
        0x00, 0xF0, 0x03, 0x7E, 0x01, 0xF7, // SysEx : ignoré
        0x00, 0x90, 0x24, 0x64, // do1, vélocité 100
        0x81, 0x70, 0x24, 0x00, // statut courant : fin de note (vélocité 0) 240 ticks plus tard
        0x00, 0xB0, 0x40, 0x7F, // pédale
        0x00, 0xE0, 0x00, 0x60, // molette (octet faible 0, fort 0x60)
        0x00, 0xD0, 0x30, // pression de canal (2 octets)
        0x60, 0x90, 0x28, 0x50, // mi1 à 96 ticks
        0x81, 0x70, 0x80, 0x28, 0x40, // fin de note (0x80) avec vélocité 64
        0x00, 0xFF, 0x2F, 0x00,
    ];
    b.extend_from_slice(b"MTrk");
    b.extend_from_slice(&(track.len() as u32).to_be_bytes());
    b.extend_from_slice(&track);
    b
}

#[test]
fn parses_a_handmade_file_with_running_status_and_ignored_messages() {
    let m = parse(&handmade()).unwrap();
    assert_eq!((m.ppq, m.tracks.len(), m.tempos.clone(), m.meters.clone()), (480, 2, vec![(0, 500_000)], vec![(0, 3, 2)]));
    assert_eq!(m.skipped, 2, "programme et SysEx");
    let t = &m.tracks[1];
    assert_eq!(t.name.as_deref(), Some("Bass!"));
    assert_eq!(
        t.events,
        vec![
            (0, MidiMsg::NoteOn { channel: 0, key: 0x24, velocity: 100.0 / 127.0 }),
            (240, MidiMsg::NoteOff { channel: 0, key: 0x24, velocity: 0.0 }),
            (240, MidiMsg::ControlChange { channel: 0, controller: 0x40, value: 127 }),
            (240, MidiMsg::PitchBend { channel: 0, value: 4096 }), // 0x00 0x60 : MSB 96 × 128 − 8192
            (240, MidiMsg::ChannelPressure { channel: 0, value: 0x30 }),
            (336, MidiMsg::NoteOn { channel: 0, key: 0x28, velocity: 80.0 / 127.0 }),
            (576, MidiMsg::NoteOff { channel: 0, key: 0x28, velocity: 64.0 / 127.0 }),
        ]
    );
}

#[test]
fn refuses_foreign_unsupported_and_truncated_files_without_panicking() {
    assert_eq!(parse(b"RIFF....").err(), Some(MidiFileError::NotMidi));
    assert_eq!(parse(b"").err(), Some(MidiFileError::NotMidi));
    let mut fmt2 = handmade();
    fmt2[9] = 2;
    assert_eq!(parse(&fmt2).err(), Some(MidiFileError::Unsupported("format 2")));
    let mut smpte = handmade();
    smpte[12] = 0xE7;
    assert!(matches!(parse(&smpte).err(), Some(MidiFileError::Unsupported(_))));
    // Tous les préfixes d'un fichier valide : jamais de panique, et seuls les fichiers complets sont acceptés.
    let ok = handmade();
    for n in 0..ok.len() {
        assert!(parse(&ok[..n]).is_err(), "préfixe de {n} octets accepté");
    }
    // Octets de données invalides.
    let mut bad = handmade();
    let at = bad.windows(3).position(|w| w == [0x90, 0x24, 0x64]).unwrap();
    bad[at + 1] = 0x90;
    assert!(matches!(parse(&bad).err(), Some(MidiFileError::Corrupt(_))));
    // Messages d'une piste corrompue par des octets quelconques : aucune panique.
    let mut x = 12345u32;
    for _ in 0..500 {
        let mut junk = handmade();
        for _ in 0..4 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let i = 14 + (x >> 8) as usize % (junk.len() - 14);
            junk[i] = (x >> 24) as u8;
        }
        let _ = parse(&junk);
    }
}

#[test]
fn write_then_parse_gives_the_same_file() {
    let m = MidiFile {
        ppq: 960,
        tracks: vec![MidiTrack {
            name: Some("Voix".into()),
            events: vec![
                (0, MidiMsg::NoteOn { channel: 3, key: 60, velocity: 1.0 }),
                (100, MidiMsg::PitchBend { channel: 3, value: 8191 }),
                (200, MidiMsg::PolyPressure { channel: 3, key: 60, value: 7 }),
                (480, MidiMsg::NoteOff { channel: 3, key: 60, velocity: 0.0 }),
                (480, MidiMsg::ChannelPressure { channel: 3, value: 99 }),
            ],
        }],
        tempos: vec![(0, 500_000), (3_840, 450_000)],
        meters: vec![(0, 4, 2), (7_680, 7, 3)],
        skipped: 0,
    };
    let back = parse(&write(&m)).unwrap();
    // La piste de tempo et de chiffrage vient en premier, puis les pistes du fichier.
    assert_eq!(back.tracks.len(), 2);
    assert!(back.tracks[0].events.is_empty());
    assert_eq!((back.ppq, back.tempos, back.meters), (960, m.tempos.clone(), m.meters.clone()));
    assert_eq!(back.tracks[1].name, m.tracks[0].name);
    // La fin de note était déjà avant la pression de canal au même instant : tout est rendu à l'identique.
    assert_eq!(back.tracks[1].events, m.tracks[0].events);
    // Et si l'ordre est inverse, l'écriture remet la fin de note d'abord (re-déclenchement net).
    let mut swapped = m.clone();
    swapped.tracks[0].events.swap(3, 4);
    assert_eq!(parse(&write(&swapped)).unwrap().tracks[1].events, m.tracks[0].events);
}

#[test]
fn session_export_then_import_reproduces_notes_events_and_tempo_exactly() {
    let (mut s, t) = session();
    s.set_tempo(f(1, 1), Tempo::new(400, 3).unwrap()).unwrap(); // 450 000 µs par noire : exact
    s.set_meter(2, Meter::new(7, 8).unwrap()).unwrap();
    let id = s.add_midi_clip(t, MClip::new(f(1, 2), f(3, 2))).unwrap();
    s.add_note(id, MNote { start: f(1, 12), len: f(1, 12), key: 64, velocity: 100.0 / 127.0, channel: 0, voice: 1 }).unwrap(); // croche de triolet
    s.add_note(id, MNote { start: f(1, 3), len: f(1, 6), key: 67, velocity: 1.0, channel: 5, voice: 1 }).unwrap();
    s.add_event(id, MEvent { start: f(1, 8), channel: 0, kind: MKind::PitchBend { value: -2_000 } }).unwrap();
    s.add_event(id, MEvent { start: f(1, 4), channel: 0, kind: MKind::ControlChange { controller: 64, value: 127 } }).unwrap();
    let bytes = s.export_midi_file().unwrap();
    let file = parse(&bytes).unwrap();
    assert_eq!(file.tracks.len(), 2);
    assert_eq!(file.tracks[1].name.as_deref(), Some("Piano"));

    let (mut s2, t2) = session();
    let r = s2.import_midi_file(&file, t2, ImportOptions { import_tempo: true }).unwrap();
    assert_eq!((r.clips.len(), r.skipped_messages, r.skipped_meters), (1, 0, 0));
    assert_eq!(s2.tempo_map(), s.tempo_map());
    assert_eq!(s2.meter_map(), s.meter_map());
    let c = s2.midi_clip(r.clips[0]).unwrap();
    // L'import place le clip à 0 et le prolonge jusqu'à la fin de la mesure : on compare notes et événements en absolu.
    let abs = |clip: &MClip| -> (Vec<_>, Vec<_>) {
        (
            clip.notes.iter().map(|n| (clip.start.checked_add(n.start).unwrap(), n.len, n.key, n.channel, n.velocity)).collect(),
            clip.events.iter().map(|e| (clip.start.checked_add(e.start).unwrap(), e.channel, e.kind)).collect(),
        )
    };
    assert_eq!(abs(c), abs(s.midi_clip(id).unwrap()));
    // Une seule action annulable.
    assert_eq!(s2.undo_label(), Some("Importer un fichier MIDI"));
    assert!(s2.undo());
    assert!(s2.midi_clip_ids(t2).is_empty());
    assert_eq!(s2.tempo_map().changes().len(), 1);
}

#[test]
fn import_at_another_resolution_is_exact_and_leaves_the_project_tempo_alone() {
    let (mut s, t) = session();
    let file = parse(&handmade()).unwrap(); // 480 ticks par noire, 3/4, 120
    s.set_tempo(Frac::ZERO, Tempo::bpm(90).unwrap()).unwrap();
    let r = s.import_midi_file(&file, t, ImportOptions::default()).unwrap();
    assert_eq!(r.skipped_messages, 2);
    assert_eq!(s.tempo_map().changes()[0].1, Tempo::bpm(90).unwrap(), "le tempo du projet est gardé");
    assert_eq!(s.meter_map().changes()[0].1, Meter::new(4, 4).unwrap());
    let c = s.midi_clip(r.clips[0]).unwrap();
    // 336 ticks de 480 par noire = 336 / 1920 de ronde, réduit.
    assert_eq!(c.notes[1].start, f(336, 1_920));
    assert_eq!(c.notes[1].len, f(240, 1_920));
    assert_eq!(c.notes[0].len, f(240, 1_920));
    // Clip prolongé jusqu'à la fin de la mesure (4/4 : ronde entière).
    assert_eq!(c.len, f(1, 1));
    assert_eq!(c.events.len(), 3);
    // Avec le tempo et le chiffrage du fichier.
    let (mut s2, t2) = session();
    s2.import_midi_file(&file, t2, ImportOptions { import_tempo: true }).unwrap();
    assert_eq!(s2.tempo_map().changes()[0].1, Tempo::bpm(120).unwrap());
    assert_eq!(s2.meter_map().changes()[0].1, Meter::new(3, 4).unwrap());
    let c2 = s2.midi_clip(s2.midi_clip_ids(t2)[0]).unwrap();
    assert_eq!(c2.len, f(3, 4)); // fin de la mesure en 3/4
}

#[test]
fn meter_changes_inside_a_bar_are_skipped_and_reported() {
    let mut file = parse(&handmade()).unwrap();
    file.meters.push((480, 5, 2)); // un temps après le début : au milieu d'une mesure
    file.meters.push((1_440, 6, 3)); // 3 temps de 480 = fin de la 1re mesure de 3/4 : bien en début de mesure
    let (mut s, t) = session();
    let r = s.import_midi_file(&file, t, ImportOptions { import_tempo: true }).unwrap();
    assert_eq!(r.skipped_meters, 1);
    assert_eq!(s.meter_map().changes().len(), 2);
    assert_eq!(s.meter_map().changes()[1], (1, Meter::new(6, 8).unwrap()));
}

#[test]
fn nothing_to_import_creates_nothing_and_unknown_tempo_is_refused_cleanly() {
    let (mut s, t) = session();
    let empty = MidiFile { ppq: 480, tracks: vec![MidiTrack::default()], tempos: vec![], meters: vec![], skipped: 0 };
    let r = s.import_midi_file(&empty, t, ImportOptions::default()).unwrap();
    assert!(r.clips.is_empty() && !s.can_undo());
    let mut zero_tempo = parse(&handmade()).unwrap();
    zero_tempo.tempos = vec![(0, 0)];
    assert!(s.import_midi_file(&zero_tempo, t, ImportOptions { import_tempo: true }).is_err());
    assert!(!s.can_undo());
}
