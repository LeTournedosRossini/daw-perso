//! Appels d'interface de plugin envoyés à un « fil principal » (exécuteur installé par l'application sous macOS).
//! Un exécuteur est global au processus : ce fichier de test l'installe seul, dans son propre processus.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use daw_clap_host::{main_thread, Bundle, Plugin};
use daw_test_plugin::{gui, PLUGIN_ID};

static JOBS: AtomicUsize = AtomicUsize::new(0);
static MAIN_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);
static ON_MAIN: AtomicUsize = AtomicUsize::new(0);

#[test]
fn every_interface_call_runs_on_the_main_thread_and_blocks_until_done() {
    // « Fil principal » de substitution : une boucle qui exécute les tâches une à une.
    let (tx, rx) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
    let tx = Mutex::new(tx);
    let main = std::thread::spawn(move || {
        *MAIN_THREAD.lock().unwrap() = Some(std::thread::current().id());
        while let Ok(job) = rx.recv() {
            JOBS.fetch_add(1, Ordering::SeqCst);
            if Some(std::thread::current().id()) == *MAIN_THREAD.lock().unwrap() {
                ON_MAIN.fetch_add(1, Ordering::SeqCst);
            }
            job();
        }
    });
    main_thread::set_runner(Box::new(move |job| tx.lock().unwrap().send(job).is_ok()));
    assert!(main_thread::has_runner());

    let bundle = Arc::new(Bundle::from_static_entry(daw_test_plugin::entry()).unwrap());
    let p = Plugin::instantiate(&bundle, PLUGIN_ID.to_str().unwrap()).unwrap();
    let h = p.handle();
    gui::clear_log();

    // Une tâche rend son résultat à l'appelant, et l'appelant n'est pas le fil principal.
    let caller = std::thread::current().id();
    let ran_on = main_thread::run(|| std::thread::current().id()).unwrap();
    assert_ne!(ran_on, caller);
    assert_eq!(Some(ran_on), *MAIN_THREAD.lock().unwrap());

    let before = JOBS.load(Ordering::SeqCst);
    assert!(h.gui_create("cocoa"));
    assert!(h.gui_set_scale(1.0));
    assert_eq!(h.gui_size(), Some(gui::DEFAULT_SIZE));
    assert!(h.gui_can_resize());
    assert_eq!(h.gui_adjust_size(301, 201), (300, 200));
    assert!(h.gui_set_size(200, 150));
    h.gui_suggest_title("Essai");
    // SAFETY: le plugin de test ne déréférence pas le handle : il le consigne seulement.
    assert!(unsafe { h.gui_set_parent("cocoa", 4321) });
    assert!(h.gui_show());
    assert!(h.gui_hide());
    h.gui_destroy();
    assert_eq!(JOBS.load(Ordering::SeqCst) - before, 11, "un envoi au fil principal par appel");
    // Le plugin a bien reçu les appels, dans l'ordre.
    let log = gui::take_log();
    assert_eq!(log.first().map(String::as_str), Some("create:cocoa:intégrée"), "{log:?}");
    assert_eq!(log.last().map(String::as_str), Some("destroy"), "{log:?}");

    // Un minuteur arrivé à échéance bat aussi sur le fil principal.
    let id = gui::register_timer(10).expect("minuteur enregistré");
    let start = Instant::now();
    while h.run_timers(Instant::now()) == 0 && start.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(gui::take_log().iter().any(|l| l.starts_with("timer")), "le minuteur {id} a battu");
    assert_eq!(ON_MAIN.load(Ordering::SeqCst), JOBS.load(Ordering::SeqCst));
    drop(main); // la boucle s'arrête avec le processus de test
}
