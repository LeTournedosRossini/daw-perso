//! Fenêtres natives des interfaces de plugins sous Linux (X11) : l'hôte fournit la fenêtre dans laquelle le plugin se loge
//! (CLAP `gui.set_parent` avec l'API `x11` et l'identifiant de la fenêtre X). Fenêtres de haut niveau ordinaires, **sans bande** (comme le
//! premier jalon Windows) : la liste de paramètres reste dans le panneau des plugins de la page.
//!
//! Aucune dépendance : la bibliothèque `libX11.so.6` est chargée à l'exécution (`libloading`) et seules les quelques fonctions utiles sont
//! résolues, ce qui laisse l'application se lancer sans X11 (Wayland pur : pas de fenêtre de plugin, un message clair). Sous Wayland avec
//! XWayland, `DISPLAY` suffit. Tout se passe sur le fil des plugins (celui de `Host::pump_windows`), jamais dans le callback audio.
//!
//! Unités : pixels. Pas d'échelle propre à X11 (1,0) : les plugins lisent la résolution du serveur eux-mêmes. La position de la fenêtre
//! n'est ni lue ni restituée : le gestionnaire de fenêtres décore et décale la fenêtre, un aller-retour dériverait de la hauteur de la barre.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CString};
use std::rc::{Rc, Weak};

use daw_ui::plugin_window::{NativeWindow, WindowSystem};
use libloading::Library;

pub(crate) type Window = c_ulong;
type Atom = c_ulong;
type Display = c_void;

const STRUCTURE_NOTIFY_MASK: c_long = 1 << 17;
const CONFIGURE_NOTIFY: c_int = 22;
const CLIENT_MESSAGE: c_int = 33;
const P_MIN_SIZE: c_long = 1 << 4;
const P_MAX_SIZE: c_long = 1 << 5;

/// `XSizeHints` (ICCCM) : seuls `flags`, `min_*` et `max_*` servent.
#[repr(C)]
#[derive(Default)]
pub(crate) struct SizeHints {
    flags: c_long,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    min_width: c_int,
    min_height: c_int,
    max_width: c_int,
    max_height: c_int,
    width_inc: c_int,
    height_inc: c_int,
    min_aspect: [c_int; 2],
    max_aspect: [c_int; 2],
    base_width: c_int,
    base_height: c_int,
    win_gravity: c_int,
}

/// Les fonctions de Xlib dont on a besoin, résolues une fois.
pub(crate) struct Xlib {
    _lib: Library,
    pub(crate) open_display: unsafe extern "C" fn(*const c_char) -> *mut Display,
    pub(crate) close_display: unsafe extern "C" fn(*mut Display) -> c_int,
    pub(crate) default_screen: unsafe extern "C" fn(*mut Display) -> c_int,
    pub(crate) root_window: unsafe extern "C" fn(*mut Display, c_int) -> Window,
    pub(crate) black_pixel: unsafe extern "C" fn(*mut Display, c_int) -> c_ulong,
    pub(crate) create_simple_window: unsafe extern "C" fn(*mut Display, Window, c_int, c_int, c_uint, c_uint, c_uint, c_ulong, c_ulong) -> Window,
    pub(crate) destroy_window: unsafe extern "C" fn(*mut Display, Window) -> c_int,
    pub(crate) map_raised: unsafe extern "C" fn(*mut Display, Window) -> c_int,
    pub(crate) resize_window: unsafe extern "C" fn(*mut Display, Window, c_uint, c_uint) -> c_int,
    pub(crate) select_input: unsafe extern "C" fn(*mut Display, Window, c_long) -> c_int,
    pub(crate) intern_atom: unsafe extern "C" fn(*mut Display, *const c_char, c_int) -> Atom,
    pub(crate) set_wm_protocols: unsafe extern "C" fn(*mut Display, Window, *mut Atom, c_int) -> c_int,
    pub(crate) change_property: unsafe extern "C" fn(*mut Display, Window, Atom, Atom, c_int, c_int, *const u8, c_int) -> c_int,
    pub(crate) set_wm_normal_hints: unsafe extern "C" fn(*mut Display, Window, *mut SizeHints),
    pub(crate) pending: unsafe extern "C" fn(*mut Display) -> c_int,
    pub(crate) next_event: unsafe extern "C" fn(*mut Display, *mut [c_long; 24]) -> c_int,
    pub(crate) flush: unsafe extern "C" fn(*mut Display) -> c_int,
    pub(crate) map_window: unsafe extern "C" fn(*mut Display, Window) -> c_int,
    pub(crate) reparent_window: unsafe extern "C" fn(*mut Display, Window, Window, c_int, c_int) -> c_int,
    pub(crate) unmap_window: unsafe extern "C" fn(*mut Display, Window) -> c_int,
    pub(crate) move_resize_window: unsafe extern "C" fn(*mut Display, Window, c_int, c_int, c_uint, c_uint) -> c_int,
    pub(crate) get_geometry: unsafe extern "C" fn(*mut Display, Window, *mut Window, *mut c_int, *mut c_int, *mut c_uint, *mut c_uint, *mut c_uint, *mut c_uint) -> c_int,
}

