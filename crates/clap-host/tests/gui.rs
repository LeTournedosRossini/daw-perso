//! Interface graphique et minuteurs côté hôte CLAP, vérifiés avec l'interface factice du plugin de test.
//! Le journal du plugin de test est global : un verrou fait passer les tests l'un après l'autre.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use daw_clap_host::{Bundle, Plugin, PluginHandle};
use daw_test_plugin::gui;
use daw_test_plugin::PLUGIN_ID;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn plugin() -> (Plugin, PluginHandle) {
    let bundle = Arc::new(Bundle::from_static_entry(daw_test_plugin::entry()).unwrap());
    let p = Plugin::instantiate(&bundle, PLUGIN_ID.to_str().unwrap()).unwrap();
    let h = p.handle();
    gui::clear_log();
    (p, h)
}

#[test]
fn the_plugin_announces_an_embedded_interface_for_the_three_window_apis() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_p, h) = plugin();
    let s = h.gui_support();
    assert!(s.has_gui);
    for api in ["win32", "cocoa", "x11"] {
        assert!(s.apis.contains(&(api.to_owned(), false)), "{api} intégrée : {:?}", s.apis);
    }
    assert!(s.apis.iter().all(|(_, floating)| !floating), "aucune interface flottante");
    assert!(!s.apis.iter().any(|(a, _)| a == "wayland"));
    assert_eq!(s.preferred, None);
}

#[test]
fn opening_follows_the_order_the_specification_requires_and_destroy_is_idempotent() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_p, h) = plugin();
    assert!(!h.gui_is_open());
    assert!(h.gui_create("x11"));
    assert!(h.gui_is_open());
    assert!(h.gui_set_scale(1.5));
    assert_eq!(h.gui_size(), Some(gui::DEFAULT_SIZE));
    assert!(h.gui_can_resize());
    h.gui_suggest_title("Gain — Basse");
    // SAFETY: le plugin de test ne déréférence pas le handle : il le consigne seulement.
    assert!(unsafe { h.gui_set_parent("x11", 1234) });
    assert!(h.gui_show());
    assert_eq!(gui::take_log(), ["create:x11:intégrée", "set_scale:1.5", "title:Gain — Basse", "set_parent:x11:1234", "show"]);
    h.gui_destroy();
    h.gui_destroy(); // sans effet la deuxième fois
    assert!(!h.gui_is_open());
    assert_eq!(gui::take_log(), ["hide", "destroy"]);
}

#[test]
fn the_interface_is_destroyed_before_the_plugin_when_the_last_handle_goes() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (p, h) = plugin();
    assert!(h.gui_create("win32"));
    drop(p); // le plugin côté audio part : la poignée garde l'instance en vie
    assert_eq!(gui::take_log(), ["create:win32:intégrée"]);
    drop(h);
    assert_eq!(gui::take_log(), ["hide", "destroy"], "l'interface est détruite avant le plugin");
}

#[test]
fn size_requests_and_adjustment() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_p, h) = plugin();
    assert!(h.gui_create("x11"));
    assert_eq!(h.take_gui_resize_request(), None);
    // Le plugin demande une taille : l'hôte la reçoit une seule fois.
    assert!(gui::request_resize(640, 480));
    assert_eq!(h.take_gui_resize_request(), Some((640, 480)));
    assert_eq!(h.take_gui_resize_request(), None);
    // L'hôte demande l'avis du plugin sur une taille : ici au moins 100 × 80, valeurs paires.
    assert_eq!(h.gui_adjust_size(51, 33), (100, 80));
    assert_eq!(h.gui_adjust_size(301, 201), (300, 200));
    gui::take_log();
    assert!(h.gui_set_size(300, 200));
    assert_eq!(gui::take_log(), ["set_size:300x200"]);
    assert_eq!(h.gui_size(), Some((300, 200)));
}

#[test]
fn the_plugin_can_ask_to_close_its_interface() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_p, h) = plugin();
    assert!(h.gui_create("x11"));
    assert!(!h.take_gui_close_request());
    assert!(gui::request_hide());
    assert!(h.take_gui_close_request());
    assert!(!h.take_gui_close_request(), "une seule fois");
}

#[test]
fn timers_fire_when_due_not_before_and_can_be_removed() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (_p, h) = plugin();
    let t0 = Instant::now();
    let id = gui::register_timer(50).expect("minuteur accordé");
    assert_eq!(h.run_timers(t0), 0, "pas encore échu");
    assert_eq!(h.run_timers(t0 + Duration::from_millis(60)), 1);
    assert_eq!(gui::take_log(), [format!("timer:{id}")]);
    // Une échéance manquée n'est pas rattrapée : un seul appel même après un long silence, puis il faut attendre la période.
    assert_eq!(h.run_timers(t0 + Duration::from_secs(10)), 1);
    assert_eq!(h.run_timers(t0 + Duration::from_secs(10) + Duration::from_millis(5)), 0);
    assert!(gui::unregister_timer(id));
    assert!(!gui::unregister_timer(id), "déjà retiré");
    gui::clear_log();
    assert_eq!(h.run_timers(t0 + Duration::from_secs(60)), 0);
    assert!(gui::take_log().is_empty());
    // Période trop courte : relevée à 10 ms.
    let fast = gui::register_timer(1).unwrap();
    let now = Instant::now();
    assert_eq!(h.run_timers(now + Duration::from_millis(5)), 0);
    assert_eq!(h.run_timers(now + Duration::from_millis(15)), 1);
    assert!(gui::unregister_timer(fast));
}

#[test]
fn gestures_and_values_made_in_the_interface_reach_the_host_in_order() {
    use daw_clap_host::ParamOut;
    use daw_test_plugin::gui::{user_action, UserAction};
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (mut p, h) = plugin();
    p.activate(48_000.0, 256).unwrap();
    p.start_processing().unwrap();
    assert_eq!(h.pop_param_event(), None);
    user_action(UserAction::Begin(0));
    for v in [0.5, 0.75, 1.25] {
        user_action(UserAction::Set(0, v));
    }
    user_action(UserAction::End(0));
    let (z, mut o1, mut o2) = ([0.0f32; 64], [0.0f32; 64], [0.0f32; 64]);
    p.process(&z, &z, &mut o1, &mut o2).unwrap();
    let got: Vec<_> = std::iter::from_fn(|| h.pop_param_event()).collect();
    assert_eq!(
        got,
        [
            ParamOut::GestureBegin { id: 0 },
            ParamOut::Value { id: 0, value: 0.5 },
            ParamOut::Value { id: 0, value: 0.75 },
            ParamOut::Value { id: 0, value: 1.25 },
            ParamOut::GestureEnd { id: 0 },
        ]
    );
    assert_eq!(h.pop_param_event(), None);
    assert_eq!(h.lost_param_events(), 0);
    // La file ne perd rien tant qu'on la vide, et compte ce qu'elle perd quand elle déborde (1 024 cases).
    for k in 0..1100 {
        user_action(UserAction::Set(0, (k % 3) as f64 * 0.5));
    }
    p.process(&z, &z, &mut o1, &mut o2).unwrap();
    assert_eq!(std::iter::from_fn(|| h.pop_param_event()).count(), 1024);
    assert_eq!(h.lost_param_events(), 76);
}
