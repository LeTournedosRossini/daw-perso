//! Fenêtres de plugin sous macOS : une fenêtre Tauri (la page `plugin.html` : bande, liste des plugins, paramètres) et, par-dessus sa
//! partie basse, une **`NSView` fille de la vue de contenu** où le plugin loge son interface native (API CLAP `cocoa`).
//!
//! **Écrit sans Mac, jamais exécuté** : seule la compilation (CI `macos-app`) et les fonctions de géométrie sont vérifiées.
//!
//! * **Fil principal** : AppKit n'accepte `NSView` / `NSWindow` que sur le fil principal du processus, que Tauri possède. Toutes les
//!   opérations AppKit de ce fichier passent par `daw_clap_host::main_thread::run` (le fil appelant attend la fin de la tâche), et les
//!   appels `gui.*` du plugin y passent aussi, par le même exécuteur installé dans `main.rs`. Le fil moteur ne reçoit jamais de
//!   rappel du fil principal : pas d'interblocage possible.
//! * **Unités** : des **points** partout (jamais des pixels physiques) : c'est l'unité de la spécification CLAP pour `cocoa`, qui
//!   ignore `set_scale` ; `scale()` rend donc 1,0. La page mesure sa zone en pixels physiques : on divise par le facteur d'échelle.
//! * **Géométrie** : la bande occupe les `TOP_CSS` premiers points de la page, la liste des plugins les `LIST_CSS` premiers à gauche
//!   (constantes partagées avec `css/app.css` et `win32_window.rs`) ; la zone native est le reste de la zone utile. Une `NSView`
//!   non retournée a son origine **en bas à gauche** : l'ordonnée est inversée avec la hauteur de la zone utile.
//! * **Un seul objet** : la zone suit la fenêtre à chaque tour de la pompe, sans fenêtre séparée (pas de décalage au déplacement,
//!   plein écran et Spaces gérés par le système).

use std::cell::RefCell;
use std::ffi::{c_char, c_void};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use daw_clap_host::main_thread;
use daw_ui::plugin_window::{NativeWindow, WindowSystem};
use tauri::{AppHandle, LogicalSize, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

static APP: OnceLock<AppHandle> = OnceLock::new();
static LABEL: AtomicU32 = AtomicU32::new(0);

/// À appeler une fois au démarrage, avant tout ouvrir de fenêtre de plugin.
pub fn set_app(app: AppHandle) {
    let _ = APP.set(app);
}

/// Hauteur de la bande, en points (= pixels CSS à 100 %) : voir `win32_window.rs` et `css/app.css`.
const TOP_CSS: f64 = 52.0;
/// Largeur de la liste des plugins de la piste, à gauche.
const LIST_CSS: f64 = 168.0;
/// Après un redimensionnement imposé, délai laissé à Tauri pour l'appliquer avant de reprendre les gestes de l'utilisateur.
const IMPOSED_GRACE: Duration = Duration::from_millis(600);
/// Intervalle minimal entre deux relevés de la géométrie de la fenêtre (chaque relevé interroge le fil principal).
const SYNC_EVERY: Duration = Duration::from_millis(30);

// ---- Géométrie pure (testée) ------------------------------------------------------------------------------------------------

/// Pixels physiques → points.
fn to_points(px: i32, scale: f64) -> i32 {
    if scale <= 0.0 {
        return px;
    }
    (px as f64 / scale).round() as i32
}

/// Rectangle de la zone native dans la vue de contenu, au format AppKit (origine en bas à gauche) : `(x, y, largeur, hauteur)` en
/// points, pour une zone dont le coin haut-gauche est à `origin` et qui va jusqu'aux bords droit et bas d'une vue de `view` points.
fn area_rect(view: (u32, u32), origin: (u32, u32)) -> (f64, f64, f64, f64) {
    let w = view.0.saturating_sub(origin.0).max(1);
    let h = view.1.saturating_sub(origin.1).max(1);
    let y = view.1 as f64 - origin.1 as f64 - h as f64;
    (origin.0 as f64, y.max(0.0), w as f64, h as f64)
}

/// Ramène un coin haut-gauche à l'intérieur de l'écran `(x, y, largeur, hauteur)` : au moins 120 × 60 de la fenêtre (sa barre de titre)
/// restent visibles.
fn on_screen(x: i32, y: i32, (sx, sy, sw, sh): (i32, i32, i32, i32)) -> (i32, i32) {
    if sw <= 0 || sh <= 0 {
        return (x, y);
    }
    (x.clamp(sx - 60, sx + sw - 120), y.clamp(sy, sy + sh - 60))
}

// ---- Objective-C écrit à la main (aucune dépendance) ------------------------------------------------------------------------

type Id = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct NsRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[cfg_attr(target_os = "macos", link(name = "objc"))]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Id;
    fn objc_msgSend();
}

