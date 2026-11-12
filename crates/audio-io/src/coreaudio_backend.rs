//! Backend CoreAudio (macOS) : une « audio unit » AUHAL qui joue (et, si on le demande, enregistre) sur un périphérique.
//!
//! **Jamais exécuté** : ce module a été écrit sans Mac. Il est vérifié par la compilation (CI macOS) et par `cargo check` avec
//! `--cfg coreaudio_check` sous Linux ; ni le son, ni les latences annoncées, ni les types exacts des structures C n'ont été essayés
//! sur un vrai système. Les déclarations C sont écrites à la main (comme `winmm_midi`) : aucune dépendance ajoutée.
//!
//! Choix : AUHAL (`kAudioUnitSubType_HALOutput`) plutôt que `AudioDeviceIOProc`, parce qu'il convertit les formats à notre place
//! (flottants non entrelacés, un tampon par canal). Le callback de rendu est appelé par le fil temps réel de CoreAudio ; il applique
//! les mêmes règles que les autres backends (aucune allocation, panics contenus et remplacés par du silence).

use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::alloc_guard::{self, RtScope};
use crate::playback::{ControlFn, PlaybackReport, StereoProcessor, StreamInfo, StreamLatency, TimedProcessor};

type OSStatus = i32;
type AudioUnit = *mut c_void;

/// Nombre maximal de canaux ouverts dans chaque sens (comme ASIO).
const MAX_CHANNELS: usize = 32;

// --- Constantes (codes de quatre caractères de CoreAudio, lus comme des entiers gros-boutistes) ---
const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | s[3] as u32
}
const SYSTEM_OBJECT: u32 = 1; // kAudioObjectSystemObject
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_INPUT: u32 = fourcc(b"inpt");
const SCOPE_OUTPUT: u32 = fourcc(b"outp");
const PROP_DEVICES: u32 = fourcc(b"dev#"); // kAudioHardwarePropertyDevices
const PROP_DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
const PROP_NAME: u32 = fourcc(b"lnam"); // kAudioObjectPropertyName
const PROP_STREAM_CONFIG: u32 = fourcc(b"slay"); // kAudioDevicePropertyStreamConfiguration
const PROP_NOMINAL_RATE: u32 = fourcc(b"nsrt"); // kAudioDevicePropertyNominalSampleRate
const PROP_BUFFER_FRAMES: u32 = fourcc(b"fsiz"); // kAudioDevicePropertyBufferFrameSize
const PROP_DEVICE_LATENCY: u32 = fourcc(b"ltnc"); // kAudioDevicePropertyLatency
const PROP_SAFETY_OFFSET: u32 = fourcc(b"saft"); // kAudioDevicePropertySafetyOffset

// Propriétés d'audio unit (AudioUnitProperties.h / AudioOutputUnit.h).
const AU_STREAM_FORMAT: u32 = 8; // kAudioUnitProperty_StreamFormat
const AU_MAX_FRAMES: u32 = 14; // kAudioUnitProperty_MaximumFramesPerSlice
const AU_SET_RENDER_CALLBACK: u32 = 23; // kAudioUnitProperty_SetRenderCallback
const AU_CURRENT_DEVICE: u32 = 2000; // kAudioOutputUnitProperty_CurrentDevice
const AU_ENABLE_IO: u32 = 2003; // kAudioOutputUnitProperty_EnableIO
const AU_SCOPE_GLOBAL: u32 = 0;
const AU_SCOPE_INPUT: u32 = 1;
const AU_SCOPE_OUTPUT: u32 = 2;

const FORMAT_LPCM: u32 = fourcc(b"lpcm");
const FLAG_FLOAT_PACKED_NONINTERLEAVED: u32 = 1 | 8 | 32; // IsFloat | IsPacked | IsNonInterleaved
const CF_UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
struct ComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct StreamDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    number_channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

