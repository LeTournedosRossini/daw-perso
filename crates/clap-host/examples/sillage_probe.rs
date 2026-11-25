//! Charge Sillage (chemin en argument), le fait traiter avec et sans chaîne latérale, transport en lecture, et liste ses paramètres.
use std::sync::Arc;
use daw_clap_host::{Bundle, Plugin};
fn main() {
    let path = std::env::args().nth(1).expect("chemin du .clap");
    let b = Arc::new(Bundle::open(std::path::Path::new(&path)).unwrap());
    for info in b.plugins().unwrap() {
        println!("{} ({}) {:?}", info.name, info.id, info.version);
        let mut p = Plugin::instantiate(&b, &info.id).unwrap();
        p.activate(48_000.0, 256).unwrap();
        p.start_processing().unwrap();
        println!("sidechain: {}  ports: {:?}", p.has_sidechain(), p.audio_ports());
        let h = p.handle();
        for q in h.params() { println!("  param {} {}", q.id, q.name); }
        let t = daw_engine::TransportInfo { playing: true, beats: 0.0, seconds: 0.0, tempo: 120.0, tsig_num: 4, tsig_den: 4, bar_start_beats: 0.0, bar_number: 1, loop_active: false, loop_start_beats: 0.0, loop_end_beats: 0.0, loop_start_seconds: 0.0, loop_end_seconds: 0.0 };
        let sig: Vec<f32> = (0..256).map(|i| (i as f32 * 0.1).sin() * 0.3).collect();
        for k in 0..400 {
            let tt = t.advanced(k * 256, 48_000);
            p.set_transport(&tt);
            let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
            let sc = if k % 2 == 0 { Some((&sig[..], &sig[..])) } else { None };
            p.process_with_sidechain(&sig, &sig, sc, &mut l, &mut r).unwrap();
        }
        println!("OK 400 blocs");
    }
}