/// Envoi de message sans argument, résultat pointeur.
///
/// # Safety
/// `obj` doit être un objet Objective-C valide (ou nul) qui répond à `sel`, appelé depuis le fil principal.
unsafe fn msg(obj: Id, sel: &[u8]) -> Id {
    let f: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(objc_msgSend as *const ());
    f(obj, sel_registerName(sel.as_ptr() as *const c_char))
}

/// # Safety
/// Comme `msg` ; `sel` prend un argument pointeur.
unsafe fn msg_id(obj: Id, sel: &[u8], a: Id) -> Id {
    let f: unsafe extern "C" fn(Id, Id, Id) -> Id = std::mem::transmute(objc_msgSend as *const ());
    f(obj, sel_registerName(sel.as_ptr() as *const c_char), a)
}

/// # Safety
/// Comme `msg` ; `sel` prend un `NSRect` et rend un pointeur (`initWithFrame:`).
unsafe fn msg_rect_id(obj: Id, sel: &[u8], r: NsRect) -> Id {
    let f: unsafe extern "C" fn(Id, Id, NsRect) -> Id = std::mem::transmute(objc_msgSend as *const ());
    f(obj, sel_registerName(sel.as_ptr() as *const c_char), r)
}

/// # Safety
/// Comme `msg` ; `sel` prend un `NSRect` et ne rend rien (`setFrame:`).
unsafe fn msg_rect(obj: Id, sel: &[u8], r: NsRect) {
    let f: unsafe extern "C" fn(Id, Id, NsRect) = std::mem::transmute(objc_msgSend as *const ());
    f(obj, sel_registerName(sel.as_ptr() as *const c_char), r)
}

/// # Safety
/// Comme `msg` ; `sel` prend un `BOOL` et ne rend rien (`setHidden:`).
unsafe fn msg_bool(obj: Id, sel: &[u8], v: bool) {
    let f: unsafe extern "C" fn(Id, Id, i8) = std::mem::transmute(objc_msgSend as *const ());
    f(obj, sel_registerName(sel.as_ptr() as *const c_char), v as i8)
}

/// La `NSWindow` d'une fenêtre Tauri. (Hors macOS ce fichier n'est compilé que pour vérification des types : jamais exécuté.)
fn ns_window_of(frame: &WebviewWindow) -> Result<usize, String> {
    #[cfg(target_os = "macos")]
    {
        frame.ns_window().map(|p| p as usize).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = frame;
        Err("NSWindow n'existe que sous macOS".into())
    }
}

/// Crée une `NSView` cachée, fille de la vue de contenu de `ns_window`. Fil principal seulement.
fn create_view(ns_window: usize, area: (f64, f64, f64, f64)) -> Result<usize, String> {
    // SAFETY: fil principal (appelée dans `main_thread::run`) ; `ns_window` est la `NSWindow` d'une fenêtre Tauri vivante ; les noms de
    // classe et de sélecteur sont des chaînes C terminées par un octet nul.
    unsafe {
        let content = msg(ns_window as Id, b"contentView\0");
        if content.is_null() {
            return Err("la fenêtre n'a pas de vue de contenu".into());
        }
        let class = objc_getClass(b"NSView\0".as_ptr() as *const c_char);
        if class.is_null() {
            return Err("classe NSView introuvable".into());
        }
        let alloc = msg(class, b"alloc\0");
        let view = msg_rect_id(alloc, b"initWithFrame:\0", NsRect { x: area.0, y: area.1, w: area.2, h: area.3 });
        if view.is_null() {
            return Err("NSView non créée".into());
        }
        msg_bool(view, b"setHidden:\0", true);
        msg_id(content, b"addSubview:\0", view); // au-dessus de la vue web
        Ok(view as usize)
    }
}