/// `AudioBufferList` : en-tête suivi de `number_buffers` tampons (tableau de longueur variable en C).
#[repr(C)]
struct AudioBufferList {
    number_buffers: u32,
    buffers: [AudioBuffer; 1],
}

type RenderProc = extern "C" fn(*mut c_void, *mut u32, *const c_void, u32, u32, *mut AudioBufferList) -> OSStatus;

#[repr(C)]
struct RenderCallback {
    input_proc: RenderProc,
    input_proc_ref_con: *mut c_void,
}

#[cfg_attr(target_os = "macos", link(name = "CoreAudio", kind = "framework"))]
extern "C" {
    fn AudioObjectGetPropertyDataSize(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, size: *mut u32) -> OSStatus;
    fn AudioObjectGetPropertyData(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, size: *mut u32, data: *mut c_void) -> OSStatus;
    fn AudioObjectSetPropertyData(object: u32, address: *const PropertyAddress, qualifier_size: u32, qualifier: *const c_void, size: u32, data: *const c_void) -> OSStatus;
}

#[cfg_attr(target_os = "macos", link(name = "AudioToolbox", kind = "framework"))]
extern "C" {
    fn AudioComponentFindNext(component: *mut c_void, description: *const ComponentDescription) -> *mut c_void;
    fn AudioComponentInstanceNew(component: *mut c_void, unit: *mut AudioUnit) -> OSStatus;
    fn AudioComponentInstanceDispose(unit: AudioUnit) -> OSStatus;
    fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
    fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStart(unit: AudioUnit) -> OSStatus;
    fn AudioOutputUnitStop(unit: AudioUnit) -> OSStatus;
    fn AudioUnitSetProperty(unit: AudioUnit, property: u32, scope: u32, element: u32, data: *const c_void, size: u32) -> OSStatus;
    fn AudioUnitGetProperty(unit: AudioUnit, property: u32, scope: u32, element: u32, data: *mut c_void, size: *mut u32) -> OSStatus;
    fn AudioUnitRender(unit: AudioUnit, action_flags: *mut u32, time_stamp: *const c_void, bus: u32, frames: u32, data: *mut AudioBufferList) -> OSStatus;
}

#[cfg_attr(target_os = "macos", link(name = "CoreFoundation", kind = "framework"))]
extern "C" {
    fn CFStringGetCString(string: *const c_void, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFRelease(object: *const c_void);
}

fn check(what: &str, status: OSStatus) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!("CoreAudio : {what} a échoué (code {status})"))
    }
}

fn address(selector: u32, scope: u32) -> PropertyAddress {
    PropertyAddress { selector, scope, element: 0 }
}

/// Lit une propriété scalaire d'un objet audio.
fn get_scalar<T: Copy + Default>(object: u32, selector: u32, scope: u32) -> Result<T, OSStatus> {
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    let a = address(selector, scope);
    // SAFETY: `value` est un `T` valide de `size` octets où CoreAudio écrit au plus `size` octets ; `a` vit pendant l'appel.
    let st = unsafe { AudioObjectGetPropertyData(object, &a, 0, std::ptr::null(), &mut size, (&mut value as *mut T).cast()) };
    if st == 0 {
        Ok(value)
    } else {
        Err(st)
    }
}

fn device_name(id: u32) -> String {
    let mut cf: *const c_void = std::ptr::null();
    let mut size = std::mem::size_of::<*const c_void>() as u32;
    let a = address(PROP_NAME, SCOPE_GLOBAL);
    // SAFETY: `cf` est un pointeur où CoreAudio dépose une `CFStringRef` (que nous devons libérer) ; `size` vaut sa taille.
    let st = unsafe { AudioObjectGetPropertyData(id, &a, 0, std::ptr::null(), &mut size, (&mut cf as *mut *const c_void).cast()) };
    if st != 0 || cf.is_null() {
        return format!("périphérique {id}");
    }
    let mut buf = [0 as c_char; 256];
    // SAFETY: `cf` est une CFString valide ; `buf` fait 256 octets et la fonction y écrit une chaîne terminée par 0 ; `cf` est ensuite
    // libérée une seule fois (la propriété « Name » est rendue avec un compte de références à notre charge).
    let name = unsafe {
        let ok = CFStringGetCString(cf, buf.as_mut_ptr(), buf.len() as isize, CF_UTF8) != 0;
        CFRelease(cf);
        if ok {
            CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
        } else {
            format!("périphérique {id}")
        }
    };
    name
}

