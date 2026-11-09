//! Export (mixdown et stems) : rendu par le vrai moteur, puis encodage WAV ou FLAC.
//!
//! * **Zone** : [`ExportRange`] (projet entier, boucle, sélection) — le moteur démarre à `start` (déplacement puis lecture) ;
//!   les notes déjà en train de sonner à `start` ne sont pas rejouées.
//! * **Queue** : après la fin de la zone on rend jusqu'à `tail_secs` de plus (réverbes, délais), puis on coupe dès que
//!   le niveau reste sous `silence_db` jusqu'au bout de la queue.
//! * **Latence** : toujours compensée (le fichier est aligné sur la ligne de temps, comme dans [`crate::render`]).
//! * **Bits** : 16 / 24 bits entiers ou flottant 32 bits ; dither TPDF **déterministe** (générateur à graine fixe : deux
//!   exports du même projet donnent les mêmes octets). Normalisation facultative à un pic cible.
//! * **Stems** : [`stem_graph`] isole la contribution d'un nœud (voir sa documentation).
//!
//! Tout se passe hors du thread audio ; le résultat est un tampon d'octets (l'appelant l'écrit sur le disque).

use daw_core_types::NodeId;
use daw_engine::{Command, GraphDesc, NodeKind};

use crate::{render, Progress, RenderError, RenderSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Wav,
    Flac,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitDepth {
    Pcm16,
    Pcm24,
    /// Flottant 32 bits (WAV seulement).
    Float32,
}

impl BitDepth {
    pub fn bits(self) -> u16 {
        match self {
            BitDepth::Pcm16 => 16,
            BitDepth::Pcm24 => 24,
            BitDepth::Float32 => 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Format {
    pub container: Container,
    pub depth: BitDepth,
    /// Dither triangulaire (TPDF) avant l'arrondi ; sans effet en flottant.
    pub dither: bool,
}

impl Format {
    pub fn wav24() -> Format {
        Format { container: Container::Wav, depth: BitDepth::Pcm24, dither: false }
    }
    pub fn wav16() -> Format {
        Format { container: Container::Wav, depth: BitDepth::Pcm16, dither: true }
    }
    pub fn extension(&self) -> &'static str {
        match self.container {
            Container::Wav => "wav",
            Container::Flac => "flac",
        }
    }
}

/// Zone à exporter, en échantillons de la ligne de temps : `[start, end[`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportRange {
    pub start: u64,
    pub end: u64,
}

impl ExportRange {
    /// Du début du projet à la fin du dernier clip.
    pub fn whole_project(graph: &GraphDesc) -> ExportRange {
        ExportRange { start: 0, end: project_end(graph) }
    }
}

/// Fin du dernier clip (audio ou notes) du graphe, en échantillons.
pub fn project_end(graph: &GraphDesc) -> u64 {
    graph
        .nodes
        .iter()
        .flat_map(|n| n.clips.iter().map(|c| c.end()).chain(n.midi_clips.iter().map(|c| c.start + c.len)))
        .max()
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct ExportSpec {
    pub sample_rate: u32,
    pub graph: GraphDesc,
    pub range: ExportRange,
    /// Durée maximale de la queue, en secondes (0 : aucune).
    pub tail_secs: f32,
    /// La queue est coupée dès que son niveau reste sous ce seuil (dBFS, ex. −90).
    pub silence_db: f32,
    pub format: Format,
    /// Pic cible en dBFS (ex. −1,0) ; `None` : niveau inchangé. Ignoré pour les stems (voir [`export_stems`]).
    pub normalize_db: Option<f32>,
    pub progress: Option<Progress>,
}

impl ExportSpec {
    pub fn new(sample_rate: u32, graph: GraphDesc, range: ExportRange, format: Format) -> ExportSpec {
        ExportSpec { sample_rate, graph, range, tail_secs: 5.0, silence_db: -90.0, format, normalize_db: None, progress: None }
    }
}

#[derive(Debug, PartialEq)]
pub enum ExportError {
    Render(RenderError),
    Cancelled,
    /// Zone vide, nœud inconnu, format incompatible…
    Invalid(String),
    Encode(String),
}

impl From<RenderError> for ExportError {
    fn from(e: RenderError) -> Self {
        if e == RenderError::Cancelled {
            ExportError::Cancelled
        } else {
            ExportError::Render(e)
        }
    }
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Render(e) => write!(f, "rendu : {e:?}"),
            ExportError::Cancelled => f.write_str("export annulé"),
            ExportError::Invalid(s) | ExportError::Encode(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for ExportError {}

/// Ce que l'on sait d'un fichier exporté.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportReport {
    /// Trames écrites (queue comprise).
    pub frames: u64,
    /// Pic du signal rendu, avant normalisation (linéaire).
    pub peak: f32,
    /// Gain appliqué par la normalisation, en dB (0 sinon).
    pub applied_gain_db: f32,
    /// Échantillons au-delà de la pleine échelle après gain (écrêtés dans un fichier entier ; conservés en flottant).
    pub clipped_samples: u64,
}

/// Fichier encodé et son rapport.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported {
    pub bytes: Vec<u8>,
    pub report: ExportReport,
}

/// Graphe qui ne laisse passer que la contribution de `node` (« stem »).
///
/// * `node` envoie directement à la sortie principale ; **ses envois sont retirés** (le stem est la sortie du nœud).
/// * Les nœuds qui l'alimentent (par leur sortie ou par un envoi, de proche en proche) sont conservés : un stem de bus
///   contient donc les pistes qui y arrivent, avec leurs effets, faders et envois vers ce bus. **Leur propre sortie
///   principale n'est pas comptée** si elle va ailleurs (piste → sortie principale ET envoi → bus : seul l'envoi est dans
///   le stem du bus).
/// * Tous les autres nœuds sont réduits au silence (clips retirés, coupés) ; les solos sont effacés.
/// * La sortie principale est neutre (gain 1, sans effet ni automation) : **le stem est pris avant le traitement de la
///   sortie principale**. Somme de tous les stems de premier niveau = mixage, si la sortie principale est neutre.
pub fn stem_graph(graph: &GraphDesc, node: NodeId) -> Result<GraphDesc, ExportError> {
    let idx = |id: NodeId| graph.nodes.iter().position(|n| n.id == id);
    let ni = idx(node).ok_or_else(|| ExportError::Invalid(format!("nœud {} inconnu", node.0)))?;
    if matches!(graph.nodes[ni].kind, NodeKind::Master) {
        return Err(ExportError::Invalid("la sortie principale n'a pas de stem : c'est le mixage".into()));
    }
    let mut feeds = vec![false; graph.nodes.len()];
    feeds[ni] = true;
    loop {
        let mut changed = false;
        for (i, n) in graph.nodes.iter().enumerate() {
            if feeds[i] {
                continue;
            }
            let reaches = |to: NodeId| idx(to).is_some_and(|t| feeds[t]);
            if n.output.is_some_and(reaches) || n.sends.iter().any(|s| reaches(s.to)) {
                feeds[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut g = graph.clone();
    // Un nœud qui alimente `node` mais dont la sortie principale va ailleurs (sortie principale, autre bus) ne doit pas
    // fuir : on redirige cette sortie vers un bus muet, ajouté en dernier (les numéros des autres nœuds ne bougent pas).
    let needs_sink = graph.nodes.iter().enumerate().any(|(i, n)| feeds[i] && i != ni && n.output.is_none_or(|o| idx(o).is_none_or(|t| !feeds[t])));
    let sink = needs_sink.then(|| {
        let id = g.add_bus("(silence du stem)");
        let n = g.node_mut(id).expect("le bus vient d'être ajouté");
        n.params.mute = true;
        n.params.gain = 0.0;
        id
    });
    let feeding = |id: NodeId| idx(id).is_some_and(|t| feeds[t]);
    for (i, n) in g.nodes.iter_mut().enumerate() {
        if i >= feeds.len() {
            continue; // le bus muet
        }
        n.params.solo = false;
        if matches!(n.kind, NodeKind::Master) {
            n.params.gain = 1.0;
            n.params.mute = false;
            n.params.invert = false;
            n.params.pan = 0.0;
            n.inserts.clear();
            n.automation.clear();
        } else if i == ni {
            n.output = Some(NodeId::MASTER);
            n.sends.clear();
        } else if feeds[i] {
            n.sends.retain(|s| feeding(s.to));
            if !n.output.is_some_and(feeding) {
                n.output = sink;
            }
        } else {
            n.clips.clear();
            n.midi_clips.clear();
            n.params.mute = true;
            n.params.gain = 0.0;
        }
    }
    Ok(g)
}

/// Rend la zone (queue comprise) avec le vrai moteur et renvoie les deux canaux.
pub fn render_range(spec: &ExportSpec) -> Result<(Vec<f32>, Vec<f32>), ExportError> {
    let ExportRange { start, end } = spec.range;
    if end <= start {
        return Err(ExportError::Invalid("zone d'export vide".into()));
    }
    let len = end - start;
    let tail = (spec.tail_secs.max(0.0) as f64 * spec.sample_rate as f64) as u64;
    let mut r = RenderSpec::new(spec.sample_rate, len + tail, spec.graph.clone());
    r.events = vec![(0, Command::Seek(start)), (0, Command::Play)];
    r.progress = spec.progress.clone();
    let mut out = render(&r)?;
    let keep = trimmed_len(&out.left, &out.right, len as usize, spec.silence_db);
    out.left.truncate(keep);
    out.right.truncate(keep);
    Ok((out.left, out.right))
}

/// Longueur à garder : au moins `min`, puis jusqu'au dernier échantillon plus fort que `silence_db`.
pub fn trimmed_len(left: &[f32], right: &[f32], min: usize, silence_db: f32) -> usize {
    let thr = 10f32.powf(silence_db / 20.0);
    let n = left.len().min(right.len());
    let mut keep = min.min(n);
    for i in (keep..n).rev() {
        if left[i].abs() > thr || right[i].abs() > thr {
            keep = i + 1;
            break;
        }
    }
    keep
}

/// Mixage de la zone, encodé.
pub fn export_mix(spec: &ExportSpec) -> Result<Exported, ExportError> {
    let (l, r) = render_range(spec)?;
    encode(&l, &r, spec.sample_rate, spec.format, spec.normalize_db)
}

/// Un fichier par nœud de `nodes`, produit un à la fois (la mémoire ne dépasse jamais un stem) : `sink(nœud, fichier)`.
/// La normalisation est ignorée : normaliser chaque stem séparément changerait l'équilibre entre eux.
/// Un échec de `sink` ou une annulation arrête tout. La progression de `spec.progress` est celle du stem en cours.
pub fn export_stems(spec: &ExportSpec, nodes: &[NodeId], mut sink: impl FnMut(NodeId, Exported) -> Result<(), ExportError>) -> Result<(), ExportError> {
    for &node in nodes {
        let mut s = spec.clone();
        s.graph = stem_graph(&spec.graph, node)?;
        s.normalize_db = None;
        sink(node, export_mix(&s)?)?;
    }
    Ok(())
}

/// Générateur xorshift64* : entiers seulement, donc identique partout.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniforme dans [0, 1) sur 24 bits.
    fn unit(&mut self) -> f64 {
        (self.next() >> 40) as f64 / 16_777_216.0
    }
}

/// Encode deux canaux (même longueur) ; applique la normalisation puis l'arrondi (avec dither si demandé).
pub fn encode(left: &[f32], right: &[f32], sample_rate: u32, format: Format, normalize_db: Option<f32>) -> Result<Exported, ExportError> {
    if left.len() != right.len() {
        return Err(ExportError::Invalid("canaux de longueurs différentes".into()));
    }
    if format.container == Container::Flac && format.depth == BitDepth::Float32 {
        return Err(ExportError::Invalid("FLAC ne gère pas le flottant : choisir 16 ou 24 bits".into()));
    }
    let clean = |v: f32| if v.is_finite() { v } else { 0.0 };
    let peak = left.iter().chain(right).fold(0.0f32, |m, &v| m.max(clean(v).abs()));
    let (gain, applied_gain_db) = match normalize_db {
        Some(target) if peak > 0.0 => {
            let g = 10f64.powf(target as f64 / 20.0) / peak as f64;
            (g, (20.0 * g.log10()) as f32)
        }
        _ => (1.0, 0.0),
    };
    let clipped_samples = left.iter().chain(right).filter(|&&v| (clean(v) as f64 * gain).abs() > 1.0).count() as u64;
    let frames = left.len();

    let bytes = match format.depth {
        BitDepth::Float32 => {
            let mut data = Vec::with_capacity(frames * 8);
            for i in 0..frames {
                data.extend_from_slice(&((clean(left[i]) as f64 * gain) as f32).to_le_bytes());
                data.extend_from_slice(&((clean(right[i]) as f64 * gain) as f32).to_le_bytes());
            }
            wav_container(sample_rate, 32, true, &data)
        }
        depth => {
            let bits = depth.bits() as u32;
            let scale = (1u64 << (bits - 1)) as f64;
            let (lo, hi) = (-scale, scale - 1.0);
            // Deux flux de bruit indépendants (un par canal), graine fixe.
            let mut rng = [Rng(0x9E37_79B9_7F4A_7C15), Rng(0xD1B5_4A32_D192_ED03)];
            let mut ints = Vec::with_capacity(frames * 2);
            for i in 0..frames {
                for (ch, x) in [left[i], right[i]].into_iter().enumerate() {
                    let mut v = clean(x) as f64 * gain * scale;
                    if format.dither {
                        v += rng[ch].unit() - rng[ch].unit();
                    }
                    ints.push((v + 0.5).floor().clamp(lo, hi) as i32);
                }
            }
            match format.container {
                Container::Wav => {
                    let w = (bits / 8) as usize;
                    let mut data = Vec::with_capacity(ints.len() * w);
                    for v in &ints {
                        data.extend_from_slice(&v.to_le_bytes()[..w]);
                    }
                    wav_container(sample_rate, bits as u16, false, &data)
                }
                Container::Flac => flac_container(&ints, sample_rate, bits as usize)?,
            }
        }
    };
    Ok(Exported { bytes, report: ExportReport { frames: frames as u64, peak, applied_gain_db, clipped_samples } })
}

/// En-tête WAV stéréo (PCM entier ou flottant) suivi des données entrelacées.
fn wav_container(rate: u32, bits: u16, float: bool, data: &[u8]) -> Vec<u8> {
    let block_align = 2 * bits / 8;
    let fact = if float { 12 } else { 0 };
    let mut v = Vec::with_capacity(data.len() + 56);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((4 + 24 + fact + 8 + data.len()) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
    v.extend_from_slice(&block_align.to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    if float {
        v.extend_from_slice(b"fact");
        v.extend_from_slice(&4u32.to_le_bytes());
        v.extend_from_slice(&((data.len() / block_align as usize) as u32).to_le_bytes());
    }
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(data);
    if data.len() % 2 == 1 {
        v.push(0);
    }
    v
}

fn flac_container(ints: &[i32], rate: u32, bits: usize) -> Result<Vec<u8>, ExportError> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let err = |e: &dyn std::fmt::Debug| ExportError::Encode(format!("FLAC : {e:?}"));
    let config = flacenc::config::Encoder::default().into_verified().map_err(|e| err(&e))?;
    // Boucle à nous plutôt que `encode_with_fixed_block_size` : elle complète le dernier bloc avec du silence, que
    // ffmpeg et libFLAC restituent (le fichier serait plus long que le projet). Ici le dernier bloc est plus court,
    // ce que le format permet. La somme MD5 reste à zéro (« inconnue », valide).
    use flacenc::source::Fill;
    let mut stream = flacenc::component::Stream::new(rate as usize, 2, bits).map_err(|e| err(&e))?;
    let frames = ints.len() / 2;
    let (mut pos, mut number) = (0usize, 0usize);
    while pos < frames {
        let n = config.block_size.min(frames - pos);
        let mut fb = flacenc::source::FrameBuf::with_size(2, n).map_err(|e| err(&e))?;
        fb.fill_interleaved(&ints[pos * 2..(pos + n) * 2]).map_err(|e| err(&e))?;
        let frame = flacenc::encode_fixed_size_frame(&config, &fb, number, stream.stream_info()).map_err(|e| err(&e))?;
        stream.add_frame(frame);
        pos += n;
        number += 1;
    }
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| err(&e))?;
    Ok(sink.as_slice().to_vec())
}
