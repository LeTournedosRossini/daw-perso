//! `Plugin::process` et `queue_param` n'allouent pas (avec le plugin de test, qui n'alloue pas non plus).

use daw_audio_io::alloc_guard::{reset_violations, violations, GuardAllocator, RtScope};
use daw_clap_host::{Bundle, Plugin};
use std::sync::Arc;

#[global_allocator]
static ALLOC: GuardAllocator = GuardAllocator;

#[test]
fn process_and_param_queue_never_allocate() {
    let bundle = Arc::new(Bundle::from_static_entry(daw_test_plugin::entry()).unwrap());
    let mut p = Plugin::instantiate(&bundle, "com.daw-perso.test-gain").unwrap();
    p.activate(48_000.0, 64).unwrap();
    p.start_processing().unwrap();
    let (input, mut out_l, mut out_r) = (vec![0.5f32; 64], vec![0.0f32; 64], vec![0.0f32; 64]);

    reset_violations();
    {
        let _rt = RtScope::enter();
        for i in 0..5000u32 {
            p.queue_param(0, (i % 20) as f64 * 0.1, i % 64);
            p.process(&input, &input, &mut out_l, &mut out_r).unwrap();
        }
    }
    assert_eq!(violations(), 0, "allocation dans process()");
    assert!(out_l.iter().any(|&s| s != 0.0));
}