/// Nombre total de canaux d'un périphérique dans un sens (`SCOPE_INPUT` ou `SCOPE_OUTPUT`).
fn channel_count(id: u32, scope: u32) -> u32 {
    let a = address(PROP_STREAM_CONFIG, scope);
    let mut size = 0u32;
    // SAFETY: appel de lecture de taille, `a` et `size` valides.
    if unsafe { AudioObjectGetPropertyDataSize(id, &a, 0, std::ptr::null(), &mut size) } != 0 || (size as usize) < std::mem::size_of::<u32>() {
        return 0;
    }
    // Stockage en mots de 8 octets : l'alignement de `AudioBufferList` (pointeurs) est ainsi respecté.
    let mut backing = vec![0u64; (size as usize).div_ceil(8)];
    let mut got = size;
    // SAFETY: `backing` fait au moins `size` octets ; CoreAudio y écrit au plus `got` octets.
    if unsafe { AudioObjectGetPropertyData(id, &a, 0, std::ptr::null(), &mut got, backing.as_mut_ptr().cast()) } != 0 {
        return 0;
    }
    // SAFETY: `backing` contient une `AudioBufferList` écrite par CoreAudio ; on ne lit que `number_buffers` tampons, tous dans
    // les `got` octets reçus (on borne le nombre par la taille reçue).
    unsafe {
        let list = backing.as_ptr() as *const AudioBufferList;
        let header = std::mem::size_of::<u64>(); // en-tête avec remplissage : `buffers` commence à l'octet 8
        let max = (got as usize).saturating_sub(header) / std::mem::size_of::<AudioBuffer>();
        let n = ((*list).number_buffers as usize).min(max);
        let first = (*list).buffers.as_ptr();
        (0..n).map(|k| (*first.add(k)).number_channels).sum()
    }
}

/// Un périphérique audio du système.
#[derive(Debug, Clone)]
pub struct DeviceEntry {
    pub id: u32,
    pub name: String,
    pub inputs: u32,
    pub outputs: u32,
    pub sample_rate: f64,
    /// Périphérique de sortie par défaut du système.
    pub default_output: bool,
}

/// Liste les périphériques audio.
pub fn list_devices() -> Result<Vec<DeviceEntry>, String> {
    let a = address(PROP_DEVICES, SCOPE_GLOBAL);
    let mut size = 0u32;
    // SAFETY: lecture de taille sur l'objet système, `a` et `size` valides.
    check("lecture de la liste des périphériques", unsafe { AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &a, 0, std::ptr::null(), &mut size) })?;
    let mut ids = vec![0u32; size as usize / std::mem::size_of::<u32>()];
    let mut got = size;
    // SAFETY: `ids` fait `size` octets.
    check("lecture de la liste des périphériques", unsafe { AudioObjectGetPropertyData(SYSTEM_OBJECT, &a, 0, std::ptr::null(), &mut got, ids.as_mut_ptr().cast()) })?;
    ids.truncate(got as usize / std::mem::size_of::<u32>());
    let default_out = get_scalar::<u32>(SYSTEM_OBJECT, PROP_DEFAULT_OUTPUT, SCOPE_GLOBAL).unwrap_or(0);
    Ok(ids
        .into_iter()
        .map(|id| DeviceEntry {
            id,
            name: device_name(id),
            inputs: channel_count(id, SCOPE_INPUT),
            outputs: channel_count(id, SCOPE_OUTPUT),
            sample_rate: get_scalar::<f64>(id, PROP_NOMINAL_RATE, SCOPE_GLOBAL).unwrap_or(0.0),
            default_output: id == default_out,
        })
        .collect())
}

