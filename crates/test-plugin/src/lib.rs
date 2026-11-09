//! Plugin CLAP de test : **gain stéréo avec une latence fixe de 64 échantillons**.
//!
//! Ce n'est pas un vrai effet : c'est une référence au comportement exactement connu, pour tester l'hôte.
//! - 1 entrée et 1 sortie stéréo ;
//! - 2 paramètres : `Gain` (0 à 2, défaut 1), appliqué à l'échantillon près selon l'instant des événements, et
//!   `Latence supplémentaire` (0 à 192 échantillons, défaut 0) qui, comme dans un vrai plugin, ne change la latence
//!   qu'à la prochaine activation et demande donc un redémarrage à l'hôte (`request_restart`) ;
//! - latence annoncée : `LATENCY` + latence supplémentaire active (le signal sort retardé d'autant) ;
//! - 1 port d'entrée de notes (dialecte CLAP) : tant que des notes sont tenues, la sortie reçoit en plus (ni retardée, ni
//!   multipliée par le gain) un continu égal à la somme de `vélocité × touche / 127` des notes tenues, à l'échantillon près ;
//! - état : 16 octets (`DTP2` + gain en f64 + latence supplémentaire en u32, petit-boutiste) ; l'ancien format `DTP1`
//!   (12 octets, gain seul) est encore lu.
//!
//! Il ne fait ni allocation ni verrou dans `process` (le retard est alloué à l'activation).

#![allow(clippy::missing_safety_doc)]

pub mod gui;

/// Dernier transport reçu par `process` (pour les tests de l'hôte) : valeurs brutes de `clap_event_transport`.
pub mod seen {
    use std::sync::atomic::{AtomicU32, AtomicU64, AtomicI64, Ordering::Relaxed};

    static GIVEN: AtomicU32 = AtomicU32::new(0);
    static FLAGS: AtomicU32 = AtomicU32::new(0);
    static BEATS: AtomicI64 = AtomicI64::new(0);
    static SECONDS: AtomicI64 = AtomicI64::new(0);
    static TEMPO: AtomicU64 = AtomicU64::new(0);
    static TSIG: AtomicU32 = AtomicU32::new(0);
    static BAR: AtomicI64 = AtomicI64::new(0);
    static LOOP_START: AtomicI64 = AtomicI64::new(0);
    static LOOP_END: AtomicI64 = AtomicI64::new(0);
    static CALLS: AtomicU32 = AtomicU32::new(0);

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Seen {
        pub given: bool,
        pub flags: u32,
        pub beats: f64,
        pub seconds: f64,
        pub tempo: f64,
        pub tsig_num: u16,
        pub tsig_den: u16,
        pub bar_number: i32,
        pub bar_start: f64,
        pub loop_start_beats: f64,
        pub loop_end_beats: f64,
        pub calls: u32,
    }

    pub(crate) fn store(t: Option<&clap_sys::events::clap_event_transport>) {
        CALLS.fetch_add(1, Relaxed);
        let Some(t) = t else { return GIVEN.store(0, Relaxed) };
        GIVEN.store(1, Relaxed);
        FLAGS.store(t.flags, Relaxed);
        BEATS.store(t.song_pos_beats, Relaxed);
        SECONDS.store(t.song_pos_seconds, Relaxed);
        TEMPO.store(t.tempo.to_bits(), Relaxed);
        TSIG.store(u32::from(t.tsig_num) << 16 | u32::from(t.tsig_denom), Relaxed);
        BAR.store(i64::from(t.bar_number) << 32 | (t.bar_start >> 16 & 0xffff_ffff), Relaxed);
        LOOP_START.store(t.loop_start_beats, Relaxed);
        LOOP_END.store(t.loop_end_beats, Relaxed);
    }

