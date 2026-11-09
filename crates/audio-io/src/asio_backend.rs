//! Backend ASIO (Windows). Passe par la couche C++ fine `native/asio_shim.cpp`.
//!
//! Tout le `unsafe` du crate pour ASIO est ici : déclarations FFI, un wrapper propriétaire du pilote
//! (`Driver`, libéré au `Drop`) et le trampoline appelé depuis le thread du pilote.

#[cfg(daw_asio)]
mod imp {
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::alloc_guard::{self, RtScope};
    use crate::latency::{Event, LatencyReport, Rig, Stats};
    use crate::playback::{ControlFn, PlaybackReport, StereoProcessor, StreamInfo, StreamLatency, TimedProcessor};
    use crate::spsc::Spsc;

    #[repr(C)]
    struct RawDriver {
        name: [c_char; 128],
        clsid: [c_char; 64],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RawInfo {
        input_channels: i32,
        output_channels: i32,
        buffer_min: i32,
        buffer_max: i32,
        buffer_preferred: i32,
        buffer_granularity: i32,
        input_latency: i32,
        output_latency: i32,
        sample_rate: f64,
        driver_name: [c_char; 64],
    }

    #[repr(C)]
    struct RawAsio {
        _private: [u8; 0],
    }

    type ProcessFn = extern "C" fn(*mut c_void, u32, *const *const f32, *const *mut f32);

    extern "C" {
        fn daw_asio_list(out: *mut RawDriver, max: i32) -> i32;
        fn daw_asio_open(clsid: *const c_char, err: *mut c_char, err_len: i32) -> *mut RawAsio;
        fn daw_asio_info(a: *mut RawAsio, out: *mut RawInfo);
        fn daw_asio_start(
            a: *mut RawAsio,
            sample_rate: f64,
            buffer_size: i32,
            in_ch: *const i32,
            n_in: i32,
            out_ch: *const i32,
            n_out: i32,
            f: ProcessFn,
            user: *mut c_void,
            err: *mut c_char,
            err_len: i32,
        ) -> i32;
        fn daw_asio_buffer_size(a: *const RawAsio) -> i32;
        fn daw_asio_sample_rate(a: *const RawAsio) -> f64;
        fn daw_asio_latencies(a: *mut RawAsio, input: *mut i32, output: *mut i32);
        fn daw_asio_overloads(a: *const RawAsio) -> u32;
        fn daw_asio_stop(a: *mut RawAsio);
        fn daw_asio_close(a: *mut RawAsio);
    }

    fn cstr(buf: &[c_char]) -> String {
        // SAFETY: les tampons venant du C sont toujours terminés par NUL (snprintf / init à zéro).
        unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
    }

    #[derive(Debug, Clone)]
    pub struct DriverEntry {
        pub name: String,
        pub clsid: String,
    }

    pub fn list_drivers() -> Vec<DriverEntry> {
        let mut raw: Vec<RawDriver> = (0..64)
            // SAFETY: structure de tableaux de `c_char` : tout-zéro est une valeur valide.
            .map(|_| unsafe { std::mem::zeroed() })
            .collect();
        // SAFETY: `raw` a 64 éléments valides, et on passe 64 comme maximum.
        let n = unsafe { daw_asio_list(raw.as_mut_ptr(), 64) };
        raw.iter().take(n.max(0) as usize).map(|d| DriverEntry { name: cstr(&d.name), clsid: cstr(&d.clsid) }).collect()
    }

    #[derive(Debug, Clone)]
    pub struct DriverInfo {
        pub name: String,
        pub input_channels: u32,
        pub output_channels: u32,
        pub buffer_min: u32,
        pub buffer_max: u32,
        pub buffer_preferred: u32,
        pub input_latency: u32,
        pub output_latency: u32,
        pub sample_rate: f64,
    }

    /// Pilote ASIO ouvert (initialisé, flux non démarré). Libéré à la destruction.
    pub struct Driver {
        raw: *mut RawAsio,
        info: DriverInfo,
    }

