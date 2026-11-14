//! Entrée MIDI macOS par CoreMIDI, sans dépendance supplémentaire : les quelques fonctions du framework sont déclarées à la main
//! (même interface que `winmm_midi` : `list_inputs`, `MidiInput::open`).
//!
//! **Écrit sans Mac, jamais exécuté** : seuls les types (compilation avec `--cfg coreaudio_check` sous Linux, ou la CI macOS) et le
//! découpage des paquets en messages sont vérifiés.
//!
//! CoreMIDI appelle notre fonction depuis **son propre fil temps réel**, avec des paquets horodatés (nanosecondes d'horloge hôte). On
//! n'utilise pas l'horodatage : comme avec WinMM, les messages sont livrés au moteur « au début du prochain bloc » (gigue ≤ un bloc).
//! Une source est ouverte par appareil ; les appareils branchés après le démarrage ne sont pas vus (liste lue une fois).
//!
//! Tout le `unsafe` du module est ici.

use std::ffi::{c_char, c_void, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

type OsStatus = i32;
type Ref = u32;
type CfString = *const c_void;

#[cfg_attr(target_os = "macos", link(name = "CoreMIDI", kind = "framework"))]
extern "C" {
    static kMIDIPropertyDisplayName: CfString;
    fn MIDIGetNumberOfSources() -> usize;
    fn MIDIGetSource(index: usize) -> Ref;
    fn MIDIObjectGetStringProperty(object: Ref, property_id: CfString, string: *mut CfString) -> OsStatus;
    fn MIDIClientCreate(name: CfString, notify: *const c_void, notify_ref_con: *mut c_void, client: *mut Ref) -> OsStatus;
    fn MIDIClientDispose(client: Ref) -> OsStatus;
    fn MIDIInputPortCreate(client: Ref, port_name: CfString, read_proc: extern "C" fn(*const u8, *mut c_void, *mut c_void), ref_con: *mut c_void, port: *mut Ref) -> OsStatus;
    fn MIDIPortConnectSource(port: Ref, source: Ref, conn_ref_con: *mut c_void) -> OsStatus;
    fn MIDIPortDisconnectSource(port: Ref, source: Ref) -> OsStatus;
    fn MIDIPortDispose(port: Ref) -> OsStatus;
}

#[cfg_attr(target_os = "macos", link(name = "CoreFoundation", kind = "framework"))]
extern "C" {
    fn CFStringCreateWithCString(allocator: *const c_void, c_str: *const c_char, encoding: u32) -> CfString;
    fn CFStringGetCString(string: CfString, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(object: *const c_void);
}

const UTF8: u32 = 0x0800_0100;

fn cf_string(s: &str) -> CfString {
    let c = CString::new(s).unwrap_or_default();
    // SAFETY: `c` est une chaîne C valide pendant l'appel ; allocateur par défaut ; le résultat est libéré par l'appelant (`CFRelease`).
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
}

fn string_of(cf: CfString) -> Option<String> {
    let mut buf = [0 as c_char; 256];
    // SAFETY: `cf` est une `CFString` valide ; `buf` est un tampon de la taille annoncée.
    let ok = unsafe { CFStringGetCString(cf, buf.as_mut_ptr(), buf.len() as isize, UTF8) } != 0;
    ok.then(|| {
        // SAFETY: en cas de succès `buf` contient une chaîne C terminée par un octet nul.
        unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
    })
}

fn source_name(source: Ref, index: usize) -> String {
    let mut name: CfString = std::ptr::null();
    // SAFETY: `source` vient de `MIDIGetSource` ; `name` est une variable locale valide ; la chaîne rendue nous appartient (`CFRelease`).
    unsafe {
        if MIDIObjectGetStringProperty(source, kMIDIPropertyDisplayName, &mut name) == 0 && !name.is_null() {
            let s = string_of(name);
            CFRelease(name);
            if let Some(s) = s {
                return s;
            }
        }
    }
    format!("(entrée {index} illisible)")
}

/// Noms des entrées MIDI du système, dans l'ordre de leur indice.
pub fn list_inputs() -> Vec<String> {
    // SAFETY: appels de lecture seule de CoreMIDI.
    unsafe { (0..MIDIGetNumberOfSources()).map(|i| source_name(MIDIGetSource(i), i)).collect() }
}

/// Découpe les octets d'un paquet CoreMIDI en messages de **3 octets** (statut et deux données, complétés par des zéros comme avec
/// WinMM). Messages système (0xF0 et plus : SysEx, horloge…) et octets de données sans statut sont ignorés.
pub fn split_messages(data: &[u8], mut each: impl FnMut([u8; 3])) {
    let mut i = 0;
    while i < data.len() {
        let status = data[i];
        if status < 0x80 {
            i += 1; // donnée sans statut : ignorée
            continue;
        }
        if status >= 0xF0 {
            // Message système : on saute jusqu'au prochain octet de statut (le SysEx finit à 0xF7).
            i += 1;
            while i < data.len() && data[i] < 0x80 {
                i += 1;
            }
            if status == 0xF0 && i < data.len() && data[i] == 0xF7 {
                i += 1;
            }
            continue;
        }
        let len = if matches!(status & 0xF0, 0xC0 | 0xD0) { 2 } else { 3 };
        if i + len > data.len() {
            break; // message tronqué
        }
        let mut m = [status, data[i + 1], 0];
        if len == 3 {
            m[2] = data[i + 2];
        }
        each(m);
        i += len;
    }
}

type Callback = Box<dyn FnMut(&[u8]) + Send>;

extern "C" fn on_packets(list: *const u8, ref_con: *mut c_void, _src: *mut c_void) {
    if list.is_null() || ref_con.is_null() {
        return;
    }
    // SAFETY: `ref_con` est le pointeur fourni à `MIDIInputPortCreate` (voir `MidiInput` : jamais libéré tant que le programme tourne).
    let cb = unsafe { &mut *(ref_con as *mut Callback) };
    // Un panic ne doit pas traverser la frontière avec le système.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `list` est une `MIDIPacketList` valide pendant l'appel : `numPackets` (u32) puis des paquets de taille variable
        // (horodatage u64, longueur u16, octets), lus sans supposer d'alignement.
        unsafe {
            let n = (list as *const u32).read_unaligned();
            let mut p = list.add(4);
            for _ in 0..n {
                let len = (p.add(8) as *const u16).read_unaligned() as usize;
                let data = std::slice::from_raw_parts(p.add(10), len.min(256));
                split_messages(data, |m| cb(&m));
                p = packet_next(p.add(10 + len));
            }
        }
    }));
}

