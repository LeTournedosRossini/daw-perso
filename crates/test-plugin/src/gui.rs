//! Interface graphique **factice** du plugin de test (extension CLAP `gui`, intégrée) et minuteur (`timer-support`).
//!
//! Aucune fenêtre n'est dessinée : chaque appel de l'hôte est consigné dans un journal que les tests relisent. Les tests peuvent aussi
//! déclencher ce qu'un vrai plugin ferait de lui-même : demander une taille, demander à se cacher, enregistrer un minuteur.
//! Le journal est global : les tests qui s'en servent doivent s'exécuter l'un après l'autre.

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::sync::Mutex;

use clap_sys::ext::gui::{clap_gui_resize_hints, clap_host_gui, clap_plugin_gui, clap_window, CLAP_EXT_GUI};
use clap_sys::ext::timer_support::{clap_host_timer_support, clap_plugin_timer_support, CLAP_EXT_TIMER_SUPPORT};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;

/// Taille de départ de l'interface factice (largeur, hauteur).
pub const DEFAULT_SIZE: (u32, u32) = (320, 200);

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static WIDTH: AtomicU32 = AtomicU32::new(DEFAULT_SIZE.0);
static HEIGHT: AtomicU32 = AtomicU32::new(DEFAULT_SIZE.1);
static LAST_HOST: AtomicPtr<clap_host> = AtomicPtr::new(std::ptr::null_mut());

pub(crate) fn remember_host(host: *const clap_host) {
    LAST_HOST.store(host as *mut clap_host, Ordering::Release);
}

pub(crate) fn last_host() -> *const clap_host {
    LAST_HOST.load(Ordering::Acquire)
}

fn log(line: impl Into<String>) {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).push(line.into());
}

/// Ce que « l'utilisateur » fait dans l'interface factice : le plugin l'annoncera à l'hôte au prochain bloc traité.
#[derive(Clone, Copy, Debug)]
pub enum UserAction {
    Begin(u32),
    Set(u32, f64),
    End(u32),
}

static USER: Mutex<std::collections::VecDeque<UserAction>> = Mutex::new(std::collections::VecDeque::new());

/// Simule un geste dans l'interface (prise, valeur, relâchement du paramètre `id`).
pub fn user_action(a: UserAction) {
    USER.lock().unwrap_or_else(|e| e.into_inner()).push_back(a);
}

/// Prochaine action en attente ; **ne bloque jamais** (appelée depuis `process`).
pub(crate) fn next_user_action() -> Option<UserAction> {
    USER.try_lock().ok()?.pop_front()
}

