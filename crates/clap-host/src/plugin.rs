//! Instance de plugin CLAP hébergée dans le processus : cycle de vie, ports, paramètres, latence, état, traitement.
//!
//! Règles de fils d'exécution de CLAP, respectées par l'appelant :
//! - *fil principal* : `instantiate`, `activate`, `deactivate`, `params`, `param_value`, `save_state`, `load_state`, `Drop` ;
//! - *fil audio* : `start_processing`, `process`, `stop_processing`, `queue_param`.
//! `process` et `queue_param` n'allouent pas et ne bloquent pas.

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::sync::Arc;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    clap_event_header, clap_event_midi, clap_event_note, clap_event_param_gesture, clap_event_param_value, CLAP_EVENT_PARAM_GESTURE_BEGIN, CLAP_EVENT_PARAM_GESTURE_END, clap_input_events, clap_output_events, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_OFF,
    CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE, clap_event_transport, CLAP_EVENT_TRANSPORT, CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_HAS_TIME_SIGNATURE,
    CLAP_TRANSPORT_IS_LOOP_ACTIVE, CLAP_TRANSPORT_IS_PLAYING,
};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::ext::note_ports::{clap_host_note_ports, clap_note_dialect, clap_note_port_info, clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI};
use clap_sys::ext::audio_ports::{clap_audio_port_info, clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS};
use clap_sys::ext::gui::{clap_host_gui, clap_plugin_gui, clap_window, clap_window_handle, CLAP_EXT_GUI};
use clap_sys::ext::timer_support::{clap_host_timer_support, clap_plugin_timer_support, CLAP_EXT_TIMER_SUPPORT};
use clap_sys::ext::latency::{clap_host_latency, clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::params::{clap_host_params, clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS};
use clap_sys::ext::state::{clap_host_state, clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{clap_process, CLAP_PROCESS_ERROR};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

use crate::Bundle;

/// Événements (paramètres et notes) en attente pour le prochain bloc.
const MAX_EVENTS: usize = 512;

#[derive(Debug, Clone, PartialEq)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub flags: u32,
}

/// Ce que le plugin dit de son interface graphique (extension CLAP `gui`), sans rien créer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GuiSupport {
    /// Le plugin déclare l'extension `gui`.
    pub has_gui: bool,
    /// API de fenêtre acceptées : `(nom, flottante)` — `win32`, `cocoa`, `x11`, `wayland` ; flottante : le plugin ouvre **sa propre
    /// fenêtre** ; sinon il se loge dans une fenêtre fournie par l'hôte (intégrée).
    pub apis: Vec<(String, bool)>,
    /// API et mode préférés du plugin, s'il en annonce.
    pub preferred: Option<(String, bool)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PortsInfo {
    /// Nombre de canaux de chaque port d'entrée / de sortie.
    pub inputs: Vec<u32>,
    pub outputs: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    Continue,
    ContinueIfNotQuiet,
    Tail,
    Sleep,
    Error,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PluginError {
    UnknownPlugin,
    CreateFailed,
    InitFailed,
    /// Opération appelée dans un état du cycle de vie qui ne la permet pas.
    State(&'static str),
    ActivateFailed,
    StartFailed,
    /// Le plugin n'offre pas l'extension demandée.
    NoExtension(&'static str),
    BadBlock,
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PluginError {}

// ---- côté hôte exposé au plugin --------------------------------------------------------------

/// Un minuteur demandé par le plugin (extension hôte `timer-support`) : l'interface graphique s'en sert pour s'animer.
struct Timer {
    id: u32,
    period: Duration,
    due: Instant,
}

#[derive(Default)]
struct Timers {
    last_id: u32,
    list: Vec<Timer>,
}

#[derive(Default)]
struct HostShared {
    /// Interface graphique : le plugin demande une nouvelle taille (largeur, hauteur), demande de se cacher, ou signale sa fermeture.
    gui_resize_w: AtomicU32,
    gui_resize_h: AtomicU32,
    gui_resize_pending: AtomicBool,
    gui_hide_requested: AtomicBool,
    gui_closed: AtomicBool,
    timers: Mutex<Timers>,
    latency_changed: AtomicBool,
    restart_requested: AtomicBool,
    process_requested: AtomicBool,
    callback_requested: AtomicBool,
    /// Événements de sortie du plugin autres que les paramètres (ignorés) : permet de voir qu'il en émet.
    output_events: AtomicU32,
    /// Valeurs et gestes de paramètres émis par le plugin (réglages faits dans son interface).
    param_out: ParamRing,
    /// Piste d'accords valable **pendant `process` seulement** (adresse d'un instantané du graphe, retirée juste après) : l'extension
    /// d'hôte `fr.flibuste.chord-track/1` ne répond que depuis le fil de traitement ; ailleurs elle dit « rien ».
    chords: std::sync::atomic::AtomicPtr<daw_engine::ChordSnapshot>,
}

/// Ce que le plugin annonce à l'hôte à propos d'un paramètre (événements de sortie CLAP).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamOut {
    /// Le plugin a changé la valeur du paramètre (curseur tiré dans son interface, par exemple).
    Value { id: u32, value: f64 },
    /// L'utilisateur prend le paramètre dans l'interface du plugin.
    GestureBegin { id: u32 },
    /// L'utilisateur lâche le paramètre.
    GestureEnd { id: u32 },
}

const PARAM_RING: usize = 1024;

/// File sans verrou, un producteur (le fil audio, dans `process`) et un consommateur (le fil de contrôle).
/// Chaque case : deux mots atomiques (type et identifiant, valeur). Pleine : l'événement est perdu et compté.
struct ParamRing {
    meta: Vec<AtomicU64>,
    val: Vec<AtomicU64>,
    head: AtomicUsize,
    tail: AtomicUsize,
    lost: AtomicU32,
}

impl Default for ParamRing {
    fn default() -> ParamRing {
        ParamRing {
            meta: (0..PARAM_RING).map(|_| AtomicU64::new(0)).collect(),
            val: (0..PARAM_RING).map(|_| AtomicU64::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            lost: AtomicU32::new(0),
        }
    }
}

impl ParamRing {
    /// Producteur. Sans allocation ni verrou.
    fn push(&self, e: ParamOut) {
        let head = self.head.load(Ordering::Relaxed);
        if head.wrapping_sub(self.tail.load(Ordering::Acquire)) >= PARAM_RING {
            self.lost.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let (kind, id, value) = match e {
            ParamOut::Value { id, value } => (1u64, id, value),
            ParamOut::GestureBegin { id } => (2, id, 0.0),
            ParamOut::GestureEnd { id } => (3, id, 0.0),
        };
        let i = head % PARAM_RING;
        self.meta[i].store((kind << 32) | u64::from(id), Ordering::Relaxed);
        self.val[i].store(value.to_bits(), Ordering::Relaxed);
        self.head.store(head.wrapping_add(1), Ordering::Release);
    }

    /// Consommateur (un seul fil à la fois).
    fn pop(&self) -> Option<ParamOut> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let i = tail % PARAM_RING;
        let meta = self.meta[i].load(Ordering::Relaxed);
        let value = f64::from_bits(self.val[i].load(Ordering::Relaxed));
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        let id = (meta & 0xFFFF_FFFF) as u32;
        Some(match meta >> 32 {
            1 => ParamOut::Value { id, value },
            2 => ParamOut::GestureBegin { id },
            _ => ParamOut::GestureEnd { id },
        })
    }
}

unsafe fn shared<'a>(h: *const clap_host) -> &'a HostShared {
    &*((*h).host_data as *const HostShared)
}

unsafe extern "C" fn host_request_restart(h: *const clap_host) {
    shared(h).restart_requested.store(true, Ordering::Release);
}
unsafe extern "C" fn host_request_process(h: *const clap_host) {
    shared(h).process_requested.store(true, Ordering::Release);
}
unsafe extern "C" fn host_request_callback(h: *const clap_host) {
    shared(h).callback_requested.store(true, Ordering::Release);
}
unsafe extern "C" fn host_latency_changed(h: *const clap_host) {
    shared(h).latency_changed.store(true, Ordering::Release);
}
unsafe extern "C" fn host_params_rescan(_: *const clap_host, _: u32) {}
unsafe extern "C" fn host_params_clear(_: *const clap_host, _: u32, _: u32) {}
unsafe extern "C" fn host_params_request_flush(h: *const clap_host) {
    shared(h).process_requested.store(true, Ordering::Release);
}
unsafe extern "C" fn host_state_mark_dirty(_: *const clap_host) {}

unsafe extern "C" fn host_gui_resize_hints_changed(_: *const clap_host) {}
unsafe extern "C" fn host_gui_request_resize(h: *const clap_host, w: u32, hh: u32) -> bool {
    let s = shared(h);
    s.gui_resize_w.store(w, Ordering::Release);
    s.gui_resize_h.store(hh, Ordering::Release);
    s.gui_resize_pending.store(true, Ordering::Release);
    true
}
unsafe extern "C" fn host_gui_request_show(_: *const clap_host) -> bool {
    false // c'est l'utilisateur qui ouvre l'interface
}
unsafe extern "C" fn host_gui_request_hide(h: *const clap_host) -> bool {
    shared(h).gui_hide_requested.store(true, Ordering::Release);
    true
}
unsafe extern "C" fn host_gui_closed(h: *const clap_host, _was_destroyed: bool) {
    shared(h).gui_closed.store(true, Ordering::Release);
}
static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(host_gui_resize_hints_changed),
    request_resize: Some(host_gui_request_resize),
    request_show: Some(host_gui_request_show),
    request_hide: Some(host_gui_request_hide),
    closed: Some(host_gui_closed),
};

/// Plus petite période de minuteur accordée (la spécification permet à l'hôte de relever une période trop courte).
const MIN_TIMER_MS: u32 = 10;

unsafe extern "C" fn host_timer_register(h: *const clap_host, period_ms: u32, id: *mut u32) -> bool {
    if id.is_null() {
        return false;
    }
    let mut t = shared(h).timers.lock().unwrap_or_else(|e| e.into_inner());
    t.last_id += 1;
    let period = Duration::from_millis(period_ms.max(MIN_TIMER_MS) as u64);
    let new = Timer { id: t.last_id, period, due: Instant::now() + period };
    *id = new.id;
    t.list.push(new);
    true
}
unsafe extern "C" fn host_timer_unregister(h: *const clap_host, id: u32) -> bool {
    let mut t = shared(h).timers.lock().unwrap_or_else(|e| e.into_inner());
    let before = t.list.len();
    t.list.retain(|x| x.id != id);
    t.list.len() != before
}
static HOST_TIMER: clap_host_timer_support = clap_host_timer_support { register_timer: Some(host_timer_register), unregister_timer: Some(host_timer_unregister) };

static HOST_LATENCY: clap_host_latency = clap_host_latency { changed: Some(host_latency_changed) };
static HOST_PARAMS: clap_host_params =
    clap_host_params { rescan: Some(host_params_rescan), clear: Some(host_params_clear), request_flush: Some(host_params_request_flush) };
static HOST_STATE: clap_host_state = clap_host_state { mark_dirty: Some(host_state_mark_dirty) };

unsafe extern "C" fn host_note_dialects(_: *const clap_host) -> clap_note_dialect {
    CLAP_NOTE_DIALECT_CLAP
}
unsafe extern "C" fn host_note_rescan(_: *const clap_host, _: u32) {}
static HOST_NOTE_PORTS: clap_host_note_ports = clap_host_note_ports { supported_dialects: Some(host_note_dialects), rescan: Some(host_note_rescan) };

// ---- extension d'hôte maison : la piste d'accords (voir docs/fr-flibuste-chord-track.h) ------------------------------------

/// Identifiant de l'extension (le plugin le demande avec `host->get_extension`).
pub const FR_EXT_CHORD_TRACK: &CStr = c"fr.flibuste.chord-track/1";

/// Un élément de la piste, tel que le voit le plugin : même disposition que `fr_item_t` du fichier d'en-tête C.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FrItem {
    pub start_beats: f64,
    pub length_beats: f64,
    pub root: u8,
    pub bass: u8,
    pub flags: u8,
    pub fifths: i8,
    /// Intervalles sur deux octaves : bit i = i demi-tons depuis la fondamentale (0 à 23), basse exclue.
    pub intervals: u32,
    pub index: u32,
    pub name: [u8; 24],
}

impl From<daw_engine::ChordItem> for FrItem {
    fn from(i: daw_engine::ChordItem) -> FrItem {
        FrItem { start_beats: i.start, length_beats: i.len, root: i.root, bass: i.bass, flags: i.flags, fifths: i.fifths, intervals: i.mask, index: i.index, name: i.name }
    }
}

#[repr(C)]
pub struct FrChordTrack {
    pub revision: Option<unsafe extern "C" fn(*const clap_host) -> u32>,
    pub count: Option<unsafe extern "C" fn(*const clap_host, u32) -> u32>,
    pub lookup: Option<unsafe extern "C" fn(*const clap_host, f64, u32, *mut FrItem) -> bool>,
    pub lookup_all: Option<unsafe extern "C" fn(*const clap_host, f64, u32, *mut FrItem, u32) -> u32>,
    pub next_after: Option<unsafe extern "C" fn(*const clap_host, f64, u32, *mut FrItem) -> bool>,
    pub index_at: Option<unsafe extern "C" fn(*const clap_host, f64, u32, *mut u32) -> bool>,
}
// SAFETY: la structure ne contient que des pointeurs de fonctions vers du code qui ne partage rien.
unsafe impl Sync for FrChordTrack {}

/// La piste d'accords en cours de traitement, ou rien (appel hors de `process`, pas de piste).
unsafe fn chord_snapshot<'a>(h: *const clap_host) -> Option<&'a daw_engine::ChordSnapshot> {
    // SAFETY: `host_data` désigne le `HostShared` de l'instance, vivant autant que l'hôte donné au plugin ; le pointeur d'instantané n'est non
    // nul que pendant l'appel de `process` (posé juste avant, retiré juste après : voir `Plugin::set_chords`), donc l'instantané existe.
    shared(h).chords.load(Ordering::Acquire).as_ref()
}
unsafe extern "C" fn chords_revision(h: *const clap_host) -> u32 {
    chord_snapshot(h).map_or(0, |s| s.revision)
}
unsafe extern "C" fn chords_count(h: *const clap_host, kind: u32) -> u32 {
    chord_snapshot(h).map_or(0, |s| s.count(kind))
}
unsafe extern "C" fn chords_lookup(h: *const clap_host, beat: f64, kind: u32, out: *mut FrItem) -> bool {
    match (chord_snapshot(h).and_then(|s| s.lookup(beat, kind)), out.is_null()) {
        (Some(i), false) => {
            *out = i.into();
            true
        }
        _ => false,
    }
}
unsafe extern "C" fn chords_lookup_all(h: *const clap_host, beat: f64, kind: u32, out: *mut FrItem, max: u32) -> u32 {
    let Some(s) = chord_snapshot(h) else { return 0 };
    if out.is_null() || max == 0 {
        return 0;
    }
    let mut tmp = [daw_engine::ChordItem::new(0.0, 0.0); daw_engine::chords::MAX_OVERLAP];
    let n = s.lookup_all(beat, kind, &mut tmp[..(max as usize).min(daw_engine::chords::MAX_OVERLAP)]);
    for (k, i) in tmp[..n].iter().enumerate() {
        *out.add(k) = (*i).into();
    }
    n as u32
}
unsafe extern "C" fn chords_next_after(h: *const clap_host, beat: f64, kind: u32, out: *mut FrItem) -> bool {
    match (chord_snapshot(h).and_then(|s| s.next_after(beat, kind)), out.is_null()) {
        (Some(i), false) => {
            *out = i.into();
            true
        }
        _ => false,
    }
}
unsafe extern "C" fn chords_index_at(h: *const clap_host, beat: f64, kind: u32, out: *mut u32) -> bool {
    match (chord_snapshot(h).and_then(|s| s.index_at(beat, kind)), out.is_null()) {
        (Some(i), false) => {
            *out = i;
            true
        }
        _ => false,
    }
}
static HOST_CHORD_TRACK: FrChordTrack = FrChordTrack {
    revision: Some(chords_revision),
    count: Some(chords_count),
    lookup: Some(chords_lookup),
    lookup_all: Some(chords_lookup_all),
    next_after: Some(chords_next_after),
    index_at: Some(chords_index_at),
};

unsafe extern "C" fn host_get_extension(_: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    let id = CStr::from_ptr(id);
    if id == CLAP_EXT_LATENCY {
        &HOST_LATENCY as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &HOST_PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &HOST_STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS {
        &HOST_NOTE_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_GUI {
        &HOST_GUI as *const _ as *const c_void
    } else if id == CLAP_EXT_TIMER_SUPPORT {
        &HOST_TIMER as *const _ as *const c_void
    } else if id == FR_EXT_CHORD_TRACK {
        &HOST_CHORD_TRACK as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

// ---- listes d'événements ---------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Slot {
    Param(clap_event_param_value),
    Note(clap_event_note),
    Midi(clap_event_midi),
}

impl Slot {
    fn header(&self) -> &clap_event_header {
        match self {
            Slot::Param(e) => &e.header,
            Slot::Note(e) => &e.header,
            Slot::Midi(e) => &e.header,
        }
    }
}

/// Événements en attente pour le prochain bloc, **triés par instant** (exigé par CLAP ; à instant égal, ordre
/// d'arrivée). Capacité fixe : pas d'allocation.
struct EventList {
    slots: [Slot; MAX_EVENTS],
    len: usize,
}

impl EventList {
    /// Insère en gardant le tri ; `false` si la liste est pleine.
    fn insert(&mut self, slot: Slot) -> bool {
        if self.len >= MAX_EVENTS {
            return false;
        }
        let t = slot.header().time;
        let mut i = self.len;
        while i > 0 && self.slots[i - 1].header().time > t {
            self.slots[i] = self.slots[i - 1];
            i -= 1;
        }
        self.slots[i] = slot;
        self.len += 1;
        true
    }
}

unsafe extern "C" fn in_events_size(list: *const clap_input_events) -> u32 {
    (*((*list).ctx as *const EventList)).len as u32
}
unsafe extern "C" fn in_events_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let el = &*((*list).ctx as *const EventList);
    if (index as usize) < el.len {
        el.slots[index as usize].header()
    } else {
        std::ptr::null()
    }
}
unsafe extern "C" fn out_events_try_push(list: *const clap_output_events, event: *const clap_event_header) -> bool {
    let shared = &*((*list).ctx as *const HostShared);
    // SAFETY: le plugin fournit un en-tête valide suivi de la structure que `type_` annonce (contrat CLAP).
    if !event.is_null() && (*event).space_id == CLAP_CORE_EVENT_SPACE_ID {
        match (*event).type_ {
            CLAP_EVENT_PARAM_VALUE if (*event).size as usize >= std::mem::size_of::<clap_event_param_value>() => {
                let e = &*(event as *const clap_event_param_value);
                if e.value.is_finite() {
                    shared.param_out.push(ParamOut::Value { id: e.param_id, value: e.value });
                }
                return true;
            }
            t @ (CLAP_EVENT_PARAM_GESTURE_BEGIN | CLAP_EVENT_PARAM_GESTURE_END) if (*event).size as usize >= std::mem::size_of::<clap_event_param_gesture>() => {
                let e = &*(event as *const clap_event_param_gesture);
                shared.param_out.push(if t == CLAP_EVENT_PARAM_GESTURE_BEGIN { ParamOut::GestureBegin { id: e.param_id } } else { ParamOut::GestureEnd { id: e.param_id } });
                return true;
            }
            _ => {}
        }
    }
    shared.output_events.fetch_add(1, Ordering::Relaxed);
    true
}

// ---- flux d'état -----------------------------------------------------------------------------

unsafe extern "C" fn ostream_write(s: *const clap_ostream, buf: *const c_void, size: u64) -> i64 {
    let v = &mut *((*s).ctx as *mut Vec<u8>);
    v.extend_from_slice(std::slice::from_raw_parts(buf as *const u8, size as usize));
    size as i64
}

struct ReadCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

unsafe extern "C" fn istream_read(s: *const clap_istream, buf: *mut c_void, size: u64) -> i64 {
    let c = &mut *((*s).ctx as *mut ReadCursor);
    let n = (size as usize).min(c.data.len() - c.pos);
    std::ptr::copy_nonoverlapping(c.data[c.pos..].as_ptr(), buf as *mut u8, n);
    c.pos += n;
    n as i64
}

// ---- instance --------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Life {
    Created = 0,
    Activated = 1,
    Processing = 2,
}

/// Noyau partagé entre l'instance côté audio ([`Plugin`]) et les poignées côté contrôle ([`PluginHandle`]) : le plugin
/// n'est détruit que quand le dernier des deux a disparu (côté contrôle, car le graphe remplacé y est libéré).
struct Core {
    raw: *const clap_plugin,
    /// État du cycle de vie, pour que `Drop` sache quoi arrêter (0 créé, 1 activé, 2 en traitement).
    life: std::sync::atomic::AtomicU8,
    _host: Box<clap_host>,
    shared: Box<HostShared>,
    /// L'interface graphique du plugin est créée (`gui.create` fait, `gui.destroy` pas encore).
    gui_open: AtomicBool,
    // Garde la bibliothèque chargée tant que l'instance existe (et la libère après elle).
    _bundle: Arc<Bundle>,
}

// SAFETY: les fonctions du plugin sont appelées selon les règles de fils de CLAP (rappelées en tête de module) : les
// fonctions « fil principal » depuis `PluginHandle`/`Plugin` côté contrôle, les fonctions « fil audio » depuis `Plugin`
// côté audio. L'appelant en est responsable ; le noyau ne contient que des pointeurs immuables et des atomiques.
unsafe impl Send for Core {}
unsafe impl Sync for Core {}

impl Drop for Core {
    fn drop(&mut self) {
        self.gui_destroy(); // la spécification exige de détruire l'interface avant le plugin
        let life = self.life.load(Ordering::Acquire);
        // SAFETY: `raw` valide et jamais utilisé après ; arrêt dans l'ordre imposé par la spécification.
        unsafe {
            if life >= 2 {
                if let Some(s) = (*self.raw).stop_processing {
                    s(self.raw);
                }
            }
            if life >= 1 {
                if let Some(d) = (*self.raw).deactivate {
                    d(self.raw);
                }
            }
            if let Some(d) = (*self.raw).destroy {
                d(self.raw);
            }
        }
    }
}

pub struct Plugin {
    life: Life,
    max_frames: u32,
    in_channels: u32,
    /// Canaux du second port d'entrée audio (chaîne latérale) ; 0 : le plugin n'en a pas.
    sc_channels: u32,
    out_channels: u32,
    /// Dialecte utilisé pour les notes : `CLAP_NOTE_DIALECT_CLAP` ou `CLAP_NOTE_DIALECT_MIDI` (0 : pas d'entrée de notes).
    note_dialect: u32,
    /// Le port de notes accepte aussi le dialecte MIDI (contrôleurs, molette, pressions).
    midi_supported: bool,
    // Boîtes : leurs adresses sont données au plugin et doivent rester stables.
    events: Box<EventList>,
    /// Transport donné au plugin à chaque `process` (adresse stable) ; `None` tant que personne ne l'a fourni.
    transport: Option<Box<clap_event_transport>>,
    in_events: Box<clap_input_events>,
    out_events: Box<clap_output_events>,
    dropped_events: u32,
    /// Déclarée en dernier : libérée après les boîtes ci-dessus.
    core: Arc<Core>,
}

// SAFETY: l'instance peut passer d'un fil à l'autre (création sur le fil principal, traitement sur le fil audio), à
// condition de respecter les règles de fils de CLAP rappelées en tête de module ; l'appelant en est responsable.
unsafe impl Send for Plugin {}

/// Poignée **fil principal** d'une instance en cours de traitement : paramètres, latence, état, ports. Peut être
/// gardée côté contrôle pendant que [`Plugin`] tourne dans le callback ; le plugin n'est détruit qu'à la disparition
/// du dernier des deux.
#[derive(Clone)]
pub struct PluginHandle {
    core: Arc<Core>,
}

impl PluginHandle {
    // ---- interface graphique : tout se fait sur le fil principal (celui où vit l'instance) ----
    pub fn gui_create(&self, api: &str) -> bool {
        self.core.gui_create(api)
    }
    pub fn gui_destroy(&self) {
        self.core.gui_destroy()
    }
    pub fn gui_is_open(&self) -> bool {
        self.core.gui_is_open()
    }
    pub fn gui_set_scale(&self, scale: f64) -> bool {
        self.core.gui_set_scale(scale)
    }
    pub fn gui_size(&self) -> Option<(u32, u32)> {
        self.core.gui_size()
    }
    pub fn gui_can_resize(&self) -> bool {
        self.core.gui_can_resize()
    }
    pub fn gui_adjust_size(&self, w: u32, h: u32) -> (u32, u32) {
        self.core.gui_adjust_size(w, h)
    }
    pub fn gui_set_size(&self, w: u32, h: u32) -> bool {
        self.core.gui_set_size(w, h)
    }
    /// # Safety
    /// Voir `Core::gui_set_parent` : `handle` est une fenêtre native valide de l'API `api`, vivante jusqu'à `gui_destroy`.
    pub unsafe fn gui_set_parent(&self, api: &str, handle: usize) -> bool {
        self.core.gui_set_parent(api, handle)
    }
    pub fn gui_suggest_title(&self, title: &str) {
        self.core.gui_suggest_title(title)
    }
    pub fn gui_show(&self) -> bool {
        self.core.gui_show()
    }
    pub fn gui_hide(&self) -> bool {
        self.core.gui_hide()
    }
    pub fn take_gui_resize_request(&self) -> Option<(u32, u32)> {
        self.core.take_gui_resize_request()
    }
    pub fn take_gui_close_request(&self) -> bool {
        self.core.take_gui_close_request()
    }
    pub fn run_timers(&self, now: Instant) -> usize {
        self.core.run_timers(now)
    }
    pub fn gui_support(&self) -> GuiSupport {
        self.core.gui_support()
    }
    pub fn audio_ports(&self) -> PortsInfo {
        self.core.audio_ports()
    }
    /// Le plugin a un second port d'entrée audio (chaîne latérale).
    pub fn has_sidechain(&self) -> bool {
        self.core.audio_ports().inputs.len() >= 2
    }
    pub fn params(&self) -> Vec<ParamInfo> {
        self.core.params()
    }
    pub fn param_value(&self, id: u32) -> Option<f64> {
        self.core.param_value(id)
    }
    pub fn param_text(&self, id: u32, value: f64) -> Option<String> {
        self.core.param_text(id, value)
    }
    /// Latence annoncée en ce moment, en échantillons.
    pub fn latency(&self) -> u32 {
        self.core.latency()
    }
    /// Vrai (une seule fois) si le plugin a signalé un changement de latence.
    pub fn take_latency_changed(&self) -> bool {
        self.core.take_latency_changed()
    }
    /// Prochain événement de paramètre émis par le plugin (voir `Core::pop_param_event`).
    pub fn pop_param_event(&self) -> Option<ParamOut> {
        self.core.pop_param_event()
    }
    pub fn lost_param_events(&self) -> u32 {
        self.core.lost_param_events()
    }
    /// Le plugin a demandé à être relancé (`request_restart`).
    pub fn restart_requested(&self) -> bool {
        self.core.restart_requested()
    }
    pub fn save_state(&self) -> Result<Vec<u8>, PluginError> {
        self.core.save_state()
    }
    pub fn load_state(&self, data: &[u8]) -> Result<(), PluginError> {
        self.core.load_state(data)
    }
}

impl Core {    fn ext<T>(&self, id: &CStr) -> Option<&T> {
        // SAFETY: `raw` valide ; `get_extension` renvoie soit nul soit un pointeur vers une structure de type `T`
        // (déterminé par l'identifiant demandé) valide tant que l'instance vit.
        unsafe {
            let get = (*self.raw).get_extension?;
            let p = get(self.raw, id.as_ptr());
            (!p.is_null()).then(|| &*(p as *const T))
        }
    }

    // ---- interface graphique (fil principal) --------------------------------------------------------

    /// Crée l'interface **intégrée** (non flottante) pour l'API de fenêtre `api` (`win32`, `cocoa`, `x11`). `false` : refusée.
    pub fn gui_create(&self, api: &str) -> bool {
        let Some(gui) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI) else { return false };
        let (Some(create), Ok(c)) = (gui.create, std::ffi::CString::new(api)) else { return false };
        // SAFETY: `raw` valide, `c` chaîne C valide pendant l'appel.
        let ok = unsafe { create(self.raw, c.as_ptr(), false) };
        if ok {
            self.gui_open.store(true, Ordering::Release);
            self.shared.gui_closed.store(false, Ordering::Release);
            self.shared.gui_hide_requested.store(false, Ordering::Release);
            self.shared.gui_resize_pending.store(false, Ordering::Release);
        }
        ok
    }

    /// Cache puis détruit l'interface (sans effet si elle n'est pas créée).
    pub fn gui_destroy(&self) {
        if !self.gui_open.swap(false, Ordering::AcqRel) {
            return;
        }
        if let Some(gui) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI) {
            // SAFETY: `raw` valide ; l'interface a été créée.
            unsafe {
                if let Some(hide) = gui.hide {
                    hide(self.raw);
                }
                if let Some(destroy) = gui.destroy {
                    destroy(self.raw);
                }
            }
        }
    }

    pub fn gui_is_open(&self) -> bool {
        self.gui_open.load(Ordering::Acquire)
    }

    /// Échelle de l'écran (1,0 = 96 ppp), à donner **avant** `gui_size`.
    pub fn gui_set_scale(&self, scale: f64) -> bool {
        let Some(f) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI).and_then(|g| g.set_scale) else { return false };
        // SAFETY: `raw` valide.
        unsafe { f(self.raw, scale) }
    }

    /// Taille de l'interface (largeur, hauteur), en pixels de la fenêtre hôte.
    pub fn gui_size(&self) -> Option<(u32, u32)> {
        let f = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI)?.get_size?;
        let (mut w, mut h) = (0u32, 0u32);
        // SAFETY: `raw` valide, pointeurs de sortie valides.
        unsafe { f(self.raw, &mut w, &mut h) }.then_some((w, h))
    }

    pub fn gui_can_resize(&self) -> bool {
        let Some(f) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI).and_then(|g| g.can_resize) else { return false };
        // SAFETY: `raw` valide.
        unsafe { f(self.raw) }
    }

    /// Taille que le plugin accepte la plus proche de (`w`, `h`) ; la demande elle-même s'il n'a pas d'avis.
    pub fn gui_adjust_size(&self, w: u32, h: u32) -> (u32, u32) {
        let Some(f) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI).and_then(|g| g.adjust_size) else { return (w, h) };
        let (mut aw, mut ah) = (w, h);
        // SAFETY: `raw` valide, pointeurs de sortie valides.
        if unsafe { f(self.raw, &mut aw, &mut ah) } { (aw, ah) } else { (w, h) }
    }

    pub fn gui_set_size(&self, w: u32, h: u32) -> bool {
        let Some(f) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI).and_then(|g| g.set_size) else { return false };
        // SAFETY: `raw` valide.
        unsafe { f(self.raw, w, h) }
    }

    /// Loge l'interface dans la fenêtre native `handle` (HWND, NSView ou identifiant de fenêtre X11, selon `api`).
    ///
    /// # Safety
    /// `handle` doit désigner une fenêtre native valide de l'API `api`, qui reste vivante jusqu'à `gui_destroy`.
    pub unsafe fn gui_set_parent(&self, api: &str, handle: usize) -> bool {
        let Some(f) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI).and_then(|g| g.set_parent) else { return false };
        let Ok(c) = std::ffi::CString::new(api) else { return false };
        // L'union est d'abord mise à zéro : `x11` (un entier C `unsigned long`) est plus court qu'un pointeur sous Windows (LLP64),
        // les octets restants ne doivent pas rester indéfinis.
        let mut specific = clap_window_handle { ptr: std::ptr::null_mut() };
        if api == "x11" {
            specific.x11 = handle as _;
        } else {
            specific.ptr = handle as *mut c_void;
        }
        let window = clap_window { api: c.as_ptr(), specific };
        f(self.raw, &window)
    }

    pub fn gui_suggest_title(&self, title: &str) {
        let Some(f) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI).and_then(|g| g.suggest_title) else { return };
        if let Ok(c) = std::ffi::CString::new(title) {
            // SAFETY: `raw` valide, chaîne C valide pendant l'appel.
            unsafe { f(self.raw, c.as_ptr()) }
        }
    }

    pub fn gui_show(&self) -> bool {
        let Some(f) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI).and_then(|g| g.show) else { return false };
        // SAFETY: `raw` valide.
        unsafe { f(self.raw) }
    }

    pub fn gui_hide(&self) -> bool {
        let Some(f) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI).and_then(|g| g.hide) else { return false };
        // SAFETY: `raw` valide.
        unsafe { f(self.raw) }
    }

    /// Nouvelle taille demandée par le plugin depuis le dernier appel (`gui.request_resize`), une seule fois.
    pub fn take_gui_resize_request(&self) -> Option<(u32, u32)> {
        self.shared.gui_resize_pending.swap(false, Ordering::AcqRel).then(|| (self.shared.gui_resize_w.load(Ordering::Acquire), self.shared.gui_resize_h.load(Ordering::Acquire)))
    }

    /// Le plugin a demandé de cacher son interface ou a signalé qu'elle s'est fermée (une seule fois).
    pub fn take_gui_close_request(&self) -> bool {
        let hide = self.shared.gui_hide_requested.swap(false, Ordering::AcqRel);
        let closed = self.shared.gui_closed.swap(false, Ordering::AcqRel);
        hide || closed
    }

    /// Appelle `on_timer` pour chaque minuteur du plugin arrivé à échéance à l'instant `now` ; rend le nombre d'appels.
    pub fn run_timers(&self, now: Instant) -> usize {
        let due: Vec<u32> = {
            let mut t = self.shared.timers.lock().unwrap_or_else(|e| e.into_inner());
            t.list.iter_mut().filter(|x| x.due <= now).map(|x| {
                x.due = now + x.period; // on ne rattrape pas les échéances manquées
                x.id
            }).collect()
        };
        let Some(f) = self.ext::<clap_plugin_timer_support>(CLAP_EXT_TIMER_SUPPORT).and_then(|t| t.on_timer) else { return 0 };
        for id in &due {
            // SAFETY: `raw` valide ; fil principal.
            unsafe { f(self.raw, *id) }
        }
        due.len()
    }

    /// Interfaces graphiques annoncées par le plugin (lecture seule : aucune fenêtre n'est créée).
    pub fn gui_support(&self) -> GuiSupport {
        let mut out = GuiSupport::default();
        let Some(gui) = self.ext::<clap_plugin_gui>(CLAP_EXT_GUI) else { return out };
        out.has_gui = true;
        if let Some(is_api_supported) = gui.is_api_supported {
            for api in ["win32", "cocoa", "x11", "wayland"] {
                let c = std::ffi::CString::new(api).expect("nom sans octet nul");
                for floating in [true, false] {
                    // SAFETY: `raw` valide, `c` est une chaîne C valide pendant l'appel.
                    if unsafe { is_api_supported(self.raw, c.as_ptr(), floating) } {
                        out.apis.push((api.to_owned(), floating));
                    }
                }
            }
        }
        if let Some(get_preferred) = gui.get_preferred_api {
            let (mut api, mut floating) = (std::ptr::null::<c_char>(), false);
            // SAFETY: `raw` valide ; les deux pointeurs de sortie sont valides ; la chaîne rendue vit autant que le plugin.
            if unsafe { get_preferred(self.raw, &mut api, &mut floating) } && !api.is_null() {
                out.preferred = Some((unsafe { CStr::from_ptr(api) }.to_string_lossy().into_owned(), floating));
            }
        }
        out
    }

    pub fn audio_ports(&self) -> PortsInfo {
        let mut out = PortsInfo::default();
        let Some(ports) = self.ext::<clap_plugin_audio_ports>(CLAP_EXT_AUDIO_PORTS) else { return out };
        let (Some(count), Some(get)) = (ports.count, ports.get) else { return out };
        for (is_input, list) in [(true, &mut out.inputs), (false, &mut out.outputs)] {
            // SAFETY: appels aux fonctions du plugin avec des arguments valides ; `info` est entièrement remplie si `get` renvoie vrai.
            unsafe {
                for i in 0..count(self.raw, is_input) {
                    let mut info = std::mem::MaybeUninit::<clap_audio_port_info>::zeroed();
                    if get(self.raw, i, is_input, info.as_mut_ptr()) {
                        list.push(info.assume_init().channel_count);
                    }
                }
            }
        }
        out
    }

    pub fn params(&self) -> Vec<ParamInfo> {
        let Some(p) = self.ext::<clap_plugin_params>(CLAP_EXT_PARAMS) else { return Vec::new() };
        let (Some(count), Some(get_info)) = (p.count, p.get_info) else { return Vec::new() };
        let mut out = Vec::new();
        // SAFETY: voir `audio_ports` ; les chaînes de `clap_param_info` sont des tableaux terminés par NUL.
        unsafe {
            for i in 0..count(self.raw) {
                let mut info = std::mem::MaybeUninit::<clap_param_info>::zeroed();
                if get_info(self.raw, i, info.as_mut_ptr()) {
                    let info = info.assume_init();
                    out.push(ParamInfo {
                        id: info.id,
                        name: CStr::from_ptr(info.name.as_ptr()).to_string_lossy().into_owned(),
                        module: CStr::from_ptr(info.module.as_ptr()).to_string_lossy().into_owned(),
                        min: info.min_value,
                        max: info.max_value,
                        default: info.default_value,
                        flags: info.flags,
                    });
                }
            }
        }
        out
    }

    pub fn param_value(&self, id: u32) -> Option<f64> {
        let get = self.ext::<clap_plugin_params>(CLAP_EXT_PARAMS)?.get_value?;
        let mut v = 0.0f64;
        // SAFETY: `v` est un f64 valide où écrire.
        unsafe { get(self.raw, id, &mut v) }.then_some(v)
    }

    pub fn param_text(&self, id: u32, value: f64) -> Option<String> {
        let f = self.ext::<clap_plugin_params>(CLAP_EXT_PARAMS)?.value_to_text?;
        let mut buf = [0 as c_char; 128];
        // SAFETY: tampon de la capacité annoncée ; le plugin termine la chaîne par NUL.
        unsafe { f(self.raw, id, value, buf.as_mut_ptr(), buf.len() as u32).then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()) }
    }

    /// Latence annoncée par le plugin, en échantillons (valable une fois activé).
    pub fn latency(&self) -> u32 {
        match self.ext::<clap_plugin_latency>(CLAP_EXT_LATENCY).and_then(|l| l.get) {
            // SAFETY: `raw` valide.
            Some(get) => unsafe { get(self.raw) },
            None => 0,
        }
    }

    /// Vrai (une seule fois) si le plugin a signalé un changement de latence : la compensation doit être recalculée.
    pub fn take_latency_changed(&self) -> bool {
        self.shared.latency_changed.swap(false, Ordering::AcqRel)
    }

    /// Prochain événement de paramètre émis par le plugin (valeur ou geste) depuis le dernier appel, dans l'ordre.
    /// **Un seul fil consommateur** (le fil de contrôle).
    pub fn pop_param_event(&self) -> Option<ParamOut> {
        self.shared.param_out.pop()
    }

    /// Événements de paramètres perdus (file pleine) depuis le début.
    pub fn lost_param_events(&self) -> u32 {
        self.shared.param_out.lost.load(Ordering::Relaxed)
    }

    pub fn restart_requested(&self) -> bool {
        self.shared.restart_requested.load(Ordering::Acquire)
    }

    pub fn save_state(&self) -> Result<Vec<u8>, PluginError> {
        let save = self.ext::<clap_plugin_state>(CLAP_EXT_STATE).and_then(|s| s.save).ok_or(PluginError::NoExtension("state"))?;
        let mut bytes: Vec<u8> = Vec::new();
        let stream = clap_ostream { ctx: &mut bytes as *mut Vec<u8> as *mut c_void, write: Some(ostream_write) };
        // SAFETY: `stream` et `bytes` vivent pendant l'appel ; `ostream_write` respecte le contrat du flux.
        if unsafe { save(self.raw, &stream) } {
            Ok(bytes)
        } else {
            Err(PluginError::State("save refusé"))
        }
    }

    pub fn load_state(&self, data: &[u8]) -> Result<(), PluginError> {
        let load = self.ext::<clap_plugin_state>(CLAP_EXT_STATE).and_then(|s| s.load).ok_or(PluginError::NoExtension("state"))?;
        let mut cursor = ReadCursor { data, pos: 0 };
        let stream = clap_istream { ctx: &mut cursor as *mut ReadCursor as *mut c_void, read: Some(istream_read) };
        // SAFETY: voir `save_state`.
        if unsafe { load(self.raw, &stream) } {
            Ok(())
        } else {
            Err(PluginError::State("load refusé"))
        }
    }

    /// Dialectes (masques) des ports d'entrée de notes : (pris en charge, préféré). Diagnostic.
    pub fn note_port_dialects(&self) -> Vec<(u32, u32)> {
        let Some(p) = self.ext::<clap_plugin_note_ports>(CLAP_EXT_NOTE_PORTS) else { return Vec::new() };
        let (Some(count), Some(get)) = (p.count, p.get) else { return Vec::new() };
        let mut out = Vec::new();
        // SAFETY: voir `note_input_ports`.
        unsafe {
            for i in 0..count(self.raw, true) {
                let mut info = std::mem::MaybeUninit::<clap_note_port_info>::zeroed();
                if get(self.raw, i, true, info.as_mut_ptr()) {
                    let info = info.assume_init();
                    out.push((info.supported_dialects, info.preferred_dialect));
                }
            }
        }
        out
    }

}


