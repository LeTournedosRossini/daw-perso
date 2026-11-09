//! Commande `midi-record` : enregistre les notes jouées sur un clavier MIDI (entrée du pilote) pendant que l'on entend
//! un synthétiseur, puis transforme la prise en clip de notes en temps musical et l'affiche.

use std::sync::{Arc, Mutex};

use daw_audio_io::playback::{ControlFn, StereoProcessor};
use daw_core_types::time::{Meter, MeterMap, Tempo, TempoMap};
use daw_core_types::NodeId;
use daw_clap_host::clap_insert_factory;
use daw_engine::{parse_midi, Command, Engine, GraphDesc, MidiInputTx, TrackFormat};
use daw_session::{MidiTakeData, Session, Timing, RECORD_PPQ};

/// Le moteur, avec en plus l'entrée MIDI du pilote dirigée vers une piste.
pub struct MidiProc {
    engine: Engine,
    /// Entrée livrée par le callback audio (JACK) ; `None` quand un autre fil fournit l'entrée (WinMM sous Windows).
    midi: Option<MidiInputTx>,
    node: u32,
}

impl StereoProcessor for MidiProc {
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        self.engine.process_io(in_l, in_r, out_l, out_r);
    }

    fn midi(&mut self, offset: u32, bytes: &[u8]) {
        if let (Some(tx), Some(msg)) = (self.midi.as_mut(), parse_midi(bytes)) {
            let _ = tx.push(self.node, msg, offset);
        }
    }
}

pub type Slot = Arc<Mutex<Option<MidiTakeData>>>;

/// Entrée MIDI confiée à un fil extérieur au callback audio (WinMM) : le moteur y dépose son `MidiInputTx`.
pub type SharedInput = Arc<Mutex<Option<MidiInputTx>>>;

/// Piste qui reçoit le clavier : la première piste du graphe (la sortie principale est le nœud 0).
pub const KEYBOARD_NODE: u32 = 1;

/// `external` : `None` = l'entrée MIDI arrive par `StereoProcessor::midi` (JACK) ; `Some` = un autre fil la fournit et
/// l'entrée du moteur est déposée dans ce conteneur.
pub fn make(seconds: u32, with_synth: bool, rate: u32, slot: &Slot, external: Option<&SharedInput>) -> (MidiProc, ControlFn) {
    let mut g = GraphDesc::new();
    let t = g.add_track("clavier", TrackFormat::Stereo);
    if with_synth {
        match crate::demo::find_analog_synth() {
            Some((bundle, id)) => {
                g.add_insert(t, "AnalogSynth", clap_insert_factory(bundle, id)).expect("la piste vient d'être créée");
            }
            None => eprintln!("AnalogSynth introuvable : les notes sont enregistrées mais on n'entend rien"),
        }
    }
    let (engine, mut handle) = Engine::new(rate, 256);
    handle.load_graph(&g).expect("graphe valide");
    let tx = handle.take_midi_input().expect("première prise de l'entrée MIDI");
    let midi = match external {
        Some(shared) => {
            *shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
            None
        }
        None => Some(tx),
    };
    debug_assert_eq!(t.0, KEYBOARD_NODE);
    let mut rx = handle.start_midi_record(t.0, 100_000).expect("file de commandes vide au départ");
    handle.send(Command::Play);
    let slot = Arc::clone(slot);
    let (mut events, mut stopped) = (Vec::new(), false);
    let control: ControlFn = Box::new(move |elapsed| {
        events.extend(rx.take_events());
        if !stopped && elapsed.as_secs_f64() >= seconds as f64 {
            handle.stop_midi_record(t.0);
            stopped = true;
        }
        if stopped && rx.is_closed() {
            events.extend(rx.take_events());
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(MidiTakeData { start_pos: rx.start_pos(), stop_pos: rx.stop_pos(), events: std::mem::take(&mut events) });
            return false;
        }
        true
    });
    (MidiProc { engine, midi, node: t.0 }, control)
}

/// Affiche la prise sous forme de clip de notes (durées et positions en ticks de 960 par noire).
pub fn report(slot: &Slot, rate: u32, bpm: u32) -> i32 {
    let Some(take) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        eprintln!("la prise n'a pas pu être terminée (le moteur n'a pas rendu la main à temps)");
        return 1;
    };
    println!("Prise : {} messages de note, de l'échantillon {} à {}", take.events.len(), take.start_pos, take.stop_pos);
    let (Some(tempo), Some(meter)) = (Tempo::bpm(bpm), Meter::new(4, 4)) else {
        eprintln!("tempo invalide");
        return 1;
    };
    let mut g = GraphDesc::new();
    let t = g.add_track("clavier", TrackFormat::Stereo);
    let mut s = Session::with_timing(g, rate, Timing { tempo: TempoMap::new(tempo), meters: MeterMap::new(meter) });
    match s.add_recorded_take(NodeId(t.0), &take, 0) {
        Ok(Some(id)) => {
            let c = s.midi_clip(id).expect("clip créé");
            let ticks = |f: daw_core_types::time::Frac| TempoMap::to_ticks(f, RECORD_PPQ).unwrap_or(0);
            println!("Clip de notes : {} notes, début {} ticks, durée {} ticks ({} par noire)", c.notes.len(), ticks(c.start), ticks(c.len), RECORD_PPQ);
            for n in &c.notes {
                println!("  touche {:3}  canal {:2}  vélocité {:3}  début {:6}  durée {:5}", n.key, n.channel + 1, (n.velocity * 127.0).round() as u32, ticks(n.start), ticks(n.len));
            }
            if !c.events.is_empty() {
                println!("Contrôleurs, molette et pressions : {}", c.events.len());
                for e in &c.events {
                    let what = match e.kind {
                        daw_session::MKind::ControlChange { controller, value } => format!("contrôleur {controller:3} = {value:3}"),
                        daw_session::MKind::PitchBend { value } => format!("molette de hauteur {value:+5}"),
                        daw_session::MKind::ChannelPressure { value } => format!("pression de canal {value:3}"),
                        daw_session::MKind::PolyPressure { key, value } => format!("pression de la touche {key:3} = {value:3}"),
                    };
                    println!("  canal {:2}  début {:6}  {what}", e.channel + 1, ticks(e.start));
                }
            }
            0
        }
        Ok(None) => {
            println!("Aucune note jouée.");
            0
        }
        Err(e) => {
            eprintln!("conversion impossible : {e:?}");
            1
        }
    }
}
