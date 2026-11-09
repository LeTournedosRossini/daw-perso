//! Garde-fou : le cœur du callback de mesure ne doit ni allouer ni libérer.

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};
use daw_audio_io::latency::Rig;
use daw_audio_io::spsc::Spsc;
use std::sync::Arc;

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;

// Un seul test : le compteur est global au processus.
#[test]
fn probe_does_not_allocate_and_guard_detects_allocation() {
    let sr = 48_000;
    let queue = Arc::new(Spsc::new(1024));
    let mut rig = Rig::new(sr, Arc::clone(&queue));
    let input = vec![0.0f32; 64];
    let mut output = vec![0.0f32; 64];

    reset_violations();
    {
        let _rt = RtScope::enter();
        // 10 s de callbacks de 64 échantillons.
        for _ in 0..(sr as usize * 10 / 64) {
            rig.process(&input, &mut output);
        }
    }
    assert_eq!(violations(), 0, "allocation dans le callback");
    assert!(rig.timing().is_some(), "le chronométrage a bien tourné");
    assert!(queue.pop().is_some(), "des événements (impulsions perdues) ont bien été produits");

    // Contre-épreuve : le garde-fou doit voir une vraie allocation.
    reset_violations();
    {
        let _rt = RtScope::enter();
        let v: Vec<u8> = Vec::with_capacity(4096);
        std::hint::black_box(&v);
    }
    assert!(violations() >= 1, "le garde-fou n'a pas détecté l'allocation");
}
