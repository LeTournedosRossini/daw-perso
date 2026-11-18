//! Backend WASAPI en **mode partagé** (Windows) : la carte son par le mélangeur du système, sans pilote ASIO.
//!
//! Pour les machines qui n'ont pas de pilote ASIO (la plupart des testeurs) ; la basse latence reste l'affaire d'ASIO, que ce module ne
//! touche pas. Mode partagé seulement : le format est celui du mélangeur du système (flottant 32 bits, souvent 48 000 Hz), aucune
//! négociation de format n'est donc nécessaire. Avec `IAudioClient3` (Windows 10 et plus) la période descend jusqu'à ce que le
//! système et le pilote permettent (3 à 10 ms) ; sinon la période par défaut du système (≈ 10 ms) sert.
//!
//! Tout le COM vit dans **un fil à part** (`daw-wasapi`) : ouverture, boucle événementielle, fermeture. Le fil appelant ne touche jamais au
//! COM, ce qui évite de changer son modèle d'appartement (le fil moteur est aussi celui d'ASIO). Le cycle : attendre l'événement du
//! système, vider la capture (lue sans attendre, file de dérive), traiter, écrire dans le tampon de lecture. Un appareil débranché ou
//! changé (`AUDCLNT_E_DEVICE_INVALIDATED`) arrête le fil avec une erreur lisible.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use windows::core::{Interface, GUID, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioClient3, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::alloc_guard::{self, RtScope};
use crate::capture_ring::CaptureRing;
use crate::playback::{ControlFn, PlaybackReport, StereoProcessor, StreamInfo, StreamLatency, TimedProcessor};

/// Nombre maximal de canaux d'entrée ouverts.
pub const MAX_INPUTS: usize = 32;

/// Un appareil audio actif du système.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEntry {
    /// Identifiant du point de terminaison (stable tant que l'appareil existe).
    pub id: String,
    /// Nom d'affichage (« Haut-parleurs (Realtek…) ») : c'est lui qu'on enregistre dans les préférences.
    pub name: String,
    pub playback: bool,
    pub capture: bool,
    /// Appareil par défaut du système pour ce sens.
    pub default: bool,
}

fn win_err(what: &str, e: windows::core::Error) -> String {
    format!("{what} : {e}")
}