/// Replace (et montre ou cache) la zone. Fil principal seulement.
fn place_view(view: usize, area: (f64, f64, f64, f64), visible: bool) {
    // SAFETY: fil principal ; `view` est une `NSView` créée par `create_view`, qui vit jusqu'à `destroy_view`.
    unsafe {
        msg_rect(view as Id, b"setFrame:\0", NsRect { x: area.0, y: area.1, w: area.2, h: area.3 });
        msg_bool(view as Id, b"setHidden:\0", !visible);
    }
}

/// Retire la zone de sa vue mère puis la libère. Fil principal seulement.
fn destroy_view(view: usize) {
    // SAFETY: fil principal ; `view` créée par `create_view` (propriétaire : le +1 de `alloc`), jamais réutilisée après.
    unsafe {
        msg(view as Id, b"removeFromSuperview\0");
        msg(view as Id, b"release\0");
    }
}

// ---- Fenêtres de plugin à bande ---------------------------------------------------------------------------------------------

struct Inner {
    frame: WebviewWindow,
    /// `NSView` de la zone native (0 une fois détruite).
    view: usize,
    area_visible: bool,
    shown: bool,
    close: Arc<AtomicBool>,
    /// Dernière taille de la zone utile connue (points).
    last_client: (u32, u32),
    /// Taille imposée en attente d'application par Tauri ; tant qu'elle n'est pas atteinte (ou le délai écoulé) ce n'est pas l'utilisateur.
    pending: Option<((u32, u32), Instant)>,
    user_resize: Option<(u32, u32)>,
    /// Dernier rectangle posé (points, repère AppKit) et visibilité associée.
    placed: Option<((f64, f64, f64, f64), bool)>,
    /// Coin haut-gauche de la zone mesuré par la page (pixels physiques, relatifs à la vue web) ; sans lui, valeurs supposées.
    slot_origin: Option<(i32, i32)>,
    want_area: Option<(u32, u32)>,
    shown_at: Option<Instant>,
    last_sync: Option<Instant>,
    logged: u8,
}

impl Inner {
    fn scale(&self) -> f64 {
        self.frame.scale_factor().unwrap_or(1.0)
    }

    /// Zone utile de la fenêtre, en points.
    fn client(&self) -> (u32, u32) {
        match self.frame.inner_size() {
            Ok(s) => {
                let k = self.scale();
                (to_points(s.width as i32, k).max(0) as u32, to_points(s.height as i32, k).max(0) as u32)
            }
            Err(_) => self.last_client,
        }
    }

    /// Où est la zone dans la vue web, en points : mesuré par la page, sinon supposé.
    fn origin(&self) -> (u32, u32) {
        match self.slot_origin {
            Some((x, y)) => {
                let k = self.scale();
                (to_points(x, k).max(0) as u32, to_points(y, k).max(0) as u32)
            }
            None => (LIST_CSS as u32, TOP_CSS as u32),
        }
    }

    fn area(&self) -> (u32, u32) {
        let (c, o) = (self.client(), self.origin());
        (c.0.saturating_sub(o.0).max(1), c.1.saturating_sub(o.1).max(1))
    }

