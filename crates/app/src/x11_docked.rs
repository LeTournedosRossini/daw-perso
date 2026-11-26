//! Fenêtres de plugin à bande sous Linux (X11) : comme sous Windows (`win32_window.rs`), une vraie fenêtre Tauri (page `plugin.html` :
//! bande, liste des plugins de la piste, bascule Interface / Paramètres) et, en dessous à droite, une **zone native** : une fenêtre X11
//! *fille* de la fenêtre Tauri où le plugin loge son interface (CLAP `gui.set_parent`, API `x11`).
//!
//! * **Fils** : la fenêtre Tauri se crée sur le fil principal (comme sous Windows) ; la zone native et l'interface du plugin vivent sur le fil
//!   des plugins, avec leur propre connexion X. Les modifications de la fenêtre Tauri passent par l'API de Tauri.
//! * **Géométrie** : la zone est le coin bas-droit de la fenêtre, à partir de l'origine **mesurée par la page** (`set_plugin_area`), sinon
//!   supposée (`LIST_CSS`, `TOP_CSS`). Une fille de la fenêtre suit ses déplacements sans qu'on s'en occupe ; seules la taille et l'origine
//!   sont recalculées à chaque tour de pompe.
//! * **Prérequis** : la fenêtre Tauri doit être une fenêtre X11 (GTK en mode X11) ; sous Wayland, `main` impose `GDK_BACKEND=x11`
//!   (XWayland) quand `DISPLAY` existe. Sinon la création échoue avec un message, et la page retombe sur le panneau.

