//! Fenêtres de plugin « à bande » : l'application ouvre une fenêtre par plugin (bande web + zone de l'interface native). Avec un faux
//! système de fenêtres qui se dit « à bande » : drapeau du projet, ouverture avec ou sans interface native, bascule de la vue.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::Engine;
use daw_test_plugin::{entry, gui, PLUGIN_ID};
use daw_ui::demo::demo_session;
use daw_ui::host::{PluginRef, PluginResolver};
use daw_ui::plugin_window::{NativeWindow, WindowSystem};
use daw_ui::protocol::ProjectInfo;
use daw_ui::{Event, Host};

const SR: u32 = 48_000;

#[derive(Default)]
struct Seen {
    /// (plugin, native) de chaque fenêtre créée par `create_for`.
    created: Vec<(u32, bool, (u32, u32))>,
    visible: Vec<bool>,
    destroyed: u32,
    api: &'static str,
}

struct Win(Rc<RefCell<Seen>>);
impl NativeWindow for Win {
    fn handle(&self) -> (&'static str, usize) {
        (self.0.borrow().api, 4242)
    }
    fn scale(&self) -> f64 {
        1.0
    }
    fn set_client_size(&mut self, _w: u32, _h: u32) {}
    fn client_size(&self) -> (u32, u32) {
        (320, 200)
    }
    fn show(&mut self) {}
    fn focus(&mut self) {}
    fn take_close_request(&mut self) -> bool {
        false
    }
    fn take_user_resize(&mut self) -> Option<(u32, u32)> {
        None
    }
    fn set_area_visible(&mut self, visible: bool) {
        self.0.borrow_mut().visible.push(visible);
    }
}
impl Drop for Win {
    fn drop(&mut self) {
        self.0.borrow_mut().destroyed += 1;
    }
}

struct Sys(Rc<RefCell<Seen>>);
impl WindowSystem for Sys {
    fn api(&self) -> &'static str {
        self.0.borrow().api
    }
    fn docked(&self) -> bool {
        true
    }
    fn create(&mut self, _title: &str, _client: (u32, u32), _resizable: bool) -> Result<Box<dyn NativeWindow>, String> {
        Err("une fenêtre à bande se crée par create_for".into())
    }
    fn create_for(&mut self, insert: u32, _title: &str, client: (u32, u32), _resizable: bool, native: bool) -> Result<Box<dyn NativeWindow>, String> {
        self.0.borrow_mut().created.push((insert, native, client));
        Ok(Box::new(Win(Rc::clone(&self.0))))
    }
    fn pump(&mut self) {}
}

fn resolver() -> PluginResolver {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let pref = PluginRef { clap_id: PLUGIN_ID.to_str().unwrap().into(), vendor: "daw-perso".into(), name: "Gain de test".into(), file: "test.clap".into() };
    Box::new(move |e| (e.clap_id == PLUGIN_ID.to_str().unwrap()).then(|| (clap_insert_factory(Arc::clone(&bundle), e.clap_id.clone()), pref.clone())))
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn setup(api: &'static str) -> (Engine, Host, Rc<RefCell<Seen>>, u32) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_plugin_resolver(resolver());
    let seen = Rc::new(RefCell::new(Seen { api, ..Seen::default() }));
    h.set_window_system(Box::new(Sys(Rc::clone(&seen))));
    gui::clear_log();
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    let node = project(&ev).nodes.iter().find(|n| n.name == "Batterie").unwrap().id;
    let ev = h.command_json(&format!(r#"{{"type":"add_plugin","node":{node},"clap_id":"{}","file":"test.clap"}}"#, PLUGIN_ID.to_str().unwrap()));
    let insert = project(&ev).nodes.iter().find(|n| n.id == node).unwrap().inserts.last().unwrap().id;
    (engine, h, seen, insert)
}

#[test]
fn a_docked_system_is_announced_to_the_page_and_opens_a_window_with_the_native_area() {
    let _guard = ONE_AT_A_TIME.lock().unwrap();
    let (_e, mut h, seen, insert) = setup("win32");
    let ev = h.command_json(r#"{"type":"get_project"}"#);
    assert!(project(&ev).plugin_windows, "la page sait que l'application ouvre une fenêtre par plugin");
    let ev = h.command_json(&format!(r#"{{"type":"open_plugin_ui","insert":{insert}}}"#));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    let s = seen.borrow();
    assert_eq!(s.created.len(), 1);
    assert_eq!((s.created[0].0, s.created[0].1), (insert, true), "créée pour ce plugin, avec la zone native");
    assert!(s.created[0].2 .0 > 0 && s.created[0].2 .1 > 0, "taille de la zone donnée par le plugin");
}

#[test]
fn the_view_toggle_shows_and_hides_the_native_area_and_refuses_unknown_views() {
    let _guard = ONE_AT_A_TIME.lock().unwrap();
    let (_e, mut h, seen, insert) = setup("win32");
    h.command_json(&format!(r#"{{"type":"open_plugin_ui","insert":{insert}}}"#));
    h.command_json(&format!(r#"{{"type":"set_plugin_view","insert":{insert},"view":"web"}}"#));
    h.command_json(&format!(r#"{{"type":"set_plugin_view","insert":{insert},"view":"native"}}"#));
    assert_eq!(seen.borrow().visible, vec![false, true]);
    let ev = h.command_json(&format!(r#"{{"type":"set_plugin_view","insert":{insert},"view":"autre"}}"#));
    assert!(ev.iter().any(|e| matches!(e, Event::Error { command, .. } if command == "set_plugin_view")));
    // Sans fenêtre ouverte, ou pour un plugin inconnu : sans effet ni erreur.
    h.command_json(&format!(r#"{{"type":"close_plugin_ui","insert":{insert}}}"#));
    let ev = h.command_json(&format!(r#"{{"type":"set_plugin_view","insert":{insert},"view":"web"}}"#));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })));
    assert_eq!(seen.borrow().visible, vec![false, true]);
    assert_eq!(seen.borrow().destroyed, 1);
}

#[test]
fn a_plugin_without_a_native_interface_here_still_gets_a_window_with_the_strip_only() {
    let _guard = ONE_AT_A_TIME.lock().unwrap();
    // « wayland » : une API que le plugin de test ne propose pas → pas d'interface native, mais la fenêtre à bande s'ouvre quand même.
    let (_e, mut h, seen, insert) = setup("wayland");
    let ev = h.command_json(&format!(r#"{{"type":"open_plugin_ui","insert":{insert}}}"#));
    assert!(!ev.iter().any(|e| matches!(e, Event::Error { .. })), "{ev:?}");
    assert_eq!((seen.borrow().created[0].0, seen.borrow().created[0].1), (insert, false));
    assert!(ev.iter().any(|e| matches!(e, Event::PluginUi { insert: i, open: true } if *i == insert)));
    // Fermer : la fenêtre est détruite sans rien demander au plugin (il n'a pas d'interface créée).
    h.command_json(&format!(r#"{{"type":"close_plugin_ui","insert":{insert}}}"#));
    assert_eq!(seen.borrow().destroyed, 1);
    // Retirer le plugin ferme sa fenêtre aussi.
    let ev = h.command_json(&format!(r#"{{"type":"open_plugin_ui","insert":{insert}}}"#));
    assert!(ev.iter().any(|e| matches!(e, Event::PluginUi { open: true, .. })));
    h.command_json(&format!(r#"{{"type":"remove_plugin","insert":{insert}}}"#));
    let closed = h.pump_windows();
    assert!(closed.iter().any(|e| matches!(e, Event::PluginUi { insert: i, open: false } if *i == insert)), "{closed:?}");
}

static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