    fn resize_for_area(&mut self, want: (u32, u32)) {
        self.want_area = Some(want);
        let o = self.origin();
        let target = (want.0 + o.0, want.1 + o.1);
        let current = self.client();
        if self.pending.map(|(t, _)| t) == Some(target) || (self.pending.is_none() && current == target) {
            return;
        }
        self.pending = Some((target, Instant::now()));
        self.user_resize = None;
        let _ = self.frame.set_size(LogicalSize::new(target.0 as f64, target.1 as f64));
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

    /// Un tour de pompe : détecte un redimensionnement de l'utilisateur et replace la zone native.
    fn sync(&mut self) {
        if self.view == 0 || self.frame.is_minimized().unwrap_or(false) {
            return;
        }
        if self.last_sync.is_some_and(|t| t.elapsed() < SYNC_EVERY) {
            return;
        }
        self.last_sync = Some(Instant::now());
        let fc = self.client();
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
                let area = self.area();
                self.want_area = Some(area);
                self.user_resize = Some(area);
            }
            None => {}
        }
        self.apply();
        self.log_geometry();
    }

    /// Replace la zone (et sa visibilité) si quelque chose a changé.
    fn apply(&mut self) {
        if self.view == 0 {
            return;
        }
        let rect = area_rect(self.client(), self.origin());
        let visible = self.shown && self.area_visible;
        if self.placed == Some((rect, visible)) {
            return;
        }
        self.placed = Some((rect, visible));
        let view = self.view;
        if main_thread::run(move || place_view(view, rect, visible)).is_none() {
            self.placed = None; // la tâche n'est pas partie : on réessaiera au prochain tour
        }
    }

    /// Consigne la géométrie (à l'affichage puis 1,5 s plus tard) : si la zone déborde, ces valeurs disent d'où vient l'écart.
    fn log_geometry(&mut self) {
        let Some(since) = self.shown_at else { return };
        let due = match self.logged {
            0 => true,
            1 => since.elapsed() > Duration::from_millis(1500),
            _ => false,
        };
        if !due {
            return;
        }
        self.logged += 1;
        crate::log(&format!(
            "fenêtre de plugin ({}) : échelle {} ; zone utile {:?} pt ; origine de la zone (mesurée par la page : {}) {:?} pt ; zone native {:?}",
            self.logged,
            self.scale(),
            self.client(),
            self.slot_origin.is_some(),
            self.origin(),
            self.placed.map(|(r, v)| (r, v)),
        ));
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // La zone native d'abord (sur le fil principal), puis sa fenêtre : l'interface du plugin a déjà été détruite par l'hôte.
        let view = std::mem::take(&mut self.view);
        if view != 0 {
            main_thread::run(move || destroy_view(view));
        }
        let _ = self.frame.destroy();
    }
}

struct DockedWindow {
    inner: Rc<RefCell<Inner>>,
}

impl NativeWindow for DockedWindow {
    fn handle(&self) -> (&'static str, usize) {
        ("cocoa", self.inner.borrow().view)
    }

    /// Points : CLAP ignore l'échelle pour `cocoa`.
    fn scale(&self) -> f64 {
        1.0
    }

    fn set_client_size(&mut self, width: u32, height: u32) {
        self.inner.borrow_mut().resize_for_area((width, height));
    }

    fn client_size(&self) -> (u32, u32) {
        self.inner.borrow().area()
    }

    fn set_area_origin(&mut self, x: i32, y: i32) {
        self.inner.borrow_mut().set_slot_origin(x, y);
    }

    fn show(&mut self) {
        let mut i = self.inner.borrow_mut();
        i.shown = true;
        i.shown_at = Some(Instant::now());
        let _ = i.frame.show();
        i.last_client = i.client();
        i.apply();
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
        if i.frame.is_minimized().unwrap_or(false) {
            return None;
        }
        i.frame.outer_position().ok().map(|p| (p.x, p.y))
    }

    fn set_position(&mut self, x: i32, y: i32) {
        let i = self.inner.borrow();
        // Rectangle englobant de tous les écrans, en pixels physiques (les positions du projet sont dans ce repère).
        let screen = i.frame.available_monitors().ok().and_then(|ms| {
            ms.iter().map(|m| (m.position().x, m.position().y, m.size().width as i32, m.size().height as i32)).reduce(|a, b| {
                let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
                let (x1, y1) = ((a.0 + a.2).max(b.0 + b.2), (a.1 + a.3).max(b.1 + b.3));
                (x0, y0, x1 - x0, y1 - y0)
            })
        });
        let (x, y) = on_screen(x, y, screen.unwrap_or((0, 0, 0, 0)));
        let _ = i.frame.set_position(PhysicalPosition::new(x, y));
    }

    fn set_title(&mut self, title: &str) {
        let _ = self.inner.borrow().frame.set_title(title);
    }

