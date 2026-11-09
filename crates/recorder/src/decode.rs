//! Lecture de fichiers audio à l'import : WAV (entier 8/16/24/32 bits, flottant 32/64 bits, « extensible »),
//! FLAC (claxon) et MP3 (symphonia). Tout est ramené à des échantillons `f32` entrelacés dans [-1, 1].
//!
//! Hors du thread audio uniquement (allocations, entrées/sorties). Aucune panique sur un fichier abîmé :
//! erreur `InvalidData`. Un MP3 est décodé jusqu'au bout ; un paquet corrompu est sauté (compté dans
//! [`Decoded::skipped_packets`]) plutôt que d'abandonner tout le fichier.

use std::fs::File;
use std::io::{self, Cursor, Read};
use std::path::Path;

use crate::wav::WavData;

/// Format reconnu d'après le contenu du fichier (jamais d'après son extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Wav,
    Flac,
    Mp3,
}

/// Résultat d'un décodage.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub audio: WavData,
    pub format: Format,
    /// Profondeur d'origine (16, 24, 32…) ; 0 pour un format à pertes (MP3).
    pub source_bits: u16,
    /// Paquets illisibles sautés (MP3).
    pub skipped_packets: u32,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Reconnaît le format d'après les premiers octets.
pub fn sniff(b: &[u8]) -> Option<Format> {
    if b.len() >= 12 && (&b[0..4] == b"RIFF" || &b[0..4] == b"RF64") && &b[8..12] == b"WAVE" {
        Some(Format::Wav)
    } else if b.len() >= 4 && &b[0..4] == b"fLaC" {
        Some(Format::Flac)
    } else if b.len() >= 3 && (&b[0..3] == b"ID3" || (b[0] == 0xFF && b[1] & 0xE0 == 0xE0)) {
        Some(Format::Mp3)
    } else {
        None
    }
}

/// Ce qu'on sait d'un fichier sans le décoder en entier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub format: Format,
    pub sample_rate: u32,
    pub channels: u16,
    /// Trames (par canal), si l'en-tête les annonce (un MP3 sans en-tête de durée : `None`).
    pub frames: Option<u64>,
}

/// Lit seulement le début du fichier (en-têtes) : format, fréquence, canaux, durée si connue.
pub fn probe_file(path: &Path) -> io::Result<Probe> {
    let len = std::fs::metadata(path)?.len();
    let mut head = Vec::new();
    File::open(path)?.take(256 * 1024).read_to_end(&mut head)?;
    probe_bytes(&head, len)
}

/// `head` : le début du fichier ; `file_len` : sa taille totale (pour la durée d'un WAV).
pub fn probe_bytes(head: &[u8], file_len: u64) -> io::Result<Probe> {
    match sniff(head).ok_or_else(|| bad("format non reconnu (WAV, FLAC et MP3 sont gérés)"))? {
        Format::Wav => {
            let mut pos = 12usize;
            let mut fmt = None;
            while pos + 8 <= head.len() {
                let size = u32le(head, pos + 4) as usize;
                let body = pos + 8;
                if &head[pos..pos + 4] == b"fmt " && body + 16 <= head.len() {
                    fmt = Some((u16le(head, body + 2), u32le(head, body + 4), u16le(head, body + 14)));
                } else if &head[pos..pos + 4] == b"data" {
                    let (channels, rate, bits) = fmt.ok_or_else(|| bad("bloc data avant fmt"))?;
                    let width = (bits as u64).div_ceil(8) * channels as u64;
                    let avail = file_len.saturating_sub(body as u64);
                    let bytes = if size == 0xFFFF_FFFF { avail } else { (size as u64).min(avail) };
                    return Ok(Probe { format: Format::Wav, sample_rate: rate, channels, frames: (width > 0).then(|| bytes / width) });
                }
                pos = body.saturating_add(size).saturating_add(size & 1);
            }
            Err(bad("WAV : en-tête incomplet"))
        }
        Format::Flac => {
            let r = claxon::FlacReader::new(Cursor::new(head)).map_err(|e| bad(format!("FLAC : {e}")))?;
            let i = r.streaminfo();
            Ok(Probe { format: Format::Flac, sample_rate: i.sample_rate, channels: i.channels as u16, frames: i.samples })
        }
        Format::Mp3 => {
            use symphonia::core::formats::FormatOptions;
            use symphonia::core::io::MediaSourceStream;
            use symphonia::core::meta::MetadataOptions;
            use symphonia::core::probe::Hint;
            let mss = MediaSourceStream::new(Box::new(Cursor::new(head.to_vec())), Default::default());
            let mut hint = Hint::new();
            hint.with_extension("mp3");
            let probed = symphonia::default::get_probe().format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default()).map_err(|e| bad(format!("MP3 : {e}")))?;
            let p = &probed.format.default_track().ok_or_else(|| bad("MP3 : aucune piste"))?.codec_params;
            Ok(Probe { format: Format::Mp3, sample_rate: p.sample_rate.ok_or_else(|| bad("MP3 : fréquence inconnue"))?, channels: p.channels.map(|c| c.count() as u16).unwrap_or(2), frames: p.n_frames })
        }
    }
}

