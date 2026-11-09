//! Entrée MIDI Windows par WinMM (`midiIn*`), sans dépendance supplémentaire : on déclare les quelques fonctions de
//! `winmm.dll` nous-mêmes.
//!
//! Limites assumées : WinMM appelle notre fonction depuis **son propre fil**, pas depuis le callback audio, et ne donne
//! qu'un horodatage en millisecondes. Les messages sont donc livrés au moteur « au début du prochain bloc » (décalage 0) :
//! gigue ≤ un bloc audio (≈ 1,5 ms à 64 échantillons, 44,1 kHz). C'est correct pour jouer et enregistrer ; un horodatage
//! plus fin (Windows MIDI Services) pourra venir plus tard.
//!
//! Tout le `unsafe` du module est ici.

use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};

type Handle = *mut c_void;

const MMSYSERR_NOERROR: u32 = 0;
const CALLBACK_FUNCTION: u32 = 0x0003_0000;
const MIM_DATA: u32 = 0x3C3;

#[repr(C)]
struct MidiInCapsW {
    manufacturer_id: u16,
    product_id: u16,
    driver_version: u32,
    name: [u16; 32],
    support: u32,
}

#[link(name = "winmm")]
extern "system" {
    fn midiInGetNumDevs() -> u32;
    fn midiInGetDevCapsW(device: usize, caps: *mut MidiInCapsW, size: u32) -> u32;
    fn midiInOpen(handle: *mut Handle, device: u32, callback: usize, instance: usize, flags: u32) -> u32;
    fn midiInStart(handle: Handle) -> u32;
    fn midiInStop(handle: Handle) -> u32;
    fn midiInReset(handle: Handle) -> u32;
    fn midiInClose(handle: Handle) -> u32;
}

/// Noms des entrées MIDI du système, dans l'ordre de leur indice.
pub fn list_inputs() -> Vec<String> {
    // SAFETY: appels de lecture seule de winmm ; `caps` est une structure à la taille annoncée et entièrement écrite par
    // le système en cas de succès.
    unsafe {
        (0..midiInGetNumDevs() as usize)
            .map(|i| {
                let mut caps = MidiInCapsW { manufacturer_id: 0, product_id: 0, driver_version: 0, name: [0; 32], support: 0 };
                if midiInGetDevCapsW(i, &mut caps, std::mem::size_of::<MidiInCapsW>() as u32) == MMSYSERR_NOERROR {
                    let len = caps.name.iter().position(|&c| c == 0).unwrap_or(caps.name.len());
                    String::from_utf16_lossy(&caps.name[..len])
                } else {
                    format!("(entrée {i} illisible)")
                }
            })
            .collect()
    }
}

type Callback = Box<dyn FnMut(&[u8]) + Send>;

/// Entrée MIDI ouverte et démarrée ; fermée proprement à la destruction.
pub struct MidiInput {
    handle: Handle,
    /// Propriétaire de la fonction rappelée ; libéré après la fermeture du port (plus aucun rappel possible).
    callback: *mut Callback,
}

// SAFETY: le handle WinMM peut être fermé depuis n'importe quel fil ; `callback` n'est lu que par le fil WinMM tant que
// le port est ouvert, et libéré seulement après `midiInClose`.
unsafe impl Send for MidiInput {}

extern "system" fn on_message(_: Handle, msg: u32, instance: usize, param1: usize, _timestamp: usize) {
    if msg != MIM_DATA {
        return;
    }
    // SAFETY: `instance` est le pointeur fourni à `midiInOpen`, valide jusqu'à `Drop` (qui ferme d'abord le port).
    let cb = unsafe { &mut *(instance as *mut Callback) };
    let bytes = [(param1 & 0xFF) as u8, ((param1 >> 8) & 0xFF) as u8, ((param1 >> 16) & 0xFF) as u8];
    // Un panic ne doit pas traverser la frontière avec le système.
    let _ = catch_unwind(AssertUnwindSafe(|| cb(&bytes)));
}

impl MidiInput {
    /// Ouvre l'entrée `index` (voir [`list_inputs`]) et appelle `callback` pour chaque message (3 octets : statut et
    /// deux données ; les messages courts sont complétés par des zéros). La fonction tourne dans un fil de WinMM.
    pub fn open(index: u32, callback: impl FnMut(&[u8]) + Send + 'static) -> Result<MidiInput, String> {
        let boxed: *mut Callback = Box::into_raw(Box::new(Box::new(callback) as Callback));
        let mut handle: Handle = std::ptr::null_mut();
        // SAFETY: `handle` est une variable locale valide ; `on_message` a la signature attendue ; `boxed` vit jusqu'à `Drop`.
        let callback_fn: extern "system" fn(Handle, u32, usize, usize, usize) = on_message;
        let r = unsafe { midiInOpen(&mut handle, index, callback_fn as usize, boxed as usize, CALLBACK_FUNCTION) };
        if r != MMSYSERR_NOERROR {
            // SAFETY: le port n'a pas été ouvert, personne ne tient `boxed`.
            drop(unsafe { Box::from_raw(boxed) });
            return Err(format!("midiInOpen({index}) a échoué (code {r}) : entrée inexistante ou déjà utilisée"));
        }
        // SAFETY: `handle` vient d'être ouvert.
        let r = unsafe { midiInStart(handle) };
        let input = MidiInput { handle, callback: boxed };
        if r != MMSYSERR_NOERROR {
            return Err(format!("midiInStart a échoué (code {r})")); // `input` est détruit ici : port fermé
        }
        Ok(input)
    }
}

impl Drop for MidiInput {
    fn drop(&mut self) {
        // SAFETY: `handle` est ouvert (invariant de `open`) ; après `Reset` puis `Close` plus aucun rappel n'aura lieu,
        // ce qui permet de libérer `callback`.
        unsafe {
            midiInStop(self.handle);
            midiInReset(self.handle);
            midiInClose(self.handle);
            drop(Box::from_raw(self.callback));
        }
    }
}