    impl Driver {
        /// `query` : fragment du nom du pilote (insensible à la casse).
        pub fn open(query: &str) -> Result<Driver, String> {
            let q = query.to_lowercase();
            let entry = list_drivers()
                .into_iter()
                .find(|d| d.name.to_lowercase().contains(&q))
                .ok_or_else(|| format!("aucun pilote ASIO ne correspond à « {query} » (voir `devices`)"))?;
            let clsid = CString::new(entry.clsid).map_err(|e| e.to_string())?;
            let mut err = [0 as c_char; 256];
            // SAFETY: `clsid` est une chaîne C valide ; `err` a la taille annoncée.
            let raw = unsafe { daw_asio_open(clsid.as_ptr(), err.as_mut_ptr(), err.len() as i32) };
            if raw.is_null() {
                return Err(format!("{} : {}", entry.name, cstr(&err)));
            }
            let mut ri = std::mem::MaybeUninit::<RawInfo>::uninit();
            // SAFETY: `raw` est un pilote ouvert ; `daw_asio_info` remplit entièrement la structure.
            let ri = unsafe {
                daw_asio_info(raw, ri.as_mut_ptr());
                ri.assume_init()
            };
            let info = DriverInfo {
                name: cstr(&ri.driver_name),
                input_channels: ri.input_channels.max(0) as u32,
                output_channels: ri.output_channels.max(0) as u32,
                buffer_min: ri.buffer_min.max(0) as u32,
                buffer_max: ri.buffer_max.max(0) as u32,
                buffer_preferred: ri.buffer_preferred.max(0) as u32,
                input_latency: ri.input_latency.max(0) as u32,
                output_latency: ri.output_latency.max(0) as u32,
                sample_rate: ri.sample_rate,
            };
            Ok(Driver { raw, info })
        }

        pub fn info(&self) -> &DriverInfo {
            &self.info
        }
    }

    impl Drop for Driver {
        fn drop(&mut self) {
            // SAFETY: `raw` vient de `daw_asio_open` et n'est libéré qu'ici.
            unsafe { daw_asio_close(self.raw) };
        }
    }

