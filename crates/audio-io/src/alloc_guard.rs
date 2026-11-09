//! Garde-fou « aucune allocation dans le callback audio ».
//!
//! On installe [`GuardAllocator`] comme allocateur global (dans les tests et dans le CLI). Tout
//! `alloc`/`dealloc`/`realloc` effectué par un thread **pendant** qu'un [`RtScope`] est actif est
//! compté. On compte au lieu de paniquer : paniquer dans l'allocateur est lui-même dangereux.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

thread_local! {
    // Initialisation `const` et type sans destructeur : l'accès ne peut pas allouer.
    static IN_RT: Cell<bool> = const { Cell::new(false) };
}

static VIOLATIONS: AtomicU64 = AtomicU64::new(0);
/// Aide au diagnostic : le fil et la pile de la **première** violation depuis la dernière remise à zéro.
static FIRST: Mutex<Option<String>> = Mutex::new(None);
static CAPTURED: AtomicBool = AtomicBool::new(false);

#[inline]
fn note() {
    // `try_with` : pendant la destruction du thread local, on ne compte rien plutôt que de paniquer.
    if IN_RT.try_with(Cell::get).unwrap_or(false) {
        VIOLATIONS.fetch_add(1, Ordering::Relaxed);
        if !CAPTURED.swap(true, Ordering::Relaxed) {
            capture_first();
        }
    }
}

/// Enregistre le fil et la pile de la première violation. La zone est quittée le temps de la capture (qui alloue elle-même).
#[cold]
fn capture_first() {
    let _ = IN_RT.try_with(|f| f.set(false));
    let t = std::thread::current();
    let text = format!("fil « {} » ({:?})\n{}", t.name().unwrap_or("sans nom"), t.id(), std::backtrace::Backtrace::force_capture());
    if let Ok(mut g) = FIRST.lock() {
        *g = Some(text);
    }
    let _ = IN_RT.try_with(|f| f.set(true));
}

/// Le fil et la pile de la première allocation constatée dans une zone temps réel (depuis la dernière remise à zéro).
pub fn first_violation() -> Option<String> {
    FIRST.lock().ok().and_then(|g| g.clone())
}

/// Allocateur global qui délègue au système et compte les appels faits dans un [`RtScope`].
pub struct GuardAllocator;

// SAFETY: délégation directe à `System`, qui respecte le contrat de `GlobalAlloc` ; on n'ajoute
// qu'un compteur atomique et un accès à un thread local sans allocation.
unsafe impl GlobalAlloc for GuardAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        System.alloc_zeroed(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        System.realloc(ptr, layout, new_size)
    }
}

/// Zone « temps réel » : tant que la valeur vit, les allocations du thread courant sont comptées.
pub struct RtScope(());

impl RtScope {
    pub fn enter() -> Self {
        IN_RT.with(|f| f.set(true));
        RtScope(())
    }
}

impl Drop for RtScope {
    fn drop(&mut self) {
        IN_RT.with(|f| f.set(false));
    }
}

/// Nombre total d'allocations/libérations constatées dans des zones temps réel.
pub fn violations() -> u64 {
    VIOLATIONS.load(Ordering::Relaxed)
}

pub fn reset_violations() {
    VIOLATIONS.store(0, Ordering::Relaxed);
    CAPTURED.store(false, Ordering::Relaxed);
    if let Ok(mut g) = FIRST.lock() {
        *g = None;
    }
}
