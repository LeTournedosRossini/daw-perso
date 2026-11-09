//! Interfaces natives de plugins : ouverture, redimensionnement, fermeture, minuteurs — avec un faux système de fenêtres et
//! l'interface factice du plugin de test (le journal du plugin est global : les tests passent l'un après l'autre).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daw_clap_host::{clap_insert_factory, Bundle};
use daw_engine::Engine;
use daw_test_plugin::{entry, gui, PLUGIN_ID};
use daw_ui::demo::demo_session;
use daw_ui::host::{PluginRef, PluginResolver};
use daw_ui::plugin_window::{NativeWindow, WindowSystem};
use daw_ui::protocol::{NodeInfo, ProjectInfo};
use daw_ui::{Event, Host};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
const SR: u32 = 48_000;

/// Ce que la fausse fenêtre a vu, partagé avec le test.
#[derive(Default)]
struct Seen {
    created: Vec<(String, (u32, u32), bool)>,
    client: (u32, u32),
    shown: bool,
    focused: u32,
    destroyed: bool,
    /// Le plugin avait-il déjà détruit son interface quand la fenêtre a été détruite ?
    plugin_destroyed_first: bool,
    close_request: bool,
    user_resize: Option<(u32, u32)>,
    scale: f64,
    api: &'static str,
    /// Où la fenêtre est à l'écran (changé par « l'utilisateur » dans le test) et où l'hôte l'a replacée.
    position: Option<(i32, i32)>,
    placed: Option<(i32, i32)>,
}

struct FakeWindow(Rc<RefCell<Seen>>, usize);
impl NativeWindow for FakeWindow {
    fn handle(&self) -> (&'static str, usize) {
        (self.0.borrow().api, self.1)
    }
    fn scale(&self) -> f64 {
        self.0.borrow().scale
    }
    fn set_client_size(&mut self, w: u32, h: u32) {
        self.0.borrow_mut().client = (w, h);
    }
    fn client_size(&self) -> (u32, u32) {
        self.0.borrow().client
    }
    fn show(&mut self) {
        self.0.borrow_mut().shown = true;
    }
    fn focus(&mut self) {
        self.0.borrow_mut().focused += 1;
    }
    fn take_close_request(&mut self) -> bool {
        std::mem::take(&mut self.0.borrow_mut().close_request)
    }
    fn take_user_resize(&mut self) -> Option<(u32, u32)> {
        self.0.borrow_mut().user_resize.take()
    }
    fn position(&self) -> Option<(i32, i32)> {
        self.0.borrow().position
    }
    fn set_position(&mut self, x: i32, y: i32) {
        self.0.borrow_mut().placed = Some((x, y));
    }
}
impl Drop for FakeWindow {
    fn drop(&mut self) {
        let mut s = self.0.borrow_mut();
        s.destroyed = true;
        s.plugin_destroyed_first = gui::peek_log().last().is_some_and(|l| l == "destroy");
    }
}

struct FakeSystem(Rc<RefCell<Seen>>, usize);
impl WindowSystem for FakeSystem {
    fn api(&self) -> &'static str {
        self.0.borrow().api
    }
    fn create(&mut self, title: &str, client: (u32, u32), resizable: bool) -> Result<Box<dyn NativeWindow>, String> {
        if title.contains("REFUSE") {
            return Err("pas de mémoire".into());
        }
        let mut s = self.0.borrow_mut();
        s.created.push((title.to_owned(), client, resizable));
        s.client = client;
        s.destroyed = false;
        self.1 += 1;
        Ok(Box::new(FakeWindow(Rc::clone(&self.0), 1000 + self.1)))
    }
    fn pump(&mut self) {}
}