fn zeroed_slot() -> Slot {
    // SAFETY: structure C de nombres et de pointeurs : tout-zéro est une valeur valide.
    Slot::Param(unsafe { std::mem::zeroed() })
}

impl Plugin {
    /// Crée l'instance et appelle `init` (fil principal).
    pub fn instantiate(bundle: &Arc<Bundle>, plugin_id: &str) -> Result<Plugin, PluginError> {
        let id = std::ffi::CString::new(plugin_id).map_err(|_| PluginError::UnknownPlugin)?;
        let factory = bundle.factory().ok_or(PluginError::CreateFailed)?;

        let shared = Box::new(HostShared::default());
        let host = Box::new(clap_host {
            clap_version: CLAP_VERSION,
            host_data: &*shared as *const HostShared as *mut c_void,
            name: c"DAW perso".as_ptr(),
            vendor: c"Jean".as_ptr(),
            url: c"".as_ptr(),
            version: c"0.1".as_ptr(),
            get_extension: Some(host_get_extension),
            request_restart: Some(host_request_restart),
            request_process: Some(host_request_process),
            request_callback: Some(host_request_callback),
        });
        let mut events = Box::new(EventList { slots: [zeroed_slot(); MAX_EVENTS], len: 0 });
        let in_events = Box::new(clap_input_events {
            ctx: &mut *events as *mut EventList as *mut c_void,
            size: Some(in_events_size),
            get: Some(in_events_get),
        });
        let out_events = Box::new(clap_output_events { ctx: &*shared as *const HostShared as *mut c_void, try_push: Some(out_events_try_push) });

        // SAFETY: `factory` valide tant que le bundle vit ; `host` reste à la même adresse jusqu'à la destruction de l'instance.
        let raw = unsafe {
            let f = &*factory;
            let create = f.create_plugin.ok_or(PluginError::CreateFailed)?;
            create(factory, &*host, id.as_ptr())
        };
        if raw.is_null() {
            return Err(PluginError::UnknownPlugin);
        }
        // SAFETY: `raw` vient d'être créé et n'a pas encore reçu `init`.
        let inited = unsafe { (*raw).init.map_or(false, |init| init(raw)) };
        // Le noyau possède désormais le plugin : même si `init` a échoué, sa destruction (exigée par la spécification)
        // se fait à la disparition du noyau.
        let core = Arc::new(Core { raw, life: std::sync::atomic::AtomicU8::new(0), _host: host, shared, gui_open: AtomicBool::new(false), _bundle: Arc::clone(bundle) });
        let mut p = Plugin {
            life: Life::Created,
            max_frames: 0,
            in_channels: 0,
            sc_channels: 0,
            out_channels: 0,
            note_dialect: 0,
            midi_supported: false,
            events,
            in_events,
            out_events,
            transport: None,
            dropped_events: 0,
            core,
        };
        if !inited {
            return Err(PluginError::InitFailed);
        }
        let ports = p.audio_ports();
        p.in_channels = ports.inputs.first().copied().unwrap_or(0);
        p.sc_channels = ports.inputs.get(1).copied().unwrap_or(0);
        p.out_channels = ports.outputs.first().copied().unwrap_or(0);
        p.note_dialect = p.choose_note_dialect();
        p.midi_supported = p.core.note_port_dialects().first().is_some_and(|&(supported, _)| supported & CLAP_NOTE_DIALECT_MIDI != 0);
        Ok(p)
    }