#[derive(Debug, Clone, Default)]
pub struct PlayOptions {
    pub seconds: u32,
    /// Morceau du nom du périphérique (sans tenir compte de la casse) ; sinon la sortie par défaut du système.
    pub device: Option<String>,
    pub buffer: Option<u32>,
    /// Nombre de canaux d'entrée ouverts (plafonné à ce que le périphérique a et à 32) ; 0 : aucune entrée — et donc aucune demande
    /// d'autorisation du micro au système.
    pub input_count: u32,
    /// Nombre de canaux de sortie ouverts (au moins 2, plafonné à ce que le périphérique a et à 32).
    pub output_count: u32,
}

/// Tout ce que le callback de rendu manipule ; vit dans une boîte fixe tant que l'unité tourne.
struct Play<P> {
    unit: AudioUnit,
    tp: TimedProcessor<P>,
    n_in: usize,
    n_out: usize,
    max_frames: usize,
    /// Stockage (alignement de 8 octets) d'une `AudioBufferList` à `n_in` tampons, remplie avant chaque `AudioUnitRender`.
    in_list: Vec<u64>,
    in_data: Vec<Vec<f32>>,
    silence: Vec<f32>,
}

impl<P: StereoProcessor> Play<P> {
    /// Rend un cycle. Faux si quelque chose cloche (le tampon de sortie est alors mis à zéro par l'appelant).
    ///
    /// # Safety
    /// `data` doit être la liste de tampons fournie par CoreAudio pour ce cycle, `flags` et `ts` ceux du même appel.
    unsafe fn run(&mut self, flags: *mut u32, ts: *const c_void, frames: usize, data: *mut AudioBufferList) -> bool {
        if frames > self.max_frames || data.is_null() || (*data).number_buffers as usize != self.n_out {
            return false;
        }
        // Entrées : CoreAudio les remplit dans nos propres tampons.
        if self.n_in > 0 {
            let list = self.in_list.as_mut_ptr() as *mut AudioBufferList;
            (*list).number_buffers = self.n_in as u32;
            let first = (*list).buffers.as_mut_ptr();
            for (k, d) in self.in_data.iter_mut().enumerate() {
                let b = first.add(k);
                (*b).number_channels = 1;
                (*b).data_byte_size = (frames * 4) as u32;
                (*b).data = d.as_mut_ptr().cast();
            }
            if AudioUnitRender(self.unit, flags, ts, 1, frames as u32, list) != 0 {
                for d in self.in_data.iter_mut() {
                    d[..frames].fill(0.0);
                }
            }
        }
        let first = (*data).buffers.as_mut_ptr();
        for k in 0..self.n_out {
            if (*first.add(k)).data.is_null() {
                return false;
            }
        }
        let in_data = &self.in_data;
        let silence = &self.silence[..frames];
        let n_in = self.n_in;
        let inputs: [&[f32]; MAX_CHANNELS + 1] = std::array::from_fn(|k| if k < n_in { &in_data[k][..frames] } else { silence });
        let mut outs: [&mut [f32]; MAX_CHANNELS] = std::array::from_fn(|k| {
            if k < self.n_out {
                // SAFETY: chaque tampon de sortie est une zone distincte de `frames` flottants fournie par CoreAudio (non entrelacé).
                std::slice::from_raw_parts_mut((*first.add(k)).data as *mut f32, frames)
            } else {
                &mut []
            }
        });
        self.tp.process_outputs(&inputs[..n_in.max(2)], &mut outs[..self.n_out]);
        true
    }
}

