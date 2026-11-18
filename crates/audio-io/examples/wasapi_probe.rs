//! Essai du backend WASAPI (Windows, mode partagé) : liste les appareils puis joue un la 440 Hz pendant N secondes, avec la capture.
//! `cargo run -p daw-audio-io --example wasapi_probe -- [appareil de lecture ou "-"] [période] [secondes] [capture 0|1]`
#[cfg(target_os = "windows")]
fn main() {
    use daw_audio_io::playback::StereoProcessor;
    use daw_audio_io::wasapi_backend::{list_devices, run_playback_ex, PlayOptions};
    struct Sine(f32, f32);
    impl StereoProcessor for Sine {
        fn process(&mut self, in_l: &[f32], in_r: &[f32], l: &mut [f32], r: &mut [f32]) {
            for i in 0..l.len() {
                let v = (self.0 * std::f32::consts::TAU).sin() * 0.1;
                self.0 = (self.0 + 440.0 / 48_000.0).fract();
                self.1 = self.1.max(in_l[i].abs()).max(in_r[i].abs());
                l[i] = v;
                r[i] = v;
            }
        }
    }
    println!("Appareils WASAPI :");
    for d in list_devices() {
        println!("  {}{} {}{}", if d.playback { "Lecture " } else { "Capture " }, if d.default { "(défaut)" } else { "        " }, d.name, "");
    }
    let a: Vec<String> = std::env::args().skip(1).collect();
    let opts = PlayOptions {
        seconds: a.get(2).and_then(|s| s.parse().ok()).unwrap_or(5),
        device: a.first().filter(|s| s.as_str() != "-").cloned(),
        buffer: a.get(1).and_then(|s| s.parse().ok()),
        capture: a.get(3).map_or(true, |s| s != "0"),
        ..PlayOptions::default()
    };
    let r = run_playback_ex(&opts, |info| {
        println!("Ouvert : {} — {} Hz, période {}, {} entrée(s), {} sortie(s)", info.driver, info.sample_rate, info.buffer_size, info.inputs, info.outputs);
        let lat = info.latency.clone();
        let mut shown = false;
        (Sine(0.0, 0.0), Box::new(move |_| {
            if !shown {
                if let Some((i, o)) = lat.get() {
                    println!("Latences annoncées : entrée {i}, sortie {o} échantillons");
                    shown = true;
                }
            }
            true
        }))
    });
    match r {
        Ok(rep) => println!("Fini : dépassements {}, allocations dans le callback {}, traitement {:?}", rep.xruns, rep.callback_allocations, rep.timing.map(|t| (t.measured, t.interval_mean_ms, t.busy_max_ms))),
        Err(e) => println!("ERREUR : {e}"),
    }
}
#[cfg(not(target_os = "windows"))]
fn main() {}
