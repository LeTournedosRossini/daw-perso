//! Fenêtres Win32 des interfaces de plugins (Windows seulement) : réalisation de `daw_ui::plugin_window` par appels système
//! directs (`user32`, sans dépendance). Tout le `unsafe` de la fenêtre est ici.
//!
//! * Une fenêtre de haut niveau ordinaire (barre de titre, croix, bords redimensionnables si le plugin l'accepte) ; le plugin y crée
//!   sa fenêtre enfant (`gui.set_parent`). `WS_CLIPCHILDREN` : l'hôte ne repeint jamais par-dessus l'interface du plugin.
//! * **Fil** : une fenêtre n'appartient qu'au fil qui l'a créée, qui doit en pomper les messages ([`WindowSystem::pump`]) : c'est le
//!   fil « moteur », celui où vivent déjà les instances de plugins (règle « fil principal » de CLAP).
//! * La croix **ne détruit pas** la fenêtre : elle lève un drapeau que l'hôte lit (`take_close_request`) pour fermer dans le bon
//!   ordre (interface du plugin d'abord, fenêtre ensuite). Un redimensionnement de l'utilisateur est distingué de ceux qu'on
//!   impose (`set_client_size`) par comparaison avec la dernière taille connue.

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
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    fn IsIconic(hwnd: Hwnd) -> i32;
    fn SetWindowPos(hwnd: Hwnd, after: Hwnd, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn AdjustWindowRectExForDpi(rect: *mut Rect, style: u32, menu: i32, ex_style: u32, dpi: u32) -> i32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
    fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
}

const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const WM_NCCREATE: u32 = 0x0081;
const WM_NCDESTROY: u32 = 0x0082;
const WM_CLOSE: u32 = 0x0010;
const WM_SIZE: u32 = 0x0005;
const SIZE_MINIMIZED: usize = 1;
const SW_SHOWNORMAL: i32 = 5;
const SW_RESTORE: i32 = 9;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const PM_REMOVE: u32 = 1;
const GWLP_USERDATA: i32 = -21;
const CW_USEDEFAULT: i32 = i32::MIN;
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

pub struct Win32Window {
    hwnd: Hwnd,
    style: u32,
    state: Box<State>,
}

impl Win32Window {
    fn dpi(&self) -> u32 {
        // SAFETY: `hwnd` est une fenêtre valide créée par nous.
        unsafe { GetDpiForWindow(self.hwnd) }.max(96)
    }

    fn outer(style: u32, client: (u32, u32), dpi: u32) -> (i32, i32) {
        let mut r = Rect { left: 0, top: 0, right: client.0 as i32, bottom: client.1 as i32 };
        // SAFETY: `r` est un `RECT` valide.
        unsafe { AdjustWindowRectExForDpi(&mut r, style, 0, 0, dpi) };
        (r.right - r.left, r.bottom - r.top)
    }
}

impl NativeWindow for Win32Window {
    fn handle(&self) -> (&'static str, usize) {
        ("win32", self.hwnd as usize)
    }

    fn scale(&self) -> f64 {
        self.dpi() as f64 / 96.0
    }

