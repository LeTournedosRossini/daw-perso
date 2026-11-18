//! Essai du backend WASAPI (Windows, mode partagé) : liste les appareils puis joue un la 440 Hz pendant N secondes, avec la capture.
//! `cargo run -p daw-audio-io --example wasapi_probe -- [lecture : numéro, nom ou "-"] [période] [secondes] [capture 0|1] [capture : numéro, nom ou "-"]`
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
    let devices = list_devices();
    let outs: Vec<_> = devices.iter().filter(|d| d.playback).collect();
    let ins: Vec<_> = devices.iter().filter(|d| d.capture).collect();
    println!("Appareils de lecture :");
    for (i, d) in outs.iter().enumerate() {
        println!("  L{} {}{}", i + 1, d.name, if d.default { "   (par defaut)" } else { "" });
    }
    println!("Appareils de capture :");
    for (i, d) in ins.iter().enumerate() {
        println!("  C{} {}{}", i + 1, d.name, if d.default { "   (par defaut)" } else { "" });
    }
    // Un appareil se désigne par son numéro (L2, C3 ou 2, 3), par un morceau de son nom, ou "-" pour celui par défaut.
    let pick = |arg: Option<&String>, list: &[&daw_audio_io::wasapi_backend::DeviceEntry]| -> Option<String> {
        let a = arg?.trim();
        if a.is_empty() || a == "-" {
            return None;
        }
        let digits = a.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        match digits.parse::<usize>() {
            Ok(n) if n >= 1 && n <= list.len() => Some(list[n - 1].name.clone()),
            _ => Some(a.to_string()),
        }
    };
    let a: Vec<String> = std::env::args().skip(1).collect();
    let opts = PlayOptions {
        seconds: a.get(2).and_then(|s| s.parse().ok()).unwrap_or(5),
        device: pick(a.first(), &outs),
        capture_device: pick(a.get(4), &ins),
        buffer: a.get(1).and_then(|s| s.parse().ok()),
        capture: a.get(3).map_or(true, |s| s != "0"),
    };
    println!("Lecture choisie : {}", opts.device.as_deref().unwrap_or("par defaut"));
    println!("Capture choisie : {}", opts.capture_device.as_deref().unwrap_or("par defaut"));
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