    /// Ce que le plugin a lu de la piste d'accords de l'hôte pendant le dernier `process`.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ChordSeen {
        pub extension: bool,
        pub revision: u32,
        pub chord_root: i32,
        pub chord_start: f64,
        pub chord_len: f64,
        pub chord_mask: u32,
        pub chord_bass: u8,
        pub chord_name: [u8; 24],
        pub scales: u32,
        pub scale_roots: [u8; 4],
        pub next_chord_start: f64,
        pub key_fifths: i8,
        pub measure_index: i32,
        pub measure_len: f64,
        pub pattern_slot: i32,
        pub chord_count: u32,
        pub lookup_when_not_processing: bool,
    }

    static CH: std::sync::Mutex<Option<ChordSeen>> = std::sync::Mutex::new(None);

    pub(crate) fn store_chords(c: ChordSeen) {
        *CH.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
    }

    pub fn last_chords() -> Option<ChordSeen> {
        *CH.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn last() -> Seen {
        let f = 2147483648.0;
        let bar = BAR.load(Relaxed);
        Seen {
            given: GIVEN.load(Relaxed) == 1,
            flags: FLAGS.load(Relaxed),
            beats: BEATS.load(Relaxed) as f64 / f,
            seconds: SECONDS.load(Relaxed) as f64 / f,
            tempo: f64::from_bits(TEMPO.load(Relaxed)),
            tsig_num: (TSIG.load(Relaxed) >> 16) as u16,
            tsig_den: (TSIG.load(Relaxed) & 0xffff) as u16,
            bar_number: (bar >> 32) as i32,
            bar_start: ((bar & 0xffff_ffff) << 16) as f64 / f,
            loop_start_beats: LOOP_START.load(Relaxed) as f64 / f,
            loop_end_beats: LOOP_END.load(Relaxed) as f64 / f,
            calls: CALLS.load(Relaxed),
        }
    }
}

use std::ffi::{c_char, c_void, CStr};

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_note, clap_event_param_gesture, clap_event_param_value, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_GESTURE_BEGIN, CLAP_EVENT_PARAM_GESTURE_END, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::ext::note_ports::{clap_note_port_info, clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP};
use clap_sys::ext::audio_ports::{clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS, CLAP_PORT_STEREO};
use clap_sys::ext::latency::{clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::params::{clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_STEPPED};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::{clap_id, CLAP_INVALID_ID};
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::plugin_features::{CLAP_PLUGIN_FEATURE_AUDIO_EFFECT, CLAP_PLUGIN_FEATURE_STEREO, CLAP_PLUGIN_FEATURE_UTILITY};
use clap_sys::process::{clap_process, CLAP_PROCESS_CONTINUE, CLAP_PROCESS_ERROR};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

pub const PLUGIN_ID: &CStr = c"com.daw-perso.test-gain";
/// Variante à **chaîne latérale** : un second port d'entrée audio ; quand il reçoit un signal, la sortie est coupée à tout échantillon où ce
/// signal dépasse 0,5 en valeur absolue (un « ducker » parfait), sinon elle se comporte comme le gain de test.
pub const SIDECHAIN_ID: &CStr = c"com.daw-perso.test-sidechain";
pub const LATENCY: u32 = 64;
pub const PARAM_GAIN: clap_id = 0;
pub const PARAM_EXTRA_LATENCY: clap_id = 1;
pub const MAX_EXTRA_LATENCY: u32 = 192;
pub const STATE_LEN: usize = 16;

// ---------------------------------------------------------------------------------------------
// Statiques exportées. Les structures de clap-sys contiennent des pointeurs bruts : on les enveloppe
// pour affirmer qu'elles sont partageables (elles sont immuables et ne pointent que vers du statique).

#[repr(transparent)]
pub struct SyncEntry(pub clap_plugin_entry);
// SAFETY: contenu immuable, pointeurs vers des fonctions et des statiques.
unsafe impl Sync for SyncEntry {}

#[repr(transparent)]
struct SyncFactory(clap_plugin_factory);
// SAFETY: idem.
unsafe impl Sync for SyncFactory {}

struct SyncFeatures([*const c_char; 4]);
// SAFETY: pointeurs vers des littéraux C statiques.
unsafe impl Sync for SyncFeatures {}

static FEATURES: SyncFeatures = SyncFeatures([
    CLAP_PLUGIN_FEATURE_AUDIO_EFFECT.as_ptr(),
    CLAP_PLUGIN_FEATURE_UTILITY.as_ptr(),
    CLAP_PLUGIN_FEATURE_STEREO.as_ptr(),
    std::ptr::null(),
]);

static DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: PLUGIN_ID.as_ptr(),
    name: c"Gain de test".as_ptr(),
    vendor: c"DAW perso (tests)".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"Gain stereo avec latence fixe, pour tester l'hote".as_ptr(),
    features: FEATURES.0.as_ptr(),
};

static FACTORY: SyncFactory = SyncFactory(clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
});

#[no_mangle]
#[used]
pub static clap_entry: SyncEntry = SyncEntry(clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
});