fn resolver() -> PluginResolver {
    let bundle = Arc::new(Bundle::from_static_entry(entry()).unwrap());
    let pref = PluginRef { clap_id: PLUGIN_ID.to_str().unwrap().into(), vendor: "daw-perso".into(), name: "Gain de test".into(), file: "test.clap".into() };
    Box::new(move |e| (e.clap_id == PLUGIN_ID.to_str().unwrap()).then(|| (clap_insert_factory(Arc::clone(&bundle), e.clap_id.clone()), pref.clone())))
}

fn setup(api: &'static str) -> (Engine, Host, Rc<RefCell<Seen>>) {
    let (engine, handle) = Engine::new(SR, 64);
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_plugin_resolver(resolver());
    let seen = Rc::new(RefCell::new(Seen { api, scale: 1.25, ..Seen::default() }));
    h.set_window_system(Box::new(FakeSystem(Rc::clone(&seen), 0)));
    gui::clear_log();
    (engine, h, seen)
}

fn project(ev: &[Event]) -> &ProjectInfo {
    ev.iter().rev().find_map(|e| if let Event::Project(p) = e { Some(p) } else { None }).expect("événement project")
}

fn node<'a>(p: &'a ProjectInfo, name: &str) -> &'a NodeInfo {
    p.nodes.iter().find(|n| n.name == name).unwrap()
}