use std::cell::RefCell;
use std::ffi::{c_uint, c_ulong};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use daw_ui::plugin_window::{NativeWindow, WindowSystem};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tauri::{AppHandle, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::x11_window::{open_conn, Conn, Window};

static APP: OnceLock<AppHandle> = OnceLock::new();
static LABEL: AtomicU32 = AtomicU32::new(0);

/// À appeler une fois au démarrage, avant d'ouvrir une fenêtre de plugin.
pub fn set_app(app: AppHandle) {
    let _ = APP.set(app);
}

/// Hauteur de la bande en pixels CSS (`.plugin-window .pv-strip` dans `css/app.css`) et largeur de la liste (`.pw-list`) : mêmes valeurs
/// que sous Windows et macOS.
const TOP_CSS: f64 = 52.0;
const LIST_CSS: f64 = 168.0;
/// Après un redimensionnement imposé, délai laissé à Tauri pour l'appliquer avant de reprendre les gestes de l'utilisateur.
const IMPOSED_GRACE: Duration = Duration::from_millis(600);

struct Inner {
    conn: Rc<Conn>,
    /// Zone native : fille de la fenêtre Tauri (une fois celle-ci affichée).
    area: Window,
    /// Fenêtre X de la fenêtre Tauri : n'existe qu'une fois la fenêtre affichée (GTK ne crée la fenêtre X qu'à l'affichage) ; la zone
    /// native, créée avant, attend en fenêtre de haut niveau jamais affichée puis y est rattachée (`XReparentWindow`).
    frame_xid: Option<Window>,
    frame: WebviewWindow,
    area_visible: bool,
    shown: bool,
    close: Arc<AtomicBool>,
    last_client: (u32, u32),
    pending: Option<((u32, u32), Instant)>,
    user_resize: Option<(u32, u32)>,
    placed: Option<(i32, i32, u32, u32)>,
    slot_origin: Option<(i32, i32)>,
    want_area: Option<(u32, u32)>,
    mapped: bool,
    shown_at: Option<Instant>,
    logged: bool,
}

impl Inner {
    /// Taille de la fenêtre Tauri (sa zone utile : la fille est repérée dans ce repère).
    fn frame_size(&self) -> (u32, u32) {
        let Some(frame_xid) = self.frame_xid else { return (0, 0) };
        let (mut root, mut x, mut y, mut w, mut h, mut b, mut d): (Window, i32, i32, c_uint, c_uint, c_uint, c_uint) = (0, 0, 0, 0, 0, 0, 0);
        // SAFETY: `display` ouvert ; les pointeurs visent des variables locales ; une fenêtre disparue rend 0 (taille nulle, ignorée).
        let ok = unsafe { (self.conn.x.get_geometry)(self.conn.display, frame_xid, &mut root, &mut x, &mut y, &mut w, &mut h, &mut b, &mut d) };
        if ok == 0 {
            (0, 0)
        } else {
            (w, h)
        }
    }

    fn origin(&self) -> (u32, u32) {
        match self.slot_origin {
            Some((x, y)) => (x.max(0) as u32, y.max(0) as u32),
            None => (LIST_CSS as u32, TOP_CSS as u32),
        }
    }

    fn area_size(&self) -> (u32, u32) {
        let f = self.frame_size();
        let o = self.origin();
        (f.0.saturating_sub(o.0).max(1), f.1.saturating_sub(o.1).max(1))
    }

    fn resize_for_area(&mut self, want: (u32, u32)) {
        self.want_area = Some(want);
        let o = self.origin();
        let target = (want.0 + o.0, want.1 + o.1);
        let current = self.frame_size();
        if self.pending.map(|(t, _)| t) == Some(target) || (self.pending.is_none() && current == target && self.frame_xid.is_some()) {
            return;
        }
        self.pending = Some((target, Instant::now()));
        self.user_resize = None;
        let _ = self.frame.set_size(PhysicalSize::new(target.0, target.1));
    }

    fn set_slot_origin(&mut self, x: i32, y: i32) {
        if self.slot_origin == Some((x, y)) {
            return;
        }
        self.slot_origin = Some((x, y));
        self.placed = None;
        if let Some(want) = self.want_area {
            self.resize_for_area(want);
        }
    }

    /// Un tour de pompe : détecte un redimensionnement de l'utilisateur, replace la zone native.
    fn sync(&mut self) {
        if self.frame_xid.is_none() && self.shown {
            // La fenêtre Tauri vient d'être affichée : sa fenêtre X existe, la zone native s'y rattache.
            if let Ok(xid) = xid_of(&self.frame) {
                self.frame_xid = Some(xid);
                let o = self.origin();
                // SAFETY: `display` ouvert ; `area` est à nous, `xid` est la fenêtre X de la fenêtre Tauri.
                unsafe {
                    (self.conn.x.reparent_window)(self.conn.display, self.area, xid, o.0 as i32, o.1 as i32);
                    (self.conn.x.flush)(self.conn.display);
                }
                self.placed = None;
                self.apply_area_visibility();
            }
        }
        let fc = self.frame_size();
        if fc.0 == 0 || fc.1 == 0 {
            return;
        }
        match self.pending {
            Some((target, since)) if fc == target || since.elapsed() > IMPOSED_GRACE => {
                self.pending = None;
                self.last_client = fc;
            }
            Some(_) => {}
            None if fc != self.last_client => {
                self.last_client = fc;
                let area = self.area_size();
                self.want_area = Some(area);
                self.user_resize = Some(area);
            }
            None => {}
        }
        let (w, h) = self.area_size();
        let o = self.origin();
        let rect = (o.0 as i32, o.1 as i32, w, h);
        if self.placed != Some(rect) {
            self.placed = Some(rect);
            // SAFETY: `display` ouvert, `area` est à nous.
            unsafe {
                (self.conn.x.move_resize_window)(self.conn.display, self.area, rect.0, rect.1, w, h);
                (self.conn.x.flush)(self.conn.display);
            }
        }
        if !self.logged && self.shown_at.map_or(false, |t| t.elapsed() > Duration::from_millis(1500)) {
            self.logged = true;
            crate::log(&format!(
                "fenêtre de plugin X11 : fenêtre Tauri {:?} {:?} ; origine de la zone (mesurée par la page : {}) {:?} ; zone native {:?}",
                self.frame_xid,
                fc,
                self.slot_origin.is_some(),
                o,
                (w, h)
            ));
        }
    }

    fn apply_area_visibility(&mut self) {
        let show = self.shown && self.area_visible && self.frame_xid.is_some();
        if show == self.mapped {
            return;
        }
        self.mapped = show;
        // SAFETY: `display` ouvert, `area` est à nous.
        unsafe {
            if show {
                (self.conn.x.map_window)(self.conn.display, self.area);
            } else {
                (self.conn.x.unmap_window)(self.conn.display, self.area);
            }
            (self.conn.x.flush)(self.conn.display);
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // SAFETY: la zone native est à nous ; elle est détruite avant sa mère. L'interface du plugin l'a déjà été (ordre imposé par CLAP).
        unsafe {
            (self.conn.x.destroy_window)(self.conn.display, self.area);
            (self.conn.x.flush)(self.conn.display);
        }
        let _ = self.frame.destroy();
    }
}

struct DockedWindow {
    inner: Rc<RefCell<Inner>>,
}

impl NativeWindow for DockedWindow {
    fn handle(&self) -> (&'static str, usize) {
        // Voir `X11Window::handle` : le plugin se logera dans cette fenêtre depuis sa propre connexion X ; le serveur doit déjà la connaître.
        // SAFETY: `display` ouvert, fil des plugins (celui qui possède la connexion).
        unsafe { (self.inner.borrow().conn.x.sync)(self.inner.borrow().conn.display, 0) };
        ("x11", self.inner.borrow().area as usize)
    }
    fn scale(&self) -> f64 {
        1.0
    }
    fn set_client_size(&mut self, width: u32, height: u32) {
        self.inner.borrow_mut().resize_for_area((width, height));
    }
    fn client_size(&self) -> (u32, u32) {
        self.inner.borrow().area_size()
    }
    fn set_area_origin(&mut self, x: i32, y: i32) {
        self.inner.borrow_mut().set_slot_origin(x, y);
    }
    fn show(&mut self) {
        let mut i = self.inner.borrow_mut();
        i.shown = true;
        i.shown_at = Some(Instant::now());
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
    conn: Option<Rc<Conn>>,
    windows: Vec<Weak<RefCell<Inner>>>,
}

impl DockedSystem {
    pub fn new() -> Self {
        Self::default()
    }
}

/// L'identifiant X11 de la fenêtre Tauri (à lire sur le fil principal), ou un message si la fenêtre n'est pas une fenêtre X11.
fn xid_of(frame: &WebviewWindow) -> Result<Window, String> {
    let handle = frame.window_handle().map_err(|e| e.to_string())?;
    match handle.as_raw() {
        RawWindowHandle::Xlib(h) => Ok(h.window as Window),
        RawWindowHandle::Xcb(h) => Ok(h.window.get() as Window),
        _ => Err("la fenêtre de l'application n'est pas une fenêtre X11 (Wayland natif : lancer avec GDK_BACKEND=x11)".into()),
    }
}

impl WindowSystem for DockedSystem {
    fn api(&self) -> &'static str {
        "x11"
    }

    fn docked(&self) -> bool {
        true
    }

    fn create(&mut self, _title: &str, _client: (u32, u32), _resizable: bool) -> Result<Box<dyn NativeWindow>, String> {
        Err("les fenêtres de plugin se créent par plugin (create_for)".into())
    }

    fn create_for(&mut self, insert: u32, title: &str, client: (u32, u32), resizable: bool, native: bool) -> Result<Box<dyn NativeWindow>, String> {
        let app = APP.get().ok_or("l'application n'est pas prête")?.clone();
        let conn = match &self.conn {
            Some(c) => Rc::clone(c),
            None => {
                let c = open_conn()?;
                self.conn = Some(Rc::clone(&c));
                c
            }
        };
        let label = format!("plugin-{insert}-{}", LABEL.fetch_add(1, Ordering::Relaxed));
        let url = format!("plugin.html?insert={insert}");
        let (tx, rx) = mpsc::channel();
        let (app2, title2) = (app.clone(), title.to_owned());
        // La fenêtre Tauri se crée sur le fil principal ; ce fil-ci attend (le fil principal ne nous attend jamais).
        app.run_on_main_thread(move || {
            let built = WebviewWindowBuilder::new(&app2, label, WebviewUrl::App(url.into()))
                .title(title2)
                .inner_size(client.0 as f64 + LIST_CSS, client.1 as f64 + TOP_CSS)
                .min_inner_size(300.0 + LIST_CSS, 200.0)
                .resizable(resizable)
                .visible(false)
                .build()
                .map_err(|e| e.to_string());
            let _ = tx.send(built);
        })
        .map_err(|e| e.to_string())?;
        let frame = rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "la fenêtre du plugin n'a pas été créée à temps".to_string())??;
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
        // SAFETY: `display` ouvert ; fenêtre de haut niveau jamais affichée (rattachée à la fenêtre Tauri à son affichage), sans bordure, fond noir.
        let area: c_ulong = unsafe { (conn.x.create_simple_window)(conn.display, conn.root, 0, 0, client.0.max(1), client.1.max(1), 0, 0, conn.black) };
        // SAFETY: `display` ouvert.
        unsafe { (conn.x.flush)(conn.display) };
        let inner = Rc::new(RefCell::new(Inner {
            conn,
            area,
            frame_xid: None,
            frame,
            area_visible: native,
            shown: false,
            close,
            last_client: (0, 0),
            pending: None,
            user_resize: None,
            placed: None,
            slot_origin: None,
            want_area: None,
            mapped: false,
            shown_at: None,
            logged: false,
        }));
        self.windows.push(Rc::downgrade(&inner));
        let mut win = DockedWindow { inner };
        win.set_client_size(client.0, client.1);
        Ok(Box::new(win))
    }

    fn pump(&mut self) {
        self.windows.retain(|w| w.strong_count() > 0);
        for w in &self.windows {
            if let Some(inner) = w.upgrade() {
                inner.borrow_mut().sync();
            }
        }
    }
}