/// Initialise COM pour le fil courant (un modèle déjà choisi ailleurs, `RPC_E_CHANGED_MODE`, n'empêche pas d'utiliser COM).
fn com_init() {
    // SAFETY: appel COM sans pointeur ; le résultat (déjà initialisé, autre modèle) est volontairement ignoré.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

fn pwstr_to_string(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: `p` vient d'une API COM qui rend une chaîne UTF-16 terminée par 0, libérée ensuite par `CoTaskMemFree`.
    let s = unsafe { p.to_string().unwrap_or_default() };
    // SAFETY: la mémoire a été allouée par COM (`CoTaskMemAlloc`) et n'est plus utilisée.
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn device_id(d: &IMMDevice) -> String {
    // SAFETY: appel COM sur un `IMMDevice` valide.
    unsafe { d.GetId() }.map(pwstr_to_string).unwrap_or_default()
}

fn device_name(d: &IMMDevice) -> String {
    // SAFETY: appels COM sur des objets valides ; le `PROPVARIANT` est vidé avant de sortir.
    unsafe {
        let Ok(store) = d.OpenPropertyStore(STGM_READ) else { return String::new() };
        let Ok(mut v) = store.GetValue(&PKEY_Device_FriendlyName) else { return String::new() };
        let name = PropVariantToStringAlloc(&v).map(pwstr_to_string).unwrap_or_default();
        let _ = PropVariantClear(&mut v);
        name
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator, String> {
    // SAFETY: création d'un objet COM standard ; COM est initialisé pour ce fil par l'appelant.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| win_err("énumérateur d'appareils audio", e))
}

/// Les appareils actifs (lecture, puis capture), l'appareil par défaut de chaque sens signalé. Le travail COM se fait dans un fil jetable :
/// le fil appelant garde son modèle d'appartement (ASIO en dépend).
pub fn list_devices() -> Vec<DeviceEntry> {
    std::thread::Builder::new()
        .name("daw-wasapi-list".into())
        .spawn(|| -> Vec<DeviceEntry> {
            com_init();
            let Ok(en) = enumerator() else { return Vec::new() };
            let mut out = Vec::new();
            for (flow, playback) in [(eRender, true), (eCapture, false)] {
                // SAFETY: appels COM sur l'énumérateur valide de ce fil.
                let default_id = unsafe { en.GetDefaultAudioEndpoint(flow, eConsole) }.map(|d| device_id(&d)).unwrap_or_default();
                // SAFETY: idem.
                let Ok(coll) = (unsafe { en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }) else { continue };
                // SAFETY: idem.
                let n = unsafe { coll.GetCount() }.unwrap_or(0);
                for i in 0..n {
                    // SAFETY: `i` est dans `0..GetCount()`.
                    let Ok(d) = (unsafe { coll.Item(i) }) else { continue };
                    let id = device_id(&d);
                    let name = device_name(&d);
                    if name.is_empty() {
                        continue;
                    }
                    out.push(DeviceEntry { default: id == default_id, id, name, playback, capture: !playback });
                }
            }
            out
        })
        .ok()
        .and_then(|h| h.join().ok())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
pub struct PlayOptions {
    pub seconds: u32,
    /// Appareil de lecture : nom (ou morceau du nom) ou identifiant ; sinon l'appareil par défaut du système.
    pub device: Option<String>,
    /// Appareil de capture ; sinon l'appareil de capture par défaut.
    pub capture_device: Option<String>,
    /// Période demandée, en échantillons (le système la ramène à ce qu'il permet) ; sinon 256.
    pub buffer: Option<u32>,
    /// Ouvre la capture (sinon silence en entrée).
    pub capture: bool,
}

fn find_device(en: &IMMDeviceEnumerator, flow: windows::Win32::Media::Audio::EDataFlow, wanted: Option<&str>) -> Result<IMMDevice, String> {
    let wanted = wanted.map(str::trim).filter(|w| !w.is_empty());
    let Some(w) = wanted else {
        // SAFETY: appel COM sur l'énumérateur valide.
        return unsafe { en.GetDefaultAudioEndpoint(flow, eConsole) }.map_err(|e| win_err("aucun appareil audio par défaut", e));
    };
    let lw = w.to_lowercase();
    // SAFETY: appels COM sur l'énumérateur valide.
    let coll = unsafe { en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) }.map_err(|e| win_err("liste des appareils", e))?;
    // SAFETY: idem.
    let n = unsafe { coll.GetCount() }.unwrap_or(0);
    let mut partial: Option<IMMDevice> = None;
    for i in 0..n {
        // SAFETY: `i` est dans `0..GetCount()`.
        let Ok(d) = (unsafe { coll.Item(i) }) else { continue };
        if device_id(&d) == w {
            return Ok(d);
        }
        let name = device_name(&d).to_lowercase();
        if name == lw {
            return Ok(d);
        }
        if partial.is_none() && name.contains(&lw) {
            partial = Some(d);
        }
    }
    partial.ok_or_else(|| format!("aucun appareil audio actif ne s'appelle « {w} »"))
}

/// Le format de mélange du système, vérifié flottant 32 bits : `(copie **complète** du format, canaux, fréquence)`. La copie garde la queue
/// du format étendu (masque des canaux, sous-format) : passer seulement l'en-tête de 18 octets fait refuser le flux (`E_INVALIDARG`).
fn mix_format(client: &IAudioClient) -> Result<(Vec<u8>, usize, u32), String> {
    // SAFETY: `GetMixFormat` rend une structure allouée par COM, lue sans alignement garanti (structure « packed ») puis libérée.
    unsafe {
        let p = client.GetMixFormat().map_err(|e| win_err("format de mélange", e))?;
        let fmt = std::ptr::read_unaligned(p);
        let float = match fmt.wFormatTag as u32 {
            t if t == WAVE_FORMAT_IEEE_FLOAT => fmt.wBitsPerSample == 32,
            t if t == WAVE_FORMAT_EXTENSIBLE => {
                let ext = std::ptr::read_unaligned(p as *const WAVEFORMATEXTENSIBLE);
                let sub: GUID = ext.SubFormat;
                fmt.wBitsPerSample == 32 && sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            }
            _ => false,
        };
        let total = std::mem::size_of::<WAVEFORMATEX>() + fmt.cbSize as usize;
        let bytes = std::slice::from_raw_parts(p as *const u8, total).to_vec();
        CoTaskMemFree(Some(p as *const _));
        if !float {
            return Err(format!("le format de mélange du système n'est pas flottant 32 bits (étiquette {}, {} bits) : non pris en charge", { fmt.wFormatTag }, { fmt.wBitsPerSample }));
        }
        Ok((bytes, fmt.nChannels as usize, fmt.nSamplesPerSec))
    }
}

/// Initialise un flux partagé : `IAudioClient3` (période courte) si possible, sinon `Initialize` classique. Rend la période en trames.
fn init_shared(client: &IAudioClient, fmt: *const WAVEFORMATEX, period_req: u32, event: bool) -> Result<u32, String> {
    let flags = if event { AUDCLNT_STREAMFLAGS_EVENTCALLBACK } else { 0 };
    if let Ok(c3) = client.cast::<IAudioClient3>() {
        let (mut def, mut fund, mut min, mut max) = (0u32, 0u32, 0u32, 0u32);
        // SAFETY: `fmt` est un format valide, les quatre sorties sont des entiers valides.
        if unsafe { c3.GetSharedModeEnginePeriod(fmt, &mut def, &mut fund, &mut min, &mut max) }.is_ok() && fund > 0 {
            let want = period_req.div_ceil(fund) * fund;
            let period = want.clamp(min.max(fund), max.max(min));
            // SAFETY: mêmes conditions ; le format n'est pas modifié.
            let init = unsafe { c3.InitializeSharedAudioStream(flags, period, fmt, None) };
            eprintln!("WASAPI IAudioClient3 : période par défaut {def}, fondamentale {fund}, min {min}, max {max} ; demandée {period_req} → {period} : {}", if init.is_ok() { "acceptée".to_string() } else { format!("refusée ({:?})", init.as_ref().err().map(|e| e.code())) });
            if init.is_ok() {
                return Ok(period);
            }
        }
    }
    // Période par défaut du système (≈ 10 ms) : 0 laisse le système choisir.
    // SAFETY: `fmt` est un format valide.
    unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, fmt, None) }.map_err(|e| win_err("initialisation du flux partagé", e))?;
    // SAFETY: appel COM sur un client initialisé.
    let frames = unsafe { client.GetBufferSize() }.map_err(|e| win_err("taille du tampon", e))?;
    Ok(frames / 2)
}

/// Format flottant 32 bits demandé à la capture quand sa fréquence n'est pas celle de la lecture (le système convertit).
fn float_format(channels: u16, rate: u32) -> WAVEFORMATEX {
    WAVEFORMATEX { wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16, nChannels: channels, nSamplesPerSec: rate, nAvgBytesPerSec: rate * channels as u32 * 4, nBlockAlign: channels * 4, wBitsPerSample: 32, cbSize: 0 }
}

struct Render {
    client: IAudioClient,
    render: IAudioRenderClient,
    channels: usize,
    rate: u32,
    buffer_frames: u32,
    period: u32,
    event: HANDLE,
}

struct Capture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    channels: usize,
}

fn open_render(en: &IMMDeviceEnumerator, wanted: Option<&str>, period_req: u32) -> Result<(Render, String), String> {
    let dev = find_device(en, eRender, wanted)?;
    let name = device_name(&dev);
    // SAFETY: activation d'un client audio sur un appareil valide.
    let client: IAudioClient = unsafe { dev.Activate(CLSCTX_ALL, None) }.map_err(|e| win_err(&format!("ouverture de « {name} »"), e))?;
    let (fmt, channels, rate) = mix_format(&client)?;
    // SAFETY: création d'un événement anonyme à réinitialisation automatique.
    let event = unsafe { CreateEventW(None, false, false, windows::core::PCWSTR::null()) }.map_err(|e| win_err("événement", e))?;
    let period = init_shared(&client, fmt.as_ptr() as *const WAVEFORMATEX, period_req, true)?;
    // SAFETY: l'événement est valide et vit autant que le flux.
    unsafe { client.SetEventHandle(event) }.map_err(|e| win_err("événement de rendu", e))?;
    // SAFETY: appels COM sur un client initialisé.
    let (buffer_frames, render) = unsafe { (client.GetBufferSize().map_err(|e| win_err("taille du tampon", e))?, client.GetService::<IAudioRenderClient>().map_err(|e| win_err("client de rendu", e))?) };
    Ok((Render { client, render, channels, rate, buffer_frames, period, event }, name))
}

/// Ouvre la capture sur la fréquence de lecture : le format de mélange tel quel s'il a la même fréquence, sinon le système convertit
/// (flottant, au plus deux canaux). Lue sans événement, par sondage à chaque cycle de lecture.
fn open_capture(en: &IMMDeviceEnumerator, wanted: Option<&str>, rate: u32, period_req: u32) -> Result<Capture, String> {
    let dev = find_device(en, eCapture, wanted)?;
    let name = device_name(&dev);
    // SAFETY: activation d'un client audio sur un appareil valide.
    let client: IAudioClient = unsafe { dev.Activate(CLSCTX_ALL, None) }.map_err(|e| win_err(&format!("ouverture de « {name} » (capture)"), e))?;
    let (fmt, channels, crate_rate) = mix_format(&client)?;
    if crate_rate == rate {
        init_shared(&client, fmt.as_ptr() as *const WAVEFORMATEX, period_req, false)?;
        // SAFETY: appel COM sur un client initialisé.
        let capture = unsafe { client.GetService::<IAudioCaptureClient>() }.map_err(|e| win_err("client de capture", e))?;
        return Ok(Capture { client, capture, channels: channels.min(MAX_INPUTS) });
    }
    if channels > 2 {
        return Err(format!("« {name} » : {crate_rate} Hz et {channels} canaux : la conversion de fréquence n'est faite que jusqu'à deux canaux"));
    }
    let want = float_format(channels as u16, rate);
    // Mémoire tampon de 100 ms ; le système convertit la fréquence.
    // SAFETY: `want` est un format valide.
    unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, 1_000_000, 0, &want, None) }
        .map_err(|e| win_err(&format!("capture de « {name} » à {rate} Hz"), e))?;
    // SAFETY: appel COM sur un client initialisé.
    let capture = unsafe { client.GetService::<IAudioCaptureClient>() }.map_err(|e| win_err("client de capture", e))?;
    Ok(Capture { client, capture, channels })
}