/// L'entrée du plugin, pour les tests qui ne passent pas par le disque.
pub fn entry() -> &'static clap_plugin_entry {
    &clap_entry.0
}

unsafe extern "C" fn entry_init(_path: *const c_char) -> bool {
    true
}
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn entry_get_factory(id: *const c_char) -> *const c_void {
    if !id.is_null() && CStr::from_ptr(id) == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY.0 as *const clap_plugin_factory as *const c_void
    } else {
        std::ptr::null()
    }
}
static DESCRIPTOR_SC: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: SIDECHAIN_ID.as_ptr(),
    name: c"Ducker de test".as_ptr(),
    vendor: c"DAW perso (tests)".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"Coupe l'entree quand la chaine laterale depasse 0,5, pour tester l'hote".as_ptr(),
    features: FEATURES.0.as_ptr(),
};

unsafe extern "C" fn factory_count(_: *const clap_plugin_factory) -> u32 {
    2
}
unsafe extern "C" fn factory_descriptor(_: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor {
    match index {
        0 => &DESCRIPTOR,
        1 => &DESCRIPTOR_SC,
        _ => std::ptr::null(),
    }
}

// ---------------------------------------------------------------------------------------------

#[repr(C)]
struct Instance {
    clap: clap_plugin,
    host: *const clap_host,
    gain: f64,
    /// Latence supplémentaire demandée (relue à l'activation) et celle qui est active.
    extra_pending: u32,
    extra_active: u32,
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    pos: usize,
    /// Vélocité de chaque touche tenue (0 = relâchée) et leur contribution cumulée au continu de sortie.
    held: [f32; 128],
    dc: f32,
    /// Variante à chaîne latérale (second port d'entrée).
    sidechain: bool,
}

unsafe fn inst<'a>(p: *const clap_plugin) -> &'a mut Instance {
    &mut *((*p).plugin_data as *mut Instance)
}

unsafe extern "C" fn factory_create(_: *const clap_plugin_factory, host: *const clap_host, id: *const c_char) -> *const clap_plugin {
    if id.is_null() {
        return std::ptr::null();
    }
    let sidechain = CStr::from_ptr(id) == SIDECHAIN_ID;
    if !sidechain && CStr::from_ptr(id) != PLUGIN_ID {
        return std::ptr::null();
    }
    gui::remember_host(host);
    let raw = Box::into_raw(Box::new(Instance {
        clap: clap_plugin {
            desc: if sidechain { &DESCRIPTOR_SC } else { &DESCRIPTOR },
            plugin_data: std::ptr::null_mut(),
            init: Some(plugin_init),
            destroy: Some(plugin_destroy),
            activate: Some(plugin_activate),
            deactivate: Some(plugin_deactivate),
            start_processing: Some(plugin_start),
            stop_processing: Some(plugin_stop),
            reset: Some(plugin_reset),
            process: Some(plugin_process),
            get_extension: Some(plugin_extension),
            on_main_thread: Some(plugin_main_thread),
        },
        host,
        gain: 1.0,
        extra_pending: 0,
        extra_active: 0,
        delay_l: Vec::new(),
        delay_r: Vec::new(),
        pos: 0,
        held: [0.0; 128],
        dc: 0.0,
        sidechain,
    }));
    (*raw).clap.plugin_data = raw as *mut c_void;
    &(*raw).clap
}

unsafe extern "C" fn plugin_init(_: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn plugin_destroy(p: *const clap_plugin) {
    drop(Box::from_raw((*p).plugin_data as *mut Instance));
}
unsafe extern "C" fn plugin_activate(p: *const clap_plugin, _rate: f64, _min: u32, _max: u32) -> bool {
    let i = inst(p);
    i.extra_active = i.extra_pending;
    let total = (LATENCY + i.extra_active) as usize;
    i.delay_l = vec![0.0; total];
    i.delay_r = vec![0.0; total];
    i.pos = 0;
    true
}
unsafe extern "C" fn plugin_deactivate(_: *const clap_plugin) {}
unsafe extern "C" fn plugin_start(_: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn plugin_stop(_: *const clap_plugin) {}
unsafe extern "C" fn plugin_reset(p: *const clap_plugin) {
    let i = inst(p);
    i.delay_l.fill(0.0);
    i.delay_r.fill(0.0);
    i.pos = 0;
    i.held = [0.0; 128];
    i.dc = 0.0;
}
unsafe extern "C" fn plugin_main_thread(_: *const clap_plugin) {}

unsafe fn apply_event(i: &mut Instance, h: *const clap_event_header) {
    if (*h).space_id != CLAP_CORE_EVENT_SPACE_ID {
        return;
    }
    match (*h).type_ {
        CLAP_EVENT_PARAM_VALUE => {
            let e = &*(h as *const clap_event_param_value);
            if e.param_id == PARAM_GAIN && e.value.is_finite() {
                i.gain = e.value.clamp(0.0, 2.0);
            } else if e.param_id == PARAM_EXTRA_LATENCY && e.value.is_finite() {
                let v = e.value.round().clamp(0.0, MAX_EXTRA_LATENCY as f64) as u32;
                if v != i.extra_pending {
                    i.extra_pending = v;
                    // La latence ne peut changer qu'à l'activation : on demande un redémarrage (fonction sûre depuis tout fil).
                    if let Some(restart) = (*i.host).request_restart {
                        restart(i.host);
                    }
                }
            }
        }
        t @ (CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF) => {
            let e = &*(h as *const clap_event_note);
            if (0..128).contains(&e.key) {
                let key = e.key as usize;
                i.dc -= i.held[key] * (key as f32 / 127.0);
                i.held[key] = if t == CLAP_EVENT_NOTE_ON { e.velocity.clamp(0.0, 1.0) as f32 } else { 0.0 };
                i.dc += i.held[key] * (key as f32 / 127.0);
            }
        }
        _ => {}
    }
}

/// Disposition C de l'extension d'hôte `fr.flibuste.chord-track/1` (voir `docs/fr-flibuste-chord-track.h`) : refaite ici à la main, comme le
/// ferait un plugin écrit en C, pour que le test vérifie aussi la disposition en mémoire.
#[repr(C)]
#[derive(Clone, Copy)]
struct FrItem {
    start_beats: f64,
    length_beats: f64,
    root: u8,
    bass: u8,
    flags: u8,
    fifths: i8,
    intervals: u32,
    index: u32,
    name: [u8; 24],
}
#[repr(C)]
struct FrChordTrack {
    revision: Option<unsafe extern "C" fn(*const clap_host) -> u32>,
    count: Option<unsafe extern "C" fn(*const clap_host, u32) -> u32>,
    lookup: Option<unsafe extern "C" fn(*const clap_host, f64, u32, *mut FrItem) -> bool>,
    lookup_all: Option<unsafe extern "C" fn(*const clap_host, f64, u32, *mut FrItem, u32) -> u32>,
    next_after: Option<unsafe extern "C" fn(*const clap_host, f64, u32, *mut FrItem) -> bool>,
    index_at: Option<unsafe extern "C" fn(*const clap_host, f64, u32, *mut u32) -> bool>,
}

unsafe fn read_chords(host: *const clap_host, beat: f64, during_process: bool) {
    let Some(get) = (*host).get_extension else { return };
    let ext = get(host, c"fr.flibuste.chord-track/1".as_ptr()) as *const FrChordTrack;
    if ext.is_null() {
        return seen::store_chords(seen::ChordSeen { extension: false, revision: 0, chord_root: -1, chord_start: 0.0, chord_len: 0.0, chord_mask: 0, chord_bass: 0, chord_name: [0; 24], scales: 0, scale_roots: [0; 4], next_chord_start: -1.0, key_fifths: 0, measure_index: -1, measure_len: 0.0, pattern_slot: -1, chord_count: 0, lookup_when_not_processing: false });
    }
    let e = &*ext;
    let mut it = FrItem { start_beats: 0.0, length_beats: 0.0, root: 0, bass: 0, flags: 0, fifths: 0, intervals: 0, index: 0, name: [0; 24] };
    let found = (e.lookup.unwrap())(host, beat, 0, &mut it);
    if !during_process {
        // Appel hors de `process` : l'hôte doit répondre « rien » (l'instantané n'existe pas pour le plugin à ce moment).
        let mut v = seen::last_chords().unwrap_or(seen::ChordSeen { extension: true, revision: 0, chord_root: -1, chord_start: 0.0, chord_len: 0.0, chord_mask: 0, chord_bass: 0, chord_name: [0; 24], scales: 0, scale_roots: [0; 4], next_chord_start: -1.0, key_fifths: 0, measure_index: -1, measure_len: 0.0, pattern_slot: -1, chord_count: 0, lookup_when_not_processing: false });
        v.lookup_when_not_processing = found;
        return seen::store_chords(v);
    }
    let mut scales = [it; 4];
    let ns = (e.lookup_all.unwrap())(host, beat, 2, scales.as_mut_ptr(), 4);
    let mut next = it;
    let has_next = (e.next_after.unwrap())(host, beat, 0, &mut next);
    let mut key = it;
    let has_key = (e.lookup.unwrap())(host, beat, 1, &mut key);
    let mut m = it;
    let has_m = (e.lookup.unwrap())(host, beat, 4, &mut m);
    let mut pat = it;
    let has_pat = (e.lookup.unwrap())(host, beat, 3, &mut pat);
    let mut sr = [0u8; 4];
    for (k, s) in scales.iter().take(ns as usize).enumerate() {
        sr[k] = s.root;
    }
    seen::store_chords(seen::ChordSeen {
        extension: true,
        revision: (e.revision.unwrap())(host),
        chord_root: if found { i32::from(it.root) } else { -1 },
        chord_start: it.start_beats,
        chord_len: it.length_beats,
        chord_mask: it.intervals,
        chord_bass: it.bass,
        chord_name: it.name,
        scales: ns,
        scale_roots: sr,
        next_chord_start: if has_next { next.start_beats } else { -1.0 },
        key_fifths: if has_key { key.fifths } else { 100 },
        measure_index: if has_m { m.index as i32 } else { -1 },
        measure_len: m.length_beats,
        pattern_slot: if has_pat { i32::from(pat.root) } else { -1 },
        chord_count: (e.count.unwrap())(host, 0),
        lookup_when_not_processing: false,
    });
}

/// À appeler depuis un fil qui ne traite pas : le plugin de test lit la piste d'accords hors de `process` (doit répondre « rien »).
pub fn probe_chords_outside_process() {
    let h = gui::last_host();
    if !h.is_null() {
        unsafe { read_chords(h, 0.0, false) };
    }
}

unsafe extern "C" fn plugin_process(p: *const clap_plugin, process: *const clap_process) -> i32 {
    let i = inst(p);
    let pr = &*process;
    seen::store(pr.transport.as_ref());
    if let Some(t) = pr.transport.as_ref() {
        read_chords(i.host, t.song_pos_beats as f64 / 2147483648.0, true);
    }
    if pr.audio_inputs_count < 1 || pr.audio_outputs_count < 1 || pr.in_events.is_null() {
        return CLAP_PROCESS_ERROR;
    }
    let (inp, out): (&clap_audio_buffer, &clap_audio_buffer) = (&*pr.audio_inputs, &*pr.audio_outputs);
    if inp.channel_count < 2 || out.channel_count < 2 || i.delay_l.is_empty() {
        return CLAP_PROCESS_ERROR;
    }
    let (in_l, in_r) = (*inp.data32, *inp.data32.add(1));
    let (out_l, out_r) = (*out.data32, *out.data32.add(1));
    let events = &*pr.in_events;
    let count = events.size.map_or(0, |f| f(events));
    let get = events.get;
    let mut next = 0u32;
    let next_time = |next: u32| -> u32 {
        match get {
            Some(g) if next < count => (*g(events, next)).time,
            _ => u32::MAX,
        }
    };
    let mut when = next_time(0);
    for n in 0..pr.frames_count {
        while when <= n {
            if let Some(g) = get {
                apply_event(i, g(events, next));
            }
            next += 1;
            when = next_time(next);
        }
        // Lecture de l'entrée avant toute écriture : le traitement en place est permis par CLAP.
        let (xl, xr) = (*in_l.add(n as usize), *in_r.add(n as usize));
        let (dl, dr) = (i.delay_l[i.pos], i.delay_r[i.pos]);
        i.delay_l[i.pos] = xl;
        i.delay_r[i.pos] = xr;
        i.pos = (i.pos + 1) % i.delay_l.len();
        let mut g = i.gain as f32;
        if i.sidechain && pr.audio_inputs_count >= 2 {
            // Chaîne latérale : second port d'entrée (premier canal) ; coupe la sortie quand il dépasse 0,5.
            let sc = &*pr.audio_inputs.add(1);
            if sc.channel_count >= 1 && !sc.data32.is_null() && (*(*sc.data32).add(n as usize)).abs() > 0.5 {
                g = 0.0;
            }
        }
        *out_l.add(n as usize) = dl * g + i.dc;
        *out_r.add(n as usize) = dr * g + i.dc;
    }
    // Événements restants (instants au-delà du bloc : ne devrait pas arriver) appliqués quand même.
    while next < count {
        if let Some(g) = get {
            apply_event(i, g(events, next));
        }
        next += 1;
    }
    // Gestes de l'« utilisateur » dans l'interface factice : annoncés à l'hôte (instant 0 du bloc suivant).
    if let Some(out) = pr.out_events.as_ref() {
        while let Some(a) = gui::next_user_action() {
            let header = |size: usize, type_| clap_event_header { size: size as u32, time: 0, space_id: CLAP_CORE_EVENT_SPACE_ID, type_, flags: 0 };
            match a {
                gui::UserAction::Set(id, value) => {
                    if id == PARAM_GAIN {
                        i.gain = value.clamp(0.0, 2.0);
                    }
                    let e = clap_event_param_value {
                        header: header(std::mem::size_of::<clap_event_param_value>(), CLAP_EVENT_PARAM_VALUE),
                        param_id: id,
                        cookie: std::ptr::null_mut(),
                        note_id: -1,
                        port_index: -1,
                        channel: -1,
                        key: -1,
                        value,
                    };
                    if let Some(push) = out.try_push {
                        push(out, &e.header);
                    }
                }
                gui::UserAction::Begin(id) | gui::UserAction::End(id) => {
                    let begin = matches!(a, gui::UserAction::Begin(_));
                    let e = clap_event_param_gesture {
                        header: header(std::mem::size_of::<clap_event_param_gesture>(), if begin { CLAP_EVENT_PARAM_GESTURE_BEGIN } else { CLAP_EVENT_PARAM_GESTURE_END }),
                        param_id: id,
                    };
                    if let Some(push) = out.try_push {
                        push(out, &e.header);
                    }
                }
            }
        }
    }
    CLAP_PROCESS_CONTINUE
}

// ---- extensions ------------------------------------------------------------------------------

static AUDIO_PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports { count: Some(ports_count), get: Some(ports_get) };
static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_info),
    get_value: Some(params_value),
    value_to_text: Some(params_to_text),
    text_to_value: Some(params_from_text),
    flush: Some(params_flush),
};
static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports { count: Some(note_ports_count), get: Some(note_ports_get) };
static LATENCY_EXT: clap_plugin_latency = clap_plugin_latency { get: Some(latency_get) };
static STATE: clap_plugin_state = clap_plugin_state { save: Some(state_save), load: Some(state_load) };

unsafe extern "C" fn plugin_extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    let id = CStr::from_ptr(id);
    if id == CLAP_EXT_AUDIO_PORTS {
        &AUDIO_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &LATENCY_EXT as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS {
        &NOTE_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE as *const _ as *const c_void
    } else {
        gui::extension(id).unwrap_or(std::ptr::null())
    }
}

fn copy_str<const N: usize>(dst: &mut [c_char; N], s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(N - 1);
    for (d, s) in dst.iter_mut().zip(&b[..n]) {
        *d = *s as c_char;
    }
    dst[n] = 0;
}

unsafe extern "C" fn ports_count(p: *const clap_plugin, is_input: bool) -> u32 {
    if is_input && inst(p).sidechain { 2 } else { 1 }
}
unsafe extern "C" fn ports_get(p: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool {
    let extra = is_input && inst(p).sidechain && index == 1;
    if index != 0 && !extra {
        return false;
    }
    let info = &mut *info;
    info.id = index;
    copy_str(&mut info.name, if extra { "Chaîne latérale" } else { "Principal" });
    info.flags = if extra { 0 } else { CLAP_AUDIO_PORT_IS_MAIN };
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = CLAP_INVALID_ID;
    true
}

unsafe extern "C" fn note_ports_count(_: *const clap_plugin, is_input: bool) -> u32 {
    u32::from(is_input)
}
unsafe extern "C" fn note_ports_get(_: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_note_port_info) -> bool {
    if index != 0 || !is_input {
        return false;
    }
    let info = &mut *info;
    info.id = 0;
    info.supported_dialects = CLAP_NOTE_DIALECT_CLAP;
    info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    copy_str(&mut info.name, "Notes");
    true
}

unsafe extern "C" fn params_count(_: *const clap_plugin) -> u32 {
    2
}
unsafe extern "C" fn params_info(_: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool {
    let info = &mut *info;
    info.cookie = std::ptr::null_mut();
    copy_str(&mut info.module, "");
    match index {
        0 => {
            info.id = PARAM_GAIN;
            info.flags = CLAP_PARAM_IS_AUTOMATABLE;
            copy_str(&mut info.name, "Gain");
            (info.min_value, info.max_value, info.default_value) = (0.0, 2.0, 1.0);
        }
        1 => {
            info.id = PARAM_EXTRA_LATENCY;
            info.flags = CLAP_PARAM_IS_STEPPED;
            copy_str(&mut info.name, "Latence supplementaire");
            (info.min_value, info.max_value, info.default_value) = (0.0, MAX_EXTRA_LATENCY as f64, 0.0);
        }
        _ => return false,
    }
    true
}
unsafe extern "C" fn params_value(p: *const clap_plugin, id: clap_id, out: *mut f64) -> bool {
    match id {
        PARAM_GAIN => *out = inst(p).gain,
        PARAM_EXTRA_LATENCY => *out = inst(p).extra_pending as f64,
        _ => return false,
    }
    true
}
unsafe extern "C" fn params_to_text(_: *const clap_plugin, id: clap_id, value: f64, buf: *mut c_char, cap: u32) -> bool {
    if id != PARAM_GAIN || cap == 0 {
        return false;
    }
    let text = format!("{value:.3}");
    let n = text.len().min(cap as usize - 1);
    std::ptr::copy_nonoverlapping(text.as_ptr() as *const c_char, buf, n);
    *buf.add(n) = 0;
    true
}
unsafe extern "C" fn params_from_text(_: *const clap_plugin, id: clap_id, text: *const c_char, out: *mut f64) -> bool {
    if id != PARAM_GAIN {
        return false;
    }
    match CStr::from_ptr(text).to_str().ok().and_then(|s| s.trim().parse::<f64>().ok()) {
        Some(v) => {
            *out = v;
            true
        }
        None => false,
    }
}
unsafe extern "C" fn params_flush(p: *const clap_plugin, in_events: *const clap_sys::events::clap_input_events, _out: *const clap_sys::events::clap_output_events) {
    let i = inst(p);
    let ev = &*in_events;
    if let (Some(size), Some(get)) = (ev.size, ev.get) {
        for k in 0..size(ev) {
            apply_event(i, get(ev, k));
        }
    }
}

unsafe extern "C" fn latency_get(p: *const clap_plugin) -> u32 {
    LATENCY + inst(p).extra_active
}

unsafe extern "C" fn state_save(p: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let i = inst(p);
    let mut bytes = [0u8; STATE_LEN];
    bytes[..4].copy_from_slice(b"DTP2");
    bytes[4..12].copy_from_slice(&i.gain.to_le_bytes());
    bytes[12..].copy_from_slice(&i.extra_pending.to_le_bytes());
    let Some(write) = (*stream).write else { return false };
    let mut done = 0usize;
    while done < STATE_LEN {
        let n = write(stream, bytes[done..].as_ptr() as *const c_void, (STATE_LEN - done) as u64);
        if n <= 0 {
            return false;
        }
        done += n as usize;
    }
    true
}

unsafe extern "C" fn state_load(p: *const clap_plugin, stream: *const clap_istream) -> bool {
    let mut bytes = [0u8; STATE_LEN];
    let Some(read) = (*stream).read else { return false };
    // Lecture jusqu'à la fin du flux (12 ou 16 octets selon la version).
    let mut done = 0usize;
    while done < STATE_LEN {
        let n = read(stream, bytes[done..].as_mut_ptr() as *mut c_void, (STATE_LEN - done) as u64);
        if n < 0 {
            return false;
        }
        if n == 0 {
            break;
        }
        done += n as usize;
    }
    let (gain, extra) = match (&bytes[..4], done) {
        (b"DTP1", 12) => (f64::from_le_bytes(bytes[4..12].try_into().unwrap()), 0),
        (b"DTP2", 16) => (f64::from_le_bytes(bytes[4..12].try_into().unwrap()), u32::from_le_bytes(bytes[12..16].try_into().unwrap())),
        _ => return false,
    };
    if !gain.is_finite() {
        return false;
    }
    let i = inst(p);
    i.gain = gain.clamp(0.0, 2.0);
    i.extra_pending = extra.min(MAX_EXTRA_LATENCY);
    true
}
