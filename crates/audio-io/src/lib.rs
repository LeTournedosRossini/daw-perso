//! Abstraction des pilotes audio et implémentations par plateforme.
//!
//! Les modules `alloc_guard`, `spsc` et `latency` sont indépendants de tout pilote et testables
//! sans carte son. Les backends (JACK aujourd'hui, ASIO ensuite) ne font que brancher le callback
//! de la plateforme sur ces briques.

pub mod alloc_guard;
pub mod latency;
pub mod playback;
pub mod rt_priority;
pub mod spsc;
pub mod timing;

#[cfg(target_os = "linux")]
pub mod jack_backend;

// Backend ASIO : Windows seulement ; réel si build.rs a trouvé le SDK Steinberg, sinon version qui échoue proprement.
#[cfg(target_os = "windows")]
pub mod asio_backend;

// Entrée MIDI Windows (WinMM).
#[cfg(target_os = "windows")]
pub mod winmm_midi;

// Backend CoreAudio (macOS). `--cfg coreaudio_check` permet de le compiler (sans le lier) sous Linux pour en vérifier les types.
#[cfg(any(target_os = "macos", coreaudio_check))]
pub mod coreaudio_backend;
#[cfg(any(target_os = "macos", coreaudio_check))]
pub mod coremidi;