extern "C" fn render<P: StereoProcessor>(ref_con: *mut c_void, flags: *mut u32, ts: *const c_void, _bus: u32, frames: u32, data: *mut AudioBufferList) -> OSStatus {
    // SAFETY: `ref_con` est le pointeur vers la boîte `Play<P>` que `run_playback_ex` garde en vie jusqu'après l'arrêt de l'unité.
    let play = unsafe { &mut *(ref_con as *mut Play<P>) };
    let ok = catch_unwind(AssertUnwindSafe(|| {
        let _rt = RtScope::enter();
        // SAFETY: arguments transmis tels que reçus de CoreAudio pour ce cycle.
        unsafe { play.run(flags, ts, frames as usize, data) }
    }))
    .unwrap_or(false);
    if !ok && !data.is_null() {
        // SAFETY: `data` est la liste de tampons de CoreAudio ; on écrit du silence dans les tampons annoncés.
        unsafe {
            let first = (*data).buffers.as_mut_ptr();
            for k in 0..(*data).number_buffers as usize {
                let b = first.add(k);
                if !(*b).data.is_null() {
                    std::ptr::write_bytes((*b).data as *mut u8, 0, (*b).data_byte_size as usize);
                }
            }
        }
    }
    0
}

/// Libère l'audio unit dans l'ordre exigé (arrêt, désinitialisation, destruction), même en cas d'erreur.
struct UnitGuard {
    unit: AudioUnit,
    started: bool,
    initialized: bool,
}

impl Drop for UnitGuard {
    fn drop(&mut self) {
        // SAFETY: `unit` est une instance valide créée par `AudioComponentInstanceNew` ; chaque étape n'est faite que si la précédente a réussi.
        unsafe {
            if self.started {
                AudioOutputUnitStop(self.unit);
            }
            if self.initialized {
                AudioUnitUninitialize(self.unit);
            }
            AudioComponentInstanceDispose(self.unit);
        }
    }
}

fn set_prop<T>(unit: AudioUnit, what: &str, property: u32, scope: u32, element: u32, value: &T) -> Result<(), String> {
    // SAFETY: `value` est un `T` valide de `size_of::<T>()` octets, lu seulement pendant l'appel.
    check(what, unsafe { AudioUnitSetProperty(unit, property, scope, element, (value as *const T).cast(), std::mem::size_of::<T>() as u32) })
}

fn stream_format(unit: AudioUnit, scope: u32, element: u32) -> Result<StreamDescription, String> {
    let mut f = StreamDescription::default();
    let mut size = std::mem::size_of::<StreamDescription>() as u32;
    // SAFETY: `f` fait `size` octets, écrits par CoreAudio.
    check("lecture du format du périphérique", unsafe { AudioUnitGetProperty(unit, AU_STREAM_FORMAT, scope, element, (&mut f as *mut StreamDescription).cast(), &mut size) })?;
    Ok(f)
}

fn float_format(rate: f64, channels: u32) -> StreamDescription {
    StreamDescription {
        sample_rate: rate,
        format_id: FORMAT_LPCM,
        format_flags: FLAG_FLOAT_PACKED_NONINTERLEAVED,
        bytes_per_packet: 4,
        frames_per_packet: 1,
        bytes_per_frame: 4,
        channels_per_frame: channels,
        bits_per_channel: 32,
        reserved: 0,
    }
}

pub fn run_playback<P: StereoProcessor>(opts: &PlayOptions, make: impl FnOnce(u32) -> (P, ControlFn)) -> Result<PlaybackReport, String> {
    run_playback_ex(opts, |info| make(info.sample_rate))
}

