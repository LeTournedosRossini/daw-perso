//! Moteur audio : graphe de pistes et de bus, mixage, transport.
//!
//! Aucune dépendance système : compile en WebAssembly. Le thread audio ne touche qu'à [`Engine`] ;
//! tout le reste (édition du graphe, compilation) se fait côté contrôle avec [`EngineHandle`].

mod audio;
pub mod chords;
mod automation;
mod compiled;
mod dynamics;
mod engine;
mod graph;
mod insert;
mod midi;
mod meters;
mod metronome;
mod midi_in;
mod parallel;
mod record;
mod smoother;
mod time;
mod velocity;

pub use audio::{crossfade, AudioData, AudioDataError, Clip, EditError, Fade, FadeShape};
pub use automation::{AutoCurve, AutoLane, AutoMode, AutoPoint, AutoTarget, AUTO_GRID};
pub use compiled::{AheadConfig, AheadParams, AheadStats, AheadSystem, CompiledGraph};
pub use engine::{Command, Engine, EngineHandle, LoadError};
pub use dynamics::InputDynamics;
pub use velocity::VelocityCurve;
pub use graph::{GraphDesc, GraphError, NodeDesc, NodeKind, NodeParams, PhysOut, SendDesc, TrackFormat, MAX_OUT_CHANNELS};
pub use metronome::{Click, ClickTrack, CountIn};
pub use record::{RecordReceiver, RecordSource, RecordStatus, MAX_INPUTS};
pub use meters::{MeterBank, MAX_METERED};
pub use midi::{CtlEvent, MidiClip, MidiEvent, MidiMsg, Note};
pub use midi_in::{parse_midi, MidiIn, MidiInputTx, MidiRecordReceiver, MidiRecordStatus, RecordedMidi};
pub use insert::{Insert, InsertControl, InsertDesc, InsertFactory, InsertId};
pub use parallel::{Pool, PoolStats};
pub use smoother::Smoother;
pub use chords::{ChordSnapshot, Item as ChordItem};
pub use time::{MeterSeg, TempoSeg, TimeMap, TimeShared, TransportInfo};