    fn set_life(&mut self, l: Life) {
        self.life = l;
        self.core.life.store(l as u8, Ordering::Release);
    }

    /// Poignée « fil principal » à garder côté contrôle (état, paramètres, latence) pendant que l'instance traite.
    pub fn handle(&self) -> PluginHandle {
        PluginHandle { core: Arc::clone(&self.core) }
    }

    // ---- fil principal (délégués au noyau) -----------------------------------------------

    pub fn gui_support(&self) -> GuiSupport {
        self.core.gui_support()
    }
    pub fn audio_ports(&self) -> PortsInfo {
        self.core.audio_ports()
    }
    pub fn params(&self) -> Vec<ParamInfo> {
        self.core.params()
    }
    pub fn param_value(&self, id: u32) -> Option<f64> {
        self.core.param_value(id)
    }
    pub fn param_text(&self, id: u32, value: f64) -> Option<String> {
        self.core.param_text(id, value)
    }
    /// Latence annoncée par le plugin, en échantillons (valable une fois activé).
    pub fn latency(&self) -> u32 {
        self.core.latency()
    }
    pub fn take_latency_changed(&self) -> bool {
        self.core.take_latency_changed()
    }
    /// Prochain événement de paramètre émis par le plugin (voir `Core::pop_param_event`).
    pub fn pop_param_event(&self) -> Option<ParamOut> {
        self.core.pop_param_event()
    }
    pub fn lost_param_events(&self) -> u32 {
        self.core.lost_param_events()
    }
    pub fn restart_requested(&self) -> bool {
        self.core.restart_requested()
    }
    pub fn save_state(&self) -> Result<Vec<u8>, PluginError> {
        self.core.save_state()
    }
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), PluginError> {
        self.core.load_state(data)
    }
    pub fn note_port_dialects(&self) -> Vec<(u32, u32)> {
        self.core.note_port_dialects()
    }

    /// Prépare le traitement par blocs d'au plus `max_frames` échantillons.
    pub fn activate(&mut self, sample_rate: f64, max_frames: u32) -> Result<(), PluginError> {
        if self.life != Life::Created {
            return Err(PluginError::State("déjà activé"));
        }
        // SAFETY: `raw` valide, état Created.
        let ok = unsafe { (*self.core.raw).activate.map_or(false, |a| a(self.core.raw, sample_rate, 1, max_frames.max(1))) };
        if !ok {
            return Err(PluginError::ActivateFailed);
        }
        self.max_frames = max_frames.max(1);
        self.set_life(Life::Activated);
        Ok(())
    }

    pub fn deactivate(&mut self) {
        self.stop_processing();
        if self.life == Life::Activated {
            // SAFETY: `raw` valide, état Activated.
            unsafe {
                if let Some(d) = (*self.core.raw).deactivate {
                    d(self.core.raw);
                }
            }
            self.set_life(Life::Created);
        }
    }

    // ---- fil audio -----------------------------------------------------------------------

    pub fn start_processing(&mut self) -> Result<(), PluginError> {
        match self.life {
            Life::Processing => Ok(()),
            Life::Created => Err(PluginError::State("pas activé")),
            Life::Activated => {
                // SAFETY: `raw` valide, état Activated.
                if unsafe { (*self.core.raw).start_processing.map_or(true, |s| s(self.core.raw)) } {
                    self.set_life(Life::Processing);
                    Ok(())
                } else {
                    Err(PluginError::StartFailed)
                }
            }
        }
    }

    pub fn stop_processing(&mut self) {
        if self.life == Life::Processing {
            // SAFETY: `raw` valide, état Processing.
            unsafe {
                if let Some(s) = (*self.core.raw).stop_processing {
                    s(self.core.raw);
                }
            }
            self.set_life(Life::Activated);
        }
    }

    /// Vide les mémoires internes du plugin (tête de lecture déplacée). Fil audio ; sans effet hors traitement.
    pub fn reset(&mut self) {
        if self.life == Life::Processing {
            // SAFETY: `raw` valide, état Processing (reset est un appel du fil audio).
            unsafe {
                if let Some(r) = (*self.core.raw).reset {
                    r(self.core.raw);
                }
            }
        }
        // Les notes en attente n'ont plus de sens après un déplacement, mais un **réglage de paramètre** reste valable (un réglage fait
        // à l'arrêt, juste avant de déplacer la tête de lecture, ne doit pas se perdre) : on ne garde que ceux-là, à l'instant 0.
        let mut kept = 0;
        for i in 0..self.events.len {
            if let Slot::Param(mut e) = self.events.slots[i] {
                e.header.time = 0;
                self.events.slots[kept] = Slot::Param(e);
                kept += 1;
            }
        }
        self.events.len = kept;
    }

    /// Met en file un changement de paramètre pour le prochain bloc, `time` échantillons après son début.
    /// Retourne `false` (et compte la perte) si la file est pleine. Sans allocation.
    pub fn queue_param(&mut self, id: u32, value: f64, time: u32) -> bool {
        let e = clap_event_param_value {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_param_value>() as u32,
                time,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_PARAM_VALUE,
                flags: 0,
            },
            param_id: id,
            cookie: std::ptr::null_mut(),
            note_id: -1,
            port_index: -1,
            channel: -1,
            key: -1,
            value,
        };
        self.push_slot(Slot::Param(e))
    }

    /// Met en file une note (début ou fin) pour le prochain bloc, `time` échantillons après son début, sur le premier
    /// port de notes. `velocity` de 0 à 1. Le format dépend du dialecte du port : événements de notes CLAP si le
    /// plugin les accepte, sinon octets MIDI (`CLAP_EVENT_MIDI`), comme l'exige la spécification. Sans allocation.
    pub fn queue_note(&mut self, on: bool, channel: u8, key: u8, velocity: f64, time: u32) -> bool {
        let header = |size: usize, type_| clap_event_header { size: size as u32, time, space_id: CLAP_CORE_EVENT_SPACE_ID, type_, flags: 0 };
        if self.note_dialect == CLAP_NOTE_DIALECT_MIDI {
            let status = (if on { 0x90 } else { 0x80 }) | (channel & 15);
            let vel = if on { (velocity * 127.0).round().clamp(1.0, 127.0) as u8 } else { 0 };
            return self.push_slot(Slot::Midi(clap_event_midi {
                header: header(std::mem::size_of::<clap_event_midi>(), CLAP_EVENT_MIDI),
                port_index: 0,
                data: [status, key & 127, vel],
            }));
        }
        let e = clap_event_note {
            header: header(std::mem::size_of::<clap_event_note>(), if on { CLAP_EVENT_NOTE_ON } else { CLAP_EVENT_NOTE_OFF }),
            note_id: -1,
            port_index: 0,
            channel: channel as i16,
            key: key as i16,
            velocity,
        };
        self.push_slot(Slot::Note(e))
    }

    /// Met en file un message MIDI brut (contrôleur, molette, pression) à `time` échantillons du début du prochain bloc.
    /// Il part en `CLAP_EVENT_MIDI`, ce qui exige que le port de notes du plugin accepte le dialecte MIDI ; sinon le
    /// message est abandonné (compté dans `dropped_events`) et la fonction retourne `false`. Sans allocation.
    pub fn queue_midi(&mut self, data: [u8; 3], time: u32) -> bool {
        if !self.midi_supported {
            self.dropped_events += 1;
            return false;
        }
        let header = clap_event_header { size: std::mem::size_of::<clap_event_midi>() as u32, time, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: CLAP_EVENT_MIDI, flags: 0 };
        self.push_slot(Slot::Midi(clap_event_midi { header, port_index: 0, data }))
    }

    fn push_slot(&mut self, slot: Slot) -> bool {
        if self.events.insert(slot) {
            true
        } else {
            self.dropped_events += 1;
            false
        }
    }

    /// Dialecte à utiliser sur le premier port d'entrée de notes : CLAP s'il est pris en charge (le dialecte préféré
    /// en premier), sinon MIDI ; 0 s'il n'y a pas de port d'entrée de notes exploitable.
    fn choose_note_dialect(&self) -> u32 {
        match self.core.note_port_dialects().first() {
            Some(&(supported, preferred)) => {
                if preferred & CLAP_NOTE_DIALECT_CLAP != 0 || supported & CLAP_NOTE_DIALECT_CLAP != 0 {
                    CLAP_NOTE_DIALECT_CLAP
                } else if supported & CLAP_NOTE_DIALECT_MIDI != 0 {
                    CLAP_NOTE_DIALECT_MIDI
                } else {
                    0
                }
            }
            None => 0,
        }
    }

    /// Le plugin a-t-il une entrée de notes exploitable (instrument) ?
    pub fn accepts_notes(&self) -> bool {
        self.note_dialect != 0
    }

    /// Nombre de ports d'entrée de notes déclarés.
    pub fn note_input_ports(&self) -> u32 {
        self.note_port_dialects().len() as u32
    }

    /// Événements de paramètres perdus faute de place (doit rester 0).
    pub fn dropped_events(&self) -> u32 {
        self.dropped_events
    }

    /// Donne (ou retire, avec `null`) la piste d'accords au plugin. À poser **juste avant** `process` avec l'adresse d'un instantané qui
    /// vivra jusqu'au retour de `process`, et à retirer juste après : hors de ce créneau l'extension répond « rien ».
    pub fn set_chords(&self, snapshot: *const daw_engine::ChordSnapshot) {
        self.core.shared.chords.store(snapshot as *mut _, Ordering::Release);
    }

    /// Position, tempo, chiffrage et boucle du transport pour le prochain `process` (et les suivants, jusqu'au prochain appel).
    /// Sans allocation après le premier appel.
    pub fn set_transport(&mut self, t: &daw_engine::TransportInfo) {
        let beat = |b: f64| (b * CLAP_BEATTIME_FACTOR as f64).round() as i64;
        let sec = |x: f64| (x * CLAP_SECTIME_FACTOR as f64).round() as i64;
        let mut flags = CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_BEATS_TIMELINE | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE | CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
        if t.playing {
            flags |= CLAP_TRANSPORT_IS_PLAYING;
        }
        if t.loop_active {
            flags |= CLAP_TRANSPORT_IS_LOOP_ACTIVE;
        }
        let value = clap_event_transport {
            header: clap_event_header { size: std::mem::size_of::<clap_event_transport>() as u32, time: 0, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: CLAP_EVENT_TRANSPORT, flags: 0 },
            flags,
            song_pos_beats: beat(t.beats),
            song_pos_seconds: sec(t.seconds),
            tempo: t.tempo,
            tempo_inc: 0.0,
            loop_start_beats: beat(t.loop_start_beats),
            loop_end_beats: beat(t.loop_end_beats),
            loop_start_seconds: sec(t.loop_start_seconds),
            loop_end_seconds: sec(t.loop_end_seconds),
            bar_start: beat(t.bar_start_beats),
            bar_number: t.bar_number,
            tsig_num: t.tsig_num,
            tsig_denom: t.tsig_den,
        };
        match self.transport.as_deref_mut() {
            Some(slot) => *slot = value,
            None => self.transport = Some(Box::new(value)),
        }
    }

    /// Traite un bloc. Les entrées sont ignorées pour un instrument (sans port d'entrée). Un plugin mono ne reçoit
    /// que le canal gauche. Sans allocation ni verrou.
    pub fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]) -> Result<ProcessStatus, PluginError> {
        self.process_with_sidechain(in_l, in_r, None, out_l, out_r)
    }

    /// Le plugin a-t-il un second port d'entrée audio (la chaîne latérale d'un compresseur, d'un gate...) ?
    pub fn has_sidechain(&self) -> bool {
        self.sc_channels > 0
    }

    /// Comme `process`, avec le signal de la chaîne latérale sur le second port d'entrée (`sc` : gauche, droite). Sans `sc`, ou si le
    /// plugin n'a pas de second port, le port est omis comme avant. Même longueur que les autres tampons exigée : sinon `BadBlock`.
    pub fn process_with_sidechain(&mut self, in_l: &[f32], in_r: &[f32], sc: Option<(&[f32], &[f32])>, out_l: &mut [f32], out_r: &mut [f32]) -> Result<ProcessStatus, PluginError> {
        if self.life != Life::Processing {
            return Err(PluginError::State("traitement non démarré"));
        }
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        if n == 0 || n as u32 > self.max_frames {
            return Err(PluginError::BadBlock);
        }
        // CLAP lit les entrées sans les modifier : le `*mut` ne sert qu'à respecter la signature C.
        let mut in_ptrs = [in_l.as_ptr() as *mut f32, in_r.as_ptr() as *mut f32];
        let mut out_ptrs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        let audio_in = clap_audio_buffer { data32: in_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: self.in_channels.min(2), latency: 0, constant_mask: 0 };
        // Second port d'entrée (chaîne latérale) : seulement si le plugin en a un et si on lui en donne un signal de la bonne longueur.
        let sc = sc.filter(|(l, r)| self.sc_channels > 0 && l.len() >= n && r.len() >= n);
        let mut sc_ptrs = sc.map_or([std::ptr::null_mut(), std::ptr::null_mut()], |(l, r)| [l.as_ptr() as *mut f32, r.as_ptr() as *mut f32]);
        let audio_sc = clap_audio_buffer { data32: sc_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: self.sc_channels.min(2), latency: 0, constant_mask: 0 };
        let audio_ins = [audio_in, audio_sc];
        let mut audio_out = clap_audio_buffer { data32: out_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: self.out_channels.min(2), latency: 0, constant_mask: 0 };
        let process = clap_process {
            steady_time: -1,
            frames_count: n as u32,
            transport: self.transport.as_deref().map_or(std::ptr::null(), |t| t as *const clap_event_transport),
            audio_inputs: audio_ins.as_ptr(),
            audio_outputs: &mut audio_out,
            audio_inputs_count: if sc.is_some() { 2 } else { u32::from(self.in_channels > 0) },
            audio_outputs_count: u32::from(self.out_channels > 0),
            in_events: &*self.in_events,
            out_events: &*self.out_events,
        };
        // SAFETY: tous les pointeurs de `process` visent des données vivantes pendant l'appel ; les tampons ont au
        // moins `n` échantillons ; `in_events` lit `self.events`, que rien ne modifie pendant l'appel.
        let status = unsafe { (*self.core.raw).process.map_or(CLAP_PROCESS_ERROR, |f| f(self.core.raw, &process)) };
        self.events.len = 0;
        Ok(match status {
            1 => ProcessStatus::Continue,
            2 => ProcessStatus::ContinueIfNotQuiet,
            3 => ProcessStatus::Tail,
            4 => ProcessStatus::Sleep,
            _ => ProcessStatus::Error,
        })
    }
}
