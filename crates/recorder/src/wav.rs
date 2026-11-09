//! WAV flottant 32 bits : écriture au fil de l'eau et lecture.
//!
//! Limite connue : l'en-tête RIFF classique plafonne à 4 Gio (environ 3 h en stéréo à 48 kHz) ; au-delà, passer à RF64.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

const HEADER_LEN: u64 = 44;
const FLUSH_BYTES: usize = 64 * 1024;

pub struct WavWriter {
    file: File,
    buf: Vec<u8>,
    sample_rate: u32,
    channels: u16,
    /// Octets de données écrits dans le fichier (hors tampon).
    data_bytes: u64,
}

fn header(sample_rate: u32, channels: u16, data_bytes: u64) -> [u8; 44] {
    let data = data_bytes.min(u32::MAX as u64 - 36) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&3u16.to_le_bytes()); // IEEE float
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    h[28..32].copy_from_slice(&(sample_rate * 4 * channels as u32).to_le_bytes());
    h[32..34].copy_from_slice(&(4 * channels).to_le_bytes());
    h[34..36].copy_from_slice(&32u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

impl WavWriter {
    pub fn create(path: &Path, sample_rate: u32, channels: u16) -> io::Result<WavWriter> {
        let mut file = File::create(path)?;
        file.write_all(&header(sample_rate, channels, 0))?;
        Ok(WavWriter { file, buf: Vec::with_capacity(FLUSH_BYTES + 8), sample_rate, channels, data_bytes: 0 })
    }

    pub fn write_samples(&mut self, samples: &[f32]) -> io::Result<()> {
        for s in samples {
            self.buf.extend_from_slice(&s.to_le_bytes());
        }
        if self.buf.len() >= FLUSH_BYTES {
            self.flush_buf()?;
        }
        Ok(())
    }

    fn flush_buf(&mut self) -> io::Result<()> {
        self.file.write_all(&self.buf)?;
        self.data_bytes += self.buf.len() as u64;
        self.buf.clear();
        Ok(())
    }

    /// Écrit ce qui est en attente et met l'en-tête à jour : le fichier est lisible même si le programme s'arrête là.
    pub fn update_header(&mut self) -> io::Result<()> {
        self.flush_buf()?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header(self.sample_rate, self.channels, self.data_bytes))?;
        self.file.seek(SeekFrom::Start(HEADER_LEN + self.data_bytes))?;
        Ok(())
    }

    /// Termine le fichier ; retourne le nombre de trames écrites.
    pub fn finish(mut self) -> io::Result<u64> {
        self.update_header()?;
        self.file.sync_all()?;
        Ok(self.data_bytes / (4 * self.channels as u64))
    }
}

/// Contenu d'un fichier WAV flottant 32 bits.
#[derive(Debug, Clone, PartialEq)]
pub struct WavData {
    pub sample_rate: u32,
    pub channels: u16,
    /// Échantillons entrelacés.
    pub samples: Vec<f32>,
}

impl WavData {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
    /// Un canal (0 = premier).
    pub fn channel(&self, c: usize) -> Vec<f32> {
        self.samples.iter().skip(c).step_by(self.channels as usize).copied().collect()
    }
}

pub fn read_wav(path: &Path) -> io::Result<WavData> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    parse_wav(&bytes)
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

pub fn parse_wav(b: &[u8]) -> io::Result<WavData> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(bad("pas un fichier WAV"));
    }
    let (mut pos, mut fmt) = (12usize, None);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        match id {
            b"fmt " if body + 16 <= b.len() => {
                let tag = u16::from_le_bytes(b[body..body + 2].try_into().unwrap());
                let channels = u16::from_le_bytes(b[body + 2..body + 4].try_into().unwrap());
                let rate = u32::from_le_bytes(b[body + 4..body + 8].try_into().unwrap());
                let bits = u16::from_le_bytes(b[body + 14..body + 16].try_into().unwrap());
                if tag != 3 || bits != 32 || channels == 0 {
                    return Err(bad("seul le WAV flottant 32 bits est géré"));
                }
                fmt = Some((channels, rate));
            }
            b"data" => {
                let (channels, sample_rate) = fmt.ok_or_else(|| bad("bloc data avant fmt"))?;
                // Un fichier interrompu peut annoncer plus que ce qui existe : on lit ce qui est là.
                let end = (body + size).min(b.len());
                let usable = (end - body) / (4 * channels as usize) * (4 * channels as usize);
                let samples = b[body..body + usable].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
                return Ok(WavData { sample_rate, channels, samples });
            }
            _ => {}
        }
        pos = body + size + (size & 1);
    }
    Err(bad("pas de bloc data"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("daw-wav-{}-{name}.wav", std::process::id()))
    }

    #[test]
    fn write_then_read_round_trip() {
        let p = temp("rt");
        let mut w = WavWriter::create(&p, 48_000, 2).unwrap();
        let samples: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.001).sin()).collect();
        w.write_samples(&samples[..7_001]).unwrap();
        w.write_samples(&samples[7_001..]).unwrap();
        assert_eq!(w.finish().unwrap(), 10_000);
        let r = read_wav(&p).unwrap();
        assert_eq!((r.sample_rate, r.channels, r.frames()), (48_000, 2, 10_000));
        assert_eq!(r.samples, samples);
        assert_eq!(r.channel(1), samples.iter().skip(1).step_by(2).copied().collect::<Vec<_>>());
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn empty_file_is_valid() {
        let p = temp("empty");
        let w = WavWriter::create(&p, 44_100, 1).unwrap();
        assert_eq!(w.finish().unwrap(), 0);
        let r = read_wav(&p).unwrap();
        assert_eq!((r.sample_rate, r.channels, r.samples.len()), (44_100, 1, 0));
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 44);
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn interrupted_file_is_readable_after_a_header_update() {
        let p = temp("crash");
        let mut w = WavWriter::create(&p, 48_000, 1).unwrap();
        w.write_samples(&[0.5; 1_000]).unwrap();
        w.update_header().unwrap();
        w.write_samples(&[0.25; 500]).unwrap(); // jamais écrit : le programme « s'arrête » ici
        let r = read_wav(&p).unwrap();
        assert_eq!(r.samples.len(), 1_000);
        assert!(r.samples.iter().all(|&s| s == 0.5));
        std::mem::forget(w); // pas de finish : simule la coupure
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn truncated_data_is_read_up_to_the_last_whole_frame() {
        let mut bytes = header(48_000, 2, 1_000).to_vec(); // annonce 1 000 octets
        bytes.extend_from_slice(&[0u8; 21]); // mais seulement 21 octets : 2 trames entières (16 octets)
        let r = parse_wav(&bytes).unwrap();
        assert_eq!(r.frames(), 2);
    }

    #[test]
    fn rejects_non_wav_and_other_formats() {
        assert!(parse_wav(b"nope").is_err());
        let mut h = header(48_000, 1, 0);
        h[20] = 1; // PCM entier
        assert!(parse_wav(&h).is_err());
    }
}