    extern "C" fn trampoline(user: *mut c_void, frames: u32, inputs: *const *const f32, outputs: *const *mut f32) {
        // Un panic ne doit jamais traverser la frontière C : on le contient.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _rt = RtScope::enter();
            // SAFETY: `user` est le `Rig` passé à `daw_asio_start`, vivant jusqu'à `daw_asio_stop` ;
            // le shim appelle ce rappel depuis un seul thread à la fois. `inputs[0]`/`outputs[0]` pointent
            // vers `frames` flottants valides pendant cet appel (un canal d'entrée et un de sortie).
            let (p, input, output) = unsafe {
                (
                    &mut *(user as *mut Rig),
                    std::slice::from_raw_parts(*inputs, frames as usize),
                    std::slice::from_raw_parts_mut(*outputs, frames as usize),
                )
            };
            p.process(input, output);
        }));
    }

    #[derive(Debug, Clone, Default)]
    pub struct LatencyOptions {
        pub seconds: u32,
        /// Fragment du nom du pilote (par défaut « UMC »).
        pub driver: String,
        pub buffer: Option<u32>,
        pub sample_rate: Option<u32>,
        /// Canaux à partir de 1, comme sur la face avant.
        pub input_channel: u32,
        pub output_channel: u32,
    }

    pub fn run_latency_test(opts: &LatencyOptions) -> Result<LatencyReport, String> {
        let driver = Driver::open(&opts.driver)?;
        let info = driver.info().clone();
        if opts.input_channel == 0 || opts.input_channel > info.input_channels {
            return Err(format!("entrée {} inexistante (le pilote en a {})", opts.input_channel, info.input_channels));
        }
        if opts.output_channel == 0 || opts.output_channel > info.output_channels {
            return Err(format!("sortie {} inexistante (le pilote en a {})", opts.output_channel, info.output_channels));
        }
        let (in_ch, out_ch) = ([opts.input_channel as i32 - 1], [opts.output_channel as i32 - 1]);

        // Fréquence connue avant de démarrer : la sonde en a besoin pour ses délais.
        let rate = opts.sample_rate.map(f64::from).unwrap_or(if info.sample_rate > 0.0 { info.sample_rate } else { 48_000.0 });
        let queue = Arc::new(Spsc::new(4096));
        let mut process = Box::new(Rig::new(rate as u32, Arc::clone(&queue)));
        let mut err = [0 as c_char; 256];
        alloc_guard::reset_violations();
        // SAFETY: les tableaux de canaux ont 1 élément (n_in = n_out = 1) ; `process` reste vivant (boîte
        // gardée dans cette fonction) jusqu'après `daw_asio_stop` ; `trampoline` respecte le contrat du rappel.
        let rc = unsafe {
            daw_asio_start(
                driver.raw,
                rate,
                opts.buffer.unwrap_or(0) as i32,
                in_ch.as_ptr(),
                1,
                out_ch.as_ptr(),
                1,
                trampoline,
                (&mut *process as *mut Rig).cast(),
                err.as_mut_ptr(),
                err.len() as i32,
            )
        };
        if rc != 0 {
            return Err(format!("{} : {}", info.name, cstr(&err)));
        }

        let (mut latencies, mut lost) = (Vec::new(), 0usize);
        let end = Instant::now() + Duration::from_secs(opts.seconds.max(1) as u64);
        while Instant::now() < end {
            while let Some(v) = queue.pop() {
                match Event::decode(v) {
                    Event::Latency(n) => latencies.push(n),
                    Event::Lost => lost += 1,
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let callback_allocations = alloc_guard::violations();
        // SAFETY: `driver.raw` est valide ; après `stop`, plus aucun rappel ne touche `process`.
        let (buffer_size, sample_rate, xruns, reported) = unsafe {
            // Les latences ne sont fiables qu'une fois les tampons créés (spécification ASIO).
            let (mut il, mut ol) = (0i32, 0i32);
            daw_asio_latencies(driver.raw, &mut il, &mut ol);
            let r = (daw_asio_buffer_size(driver.raw), daw_asio_sample_rate(driver.raw), daw_asio_overloads(driver.raw), (il.max(0) as u32, ol.max(0) as u32));
            daw_asio_stop(driver.raw);
            r
        };
        let timing = process.timing();
    drop(process);

        let skip = usize::from(latencies.len() > 2);
        Ok(LatencyReport {
            sample_rate: sample_rate as u32,
            buffer_size: buffer_size as u32,
            input: format!("{} entrée {}", info.name, opts.input_channel),
            output: format!("{} sortie {}", info.name, opts.output_channel),
            stats: Stats::from_samples(&latencies[skip..], lost),
            lost,
            xruns,
            callback_allocations,
            reported_latency: Some(reported),
        timing,
        })
    }

    /// Nombre maximal de canaux d'entrée qu'un flux ouvre (la taille du tableau de tampons du rappel, sur la pile).
    const MAX_STREAM_INPUTS: usize = 32;
    /// Nombre maximal de canaux de sortie qu'un flux ouvre (même raison).
    const MAX_STREAM_OUTPUTS: usize = 32;

    extern "C" fn play_trampoline<P: StereoProcessor>(user: *mut c_void, frames: u32, inputs: *const *const f32, outputs: *const *mut f32) {
        let ok = catch_unwind(AssertUnwindSafe(|| {
            let _rt = RtScope::enter();
            // SAFETY: `user` est le `AsioPlay<P>` passé à `daw_asio_start`, vivant jusqu'à `daw_asio_stop` ; le shim
            // appelle ce rappel depuis un seul thread à la fois. `outputs[0]` et `outputs[1]` pointent vers `frames`
            // flottants valides (les deux canaux de sortie demandés) ; `inputs[0..n_in]` aussi (les canaux d'entrée
            // demandés, `n_in <= MAX_STREAM_INPUTS`). Les canaux qui manquent pour atteindre deux entrées lisent `silence`,
            // dimensionné au plus grand tampon du pilote et jamais modifié.
            unsafe {
                let play = user as *mut AsioPlay<P>;
                let n = frames as usize;
                let silence = std::slice::from_raw_parts((*play).silence.as_ptr(), n.min((*play).silence.len()));
                let mut view: [&[f32]; MAX_STREAM_INPUTS] = [&[]; MAX_STREAM_INPUTS];
                let n_in = (*play).n_in;
                for (c, slot) in view.iter_mut().enumerate().take(n_in) {
                    *slot = std::slice::from_raw_parts(*inputs.add(c), n);
                }
                let count = n_in.max(2);
                for slot in view.iter_mut().take(count).skip(n_in) {
                    *slot = silence;
                }
                // Tous les canaux de sortie ouverts (au moins deux), du pilote lui-même : un tableau sur la pile, sans allocation.
                let n_out = (*play).n_out.max(2);
                let mut outs: [&mut [f32]; MAX_STREAM_OUTPUTS] = std::array::from_fn(|_| &mut [][..]);
                for (c, slot) in outs.iter_mut().enumerate().take(n_out) {
                    *slot = std::slice::from_raw_parts_mut(*outputs.add(c), n);
                }
                // `view` ne recouvre que `silence` ou la mémoire du pilote, jamais `tp` ; `play` est le seul accès modifiable.
                (*play).tp.process_outputs(&view[..count], &mut outs[..n_out]);
            }
        }))
        .is_ok();
        if !ok {
            // SAFETY: mêmes pointeurs et mêmes durées de validité que ci-dessus. Silence plutôt que du bruit.
            unsafe {
                let n_out = (*(user as *mut AsioPlay<P>)).n_out.max(2);
                for c in 0..n_out {
                    std::slice::from_raw_parts_mut(*outputs.add(c), frames as usize).fill(0.0);
                }
            }
        }
    }

    /// État passé au rappel : le processeur chronométré, le nombre d'entrées ouvertes et du silence pour compléter.
    struct AsioPlay<P> {
        tp: TimedProcessor<P>,
        n_in: usize,
        /// Nombre de canaux de sortie ouverts (au moins 2).
        n_out: usize,
        silence: Vec<f32>,
    }

    #[derive(Debug, Clone, Default)]
    pub struct PlayOptions {
        pub seconds: u32,
        pub driver: String,
        pub buffer: Option<u32>,
        pub sample_rate: Option<u32>,
        /// Première sortie (à partir de 1) ; la suivante sert de canal droit.
        pub output_channel: u32,
        /// Première entrée (à partir de 1). 0 : aucune entrée ouverte.
        pub input_channel: u32,
        /// Nombre d'entrées consécutives ouvertes à partir de `input_channel` (au plus 32, et au plus ce que le pilote a).
        /// 0 : comportement historique, deux entrées si `input_channel > 0`.
        pub input_count: u32,
        /// Nombre de sorties consécutives ouvertes à partir de `output_channel` (plafonné à ce que le pilote a et à 32) : 0 ou 2 = la
        /// paire principale seule ; plus : les canaux suivants servent aux retours de monitoring.
        pub output_count: u32,
    }

    pub fn run_playback<P: StereoProcessor>(opts: &PlayOptions, make: impl FnOnce(u32) -> (P, ControlFn)) -> Result<PlaybackReport, String> {
        run_playback_ex(opts, |info| make(info.sample_rate))
    }

    /// Comme `run_playback`, avec ce que le backend sait du flux (pilote, fréquence, taille de tampon, nombre d'entrées, latences
    /// à lire plus tard) au moment de créer le processeur.
    pub fn run_playback_ex<P: StereoProcessor>(opts: &PlayOptions, make: impl FnOnce(&StreamInfo) -> (P, ControlFn)) -> Result<PlaybackReport, String> {
        let driver = Driver::open(&opts.driver)?;
        let info = driver.info().clone();
        let first = opts.output_channel.max(1);
        if first + 1 > info.output_channels {
            return Err(format!("sorties {first} et {} inexistantes (le pilote en a {})", first + 1, info.output_channels));
        }
        let available_out = info.output_channels - (first - 1);
        let n_out = (if opts.output_count == 0 { 2 } else { opts.output_count.max(2) }).min(available_out).min(MAX_STREAM_OUTPUTS as u32) as usize;
        let out_ch: Vec<i32> = (0..n_out as i32).map(|k| first as i32 - 1 + k).collect();
        let rate = opts.sample_rate.map(f64::from).unwrap_or(if info.sample_rate > 0.0 { info.sample_rate } else { 48_000.0 });
        // Entrées : `input_count` canaux consécutifs (plafonnés à ce que le pilote a), ou deux si on ne précise pas.
        let n_in: usize = if opts.input_channel == 0 {
            0
        } else if opts.input_count == 0 {
            if opts.input_channel + 1 > info.input_channels {
                return Err(format!("entrées {} et {} inexistantes (le pilote en a {})", opts.input_channel, opts.input_channel + 1, info.input_channels));
            }
            2
        } else {
            let available = info.input_channels.saturating_sub(opts.input_channel - 1);
            if available == 0 {
                return Err(format!("entrée {} inexistante (le pilote en a {})", opts.input_channel, info.input_channels));
            }
            opts.input_count.min(available).min(MAX_STREAM_INPUTS as u32) as usize
        };
        let in_ch: Vec<i32> = (0..n_in as i32).map(|k| opts.input_channel as i32 - 1 + k).collect();
        let buffer_size = opts.buffer.filter(|&b| b > 0).unwrap_or(info.buffer_preferred);
        let latency = Arc::new(StreamLatency::default());
        let stream = StreamInfo { driver: info.name.clone(), sample_rate: rate as u32, buffer_size, inputs: n_in as u32, outputs: n_out as u32, latency: Arc::clone(&latency) };
        let (processor, mut control) = make(&stream);
        let mut tp = Box::new(AsioPlay { tp: TimedProcessor::new(processor, rate as u32), n_in, n_out, silence: vec![0.0; info.buffer_max.max(1) as usize] });
        let mut err = [0 as c_char; 256];
        alloc_guard::reset_violations();
        // SAFETY: `out_ch` a `n_out` éléments, `in_ch` en a `n_in` ; `tp` reste vivant (boîte gardée ici) jusqu'après
        // `daw_asio_stop` ; `play_trampoline::<P>` respecte le contrat du rappel.
        let rc = unsafe {
            daw_asio_start(
                driver.raw,
                rate,
                opts.buffer.unwrap_or(0) as i32,
                in_ch.as_ptr(),
                n_in as i32,
                out_ch.as_ptr(),
                n_out as i32,
                play_trampoline::<P>,
                (&mut *tp as *mut AsioPlay<P>).cast(),
                err.as_mut_ptr(),
                err.len() as i32,
            )
        };
        if rc != 0 {
            return Err(format!("{} : {}", info.name, cstr(&err)));
        }
        // Les latences ne sont fiables qu'une fois les tampons créés : on les publie maintenant pour le contrôle.
        // SAFETY: `driver.raw` est valide et le flux est démarré ; deux entiers valides où écrire.
        unsafe {
            let (mut il, mut ol) = (0i32, 0i32);
            daw_asio_latencies(driver.raw, &mut il, &mut ol);
            latency.set(il.max(0) as u32, ol.max(0) as u32);
        }
        let start = Instant::now();
        let end = Duration::from_secs(opts.seconds.max(1) as u64);
        loop {
            let elapsed = start.elapsed();
            if elapsed >= end || !control(elapsed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let callback_allocations = alloc_guard::violations();
        // SAFETY: `driver.raw` est valide ; après `stop`, plus aucun rappel ne touche `tp`.
        let (buffer_size, sample_rate, xruns) = unsafe {
            let r = (daw_asio_buffer_size(driver.raw), daw_asio_sample_rate(driver.raw), daw_asio_overloads(driver.raw));
            daw_asio_stop(driver.raw);
            r
        };
        let timing = tp.tp.timing();
        drop(tp);
        Ok(PlaybackReport {
            sample_rate: sample_rate as u32,
            buffer_size: buffer_size as u32,
            output: if n_out > 2 { format!("{} sorties {} à {}", info.name, first, first + n_out as u32 - 1) } else { format!("{} sorties {} + {}", info.name, first, first + 1) },
            timing,
            xruns,
            callback_allocations,
        })
    }
}

/// Version sans SDK ASIO (build.rs n'a pas trouvé `ASIO_SDK_DIR`) : même API, mais tout échoue proprement.
#[cfg(not(daw_asio))]
mod imp {
    use crate::latency::LatencyReport;

    const MSG: &str = "backend ASIO absent de cette compilation : définir ASIO_SDK_DIR (dossier du SDK Steinberg) et recompiler";

    #[derive(Debug, Clone)]
    pub struct DriverEntry {
        pub name: String,
        pub clsid: String,
    }
    #[derive(Debug, Clone)]
    pub struct DriverInfo {
        pub name: String,
        pub input_channels: u32,
        pub output_channels: u32,
        pub buffer_min: u32,
        pub buffer_max: u32,
        pub buffer_preferred: u32,
        pub input_latency: u32,
        pub output_latency: u32,
        pub sample_rate: f64,
    }
    pub struct Driver(DriverInfo);
    impl Driver {
        pub fn open(_: &str) -> Result<Driver, String> {
            Err(MSG.into())
        }
        pub fn info(&self) -> &DriverInfo {
            &self.0
        }
    }
    pub fn list_drivers() -> Vec<DriverEntry> {
        Vec::new()
    }
    #[derive(Debug, Clone, Default)]
    pub struct LatencyOptions {
        pub seconds: u32,
        pub driver: String,
        pub buffer: Option<u32>,
        pub sample_rate: Option<u32>,
        pub input_channel: u32,
        pub output_channel: u32,
    }
    pub fn run_latency_test(_: &LatencyOptions) -> Result<LatencyReport, String> {
        Err(MSG.into())
    }
    use crate::playback::{ControlFn, PlaybackReport, StereoProcessor, StreamInfo};
    #[derive(Debug, Clone, Default)]
    pub struct PlayOptions {
        pub seconds: u32,
        pub driver: String,
        pub buffer: Option<u32>,
        pub sample_rate: Option<u32>,
        pub output_channel: u32,
        pub input_channel: u32,
        pub input_count: u32,
        pub output_count: u32,
    }
    pub fn run_playback<P: StereoProcessor>(_: &PlayOptions, _: impl FnOnce(u32) -> (P, ControlFn)) -> Result<PlaybackReport, String> {
        Err(MSG.into())
    }
    pub fn run_playback_ex<P: StereoProcessor>(_: &PlayOptions, _: impl FnOnce(&StreamInfo) -> (P, ControlFn)) -> Result<PlaybackReport, String> {
        Err(MSG.into())
    }
}

pub use imp::*;