macro_rules! sym {
    ($lib:expr, $name:literal) => {
        // SAFETY: le type de la fonction est celui déclaré par Xlib pour ce symbole (voir `Xlib`).
        *unsafe { $lib.get(concat!($name, "\0").as_bytes()) }.map_err(|e| format!("libX11 : {} introuvable ({e})", $name))?
    };
}

impl Xlib {
    fn load() -> Result<Xlib, String> {
        // SAFETY: chargement d'une bibliothèque système standard ; ses initialiseurs sont ceux de Xlib.
        let lib = unsafe { Library::new("libX11.so.6") }.map_err(|e| format!("libX11 absente ({e}) : pas d'interface de plugin sans X11"))?;
        Ok(Xlib {
            open_display: sym!(lib, "XOpenDisplay"),
            close_display: sym!(lib, "XCloseDisplay"),
            default_screen: sym!(lib, "XDefaultScreen"),
            root_window: sym!(lib, "XRootWindow"),
            black_pixel: sym!(lib, "XBlackPixel"),
            create_simple_window: sym!(lib, "XCreateSimpleWindow"),
            destroy_window: sym!(lib, "XDestroyWindow"),
            map_raised: sym!(lib, "XMapRaised"),
            resize_window: sym!(lib, "XResizeWindow"),
            select_input: sym!(lib, "XSelectInput"),
            intern_atom: sym!(lib, "XInternAtom"),
            set_wm_protocols: sym!(lib, "XSetWMProtocols"),
            change_property: sym!(lib, "XChangeProperty"),
            set_wm_normal_hints: sym!(lib, "XSetWMNormalHints"),
            pending: sym!(lib, "XPending"),
            next_event: sym!(lib, "XNextEvent"),
            flush: sym!(lib, "XFlush"),
            map_window: sym!(lib, "XMapWindow"),
            reparent_window: sym!(lib, "XReparentWindow"),
            unmap_window: sym!(lib, "XUnmapWindow"),
            move_resize_window: sym!(lib, "XMoveResizeWindow"),
            get_geometry: sym!(lib, "XGetGeometry"),
            _lib: lib,
        })
    }
}

/// La connexion au serveur X, partagée par les fenêtres ; fermée quand la dernière disparaît.
pub(crate) struct Conn {
    pub(crate) x: Xlib,
    pub(crate) display: *mut Display,
    pub(crate) root: Window,
    pub(crate) black: c_ulong,
    screen: c_int,
    wm_delete: Atom,
    wm_protocols: Atom,
    net_wm_name: Atom,
    utf8: Atom,
}

impl Conn {
    fn atom(&self, name: &str) -> Atom {
        let c = CString::new(name).unwrap_or_default();
        // SAFETY: `display` ouvert, `c` terminée par 0 ; 0 = créer l'atome s'il n'existe pas.
        unsafe { (self.x.intern_atom)(self.display, c.as_ptr(), 0) }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        // SAFETY: `display` a été ouvert par `XOpenDisplay` et n'est plus utilisé.
        unsafe { (self.x.close_display)(self.display) };
    }
}

/// Ce que la pompe apprend d'une fenêtre (écrit par `pump`, lu par la fenêtre).
struct WinState {
    id: Window,
    size: (u32, u32),
    /// Taille qu'on vient d'imposer : la notification de configuration qui la confirme n'est pas un geste de l'utilisateur.
    expected: Option<(u32, u32)>,
    closing: bool,
    user_resize: Option<(u32, u32)>,
}