/// Début du paquet suivant : sur ARM `MIDIPacketNext` aligne l'adresse sur 4 octets, pas sur x86.
fn packet_next(end_of_data: *const u8) -> *const u8 {
    if cfg!(any(target_arch = "aarch64", target_arch = "arm")) {
        ((end_of_data as usize + 3) & !3) as *const u8
    } else {
        end_of_data
    }
}

/// Entrée MIDI ouverte et connectée ; déconnectée à la destruction.
pub struct MidiInput {
    client: Ref,
    port: Ref,
    source: Ref,
}

// SAFETY: les références CoreMIDI sont de simples entiers, utilisables depuis n'importe quel fil.
unsafe impl Send for MidiInput {}

impl MidiInput {
    /// Ouvre l'entrée `index` (voir [`list_inputs`]) et appelle `callback` pour chaque message (3 octets, voir
    /// [`split_messages`]). La fonction tourne dans un fil de CoreMIDI.
    pub fn open(index: u32, callback: impl FnMut(&[u8]) + Send + 'static) -> Result<MidiInput, String> {
        // SAFETY: lecture du nombre de sources.
        let count = unsafe { MIDIGetNumberOfSources() };
        if index as usize >= count {
            return Err(format!("entrée MIDI {index} inexistante ({count} entrée(s))"));
        }
        // SAFETY: indice vérifié ci-dessus.
        let source = unsafe { MIDIGetSource(index as usize) };
        // Propriétaire de la fonction rappelée : **volontairement jamais libéré** (une fuite de quelques octets à l'arrêt) car rien ne
        // garantit qu'aucun rappel n'est encore en vol quand le port est détruit.
        let boxed: *mut Callback = Box::into_raw(Box::new(Box::new(callback) as Callback));
        let (name_c, name_p) = (cf_string("DAW"), cf_string("DAW entrée"));
        let (mut client, mut port) = (0 as Ref, 0 as Ref);
        // SAFETY: chaînes CF valides ; pointeurs de sortie valides ; `on_packets` a la signature attendue ; `boxed` vit jusqu'à la fin du
        // programme ; les chaînes CF sont libérées après usage (CoreMIDI les copie).
        let status = unsafe {
            let mut s = MIDIClientCreate(name_c, std::ptr::null(), std::ptr::null_mut(), &mut client);
            if s == 0 {
                s = MIDIInputPortCreate(client, name_p, on_packets, boxed as *mut c_void, &mut port);
            }
            if s == 0 {
                s = MIDIPortConnectSource(port, source, std::ptr::null_mut());
            }
            CFRelease(name_c);
            CFRelease(name_p);
            s
        };
        let input = MidiInput { client, port, source };
        if status != 0 {
            return Err(format!("CoreMIDI a refusé d'ouvrir l'entrée {index} (code {status})")); // `input` détruit ici
        }
        Ok(input)
    }
}

impl Drop for MidiInput {
    fn drop(&mut self) {
        // SAFETY: références créées par `open` (ou nulles si l'ouverture a échoué avant : les appels échouent alors sans effet).
        unsafe {
            if self.port != 0 {
                MIDIPortDisconnectSource(self.port, self.source);
                MIDIPortDispose(self.port);
            }
            if self.client != 0 {
                MIDIClientDispose(self.client);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::split_messages;

    fn all(data: &[u8]) -> Vec<[u8; 3]> {
        let mut v = Vec::new();
        split_messages(data, |m| v.push(m));
        v
    }

    #[test]
    fn notes_and_controllers_are_split_into_three_byte_messages() {
        assert_eq!(all(&[0x90, 60, 100, 0x80, 60, 0]), [[0x90, 60, 100], [0x80, 60, 0]]);
        assert_eq!(all(&[0xB0, 64, 127]), [[0xB0, 64, 127]]);
        assert_eq!(all(&[0xE2, 0, 64]), [[0xE2, 0, 64]]);
    }

    #[test]
    fn two_byte_messages_are_padded() {
        assert_eq!(all(&[0xC3, 5, 0xD0, 90]), [[0xC3, 5, 0], [0xD0, 90, 0]]);
    }

    #[test]
    fn system_messages_and_stray_data_are_ignored_and_truncated_ones_stop() {
        assert_eq!(all(&[0xF8, 0x90, 60, 100]), [[0x90, 60, 100]]);
        assert_eq!(all(&[0xF0, 1, 2, 3, 0xF7, 0x90, 61, 90]), [[0x90, 61, 90]]);
        assert_eq!(all(&[7, 8, 0x90, 62, 80]), [[0x90, 62, 80]]);
        assert_eq!(all(&[0x90, 60]), Vec::<[u8; 3]>::new());
        assert_eq!(all(&[]), Vec::<[u8; 3]>::new());
    }
}