/// Latence d'un flux en trames (latence annoncée par le système + une période).
fn latency_frames(client: &IAudioClient, rate: u32, period: u32) -> u32 {
    // SAFETY: appel COM sur un client initialisé.
    let hns = unsafe { client.GetStreamLatency() }.unwrap_or(0).max(0) as u64;
    (hns * rate as u64 / 10_000_000) as u32 + period
}

struct Worker<P> {
    render: Render,
    capture: Option<Capture>,
    tp: TimedProcessor<P>,
    stop: Arc<AtomicBool>,
    xruns: Arc<AtomicU32>,
}

/// Cycle audio : attend l'événement du système, vide la capture, traite, écrit. Rend le résumé des temps, ou l'erreur qui l'a arrêté.
fn run_loop<P: StereoProcessor>(mut w: Worker<P>) -> Result<Option<crate::timing::TimingSummary>, String> {
    let r = &w.render;
    let cap_frames = r.buffer_frames as usize;
    let n_in = w.capture.as_ref().map_or(0, |c| c.channels);
    let mut out_l = vec![0.0f32; cap_frames];
    let mut out_r = vec![0.0f32; cap_frames];
    let mut in_planar: Vec<Vec<f32>> = (0..n_in.max(2)).map(|_| vec![0.0; cap_frames]).collect();
    let mut ring = CaptureRing::new(n_in, (r.period as usize).max(64) * 16);
    let mut scratch = vec![0.0f32; 0];
    let mut silence = vec![0.0f32; 0];
    if let Some(c) = &w.capture {
        // Un paquet de capture est au plus la taille du tampon de capture.
        // SAFETY: appel COM sur un client initialisé.
        let frames = unsafe { c.client.GetBufferSize() }.unwrap_or(0) as usize;
        scratch = vec![0.0; frames.max(cap_frames) * c.channels];
        silence = vec![0.0; frames.max(cap_frames) * c.channels];
    }
    let out_ch = r.channels.max(1);
    // Départ : un tampon de silence, puis les deux flux.
    // SAFETY: appels COM sur des clients initialisés ; le tampon rendu par `GetBuffer` a la taille demandée.
    unsafe {
        let n = r.buffer_frames;
        let p = r.render.GetBuffer(n).map_err(|e| win_err("premier tampon", e))?;
        std::ptr::write_bytes(p, 0, n as usize * out_ch * 4);
        r.render.ReleaseBuffer(n, 0).map_err(|e| win_err("premier tampon", e))?;
        if let Some(c) = &w.capture {
            c.client.Start().map_err(|e| win_err("démarrage de la capture", e))?;
        }
        r.client.Start().map_err(|e| win_err("démarrage de la lecture", e))?;
    }
    let mut timeouts = 0u32;
    let mut first_packet = true;
    while !w.stop.load(Ordering::Relaxed) {
        // SAFETY: attente sur un événement valide, au plus 500 ms pour pouvoir observer l'arrêt.
        let waited = unsafe { WaitForSingleObject(r.event, 500) };
        if waited != WAIT_OBJECT_0 {
            timeouts += 1;
            if timeouts > 6 {
                return Err("le système n'appelle plus le flux de lecture (appareil suspendu ou changé)".into());
            }
            continue;
        }
        timeouts = 0;
        let _rt = RtScope::enter();
        // SAFETY: appels COM sur des clients initialisés, sur le fil qui les a créés.
        unsafe {
            let padding = match r.client.GetCurrentPadding() {
                Ok(p) => p,
                Err(e) if e.code() == AUDCLNT_E_DEVICE_INVALIDATED => return Err("l'appareil audio a été débranché ou changé".into()),
                Err(e) => return Err(win_err("remplissage du tampon", e)),
            };
            let frames = r.buffer_frames.saturating_sub(padding) as usize;
            if frames == 0 {
                continue;
            }
            let n = frames.min(cap_frames);
            // Capture : tout ce qui est arrivé, sans attendre.
            if let Some(c) = &w.capture {
                while let Ok(pkt) = c.capture.GetNextPacketSize() {
                    if pkt == 0 {
                        break;
                    }
                    let (mut data, mut got, mut flags) = (std::ptr::null_mut::<u8>(), 0u32, 0u32);
                    if c.capture.GetBuffer(&mut data, &mut got, &mut flags, None, None).is_err() {
                        break;
                    }
                    let samples = got as usize * c.channels;
                    // Le tout premier paquet porte toujours le drapeau de discontinuité (démarrage du flux) : pas un dépassement.
                    if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 && !first_packet {
                        w.xruns.fetch_add(1, Ordering::Relaxed);
                    }
                    first_packet = false;
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        ring.push(&silence[..samples.min(silence.len())], c.channels, (samples / c.channels.max(1)).min(silence.len() / c.channels.max(1)));
                    } else {
                        let src = std::slice::from_raw_parts(data as *const f32, samples);
                        let k = samples.min(scratch.len());
                        scratch[..k].copy_from_slice(&src[..k]);
                        ring.push(&scratch[..k], c.channels, k / c.channels.max(1));
                    }
                    let _ = c.capture.ReleaseBuffer(got);
                }
                ring.trim((r.period as usize).max(64) * 3);
                if !ring.pop_n(&mut in_planar[..n_in], n) {
                    for p in in_planar.iter_mut() {
                        p[..n].fill(0.0);
                    }
                }
            } else {
                for p in in_planar.iter_mut() {
                    p[..n].fill(0.0);
                }
            }
            {
                let ins: [&[f32]; MAX_INPUTS] = std::array::from_fn(|i| in_planar.get(i).map_or(&[][..], |v| &v[..n]));
                let n_ins = n_in.max(2);
                let mut outs: [&mut [f32]; 2] = [&mut out_l[..n], &mut out_r[..n]];
                // Un panic dans le processeur ne doit pas arrêter le flux : du silence à la place.
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| w.tp.process_outputs(&ins[..n_ins], &mut outs)));
                if res.is_err() {
                    out_l[..n].fill(0.0);
                    out_r[..n].fill(0.0);
                }
            }
            let ptr = match r.render.GetBuffer(n as u32) {
                Ok(p) => p as *mut f32,
                Err(e) if e.code() == AUDCLNT_E_DEVICE_INVALIDATED => return Err("l'appareil audio a été débranché ou changé".into()),
                Err(_) => {
                    w.xruns.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            let buf = std::slice::from_raw_parts_mut(ptr, n * out_ch);
            for i in 0..n {
                let (l, rr) = (if out_l[i].is_finite() { out_l[i] } else { 0.0 }, if out_r[i].is_finite() { out_r[i] } else { 0.0 });
                let frame = &mut buf[i * out_ch..(i + 1) * out_ch];
                if out_ch == 1 {
                    frame[0] = (l + rr) * 0.5;
                } else {
                    frame[0] = l;
                    frame[1] = rr;
                    frame[2..].fill(0.0);
                }
            }
            let _ = r.render.ReleaseBuffer(n as u32, 0);
        }
    }
    // SAFETY: arrêt des flux initialisés par ce fil.
    unsafe {
        let _ = r.client.Stop();
        if let Some(c) = &w.capture {
            let _ = c.client.Stop();
        }
        let _ = CloseHandle(r.event);
    }
    Ok(w.tp.timing())
}