pub struct X11Window {
    conn: Rc<Conn>,
    state: Rc<RefCell<WinState>>,
    resizable: bool,
}

impl X11Window {
    fn hints(&self, w: u32, h: u32) {
        let mut hints = SizeHints::default();
        if !self.resizable {
            hints.flags = P_MIN_SIZE | P_MAX_SIZE;
            hints.min_width = w as c_int;
            hints.max_width = w as c_int;
            hints.min_height = h as c_int;
            hints.max_height = h as c_int;
        }
        // SAFETY: `display` ouvert, fenêtre existante, `hints` valide (disposition de `XSizeHints`).
        unsafe { (self.conn.x.set_wm_normal_hints)(self.conn.display, self.state.borrow().id, &mut hints) };
    }
}

impl NativeWindow for X11Window {
    fn handle(&self) -> (&'static str, usize) {
        ("x11", self.state.borrow().id as usize)
    }
    fn scale(&self) -> f64 {
        1.0
    }
    fn set_client_size(&mut self, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        {
            let mut s = self.state.borrow_mut();
            if s.size == (w, h) {
                return;
            }
            s.expected = Some((w, h));
            s.size = (w, h);
        }
        self.hints(w, h);
        // SAFETY: `display` ouvert, fenêtre existante.
        unsafe {
            (self.conn.x.resize_window)(self.conn.display, self.state.borrow().id, w, h);
            (self.conn.x.flush)(self.conn.display);
        }
    }
    fn client_size(&self) -> (u32, u32) {
        self.state.borrow().size
    }
    fn show(&mut self) {
        // SAFETY: `display` ouvert, fenêtre existante.
        unsafe {
            (self.conn.x.map_raised)(self.conn.display, self.state.borrow().id);
            (self.conn.x.flush)(self.conn.display);
        }
    }
    fn focus(&mut self) {
        self.show();
    }
    fn take_close_request(&mut self) -> bool {
        std::mem::take(&mut self.state.borrow_mut().closing)
    }
    fn take_user_resize(&mut self) -> Option<(u32, u32)> {
        self.state.borrow_mut().user_resize.take()
    }
    fn set_title(&mut self, title: &str) {
        let id = self.state.borrow().id;
        let c = &self.conn;
        // SAFETY: `display` ouvert, fenêtre existante ; les octets restent valides pendant l'appel (format 8 bits, `len` octets).
        unsafe {
            (c.x.change_property)(c.display, id, c.net_wm_name, c.utf8, 8, 0, title.as_ptr(), title.len() as c_int);
            (c.x.flush)(c.display);
        }
    }
}

impl Drop for X11Window {
    fn drop(&mut self) {
        let id = self.state.borrow().id;
        // SAFETY: `display` ouvert ; la fenêtre est à nous. L'interface du plugin a déjà été détruite par l'hôte (ordre imposé par CLAP).
        unsafe {
            (self.conn.x.destroy_window)(self.conn.display, id);
            (self.conn.x.flush)(self.conn.display);
        }
    }
}

/// Fabrique les fenêtres X11 et pompe leurs événements.
#[derive(Default)]
pub struct X11System {
    conn: Option<Rc<Conn>>,
    windows: Vec<Weak<RefCell<WinState>>>,
}

impl X11System {
    pub fn new() -> Self {
        X11System::default()
    }

    fn connect(&mut self) -> Result<Rc<Conn>, String> {
        if let Some(c) = &self.conn {
            return Ok(Rc::clone(c));
        }
        let c = open_conn()?;
        self.conn = Some(Rc::clone(&c));
        Ok(c)
    }
}