fn add(h: &mut Host, node_id: u32) -> u32 {
    let ev = h.command_json(&format!(r#"{{"type":"add_plugin","node":{node_id},"clap_id":"com.daw-perso.test-gain","file":"test.clap"}}"#));
    project(&ev).nodes.iter().find(|n| n.id == node_id).unwrap().inserts.last().unwrap().id
}

fn ui_events(ev: &[Event]) -> Vec<(u32, bool)> {
    ev.iter().filter_map(|e| if let Event::PluginUi { insert, open } = e { Some((*insert, *open)) } else { None }).collect()
}

fn open(h: &mut Host, insert: u32) -> Vec<Event> {
    h.command_json(&format!(r#"{{"type":"open_plugin_ui","insert":{insert}}}"#))
}

#[test]
fn without_a_window_system_there_is_no_native_interface() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (engine, handle) = Engine::new(SR, 64);
    let _ = engine;
    let (session, meta) = demo_session(SR);
    let mut h = Host::new(handle, session, meta).unwrap();
    h.set_plugin_resolver(resolver());
    let id = add(&mut h, 2);
    let p = project(&h.command_json(r#"{"type":"get_project"}"#)).clone();
    assert!(!node(&p, "Basse").inserts[0].gui);
    let ev = open(&mut h, id);
    assert!(matches!(&ev[0], Event::Error { message, .. } if message.contains("pas encore disponibles")), "{ev:?}");
    assert!(h.pump_windows().is_empty());
}

#[test]
fn opening_creates_the_window_then_follows_the_spec_order_and_a_second_open_only_focuses() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_e, mut h, seen) = setup("x11");
    let id = add(&mut h, 2);
    let p = project(&h.command_json(r#"{"type":"get_project"}"#)).clone();
    assert!(node(&p, "Basse").inserts[0].gui && !node(&p, "Basse").inserts[0].ui_open);
    gui::clear_log();
    let ev = open(&mut h, id);
    assert_eq!(ui_events(&ev), [(id, true)]);
    assert!(node(project(&ev), "Basse").inserts[0].ui_open);
    {
        let s = seen.borrow();
        assert_eq!(s.created, [("Gain de test — Basse".to_owned(), gui::DEFAULT_SIZE, true)], "fenêtre créée avec la taille du plugin, redimensionnable");
        assert!(s.shown);
        assert_eq!(s.client, gui::DEFAULT_SIZE);
    }
    // Ordre de la spécification : create, set_scale (avec l'échelle de la fenêtre), set_parent (avec SA fenêtre), titre, show.
    assert_eq!(gui::take_log(), ["create:x11:intégrée", "set_scale:1.25", "set_parent:x11:1001", "title:Gain de test — Basse", "show"]);
    // Ouvrir de nouveau : la fenêtre passe au premier plan, aucune seconde interface.
    let again = open(&mut h, id);
    assert_eq!(ui_events(&again), [(id, true)]);
    assert_eq!(seen.borrow().focused, 1);
    assert_eq!(seen.borrow().created.len(), 1);
    assert!(gui::take_log().is_empty());
}

#[test]
fn closing_destroys_the_plugin_interface_before_the_native_window() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_e, mut h, seen) = setup("x11");
    let id = add(&mut h, 2);
    open(&mut h, id);
    gui::clear_log();
    let ev = h.command_json(&format!(r#"{{"type":"close_plugin_ui","insert":{id}}}"#));
    assert_eq!(ui_events(&ev), [(id, false)]);
    assert!(!node(project(&ev), "Basse").inserts[0].ui_open);
    assert_eq!(gui::take_log(), ["hide", "destroy"]);
    assert!(seen.borrow().destroyed && seen.borrow().plugin_destroyed_first, "le plugin détruit son interface AVANT la fenêtre");
    // Fermer ce qui n'est pas ouvert : rien.
    assert!(h.command_json(&format!(r#"{{"type":"close_plugin_ui","insert":{id}}}"#)).is_empty());
}

#[test]
fn the_cross_of_the_window_closes_it_and_so_does_a_plugin_that_asks_to_hide() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_e, mut h, seen) = setup("win32");
    let id = add(&mut h, 2);
    open(&mut h, id);
    assert!(h.pump_windows().is_empty(), "rien à faire : la fenêtre reste ouverte");
    seen.borrow_mut().close_request = true;
    assert_eq!(ui_events(&h.pump_windows()), [(id, false)]);
    assert!(seen.borrow().destroyed);
    // Le plugin se cache lui-même.
    open(&mut h, id);
    assert!(gui::request_hide());
    assert_eq!(ui_events(&h.pump_windows()), [(id, false)]);
}

#[test]
fn the_window_follows_the_plugin_size_and_the_user_resize_goes_through_adjust_size() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_e, mut h, seen) = setup("x11");
    let id = add(&mut h, 2);
    open(&mut h, id);
    // Le plugin change de taille : la fenêtre suit.
    assert!(gui::request_resize(640, 480));
    h.pump_windows();
    assert_eq!(seen.borrow().client, (640, 480));
    // L'utilisateur tire un bord : taille proposée au plugin (ajustée : paire, au moins 100 × 80), qui la reçoit, et la fenêtre l'adopte.
    gui::clear_log();
    seen.borrow_mut().user_resize = Some((801, 603));
    h.pump_windows();
    assert_eq!(gui::take_log(), ["set_size:800x602"]);
    assert_eq!(seen.borrow().client, (800, 602));
    // Trop petit : relevé au minimum du plugin.
    seen.borrow_mut().user_resize = Some((10, 10));
    h.pump_windows();
    assert_eq!(seen.borrow().client, (100, 80));
}

#[test]
fn timers_of_the_interface_beat_while_the_window_is_open() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_e, mut h, _seen) = setup("x11");
    let id = add(&mut h, 2);
    open(&mut h, id);
    let t = gui::register_timer(20).unwrap();
    gui::clear_log();
    h.pump_windows();
    assert!(gui::take_log().is_empty(), "pas encore échu");
    std::thread::sleep(Duration::from_millis(30));
    h.pump_windows();
    assert_eq!(gui::take_log(), [format!("timer:{t}")]);
    // Fenêtre fermée : plus de battement.
    h.command_json(&format!(r#"{{"type":"close_plugin_ui","insert":{id}}}"#));
    gui::clear_log();
    std::thread::sleep(Duration::from_millis(30));
    h.pump_windows();
    assert!(gui::take_log().is_empty());
    gui::unregister_timer(t);
}

#[test]
fn removing_the_plugin_or_changing_project_closes_its_window() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_e, mut h, seen) = setup("x11");
    let id = add(&mut h, 2);
    open(&mut h, id);
    // Le plugin est retiré du graphe : sa fenêtre se ferme à la pompe suivante (l'instance restait vivante grâce à la fenêtre).
    h.command_json(&format!(r#"{{"type":"remove_plugin","insert":{id}}}"#));
    assert!(!seen.borrow().destroyed, "pas encore : la pompe n'a pas tourné");
    assert_eq!(ui_events(&h.pump_windows()), [(id, false)]);
    assert!(seen.borrow().destroyed && seen.borrow().plugin_destroyed_first);
    // Annulation : le plugin revient (même identifiant) ; on rouvre, puis « Nouveau projet » ferme tout.
    h.command_json(r#"{"type":"undo"}"#);
    let back = open(&mut h, id);
    assert_eq!(ui_events(&back), [(id, true)]);
    let ev = h.command_json(r#"{"type":"new_project"}"#);
    assert_eq!(ui_events(&ev), [(id, false)]);
    assert!(seen.borrow().destroyed);
}

#[test]
fn refusals_leave_nothing_open() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_e, mut h, seen) = setup("wayland"); // le plugin n'accepte que win32, cocoa et x11
    let id = add(&mut h, 2);
    let p = project(&h.command_json(r#"{"type":"get_project"}"#)).clone();
    assert!(!node(&p, "Basse").inserts[0].gui);
    let ev = open(&mut h, id);
    assert!(matches!(&ev[0], Event::Error { message, .. } if message.contains("pas d'interface pour cette plateforme (wayland)")), "{ev:?}");
    assert!(seen.borrow().created.is_empty());
    assert!(matches!(&open(&mut h, 999)[0], Event::Error { message, .. } if message.contains("inconnu")));
    // Fenêtre qui ne se crée pas : l'interface déjà créée chez le plugin est détruite, rien ne reste ouvert.
    let (_e2, mut h2, _s2) = setup("x11");
    let id2 = add(&mut h2, 2);
    h2.command_json(r#"{"type":"rename_track","node":2,"name":"REFUSE"}"#);
    gui::clear_log();
    let ev = open(&mut h2, id2);
    assert!(matches!(&ev[0], Event::Error { message, .. } if message.contains("fenêtre impossible")), "{ev:?}");
    assert_eq!(gui::take_log(), ["create:x11:intégrée", "hide", "destroy"]);
    assert!(h2.pump_windows().is_empty());
}

fn run_blocks(e: &mut Engine, h: &mut Host, blocks: usize) -> Vec<Event> {
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let mut events = Vec::new();
    for b in 0..blocks {
        e.process(&mut l, &mut r);
        if b % 4 == 3 {
            events.extend(h.tick());
        }
    }
    events
}

fn plugin_params(ev: &[Event], insert: u32, param: u32) -> Vec<f64> {
    ev.iter().filter_map(|e| if let Event::PluginParam { insert: i, param: p, value, .. } = e { (*i == insert && *p == param).then_some(*value) } else { None }).collect()
}

#[test]
fn a_value_changed_in_the_plugin_interface_reaches_the_page_and_the_parameter_list() {
    use gui::UserAction::*;
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (mut e, mut h, _seen) = setup("x11");
    let id = add(&mut h, 2);
    run_blocks(&mut e, &mut h, 8);
    // Rien ne bouge tant que l'utilisateur ne touche à rien.
    assert!(plugin_params(&run_blocks(&mut e, &mut h, 8), id, 0).is_empty());
    gui::user_action(Begin(0));
    gui::user_action(Set(0, 0.5));
    gui::user_action(Set(0, 0.75));
    gui::user_action(End(0));
    let ev = run_blocks(&mut e, &mut h, 8);
    // La page ne reçoit que la dernière valeur du passage, avec le texte du plugin.
    assert_eq!(plugin_params(&ev, id, 0), [0.75]);
    // Le curseur de la liste de paramètres lit désormais cette valeur.
    let params = h.command_json(&format!(r#"{{"type":"get_plugin_params","insert":{id}}}"#));
    let value = params.iter().find_map(|e| if let Event::PluginParams { params, .. } = e { params.iter().find(|p| p.id == 0).map(|p| p.value) } else { None }).unwrap();
    assert_eq!(value, 0.75);
}

#[test]
fn gestures_in_the_plugin_interface_are_written_as_automation_in_touch_mode() {
    use gui::UserAction::*;
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (mut e, mut h, _seen) = setup("x11");
    let id = add(&mut h, 2);
    run_blocks(&mut e, &mut h, 8);
    h.command_json(&format!(r#"{{"type":"set_write_mode","node":2,"target":{{"kind":"insert","insert":{id},"param":0}},"mode":"touch"}}"#));
    h.command_json(r#"{"type":"transport","action":"play"}"#);
    run_blocks(&mut e, &mut h, 200); // la lecture avance : rien n'est écrit tant qu'on ne touche pas
    let lane_of = |h: &mut Host| project(&h.command_json(r#"{"type":"get_project"}"#)).nodes.iter().find(|n| n.id == 2).unwrap().automation.clone();
    assert!(lane_of(&mut h).is_empty());
    gui::user_action(Begin(0));
    gui::user_action(Set(0, 0.5));
    run_blocks(&mut e, &mut h, 200);
    gui::user_action(Set(0, 1.5));
    run_blocks(&mut e, &mut h, 200);
    gui::user_action(End(0));
    // Le relâchement termine l'enregistrement (Toucher) : la courbe est écrite en une seule action annulable.
    let ev = run_blocks(&mut e, &mut h, 40);
    h.command_json(r#"{"type":"transport","action":"stop"}"#);
    assert!(ev.iter().any(|e| matches!(e, Event::History { undo_label: Some(l), .. } if l == "Modifier l'automation")), "{ev:?}");
    let lane = lane_of(&mut h);
    assert_eq!(lane.len(), 1);
    let values: Vec<f64> = lane[0].points.iter().map(|p| p.value).collect();
    assert!(values.iter().any(|v| (v - 0.5).abs() < 0.02) && values.iter().any(|v| (v - 1.5).abs() < 0.02), "{values:?}");
}

#[test]
fn the_window_comes_back_where_the_user_left_it_and_the_place_is_saved_with_the_project() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_e, mut h, seen) = setup("win32");
    let id = add(&mut h, 2);
    open(&mut h, id);
    assert_eq!(seen.borrow().placed, None, "première ouverture : le système choisit");
    seen.borrow_mut().position = Some((300, 200));
    h.pump_windows();
    seen.borrow_mut().position = Some((410, 220)); // l'utilisateur la déplace encore, puis ferme
    h.command_json(&format!(r#"{{"type":"close_plugin_ui","insert":{id}}}"#));
    open(&mut h, id);
    assert_eq!(seen.borrow().placed, Some((410, 220)));
    // Enregistré dans le projet, et relu à l'ouverture.
    let dir = std::env::temp_dir().join(format!("daw-places-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("P.daw");
    h.command_json(&format!(r#"{{"type":"save_project","path":{:?}}}"#, path.to_string_lossy()));
    let text = std::fs::read_to_string(path.join("projet.json")).unwrap();
    assert!(text.contains("plugin_windows") && text.contains("Gain de test"), "{}", &text[text.len().saturating_sub(300)..]);
    let (_e2, mut b, seen2) = setup("win32");
    b.command_json(&format!(r#"{{"type":"open_project","path":{:?}}}"#, path.to_string_lossy()));
    let ev = b.command_json(r#"{"type":"get_project"}"#);
    let again = project(&ev).nodes.iter().flat_map(|n| n.inserts.iter()).next().expect("le plugin est revenu").id;
    open(&mut b, again);
    assert_eq!(seen2.borrow().placed, Some((410, 220)));
    let _ = std::fs::remove_dir_all(dir);
}