/// Joue (et enregistre) en WASAPI partagé : même contrat que les autres backends. `make` reçoit ce qu'on sait du flux et rend le processeur
/// et la fonction de contrôle (appelée toutes les ~10 ms sur le fil appelant).
pub fn run_playback_ex<P: StereoProcessor>(opts: &PlayOptions, make: impl FnOnce(&StreamInfo) -> (P, ControlFn)) -> Result<PlaybackReport, String> {
    struct Opened {
        info: StreamInfo,
        name: String,
    }
    let latency = Arc::new(StreamLatency::default());
    let stop = Arc::new(AtomicBool::new(false));
    let xruns = Arc::new(AtomicU32::new(0));
    let (tx_info, rx_info) = mpsc::channel::<Result<Opened, String>>();
    let (tx_proc, rx_proc) = mpsc::channel::<Option<P>>();
    let (opts2, latency2, stop2, xruns2) = (opts.clone(), Arc::clone(&latency), Arc::clone(&stop), Arc::clone(&xruns));
    let worker = std::thread::Builder::new()
        .name("daw-wasapi".into())
        .spawn(move || -> Result<(u32, Option<crate::timing::TimingSummary>), String> {
            com_init();
            let opened = (|| -> Result<(Render, Option<Capture>, String), String> {
                let en = enumerator()?;
                let period_req = opts2.buffer.unwrap_or(256).clamp(16, 8192);
                let (render, name) = open_render(&en, opts2.device.as_deref(), period_req)?;
                let capture = if opts2.capture {
                    match open_capture(&en, opts2.capture_device.as_deref(), render.rate, period_req) {
                        Ok(c) => Some(c),
                        Err(e) => {
                            eprintln!("capture ignorée : {e}");
                            None
                        }
                    }
                } else {
                    None
                };
                Ok((render, capture, name))
            })();
            let (render, capture, name) = match opened {
                Ok(o) => o,
                Err(e) => {
                    let _ = tx_info.send(Err(e.clone()));
                    return Err(e);
                }
            };
            let in_lat = capture.as_ref().map_or(0, |c| latency_frames(&c.client, render.rate, render.period));
            latency2.set(in_lat, latency_frames(&render.client, render.rate, render.period));
            let info = StreamInfo {
                driver: format!("WASAPI partagé « {name} »"),
                sample_rate: render.rate,
                buffer_size: render.period,
                inputs: capture.as_ref().map_or(0, |c| c.channels as u32),
                outputs: 2,
                latency: Arc::clone(&latency2),
            };
            let (rate, period) = (render.rate, render.period);
            let _ = tx_info.send(Ok(Opened { info, name }));
            let Ok(Some(processor)) = rx_proc.recv() else { return Ok((period, None)) };
            let _ = crate::rt_priority::promote_current_thread();
            let w = Worker { render, capture, tp: TimedProcessor::new(processor, rate), stop: stop2, xruns: xruns2 };
            run_loop(w).map(|t| (period, t))
        })
        .map_err(|e| format!("fil audio : {e}"))?;

    let opened = match rx_info.recv() {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            let _ = worker.join();
            return Err(e);
        }
        Err(_) => return Err(worker.join().map_err(|_| "le fil audio a paniqué".to_string()).and_then(|r| r.map(|_| ())).err().unwrap_or_else(|| "ouverture du flux interrompue".into())),
    };
    let (processor, mut control) = make(&opened.info);
    alloc_guard::reset_violations();
    let _ = tx_proc.send(Some(processor));
    let start = Instant::now();
    let end = Duration::from_secs(opts.seconds.max(1) as u64);
    loop {
        let elapsed = start.elapsed();
        if elapsed >= end || !control(elapsed) || worker.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let died = worker.is_finished();
    stop.store(true, Ordering::Relaxed);
    let (period, timing) = worker.join().map_err(|_| "le fil audio a paniqué".to_string())??;
    if died && timing.is_none() {
        return Err(format!("« {} » : le flux s'est arrêté", opened.name));
    }
    Ok(PlaybackReport { sample_rate: opened.info.sample_rate, buffer_size: period, output: opened.name, timing, xruns: xruns.load(Ordering::Relaxed), callback_allocations: alloc_guard::violations() })
}