/// Le journal des appels reçus de l'hôte depuis le dernier `clear_log`.
pub fn take_log() -> Vec<String> {
    std::mem::take(&mut *LOG.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Copie du journal sans le vider.
pub fn peek_log() -> Vec<String> {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn clear_log() {
    take_log();
    WIDTH.store(DEFAULT_SIZE.0, Ordering::Release);
    HEIGHT.store(DEFAULT_SIZE.1, Ordering::Release);
}

/// Taille courante de l'interface factice.
pub fn size() -> (u32, u32) {
    (WIDTH.load(Ordering::Acquire), HEIGHT.load(Ordering::Acquire))
}

unsafe fn host_gui() -> Option<&'static clap_host_gui> {
    let host = LAST_HOST.load(Ordering::Acquire);
    if host.is_null() {
        return None;
    }
    let get = (*host).get_extension?;
    let p = get(host, CLAP_EXT_GUI.as_ptr()) as *const clap_host_gui;
    (!p.is_null()).then(|| &*p)
}

/// Ce que ferait un plugin qui change sa taille : `gui.request_resize` auprès de l'hôte (et il adopte cette taille).
pub fn request_resize(w: u32, h: u32) -> bool {
    WIDTH.store(w, Ordering::Release);
    HEIGHT.store(h, Ordering::Release);
    // SAFETY: l'hôte du dernier plugin créé est valide tant que ce plugin vit (les tests le garantissent).
    unsafe {
        let host = LAST_HOST.load(Ordering::Acquire);
        host_gui().and_then(|g| g.request_resize).is_some_and(|f| f(host, w, h))
    }
}

/// Ce que ferait un plugin qui se ferme tout seul : `gui.request_hide`.
pub fn request_hide() -> bool {
    // SAFETY: voir `request_resize`.
    unsafe {
        let host = LAST_HOST.load(Ordering::Acquire);
        host_gui().and_then(|g| g.request_hide).is_some_and(|f| f(host))
    }
}

/// Enregistre un minuteur auprès de l'hôte ; rend son identifiant.
pub fn register_timer(period_ms: u32) -> Option<u32> {
    // SAFETY: voir `request_resize`.
    unsafe {
        let host = LAST_HOST.load(Ordering::Acquire);
        if host.is_null() {
            return None;
        }
        let ext = ((*host).get_extension?)(host, CLAP_EXT_TIMER_SUPPORT.as_ptr()) as *const clap_host_timer_support;
        let mut id = 0u32;
        ((*ext).register_timer?)(host, period_ms, &mut id).then_some(id)
    }
}

pub fn unregister_timer(id: u32) -> bool {
    // SAFETY: voir `request_resize`.
    unsafe {
        let host = LAST_HOST.load(Ordering::Acquire);
        let ext = ((*host).get_extension.expect("get_extension"))(host, CLAP_EXT_TIMER_SUPPORT.as_ptr()) as *const clap_host_timer_support;
        ((*ext).unregister_timer.expect("unregister_timer"))(host, id)
    }
}

unsafe extern "C" fn api_supported(_: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    !floating && matches!(CStr::from_ptr(api).to_str(), Ok("win32" | "cocoa" | "x11"))
}
unsafe extern "C" fn preferred_api(_: *const clap_plugin, _: *mut *const c_char, _: *mut bool) -> bool {
    false
}
unsafe extern "C" fn create(_: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    log(format!("create:{}:{}", CStr::from_ptr(api).to_string_lossy(), if floating { "flottante" } else { "intégrée" }));
    !floating
}
unsafe extern "C" fn destroy(_: *const clap_plugin) {
    log("destroy");
}
unsafe extern "C" fn set_scale(_: *const clap_plugin, scale: f64) -> bool {
    log(format!("set_scale:{scale}"));
    true
}
unsafe extern "C" fn get_size(_: *const clap_plugin, w: *mut u32, h: *mut u32) -> bool {
    *w = WIDTH.load(Ordering::Acquire);
    *h = HEIGHT.load(Ordering::Acquire);
    true
}
unsafe extern "C" fn can_resize(_: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn resize_hints(_: *const clap_plugin, _: *mut clap_gui_resize_hints) -> bool {
    false
}
/// Taille acceptée : au moins 100 × 80, largeur et hauteur paires (de quoi vérifier que l'hôte passe par `adjust_size`).
unsafe extern "C" fn adjust_size(_: *const clap_plugin, w: *mut u32, h: *mut u32) -> bool {
    *w = (*w).max(100) & !1;
    *h = (*h).max(80) & !1;
    true
}
unsafe extern "C" fn set_size(_: *const clap_plugin, w: u32, h: u32) -> bool {
    log(format!("set_size:{w}x{h}"));
    WIDTH.store(w, Ordering::Release);
    HEIGHT.store(h, Ordering::Release);
    true
}
unsafe extern "C" fn set_parent(_: *const clap_plugin, window: *const clap_window) -> bool {
    let w = &*window;
    log(format!("set_parent:{}:{}", CStr::from_ptr(w.api).to_string_lossy(), w.specific.ptr as usize));
    true
}
unsafe extern "C" fn set_transient(_: *const clap_plugin, _: *const clap_window) -> bool {
    false
}
unsafe extern "C" fn suggest_title(_: *const clap_plugin, title: *const c_char) {
    log(format!("title:{}", CStr::from_ptr(title).to_string_lossy()));
}
unsafe extern "C" fn show(_: *const clap_plugin) -> bool {
    log("show");
    true
}
unsafe extern "C" fn hide(_: *const clap_plugin) -> bool {
    log("hide");
    true
}

pub(crate) static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(api_supported),
    get_preferred_api: Some(preferred_api),
    create: Some(create),
    destroy: Some(destroy),
    set_scale: Some(set_scale),
    get_size: Some(get_size),
    can_resize: Some(can_resize),
    get_resize_hints: Some(resize_hints),
    adjust_size: Some(adjust_size),
    set_size: Some(set_size),
    set_parent: Some(set_parent),
    set_transient: Some(set_transient),
    suggest_title: Some(suggest_title),
    show: Some(show),
    hide: Some(hide),
};

unsafe extern "C" fn on_timer(_: *const clap_plugin, id: u32) {
    log(format!("timer:{id}"));
}

pub(crate) static TIMER: clap_plugin_timer_support = clap_plugin_timer_support { on_timer: Some(on_timer) };

/// Réponse de `get_extension` pour les extensions de cette interface.
pub(crate) fn extension(id: &CStr) -> Option<*const c_void> {
    if id == CLAP_EXT_GUI {
        Some(&GUI as *const _ as *const c_void)
    } else if id == CLAP_EXT_TIMER_SUPPORT {
        Some(&TIMER as *const _ as *const c_void)
    } else {
        None
    }
}
