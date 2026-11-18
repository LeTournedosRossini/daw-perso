//! Essai du backend ALSA : liste les périphériques puis joue un la 440 Hz pendant N secondes (périphérique et tampon au choix).
//! `cargo run -p daw-audio-io --example alsa_probe -- [périphérique] [tampon] [secondes]`
#[cfg(target_os = "linux")]
fn main() {
    use daw_audio_io::alsa_backend::{list_devices, run_playback_ex, PlayOptions};
    use daw_audio_io::playback::StereoProcessor;
    struct Sine(f32);
    impl StereoProcessor for Sine {
        fn process(&mut self, in_l: &[f32], _: &[f32], l: &mut [f32], r: &mut [f32]) {
            for i in 0..l.len() {
                let v = (self.0 * std::f32::consts::TAU).sin() * 0.1;
                self.0 = (self.0 + 440.0 / 48_000.0).fract();
                l[i] = v + in_l[i] * 0.0;
                r[i] = v;
            }
        }
    }
    println!("Périphériques ALSA :");
    for d in list_devices() {
        println!("  {:<28} {}{} {}", d.name, if d.playback { "L" } else { "-" }, if d.capture { "C" } else { "-" }, d.description);
    }
    let a: Vec<String> = std::env::args().skip(1).collect();
    let opts = PlayOptions {
        seconds: a.get(2).and_then(|s| s.parse().ok()).unwrap_or(4),
        device: a.first().cloned(),
        buffer: a.get(1).and_then(|s| s.parse().ok()),
        input_count: a.get(3).and_then(|s| s.parse().ok()).unwrap_or(2),
        ..PlayOptions::default()
    };
    let r = run_playback_ex(&opts, |info| {
        println!("Ouvert : {} — {} Hz, période {}, {} entrée(s), {} sortie(s)", info.driver, info.sample_rate, info.buffer_size, info.inputs, info.outputs);
        let lat = info.latency.clone();
        let mut shown = false;
        (Sine(0.0), Box::new(move |_| {
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
        Ok(rep) => println!("Fini : xruns {}, allocations dans le callback {}, traitement {:?}", rep.xruns, rep.callback_allocations, rep.timing.map(|t| (t.measured, t.interval_mean_ms, t.busy_max_ms))),
        Err(e) => println!("ERREUR : {e}"),
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {}