/// Ouvre une connexion au serveur X (`DISPLAY`) : partagée par les fenêtres d'un même système.
pub(crate) fn open_conn() -> Result<Rc<Conn>, String> {
    let x = Xlib::load()?;
    // SAFETY: `XOpenDisplay(NULL)` lit `DISPLAY` ; rend NULL en cas d'échec.
    let display = unsafe { (x.open_display)(std::ptr::null()) };
    if display.is_null() {
        return Err("aucun serveur X joignable (variable DISPLAY) : pas d'interface de plugin (Wayland pur ?)".into());
    }
    // SAFETY: `display` vient d'être ouvert.
    let (screen, root, black) = unsafe {
        let screen = (x.default_screen)(display);
        (screen, (x.root_window)(display, screen), (x.black_pixel)(display, screen))
    };
    let mut c = Conn { x, display, root, black, screen, wm_delete: 0, wm_protocols: 0, net_wm_name: 0, utf8: 0 };
    c.wm_delete = c.atom("WM_DELETE_WINDOW");
    c.wm_protocols = c.atom("WM_PROTOCOLS");
    c.net_wm_name = c.atom("_NET_WM_NAME");
    c.utf8 = c.atom("UTF8_STRING");
    let _ = c.screen;
    Ok(Rc::new(c))
}

impl WindowSystem for X11System {
    fn api(&self) -> &'static str {
        "x11"
    }

    fn create(&mut self, title: &str, client: (u32, u32), resizable: bool) -> Result<Box<dyn NativeWindow>, String> {
        let conn = self.connect()?;
        let (w, h) = (client.0.max(1), client.1.max(1));
        // SAFETY: `display` ouvert ; fenêtre de haut niveau (parent : la racine), fond noir, sans bordure.
        let id = unsafe {
            let id = (conn.x.create_simple_window)(conn.display, conn.root, 120, 120, w, h, 0, 0, conn.black);
            (conn.x.select_input)(conn.display, id, STRUCTURE_NOTIFY_MASK);
            let mut protocols = [conn.wm_delete];
            (conn.x.set_wm_protocols)(conn.display, id, protocols.as_mut_ptr(), 1);
            id
        };
        let state = Rc::new(RefCell::new(WinState { id, size: (w, h), expected: None, closing: false, user_resize: None }));
        self.windows.push(Rc::downgrade(&state));
        let mut win = X11Window { conn, state, resizable };
        win.hints(w, h);
        win.set_title(title);
        Ok(Box::new(win))
    }

    fn pump(&mut self) {
        let Some(conn) = &self.conn else { return };
        self.windows.retain(|w| w.strong_count() > 0);
        loop {
            // SAFETY: `display` ouvert ; `ev` est la structure `XEvent` (union de 24 entiers longs : 192 octets), remplie par `XNextEvent`.
            let mut ev = [0 as c_long; 24];
            unsafe {
                if (conn.x.pending)(conn.display) <= 0 {
                    break;
                }
                (conn.x.next_event)(conn.display, &mut ev);
            }
            // Disposition de `XEvent` sur 64 bits : `type` (int) à 0 ; `XConfigureEvent` : fenêtre à 40, largeur à 56, hauteur à 60 ;
            // `XClientMessageEvent` : fenêtre à 32, `data.l[0]` à 56. Lus en octets pour ne pas dépendre de l'alignement.
            let bytes: &[u8] = unsafe { std::slice::from_raw_parts(ev.as_ptr() as *const u8, std::mem::size_of_val(&ev)) };
            let int_at = |o: usize| i32::from_ne_bytes(bytes[o..o + 4].try_into().unwrap_or([0; 4]));
            let long_at = |o: usize| i64::from_ne_bytes(bytes[o..o + 8].try_into().unwrap_or([0; 8]));
            match int_at(0) {
                CLIENT_MESSAGE => {
                    let (window, data0) = (long_at(32) as Window, long_at(56) as Atom);
                    if data0 == conn.wm_delete {
                        if let Some(s) = self.windows.iter().filter_map(|w| w.upgrade()).find(|s| s.borrow().id == window) {
                            s.borrow_mut().closing = true;
                        }
                    }
                }
                CONFIGURE_NOTIFY => {
                    let window = long_at(40) as Window;
                    let (w, h) = (int_at(56).max(1) as u32, int_at(60).max(1) as u32);
                    if let Some(s) = self.windows.iter().filter_map(|w| w.upgrade()).find(|s| s.borrow().id == window) {
                        let mut s = s.borrow_mut();
                        if s.expected == Some((w, h)) {
                            s.expected = None;
                        } else if s.size != (w, h) {
                            s.size = (w, h);
                            s.user_resize = Some((w, h));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