    fn set_client_size(&mut self, width: u32, height: u32) {
        // On note la taille avant de l'imposer : le `WM_SIZE` que `SetWindowPos` provoque aussitôt n'est pas un geste de l'utilisateur.
        self.state.last_client.set((width, height));
        self.state.user_resize.set(None);
        let (ow, oh) = Self::outer(self.style, (width, height), self.dpi());
        // SAFETY: `hwnd` valide ; pas d'ordre Z, ni de déplacement, ni d'activation.
        unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), 0, 0, ow, oh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    fn client_size(&self) -> (u32, u32) {
        let mut r = Rect::default();
        // SAFETY: `hwnd` valide, `r` valide.
        unsafe { GetClientRect(self.hwnd, &mut r) };
        ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32)
    }

    fn show(&mut self) {
        // SAFETY: `hwnd` valide.
        unsafe { ShowWindow(self.hwnd, SW_SHOWNORMAL) };
    }

    fn focus(&mut self) {
        // SAFETY: `hwnd` valide.
        unsafe {
            if IsIconic(self.hwnd) != 0 {
                ShowWindow(self.hwnd, SW_RESTORE);
            }
            SetForegroundWindow(self.hwnd);
        }
    }

    fn take_close_request(&mut self) -> bool {
        self.state.close_requested.replace(false)
    }

    fn take_user_resize(&mut self) -> Option<(u32, u32)> {
        self.state.user_resize.take()
    }

    fn position(&self) -> Option<(i32, i32)> {
        let mut r = Rect::default();
        // SAFETY: `hwnd` valide, `r` valide. Une fenêtre réduite est rangée à (-32000, -32000) : ce n'est pas sa place.
        unsafe {
            if IsIconic(self.hwnd) != 0 || GetWindowRect(self.hwnd, &mut r) == 0 {
                return None;
            }
        }
        Some((r.left, r.top))
    }

    fn set_position(&mut self, x: i32, y: i32) {
        // SAFETY: appels simples à user32 avec des arguments valides.
        let screen = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
        let (x, y) = on_screen(x, y, screen);
        // SAFETY: `hwnd` valide ; ni taille, ni ordre Z, ni activation.
        unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }
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

impl Drop for Win32Window {
    fn drop(&mut self) {
        // SAFETY: `hwnd` valide, détruite sur le fil qui l'a créée (l'hôte ferme ses fenêtres sur ce fil). L'état (`self.state`) survit
        // à cet appel : il n'est libéré qu'ensuite, quand les champs sont détruits.
        unsafe { DestroyWindow(self.hwnd) };
    }
}

#[derive(Default)]
pub struct Win32System {
    registered: bool,
}

impl Win32System {
    pub fn new() -> Self {
        Self::default()
    }

    fn register(&mut self) -> Result<(), String> {
        if self.registered {
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
        self.registered = true;
        Ok(())
    }
}

impl WindowSystem for Win32System {
    fn api(&self) -> &'static str {
        "win32"
    }

    fn create(&mut self, title: &str, client: (u32, u32), resizable: bool) -> Result<Box<dyn NativeWindow>, String> {
        self.register()?;
        let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN | WS_CLIPSIBLINGS | if resizable { WS_THICKFRAME } else { 0 };
        let state = Box::new(State { close_requested: Cell::new(false), user_resize: Cell::new(None), last_client: Cell::new(client) });
        let (ow, oh) = Win32Window::outer(style, client, 96);
        let (class_name, title) = (wide(CLASS_NAME), wide(title));
        // SAFETY: classe enregistrée ; chaînes valides pendant l'appel ; `state` (dans une `Box`, adresse stable) est transmis à
        // `WM_NCCREATE` et survit à la fenêtre (voir `Drop`).
        let hwnd = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            CreateWindowExW(0, class_name.as_ptr(), title.as_ptr(), style, CW_USEDEFAULT, CW_USEDEFAULT, ow, oh, std::ptr::null_mut(), std::ptr::null_mut(), instance, &*state as *const State as *mut c_void)
        };
        if hwnd.is_null() {
            return Err("Windows n'a pas créé la fenêtre".into());
        }
        let mut window = Win32Window { hwnd, style, state };
        window.set_client_size(client.0, client.1); // à l'échelle réelle de l'écran de la fenêtre
        Ok(Box::new(window))
    }

    fn pump(&mut self) {
        let mut msg = Msg { hwnd: std::ptr::null_mut(), message: 0, w_param: 0, l_param: 0, time: 0, pt: Point { x: 0, y: 0 } };
        // SAFETY: `msg` est un `MSG` valide ; on ne traite que les messages du fil courant (celui des fenêtres).
        unsafe {
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