pub fn decode_file(path: &Path) -> io::Result<Decoded> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    decode_bytes(&bytes).map_err(|e| io::Error::new(e.kind(), format!("{} : {e}", path.display())))
}

pub fn decode_bytes(bytes: &[u8]) -> io::Result<Decoded> {
    match sniff(bytes).ok_or_else(|| bad("format non reconnu (WAV, FLAC et MP3 sont gérés)"))? {
        Format::Wav => decode_wav(bytes),
        Format::Flac => decode_flac(bytes),
        Format::Mp3 => decode_mp3(bytes),
    }
}

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn decode_wav(b: &[u8]) -> io::Result<Decoded> {
    let (mut pos, mut fmt) = (12usize, None::<(u16, u16, u32, u16)>);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32le(b, pos + 4) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= b.len() {
            let mut tag = u16le(b, body);
            let (channels, rate, bits) = (u16le(b, body + 2), u32le(b, body + 4), u16le(b, body + 14));
            if tag == 0xFFFE {
                // WAVE_FORMAT_EXTENSIBLE : le vrai format est dans les 2 premiers octets du sous-format.
                if size < 26 || body + 26 > b.len() {
                    return Err(bad("bloc fmt « extensible » tronqué"));
                }
                tag = u16le(b, body + 24);
            }
            fmt = Some((tag, channels, rate, bits));
        } else if id == b"data" {
            let (tag, channels, rate, bits) = fmt.ok_or_else(|| bad("bloc data avant fmt"))?;
            if channels == 0 || rate == 0 {
                return Err(bad("fmt invalide (0 canal ou 0 Hz)"));
            }
            let width = (bits as usize).div_ceil(8);
            let end = if size == 0xFFFF_FFFF { b.len() } else { (body + size).min(b.len()) };
            let frame = width * channels as usize;
            if width == 0 || frame == 0 {
                return Err(bad("fmt invalide"));
            }
            let usable = (end - body) / frame * frame;
            let raw = &b[body..body + usable];
            let samples: Vec<f32> = match (tag, bits) {
                (1, 8) => raw.iter().map(|&v| (v as f32 - 128.0) / 128.0).collect(),
                (1, 16) => raw.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0).collect(),
                (1, 24) => raw
                    .chunks_exact(3)
                    .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
                    .collect(),
                (1, 32) => raw.chunks_exact(4).map(|c| (i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64 / 2_147_483_648.0) as f32).collect(),
                (3, 32) => raw.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
                (3, 64) => raw.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap()) as f32).collect(),
                _ => return Err(bad(format!("WAV non géré (type {tag}, {bits} bits)"))),
            };
            let samples = samples.into_iter().map(|v| if v.is_finite() { v } else { 0.0 }).collect();
            return Ok(Decoded { audio: WavData { sample_rate: rate, channels, samples }, format: Format::Wav, source_bits: bits, skipped_packets: 0 });
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    Err(bad("pas de bloc data"))
}

