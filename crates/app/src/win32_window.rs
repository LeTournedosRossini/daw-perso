//! Fenêtres Win32 des interfaces de plugins (Windows seulement) : réalisation de `daw_ui::plugin_window` par appels système
//! directs (`user32`, sans dépendance) et par l'API de Tauri. Tout le `unsafe` des fenêtres est ici.
//!
//! * Une fenêtre de plugin est une fenêtre Tauri (la page `plugin.html` : bande + paramètres) surmontée, sur sa partie basse, d'une
//!   zone native sans cadre où le plugin crée sa fenêtre enfant (`gui.set_parent`) : voir la deuxième moitié du fichier.
//! * **Fil** : une fenêtre n'appartient qu'au fil qui l'a créée, qui doit en pomper les messages ([`WindowSystem::pump`]) : c'est le
//!   fil « moteur », celui où vivent déjà les instances de plugins (règle « fil principal » de CLAP).
//! * La croix **ne détruit pas** la fenêtre : elle lève un drapeau que l'hôte lit (`take_close_request`) pour fermer dans le bon
//!   ordre (interface du plugin d'abord, fenêtre ensuite).

use std::cell::Cell;
use std::ffi::c_void;

use daw_ui::plugin_window::{NativeWindow, WindowSystem};

type Hwnd = *mut c_void;
type Handle = *mut c_void;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    wnd_proc: Option<WndProc>,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: Handle,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt: Point,
}

#[repr(C)]
struct CreateStructW {
    create_params: *mut c_void,
    instance: Handle,
    menu: Handle,
    parent: Hwnd,
    cy: i32,
    cx: i32,
    y: i32,
    x: i32,
    style: i32,
    name: *const u16,
    class: *const u16,
    ex_style: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Handle;
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(ex_style: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: Hwnd, menu: Handle, instance: Handle, param: *mut c_void) -> Hwnd;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn IsIconic(hwnd: Hwnd) -> i32;
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn ClientToScreen(hwnd: Hwnd, point: *mut Point) -> i32;
    fn IsWindow(hwnd: Hwnd) -> i32;
}

const WS_POPUP: u32 = 0x8000_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const WM_NCCREATE: u32 = 0x0081;
const WM_NCDESTROY: u32 = 0x0082;
const WM_CLOSE: u32 = 0x0010;
const WM_SIZE: u32 = 0x0005;
const SIZE_MINIMIZED: usize = 1;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const PM_REMOVE: u32 = 1;
const GWLP_USERDATA: i32 = -21;
const IDC_ARROW: usize = 32512;
const COLOR_WINDOW: usize = 5;
const CLASS_NAME: &str = "DAWPluginWindow";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// État partagé entre la fenêtre et sa procédure de fenêtre : tout se passe sur le fil de la fenêtre, d'où les `Cell`.
struct State {
    close_requested: Cell<bool>,
    user_resize: Cell<Option<(u32, u32)>>,
    /// Dernière taille de la zone utile connue (imposée par nous ou constatée) : `WM_SIZE` identique = pas un geste de l'utilisateur.
    last_client: Cell<(u32, u32)>,
}

unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
    match msg {
        WM_NCCREATE => {
            // SAFETY: `l` pointe vers la `CREATESTRUCTW` de ce message ; `create_params` est le `State` passé à `CreateWindowExW`.
            let cs = &*(l as *const CreateStructW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.create_params as isize);
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
        WM_CLOSE | WM_SIZE => {
            // SAFETY: la donnée utilisateur est soit nulle, soit le `State` de la fenêtre, vivant jusqu'à `DestroyWindow`.
            let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State;
            if let Some(state) = state.as_ref() {
                if msg == WM_CLOSE {
                    state.close_requested.set(true);
                    return 0; // on ne détruit pas : l'hôte ferme dans le bon ordre
                }
                let (width, height) = ((l & 0xFFFF) as u32, ((l >> 16) & 0xFFFF) as u32);
                if w != SIZE_MINIMIZED && width > 0 && height > 0 && (width, height) != state.last_client.get() {
                    state.last_client.set((width, height));
                    state.user_resize.set(Some((width, height)));
                }
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, w, l)
}

const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;

/// Ramène un coin haut-gauche à l'intérieur de l'écran virtuel `(x, y, largeur, hauteur)` : au moins 120 × 60 pixels de la fenêtre
/// (sa barre de titre) restent visibles, pour qu'elle ne soit jamais perdue (écran débranché, résolution changée).
fn on_screen(x: i32, y: i32, (sx, sy, sw, sh): (i32, i32, i32, i32)) -> (i32, i32) {
    if sw <= 0 || sh <= 0 {
        return (x, y); // dimensions inconnues : on ne corrige rien
    }
    (x.clamp(sx - 60, sx + sw - 120), y.clamp(sy, sy + sh - 60))
}

#[cfg(test)]
mod tests {
    use super::on_screen;

    #[test]
    fn a_window_outside_every_screen_comes_back_and_one_inside_stays() {
        let screen = (0, 0, 1920, 1080);
        assert_eq!(on_screen(300, 200, screen), (300, 200));
        assert_eq!(on_screen(5000, 200, screen), (1800, 200));
        assert_eq!(on_screen(-3000, -50, screen), (-60, 0));
        assert_eq!(on_screen(100, 9000, screen), (100, 1020));
        // Deux écrans, le second à gauche de l'écran principal : les coordonnées négatives sont valables.
        assert_eq!(on_screen(-1500, 100, (-1920, 0, 3840, 1080)), (-1500, 100));
        assert_eq!(on_screen(7, 7, (0, 0, 0, 0)), (7, 7));
    }
}

static CLASS_REGISTERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Enregistre la classe de fenêtre (une seule fois pour tout le processus).
fn ensure_class() -> Result<(), String> {
    {
        if CLASS_REGISTERED.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
        let class_name = wide(CLASS_NAME);
        // SAFETY: tous les pointeurs passés sont valides pendant l'appel ; `wnd_proc` est une fonction `extern "system"` statique.
        let atom = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = WndClassExW {
                size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                wnd_proc: Some(wnd_proc),
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon: std::ptr::null_mut(),
                cursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
                background: (COLOR_WINDOW + 1) as Handle,
                menu_name: std::ptr::null(),
                class_name: class_name.as_ptr(),
                icon_small: std::ptr::null_mut(),
            };
            RegisterClassExW(&class)
        };
        if atom == 0 {
            return Err("classe de fenêtre refusée par Windows".into());
        }
        CLASS_REGISTERED.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// Fenêtres de plugin « à bande » : une fenêtre Tauri (la page `plugin.html` : bande + liste de paramètres) et, par-dessus sa partie
// basse, une **zone native** (fenêtre Win32 sans cadre, dont la fenêtre Tauri est la propriétaire) où le plugin loge son interface.
// La zone suit la fenêtre (position, taille) à chaque tour de la pompe ; elle est cachée quand la bande montre les paramètres.
//
// * **Fils** : la fenêtre Tauri vit sur le fil principal de l'application ; la zone native et l'interface du plugin vivent sur le fil
//   moteur (règle « fil principal » de CLAP : celui des instances). Une fenêtre *propriétaire* d'un autre fil est permise : seul l'ordre Z
//   est partagé (la zone reste au-dessus de sa propriétaire, se réduit avec elle). Les appels qui modifient la fenêtre Tauri passent par
//   l'API de Tauri (envoyés au fil principal), jamais par `SetWindowPos` d'un autre fil.
// * **Géométrie** : la bande occupe les `TOP_CSS` premiers pixels CSS de la page (constante partagée avec la feuille de style) ; la zone
//   native est le reste de la zone utile de la fenêtre.
// ---------------------------------------------------------------------------------------------------------------------------

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use tauri::{AppHandle, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

static APP: OnceLock<AppHandle> = OnceLock::new();
static LABEL: AtomicU32 = AtomicU32::new(0);

/// À appeler une fois au démarrage, avant tout ouvrir de fenêtre de plugin.
pub fn set_app(app: AppHandle) {
    let _ = APP.set(app);
}

/// Hauteur de la bande en pixels CSS (`.plugin-window .pv-strip` dans `css/app.css` : 6 de marge + 34 de bande + 12 de marge basse,
/// pour que la zone native ne morde jamais sur la bande, même avec l'arrondi des échelles d'affichage fractionnaires).
const TOP_CSS: f64 = 52.0;
/// Largeur de la liste des plugins de la piste, à gauche, en pixels CSS (`.pw-list` dans `css/app.css`).
const LIST_CSS: f64 = 168.0;
/// Après un redimensionnement imposé, on laisse ce délai à Tauri pour l'appliquer avant de reprendre les gestes de l'utilisateur.
const IMPOSED_GRACE: Duration = Duration::from_millis(600);

/// La zone native : fenêtre Win32 sans cadre, propriétaire = la fenêtre Tauri.
struct Popup {
    hwnd: Hwnd,
    _state: Box<State>,
}

impl Drop for Popup {
    fn drop(&mut self) {
        // SAFETY: `hwnd` valide, créée sur ce fil, qui est aussi celui qui la détruit.
        unsafe { DestroyWindow(self.hwnd) };
    }
}

struct Inner {
    // L'ordre compte : la zone native est détruite avant sa propriétaire (voir `Drop`).
    popup: Option<Popup>,
    frame: WebviewWindow,
    frame_hwnd: Hwnd,
    area_visible: bool,
    shown: bool,
    close: Arc<AtomicBool>,
    /// Dernière taille de la zone utile de la fenêtre Tauri connue (imposée par nous ou constatée).
    last_client: (u32, u32),
    /// Taille imposée en attente d'application par Tauri : tant qu'elle n'est pas atteinte (ou le délai écoulé), on ne la prend pas
    /// pour un geste de l'utilisateur.
    pending: Option<((u32, u32), Instant)>,
    user_resize: Option<(u32, u32)>,
    placed: Option<(i32, i32, u32, u32)>,
    /// La géométrie a été consignée dans le journal (une fois, à l'affichage) : de quoi comprendre un décalage constaté à l'écran.
    logged: bool,
}

impl Inner {
    fn dpi(&self) -> u32 {
        // SAFETY: `frame_hwnd` est la fenêtre de l'application ; la lecture du DPI est sans effet.
        unsafe { GetDpiForWindow(self.frame_hwnd) }.max(96)
    }

    fn strip_px(&self) -> u32 {
        (TOP_CSS * self.dpi() as f64 / 96.0).round() as u32
    }

    fn list_px(&self) -> u32 {
        (LIST_CSS * self.dpi() as f64 / 96.0).round() as u32
    }

    fn client(&self) -> (u32, u32) {
        let mut r = Rect::default();
        // SAFETY: `frame_hwnd` valide ; lecture seule.
        unsafe { GetClientRect(self.frame_hwnd, &mut r) };
        ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32)
    }

    /// Un tour de pompe : détecte un redimensionnement de l'utilisateur et replace la zone native sous la bande.
    fn sync(&mut self) {
        // SAFETY: lecture d'état d'une fenêtre qui peut avoir disparu : `IsWindow` répond sans risque.
        if unsafe { IsWindow(self.frame_hwnd) } == 0 || unsafe { IsIconic(self.frame_hwnd) } != 0 {
            return;
        }
        let size = self.client();
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        let strip = self.strip_px();
        let list = self.list_px();
        match self.pending {
            Some((target, since)) if size == target || since.elapsed() > IMPOSED_GRACE => {
                self.pending = None;
                self.last_client = size;
            }
            Some(_) => {}
            None if size != self.last_client => {
                self.last_client = size;
                self.user_resize = Some((size.0.saturating_sub(list).max(1), size.1.saturating_sub(strip).max(1)));
            }
            None => {}
        }
        let Some(popup) = self.popup.as_ref() else { return };
        let (w, h) = (size.0.saturating_sub(list).max(1), size.1.saturating_sub(strip).max(1));
        let mut origin = Point { x: list as i32, y: strip as i32 };
        // SAFETY: `frame_hwnd` valide ; `origin` est un `POINT` valide.
        unsafe { ClientToScreen(self.frame_hwnd, &mut origin) };
        let rect = (origin.x, origin.y, w, h);
        if !self.logged && self.shown {
            self.logged = true;
            crate::log(&format!("fenêtre de plugin : échelle {} ppp, bande {strip} px, liste {list} px, zone utile {}x{}, zone native à ({}, {}) de {w}x{h}", self.dpi(), size.0, size.1, rect.0, rect.1));
        }
        if self.placed != Some(rect) {
            self.placed = Some(rect);
            // SAFETY: `popup.hwnd` créée par nous sur ce fil ; ni activation ni ordre Z (la propriétaire s'en charge).
            unsafe { SetWindowPos(popup.hwnd, std::ptr::null_mut(), rect.0, rect.1, w as i32, h as i32, SWP_NOZORDER | SWP_NOACTIVATE) };
        }
    }

    fn apply_area_visibility(&self) {
        if let Some(p) = self.popup.as_ref() {
            let show = self.shown && self.area_visible;
            // SAFETY: `p.hwnd` valide, ce fil ; on ne prend pas le focus de la fenêtre de la bande.
            unsafe { ShowWindow(p.hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.popup.take(); // la zone native d'abord : sa propriétaire ne doit pas la détruire depuis un autre fil
        let _ = self.frame.destroy();
    }
}

struct DockedWindow {
    inner: Rc<RefCell<Inner>>,
}

impl NativeWindow for DockedWindow {
    fn handle(&self) -> (&'static str, usize) {
        ("win32", self.inner.borrow().popup.as_ref().map_or(0, |p| p.hwnd as usize))
    }

    fn scale(&self) -> f64 {
        self.inner.borrow().dpi() as f64 / 96.0
    }

    fn set_client_size(&mut self, width: u32, height: u32) {
        let mut i = self.inner.borrow_mut();
        let target = (width + i.list_px(), height + i.strip_px());
        i.pending = Some((target, Instant::now()));
        i.user_resize = None;
        let _ = i.frame.set_size(PhysicalSize::new(target.0, target.1));
    }

    fn client_size(&self) -> (u32, u32) {
        let i = self.inner.borrow();
        let c = i.client();
        (c.0.saturating_sub(i.list_px()), c.1.saturating_sub(i.strip_px()))
    }

    fn show(&mut self) {
        let mut i = self.inner.borrow_mut();
        i.shown = true;
        i.sync();
        let _ = i.frame.show();
        i.apply_area_visibility();
    }

    fn focus(&mut self) {
        let i = self.inner.borrow();
        let _ = i.frame.unminimize();
        let _ = i.frame.set_focus();
    }

    fn take_close_request(&mut self) -> bool {
        self.inner.borrow().close.swap(false, Ordering::Relaxed)
    }

    fn take_user_resize(&mut self) -> Option<(u32, u32)> {
        self.inner.borrow_mut().user_resize.take()
    }

    fn position(&self) -> Option<(i32, i32)> {
        let i = self.inner.borrow();
        let mut r = Rect::default();
        // SAFETY: lecture d'état ; une fenêtre réduite est rangée à (-32000, -32000) : ce n'est pas sa place.
        unsafe {
            if IsIconic(i.frame_hwnd) != 0 || GetWindowRect(i.frame_hwnd, &mut r) == 0 {
                return None;
            }
        }
        Some((r.left, r.top))
    }

    fn set_position(&mut self, x: i32, y: i32) {
        // SAFETY: appels simples à user32.
        let screen = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
        let (x, y) = on_screen(x, y, screen);
        let _ = self.inner.borrow().frame.set_position(PhysicalPosition::new(x, y));
    }

    fn set_title(&mut self, title: &str) {
        let _ = self.inner.borrow().frame.set_title(title);
    }

    fn set_area_visible(&mut self, visible: bool) {
        let mut i = self.inner.borrow_mut();
        i.area_visible = visible;
        i.apply_area_visibility();
    }
}

#[derive(Default)]
pub struct DockedSystem {
    windows: Vec<Weak<RefCell<Inner>>>,
}

impl DockedSystem {
    pub fn new() -> Self {
        Self::default()
    }
}

impl WindowSystem for DockedSystem {
    fn api(&self) -> &'static str {
        "win32"
    }

    fn docked(&self) -> bool {
        true
    }

    fn create(&mut self, _title: &str, _client: (u32, u32), _resizable: bool) -> Result<Box<dyn NativeWindow>, String> {
        Err("les fenêtres de plugin se créent par plugin (create_for)".into())
    }

    fn create_for(&mut self, insert: u32, title: &str, client: (u32, u32), resizable: bool, native: bool) -> Result<Box<dyn NativeWindow>, String> {
        let app = APP.get().ok_or("l'application n'est pas prête")?.clone();
        let label = format!("plugin-{insert}-{}", LABEL.fetch_add(1, Ordering::Relaxed));
        let url = format!("plugin.html?insert={insert}");
        let (tx, rx) = mpsc::channel();
        let (app2, title2) = (app.clone(), title.to_owned());
        // La fenêtre Tauri se crée sur le fil principal de l'application ; ce fil-ci attend (le fil principal ne nous attend jamais).
        app.run_on_main_thread(move || {
            let built = WebviewWindowBuilder::new(&app2, label, WebviewUrl::App(url.into()))
                .title(title2)
                .inner_size(client.0 as f64 + LIST_CSS, client.1 as f64 + TOP_CSS)
                .min_inner_size(300.0 + LIST_CSS, 200.0)
                .resizable(resizable)
                .visible(false)
                .build();
            let _ = tx.send(built.map_err(|e| e.to_string()));
        })
        .map_err(|e| e.to_string())?;
        let frame = rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "la fenêtre du plugin n'a pas été créée à temps".to_string())??;
        let hwnd = frame.hwnd().map_err(|e| e.to_string())?;
        let frame_hwnd = hwnd.0 as usize as Hwnd;
        // La croix ne détruit pas la fenêtre : elle lève un drapeau que l'hôte lit (interface du plugin détruite avant la fenêtre).
        let close = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&close);
        frame.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                flag.store(true, Ordering::Relaxed);
            }
        });
        // La zone native existe toujours (cachée si le plugin n'a pas d'interface) : la fenêtre peut passer à un plugin qui en a une.
        let popup = {
            ensure_class()?;
            let state = Box::new(State { close_requested: Cell::new(false), user_resize: Cell::new(None), last_client: Cell::new(client) });
            let (class_name, empty) = (wide(CLASS_NAME), wide(""));
            // SAFETY: classe enregistrée ; chaînes valides pendant l'appel ; `frame_hwnd` est la propriétaire (fenêtre de l'application) ;
            // `state` (dans une `Box`, adresse stable) est transmis à `WM_NCCREATE` et survit à la fenêtre (voir `Popup`).
            let popup_hwnd = unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                CreateWindowExW(WS_EX_TOOLWINDOW, class_name.as_ptr(), empty.as_ptr(), WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS, 0, 0, client.0 as i32, client.1 as i32, frame_hwnd, std::ptr::null_mut(), instance, &*state as *const State as *mut c_void)
            };
            if popup_hwnd.is_null() {
                let _ = frame.destroy();
                return Err("Windows n'a pas créé la zone de l'interface".into());
            }
            Some(Popup { hwnd: popup_hwnd, _state: state })
        };
        let inner = Rc::new(RefCell::new(Inner { popup, frame, frame_hwnd, area_visible: native, shown: false, close, last_client: (0, 0), pending: None, user_resize: None, placed: None, logged: false }));
        self.windows.push(Rc::downgrade(&inner));
        let mut win = DockedWindow { inner };
        win.set_client_size(client.0, client.1); // à l'échelle réelle de l'écran de la fenêtre
        Ok(Box::new(win))
    }

    fn pump(&mut self) {
        let mut msg = Msg { hwnd: std::ptr::null_mut(), message: 0, w_param: 0, l_param: 0, time: 0, pt: Point { x: 0, y: 0 } };
        // SAFETY: `msg` est un `MSG` valide ; on ne traite que les messages du fil courant (celui des zones natives).
        unsafe {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        self.windows.retain(|w| w.strong_count() > 0);
        for w in &self.windows {
            if let Some(inner) = w.upgrade() {
                inner.borrow_mut().sync();
            }
        }
    }
}
