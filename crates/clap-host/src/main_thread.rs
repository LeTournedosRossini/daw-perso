//! Exécution de certains appels de plugin sur le **fil principal du processus** (macOS : exigé par AppKit).
//!
//! Sous macOS, l'interface d'un plugin (`gui.*`, minuteurs) crée et manipule des `NSView` : cela ne peut se faire que sur le fil
//! principal de l'application, que Tauri possède. L'hôte, lui, appelle ses plugins depuis son fil moteur. L'application installe donc ici
//! un *exécuteur* (`set_runner`) qui sait poster une tâche sur le fil principal ; `run` y envoie la tâche et **attend sa fin**.
//! Sans exécuteur (Windows, Linux, tests) la tâche s'exécute tout de suite sur le fil appelant : rien ne change.
//!
//! **Règle d'interblocage** : le fil principal ne doit jamais attendre le fil moteur (il ne le fait pas : les commandes de la page
//! arrivent par une file que le fil moteur vide). Si l'exécuteur disparaît (application qui se ferme), la tâche n'est pas exécutée
//! et `run` rend `None` plutôt que de bloquer.

use std::sync::mpsc;
use std::sync::OnceLock;

/// Poste une tâche sur le fil principal ; `false` si c'est impossible (boucle d'événements arrêtée).
pub type Runner = dyn Fn(Box<dyn FnOnce() + Send>) -> bool + Send + Sync;

static RUNNER: OnceLock<Box<Runner>> = OnceLock::new();

/// Installe l'exécuteur (une seule fois, au démarrage de l'application, avant tout appel d'interface de plugin).
pub fn set_runner(runner: Box<Runner>) {
    let _ = RUNNER.set(runner);
}

/// Vrai si un exécuteur est installé (les appels d'interface passent alors par le fil principal).
pub fn has_runner() -> bool {
    RUNNER.get().is_some()
}

#[cfg(target_os = "macos")]
fn on_process_main_thread() -> bool {
    extern "C" {
        fn pthread_main_np() -> i32;
    }
    // SAFETY: appel sans argument ni effet de bord.
    unsafe { pthread_main_np() != 0 }
}

#[cfg(not(target_os = "macos"))]
fn on_process_main_thread() -> bool {
    false
}

/// Exécute `f` sur le fil principal et rend son résultat. Sans exécuteur, ou déjà sur le fil principal : exécution directe.
/// `None` : l'exécuteur a refusé la tâche ou l'a abandonnée sans l'exécuter (la tâche n'a alors **pas** tourné).
pub fn run<R: Send, F: FnOnce() -> R + Send>(f: F) -> Option<R> {
    let Some(runner) = RUNNER.get() else { return Some(f()) };
    if on_process_main_thread() {
        return Some(f());
    }
    let (tx, rx) = mpsc::channel::<R>();
    let job: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
        let _ = tx.send(f());
    });
    // SAFETY: `f` peut emprunter l'appelant (`&Core`…), alors que l'exécuteur exige une tâche `'static`. On prolonge la durée de vie
    // **parce que cette fonction ne rend la main qu'après la fin de la tâche ou sa destruction** : `rx.recv()` revient soit avec le
    // résultat (tâche terminée), soit en erreur quand l'émetteur est détruit, c'est-à-dire quand la tâche a fini de s'exécuter ou
    // a été détruite sans l'être. Aucun délai d'attente n'abandonne la tâche pendant qu'elle emprunte l'appelant.
    let job: Box<dyn FnOnce() + Send + 'static> = unsafe { std::mem::transmute(job) };
    if !runner(job) {
        return None;
    }
    rx.recv().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_runner_the_task_runs_in_place() {
        // Pas d'exécuteur installé dans ce processus de test : exécution directe sur le fil appelant.
        let here = std::thread::current().id();
        assert_eq!(run(|| std::thread::current().id()), Some(here));
    }
}