    fn set_area_visible(&mut self, visible: bool) {
        let mut i = self.inner.borrow_mut();
        i.area_visible = visible;
        i.apply();
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
        "cocoa"
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
        // La fenêtre Tauri et la zone native se créent sur le fil principal ; ce fil-ci attend (le fil principal ne nous attend jamais).
        app.run_on_main_thread(move || {
            let result = (|| -> Result<(WebviewWindow, usize), String> {
                let frame = WebviewWindowBuilder::new(&app2, label, WebviewUrl::App(url.into()))
                    .title(title2)
                    .inner_size(client.0 as f64 + LIST_CSS, client.1 as f64 + TOP_CSS)
                    .min_inner_size(300.0 + LIST_CSS, 200.0)
                    .resizable(resizable)
                    .visible(false)
                    .build()
                    .map_err(|e| e.to_string())?;
                let ns_window = ns_window_of(&frame)?;
                let area = area_rect((client.0 + LIST_CSS as u32, client.1 + TOP_CSS as u32), (LIST_CSS as u32, TOP_CSS as u32));
                match create_view(ns_window, area) {
                    Ok(view) => Ok((frame, view)),
                    Err(e) => {
                        let _ = frame.destroy();
                        Err(e)
                    }
                }
            })();
            let _ = tx.send(result);
        })
        .map_err(|e| e.to_string())?;
        let (frame, view) = rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "la fenêtre du plugin n'a pas été créée à temps".to_string())??;
        // La croix ne détruit pas la fenêtre : elle lève un drapeau que l'hôte lit (interface du plugin détruite avant la fenêtre).
        let close = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&close);
        frame.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                flag.store(true, Ordering::Relaxed);
            }
        });
        let inner = Rc::new(RefCell::new(Inner {
            frame,
            view,
            area_visible: native,
            shown: false,
            close,
            last_client: (client.0 + LIST_CSS as u32, client.1 + TOP_CSS as u32),
            pending: None,
            user_resize: None,
            placed: None,
            slot_origin: None,
            want_area: None,
            shown_at: None,
            last_sync: None,
            logged: 0,
        }));
        self.windows.push(Rc::downgrade(&inner));
        let mut win = DockedWindow { inner };
        win.set_client_size(client.0, client.1);
        Ok(Box::new(win))
    }

    fn pump(&mut self) {
        // Pas de pompe de messages : le fil principal de Tauri fait tourner la boucle d'événements ; on ne fait que replacer les zones.
        self.windows.retain(|w| w.strong_count() > 0);
        for w in &self.windows {
            if let Some(inner) = w.upgrade() {
                inner.borrow_mut().sync();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_follow_the_scale_factor() {
        assert_eq!(to_points(336, 2.0), 168);
        assert_eq!(to_points(105, 1.0), 105);
        assert_eq!(to_points(105, 1.5), 70);
        assert_eq!(to_points(50, 0.0), 50);
    }

    #[test]
    fn the_area_is_flipped_into_appkit_coordinates() {
        // Vue de 900 × 700 points, zone à 168 de la gauche et 52 du haut : elle va jusqu'aux bords droit et bas.
        let (x, y, w, h) = area_rect((900, 700), (168, 52));
        assert_eq!((x, w, h), (168.0, 732.0, 648.0));
        assert_eq!(y, 0.0, "la zone touche le bas : origine AppKit en bas");
        // Si la page mesure une origine plus basse, la zone raccourcit par le bas… et reste dans la vue.
        let (_, y, _, h) = area_rect((900, 700), (168, 100));
        assert_eq!((y, h), (0.0, 600.0));
        // Jamais de zone nulle ni de coordonnée négative.
        let (x, y, w, h) = area_rect((100, 40), (168, 52));
        assert!(w >= 1.0 && h >= 1.0 && x >= 0.0 && y >= 0.0);
    }

    #[test]
    fn a_window_outside_every_screen_comes_back() {
        let screen = (0, 0, 2560, 1440);
        assert_eq!(on_screen(300, 200, screen), (300, 200));
        assert_eq!(on_screen(9000, 200, screen), (2440, 200));
        assert_eq!(on_screen(-3000, -50, screen), (-60, 0));
        assert_eq!(on_screen(7, 7, (0, 0, 0, 0)), (7, 7));
    }
}