/// Joue le processeur fabriqué par `make` pendant `opts.seconds` secondes ou jusqu'à ce que la fonction de contrôle rende faux.
pub fn run_playback_ex<P: StereoProcessor>(opts: &PlayOptions, make: impl FnOnce(&StreamInfo) -> (P, ControlFn)) -> Result<PlaybackReport, String> {
    let devices = list_devices()?;
    let device = match &opts.device {
        Some(q) => {
            let q = q.to_lowercase();
            devices.iter().find(|d| d.outputs >= 2 && d.name.to_lowercase().contains(&q)).ok_or_else(|| format!("aucun périphérique de sortie ne contient « {q} » (voir `devices`)"))?
        }
        None => devices.iter().find(|d| d.default_output).or_else(|| devices.iter().find(|d| d.outputs >= 2)).ok_or("aucun périphérique de sortie trouvé")?,
    };
    if device.outputs < 2 {
        return Err(format!("{} n'a que {} sortie(s) : au moins deux sont nécessaires", device.name, device.outputs));
    }
    let n_out = (if opts.output_count == 0 { 2 } else { opts.output_count.max(2) }).min(device.outputs).min(MAX_CHANNELS as u32) as usize;
    let n_in = opts.input_count.min(device.inputs).min(MAX_CHANNELS as u32) as usize;

    let desc = ComponentDescription { component_type: fourcc(b"auou"), component_sub_type: fourcc(b"ahal"), component_manufacturer: fourcc(b"appl"), component_flags: 0, component_flags_mask: 0 };
    // SAFETY: `desc` est valide pendant l'appel.
    let component = unsafe { AudioComponentFindNext(std::ptr::null_mut(), &desc) };
    if component.is_null() {
        return Err("CoreAudio : composant de sortie introuvable".into());
    }
    let mut unit: AudioUnit = std::ptr::null_mut();
    // SAFETY: `unit` est un pointeur où l'instance est déposée.
    check("création de l'audio unit", unsafe { AudioComponentInstanceNew(component, &mut unit) })?;
    let mut guard = UnitGuard { unit, started: false, initialized: false };

    // Entrées et sorties activées avant le choix du périphérique (ordre exigé par AUHAL).
    set_prop(unit, "activation de la sortie", AU_ENABLE_IO, AU_SCOPE_OUTPUT, 0, &1u32)?;
    set_prop(unit, "activation de l'entrée", AU_ENABLE_IO, AU_SCOPE_INPUT, 1, &(n_in > 0) .then_some(1u32).unwrap_or(0))?;
    set_prop(unit, "choix du périphérique", AU_CURRENT_DEVICE, AU_SCOPE_GLOBAL, 0, &device.id)?;

    if let Some(b) = opts.buffer.filter(|&b| b > 0) {
        let a = address(PROP_BUFFER_FRAMES, SCOPE_GLOBAL);
        // SAFETY: `b` est un entier de 4 octets lu pendant l'appel.
        check("réglage de la taille de tampon", unsafe { AudioObjectSetPropertyData(device.id, &a, 0, std::ptr::null(), 4, (&b as *const u32).cast()) })?;
    }
    let buffer_size = get_scalar::<u32>(device.id, PROP_BUFFER_FRAMES, SCOPE_GLOBAL).unwrap_or(0);

    // Formats : côté périphérique (lecture) puis côté application (flottants non entrelacés, même fréquence).
    let hw_out = stream_format(unit, AU_SCOPE_OUTPUT, 0)?;
    let rate = if hw_out.sample_rate > 0.0 { hw_out.sample_rate } else { device.sample_rate };
    set_prop(unit, "format de sortie", AU_STREAM_FORMAT, AU_SCOPE_INPUT, 0, &float_format(rate, n_out as u32))?;
    if n_in > 0 {
        set_prop(unit, "format d'entrée", AU_STREAM_FORMAT, AU_SCOPE_OUTPUT, 1, &float_format(rate, n_in as u32))?;
    }
    let max_frames = (buffer_size as usize).max(4096);
    set_prop(unit, "taille maximale de tranche", AU_MAX_FRAMES, AU_SCOPE_GLOBAL, 0, &(max_frames as u32))?;

    // Latences annoncées (périphérique + marge de sécurité + tampon), publiées comme le font les autres pilotes.
    let lat = |scope: u32| get_scalar::<u32>(device.id, PROP_DEVICE_LATENCY, scope).unwrap_or(0) + get_scalar::<u32>(device.id, PROP_SAFETY_OFFSET, scope).unwrap_or(0) + buffer_size;
    let latency = Arc::new(StreamLatency::default());
    let stream = StreamInfo { driver: device.name.clone(), sample_rate: rate as u32, buffer_size, inputs: n_in as u32, outputs: n_out as u32, latency: Arc::clone(&latency) };
    let (processor, mut control) = make(&stream);

    // Un `u64` de plus : l'en-tête de la liste occupe 8 octets, puis chaque tampon 16.
    let list_words = (8 + n_in.max(1) * std::mem::size_of::<AudioBuffer>()).div_ceil(8);
    let mut play = Box::new(Play {
        unit,
        tp: TimedProcessor::new(processor, rate as u32),
        n_in,
        n_out,
        max_frames,
        in_list: vec![0u64; list_words],
        in_data: (0..n_in).map(|_| vec![0.0f32; max_frames]).collect(),
        silence: vec![0.0f32; max_frames],
    });
    let cb = RenderCallback { input_proc: render::<P>, input_proc_ref_con: (&mut *play as *mut Play<P>).cast() };
    set_prop(unit, "installation du callback", AU_SET_RENDER_CALLBACK, AU_SCOPE_INPUT, 0, &cb)?;

    alloc_guard::reset_violations();
    // SAFETY: `unit` est configuré ; `play` (cible du callback) reste vivant jusqu'à la fin de cette fonction, après l'arrêt par `guard`.
    check("initialisation", unsafe { AudioUnitInitialize(unit) })?;
    guard.initialized = true;
    // SAFETY: idem.
    check("démarrage", unsafe { AudioOutputUnitStart(unit) })?;
    guard.started = true;
    latency.set(lat(SCOPE_INPUT), lat(SCOPE_OUTPUT));

    let start = Instant::now();
    let end = Duration::from_secs(opts.seconds.max(1) as u64);
    loop {
        let elapsed = start.elapsed();
        if elapsed >= end || !control(elapsed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let callback_allocations = alloc_guard::violations();
    drop(guard); // arrêt de l'unité : plus aucun callback ne touche `play`
    let timing = play.tp.timing();
    drop(play);
    Ok(PlaybackReport { sample_rate: rate as u32, buffer_size, output: format!("{} ({} sorties)", device.name, n_out), timing, xruns: 0, callback_allocations })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_character_codes_match_coreaudio_values() {
        assert_eq!(fourcc(b"auou"), 0x6175_6F75);
        assert_eq!(fourcc(b"ahal"), 0x6168_616C);
        assert_eq!(fourcc(b"lpcm"), 0x6C70_636D);
        assert_eq!(fourcc(b"glob"), 0x676C_6F62);
    }

    #[test]
    fn c_structures_have_the_sizes_coreaudio_expects() {
        assert_eq!(std::mem::size_of::<StreamDescription>(), 40);
        assert_eq!(std::mem::size_of::<ComponentDescription>(), 20);
        assert_eq!(std::mem::size_of::<PropertyAddress>(), 12);
        assert_eq!(std::mem::size_of::<AudioBuffer>(), 16);
        // En-tête (4 octets + 4 de remplissage) puis le premier tampon, aligné sur 8 octets.
        assert_eq!(std::mem::size_of::<AudioBufferList>(), 24);
        assert_eq!(std::mem::offset_of!(AudioBufferList, buffers), 8);
        assert_eq!(std::mem::size_of::<RenderCallback>(), 16);
    }

    #[test]
    fn float_format_is_non_interleaved_packed_float() {
        let f = float_format(48_000.0, 6);
        assert_eq!((f.format_flags, f.bytes_per_frame, f.channels_per_frame, f.bits_per_channel), (41, 4, 6, 32));
    }
}