fn decode_flac(b: &[u8]) -> io::Result<Decoded> {
    let mut r = claxon::FlacReader::new(Cursor::new(b)).map_err(|e| bad(format!("FLAC : {e}")))?;
    let info = r.streaminfo();
    let (rate, channels, bits) = (info.sample_rate, info.channels as u16, info.bits_per_sample);
    if channels == 0 || rate == 0 || !(1..=32).contains(&bits) {
        return Err(bad("FLAC : en-tête invalide"));
    }
    let scale = 1.0 / (1u64 << (bits - 1)) as f64;
    let mut samples = Vec::with_capacity(info.samples.unwrap_or(0) as usize * channels as usize);
    for s in r.samples() {
        samples.push((s.map_err(|e| bad(format!("FLAC : {e}")))? as f64 * scale) as f32);
    }
    samples.truncate(samples.len() / channels as usize * channels as usize);
    // STREAMINFO donne la vraie longueur : certains encodeurs complètent le dernier bloc avec du silence.
    if let Some(n) = info.samples {
        samples.truncate((n as usize).saturating_mul(channels as usize));
    }
    Ok(Decoded { audio: WavData { sample_rate: rate, channels, samples }, format: Format::Flac, source_bits: bits as u16, skipped_packets: 0 })
}

fn decode_mp3(b: &[u8]) -> io::Result<Decoded> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SErr;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let mss = MediaSourceStream::new(Box::new(Cursor::new(b.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
        .map_err(|e| bad(format!("MP3 : {e}")))?;
    let mut format = probed.format;
    let track = format.default_track().ok_or_else(|| bad("MP3 : aucune piste"))?.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| bad(format!("MP3 : {e}")))?;
    let (mut rate, mut channels) = (track.codec_params.sample_rate.unwrap_or(0), track.codec_params.channels.map(|c| c.count() as u16).unwrap_or(0));
    let (mut samples, mut skipped) = (Vec::new(), 0u32);
    let mut buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SErr::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(SErr::ResetRequired) => break,
            Err(e) => return Err(bad(format!("MP3 : {e}"))),
        };
        if packet.track_id() != track.id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                rate = spec.rate;
                channels = spec.channels.count() as u16;
                if buf.as_ref().map(|s| s.capacity() < decoded.capacity() * channels as usize).unwrap_or(true) {
                    buf = Some(SampleBuffer::new(decoded.capacity() as u64, spec));
                }
                let sb = buf.as_mut().unwrap();
                sb.copy_interleaved_ref(decoded);
                samples.extend_from_slice(sb.samples());
            }
            Err(SErr::DecodeError(_)) | Err(SErr::IoError(_)) => skipped += 1,
            Err(e) => return Err(bad(format!("MP3 : {e}"))),
        }
    }
    if rate == 0 || channels == 0 || samples.is_empty() {
        return Err(bad("MP3 : aucun son décodé"));
    }
    let samples = samples.into_iter().map(|v| if v.is_finite() { v } else { 0.0 }).collect();
    Ok(Decoded { audio: WavData { sample_rate: rate, channels, samples }, format: Format::Mp3, source_bits: 0, skipped_packets: skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
    }

    /// Amplitude 0,0625 : 1 kHz à gauche, 2 kHz à droite (voir `tests/fixtures/README.md`).
    fn check_stereo_tones(d: &Decoded, tolerance: f64, skip: usize) {
        assert_eq!((d.audio.sample_rate, d.audio.channels), (44_100, 2));
        for (ch, f) in [(0usize, 1_000.0f64), (1, 2_000.0)] {
            let x = d.audio.channel(ch);
            assert!(x.len() >= 22_050 - 10 && x.len() <= 22_050 + 3_000, "{} trames", x.len());
            // Écart avec la sinusoïde idéale ; la phase de départ est déterminée par ajustement (sin/cos).
            let n = x.len() - 2 * skip;
            let w = 2.0 * std::f64::consts::PI * f / 44_100.0;
            let (mut a, mut b) = (0.0, 0.0);
            for i in skip..skip + n {
                a += x[i] as f64 * (w * i as f64).sin();
                b += x[i] as f64 * (w * i as f64).cos();
            }
            let (a, b) = (2.0 * a / n as f64, 2.0 * b / n as f64);
            assert!(((a * a + b * b).sqrt() - 0.0625).abs() < 0.002, "amplitude du canal {ch} : {}", (a * a + b * b).sqrt());
            let mut sq = 0.0;
            for i in skip..skip + n {
                sq += (x[i] as f64 - a * (w * i as f64).sin() - b * (w * i as f64).cos()).powi(2);
            }
            let resid = (sq / n as f64).sqrt();
            assert!(resid < tolerance, "résidu du canal {ch} : {resid}");
        }
    }

    #[test]
    fn sniffing_uses_content_not_extension() {
        assert_eq!(sniff(&fixture("st16.wav")), Some(Format::Wav));
        assert_eq!(sniff(&fixture("st16.flac")), Some(Format::Flac));
        assert_eq!(sniff(&fixture("st.mp3")), Some(Format::Mp3));
        assert_eq!(sniff(b"hello world, not audio"), None);
        assert!(decode_bytes(b"").is_err());
    }

    #[test]
    fn wav_integer_depths_decode_to_the_same_signal() {
        for (name, bits, tol) in [("st16.wav", 16, 1e-4), ("st24.wav", 24, 2e-5), ("st8.wav", 8, 0.004)] {
            let d = decode_bytes(&fixture(name)).unwrap();
            assert_eq!((d.format, d.source_bits), (Format::Wav, bits), "{name}");
            assert_eq!(d.audio.frames(), 22_050);
            check_stereo_tones(&d, tol, 0);
        }
    }

    #[test]
    fn flac_16_and_24_bits_match_the_wav_exactly() {
        for (flac, wav) in [("st16.flac", "st16.wav"), ("st24.flac", "st24.wav")] {
            let (f, w) = (decode_bytes(&fixture(flac)).unwrap(), decode_bytes(&fixture(wav)).unwrap());
            assert_eq!(f.format, Format::Flac);
            // FLAC est sans perte : mêmes entiers, donc mêmes flottants.
            assert_eq!(f.audio, w.audio, "{flac}");
        }
        assert_eq!(decode_bytes(&fixture("st16.flac")).unwrap().source_bits, 16);
        assert_eq!(decode_bytes(&fixture("st24.flac")).unwrap().source_bits, 24);
    }

    #[test]
    fn mp3_decodes_to_the_right_tones() {
        let d = decode_bytes(&fixture("st.mp3")).unwrap();
        assert_eq!((d.format, d.source_bits, d.skipped_packets), (Format::Mp3, 0, 0));
        // Le MP3 ajoute du bruit de codage et un peu de silence au début : on ignore les bords.
        check_stereo_tones(&d, 0.004, 3_000);
    }

    fn wav_bytes(tag: u16, bits: u16, channels: u16, data: &[u8], extensible: bool) -> Vec<u8> {
        let fmt_len: u32 = if extensible { 40 } else { 16 };
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&(4 + 8 + fmt_len + 8 + data.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&fmt_len.to_le_bytes());
        v.extend_from_slice(&(if extensible { 0xFFFEu16 } else { tag }).to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&48_000u32.to_le_bytes());
        v.extend_from_slice(&(48_000 * channels as u32 * bits as u32 / 8).to_le_bytes());
        v.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        if extensible {
            v.extend_from_slice(&22u16.to_le_bytes());
            v.extend_from_slice(&bits.to_le_bytes());
            v.extend_from_slice(&3u32.to_le_bytes());
            v.extend_from_slice(&tag.to_le_bytes());
            v.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
        }
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn wav_float_64_float_32_extensible_and_exact_integer_scaling() {
        let mut d64 = Vec::new();
        for v in [0.25f64, -0.5, 1.0, 0.0] {
            d64.extend_from_slice(&v.to_le_bytes());
        }
        let d = decode_bytes(&wav_bytes(3, 64, 2, &d64, false)).unwrap();
        assert_eq!((d.audio.channels, d.audio.samples.clone()), (2, vec![0.25, -0.5, 1.0, 0.0]));

        let d32: Vec<u8> = [0.125f32, -1.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(decode_bytes(&wav_bytes(3, 32, 1, &d32, true)).unwrap().audio.samples, vec![0.125, -1.0]);

        // 24 bits : -8388608 → -1,0 exactement ; 4194304 → 0,5 exactement.
        let d24 = [0x00, 0x00, 0x80, 0x00, 0x00, 0x40];
        assert_eq!(decode_bytes(&wav_bytes(1, 24, 1, &d24, true)).unwrap().audio.samples, vec![-1.0, 0.5]);
        // 16 bits : -32768 → -1,0 ; 16384 → 0,5.
        let d16 = [0x00, 0x80, 0x00, 0x40];
        assert_eq!(decode_bytes(&wav_bytes(1, 16, 1, &d16, false)).unwrap().audio.samples, vec![-1.0, 0.5]);
    }

    #[test]
    fn wav_non_finite_values_become_silence_and_partial_frames_are_dropped() {
        let data: Vec<u8> = [f32::NAN, 0.5, f32::INFINITY].iter().flat_map(|v| v.to_le_bytes()).collect();
        let d = decode_bytes(&wav_bytes(3, 32, 2, &data, false)).unwrap();
        assert_eq!(d.audio.samples, vec![0.0, 0.5]); // 3 valeurs en stéréo : la trame incomplète est ignorée
    }

    #[test]
    fn probing_reads_the_headers_without_decoding() {
        let wav = probe_bytes(&fixture("st16.wav"), fixture("st16.wav").len() as u64).unwrap();
        assert_eq!(wav, Probe { format: Format::Wav, sample_rate: 44_100, channels: 2, frames: Some(22_050) });
        let w8 = probe_bytes(&fixture("st8.wav"), fixture("st8.wav").len() as u64).unwrap();
        assert_eq!((w8.channels, w8.frames), (2, Some(22_050)));
        let f = probe_bytes(&fixture("st24.flac"), 0).unwrap();
        assert_eq!((f.format, f.sample_rate, f.channels, f.frames), (Format::Flac, 44_100, 2, Some(22_050)));
        let m = probe_bytes(&fixture("st.mp3"), 0).unwrap();
        assert_eq!((m.format, m.sample_rate, m.channels), (Format::Mp3, 44_100, 2));
        // Un fichier à 6 canaux annonce 6 canaux.
        let six: Vec<u8> = (0..6 * 100).flat_map(|_| 0.25f32.to_le_bytes()).collect();
        let six_wav = wav_bytes(3, 32, 6, &six, true);
        let p = probe_bytes(&six_wav, six_wav.len() as u64).unwrap();
        assert_eq!((p.channels, p.frames), (6, Some(100)));
        assert!(probe_bytes(b"pas de l'audio", 14).is_err());
        assert!(probe_bytes(&fixture("st16.wav")[..20], 20).is_err());
    }

    #[test]
    fn damaged_files_are_errors_never_panics() {
        for name in ["st16.wav", "st16.flac", "st.mp3", "st24.flac"] {
            let full = fixture(name);
            // Tous les préfixes à pas serré + quelques octets abîmés.
            for cut in (0..full.len().min(3000)).step_by(7) {
                let _ = decode_bytes(&full[..cut]);
            }
            let mut mangled = full.clone();
            for k in (0..mangled.len()).step_by(97) {
                mangled[k] ^= 0x5A;
            }
            let _ = decode_bytes(&mangled);
        }
        assert!(decode_bytes(&wav_bytes(1, 12, 1, &[0, 0], false)).is_err()); // 12 bits : non géré
        assert!(decode_bytes(&wav_bytes(1, 16, 0, &[0, 0], false)).is_err()); // 0 canal
        assert!(decode_bytes(b"fLaC\0\0\0\0").is_err());
    }

    #[test]
    fn truncated_flac_and_mp3_report_errors_or_partial_audio_without_panicking() {
        let flac = fixture("st16.flac");
        assert!(decode_bytes(&flac[..flac.len() / 2]).is_err() || decode_bytes(&flac[..flac.len() / 2]).unwrap().audio.frames() < 22_050);
        let mp3 = fixture("st.mp3");
        let half = decode_bytes(&mp3[..mp3.len() / 2]).unwrap();
        assert!(half.audio.frames() > 5_000 && half.audio.frames() < 22_050 + 3_000);
    }
}
