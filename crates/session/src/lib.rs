//! Modèle de projet et annulation / rétablissement.
//!
//! `Session` possède le [`GraphDesc`] que le moteur compile. Toute modification passe par une [`Edit`] réversible
//! (patron « commande » de Scorsaire : `flip()` renvoie l'édition inverse). Les actions de l'utilisateur sont des
//! transactions : une action = un pas d'annulation, même si elle touche plusieurs clips (découpe, fondu enchaîné).
//! Les clips sont désignés par un [`ClipId`] stable, qui survit aux déplacements et à l'annulation.
//!
//! Aucune dépendance système : compile en WebAssembly.

pub mod automation_rec;
pub mod chords;
pub mod quality_catalog;
pub mod scale_catalog;
pub mod effects;
pub mod midifile;
pub mod music;
pub mod peaks;
pub mod nodes;
pub mod project;
pub mod groups;
pub mod midi_filter;
pub mod lyrics;
pub mod structure;
pub mod takes;
pub mod xfade;

use std::sync::Arc;

use daw_core_types::time::{Frac, Meter, MeterMap, Tempo, TempoMap};
use daw_core_types::NodeId;
use daw_engine::{crossfade, AudioData, AutoTarget, Clip, EditError, FadeShape, GraphDesc, NodeParams, RecordedMidi};

pub use music::{MClip, MEvent, MKind, MLane, MNote, MPoint, MidiClipId};

/// Résolution des positions des notes enregistrées : 960 ticks par noire (comme Scorsaire et la plupart des fichiers MIDI).
pub const RECORD_PPQ: u32 = 960;

/// Fréquence d'échantillonnage par défaut d'une session créée sans (`Session::new`).
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// Identifiant stable d'un clip (jamais réutilisé dans une session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClipId(pub u64);

#[derive(Debug, PartialEq)]
pub enum SessionError {
    UnknownNode(NodeId),
    UnknownClip(ClipId),
    Edit(EditError),
    UnknownMidiClip(MidiClipId),
    /// La destination d'un clip n'est pas une piste (sortie principale ou bus).
    NotATrack(NodeId),
    /// Note ou clip de notes invalide (touche, canal, vélocité, durée nulle, début hors du clip).
    BadMidi,
    /// Courbe d'automation invalide (vide, positions non croissantes, valeur non finie).
    BadLane,
    /// Un indice de note hors du clip.
    BadNoteIndex(usize),
    /// Conversion temps musical → échantillons impossible (dépassement d'entier).
    TimeOverflow,
    /// Routage refusé (sortie qui n'est pas un bus, boucle, nœud lui-même...) : le message vient de la validation du graphe.
    Routing(String),
    /// Nom de piste vide ou trop long.
    BadName,
    /// Opération impossible sur la sortie principale (supprimer, renommer, déplacer, rediriger).
    MasterNode,
    /// Effet inséré inexistant (identifiant inconnu, ou pas sur le même nœud que la destination).
    UnknownInsert(u32),
    /// Piste d'accords invalide ou fichier de carte refusé (le message dit pourquoi).
    BadChordTrack(String),
    /// Dynamique d'entrée hors de ses plages (seuil de porte −90 à −20 dB, seuil −60 à 0, rapport 1 à 20, attaque 0,1 à 100 ms, relâchement
    /// 10 à 2 000 ms, compensation 0 à 24 dB).
    BadInputDynamics,
    BadVelocityCurve,
    /// Filtre d'entrée MIDI invalide (le message dit pourquoi).
    BadMidiFilter(String),
    /// Groupes de pistes invalides (le message dit pourquoi).
    BadGroups(String),
    /// Carte de structure invalide (le message dit pourquoi).
    BadStructure(String),
    /// Paroles invalides (le message dit pourquoi).
    BadLyrics(String),
    /// Sortie physique refusée (pas un bus, canaux de la sortie principale, trop de canaux, canal déjà pris).
    BadPhysOut(NodeId),
}

impl From<EditError> for SessionError {
    fn from(e: EditError) -> Self {
        SessionError::Edit(e)
    }
}

/// Modification élémentaire et réversible du projet.
#[derive(Debug, Clone)]
pub enum Edit {
    InsertClip { node: NodeId, index: usize, id: ClipId, clip: Clip },
    RemoveClip { node: NodeId, index: usize, id: ClipId, clip: Clip },
    ReplaceClip { id: ClipId, old: Clip, new: Clip },
    SetParams { node: NodeId, old: NodeParams, new: NodeParams },
    InsertMidi { node: NodeId, index: usize, id: MidiClipId, clip: MClip },
    RemoveMidi { node: NodeId, index: usize, id: MidiClipId, clip: MClip },
    ReplaceMidi { id: MidiClipId, old: MClip, new: MClip },
    /// Remplace (ou ajoute, ou retire) la courbe qui vise `target` ; `index` = sa place dans la liste du nœud.
    SetLane { node: NodeId, target: AutoTarget, index: usize, old: Option<MLane>, new: Option<MLane> },
    SetTiming { old: Timing, new: Timing },
    /// Piste d'accords remplacée en bloc (modifiée, importée...) : voir `chords`.
    SetChordTrack { old: Box<chords::ChordTrack>, new: Box<chords::ChordTrack> },
    /// Carte de structure remplacée en bloc : voir `structure`.
    SetStructure { old: Vec<structure::Section>, new: Vec<structure::Section> },
    /// Paroles remplacées en bloc : voir `lyrics`.
    SetLyrics { old: Vec<lyrics::Syllable>, new: Vec<lyrics::Syllable> },
    /// Groupes de pistes et appartenance, en bloc : voir `groups`.
    SetGroups { old: Box<groups::GroupState>, new: Box<groups::GroupState> },
    /// Groupes propres à un retour (remplacent ceux du projet dans son mix) ; `None` : ceux du projet.
    SetReturnGroups { ret: NodeId, old: Option<Box<groups::GroupState>>, new: Option<Box<groups::GroupState>> },
    /// Insère (ou retire, selon le sens) un nœud entier avec tout ce qui lui appartient : voir `nodes::NodeRecord`.
    InsertNode { index: usize, record: Box<nodes::NodeRecord> },
    RemoveNode { index: usize, record: Box<nodes::NodeRecord> },
    SetName { node: NodeId, old: String, new: String },
    SetColor { node: NodeId, old: String, new: String },
    SetOutput { node: NodeId, old: NodeId, new: NodeId },
    /// Liste des envois d'un nœud (destinations, niveaux, avant / après fader), remplacée en bloc.
    SetSends { node: NodeId, old: Vec<daw_engine::SendDesc>, new: Vec<daw_engine::SendDesc> },
    /// Ordre d'affichage des pistes et bus (sans la sortie principale).
    SetOrder { old: Vec<NodeId>, new: Vec<NodeId> },
    /// Sortie physique d'un bus (retour de monitoring) : voir `daw_engine::PhysOut`.
    SetPhys { node: NodeId, old: Option<daw_engine::PhysOut>, new: Option<daw_engine::PhysOut> },
    /// Insère (ou retire) un effet dans la chaîne d'un nœud.
    AddEffect { node: NodeId, index: usize, desc: Box<daw_engine::InsertDesc> },
    RemoveEffect { node: NodeId, index: usize, desc: Box<daw_engine::InsertDesc> },
    /// Déplace un effet dans la chaîne de son nœud : `to` est sa place **une fois retiré** de l'ancienne.
    MoveEffect { node: NodeId, from: usize, to: usize },
}

/// Cartes des tempos et des chiffrages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    pub tempo: TempoMap,
    pub meters: MeterMap,
}

impl Edit {
    /// L'édition qui défait celle-ci.
    pub fn flip(&self) -> Edit {
        match self.clone() {
            Edit::InsertClip { node, index, id, clip } => Edit::RemoveClip { node, index, id, clip },
            Edit::RemoveClip { node, index, id, clip } => Edit::InsertClip { node, index, id, clip },
            Edit::ReplaceClip { id, old, new } => Edit::ReplaceClip { id, old: new, new: old },
            Edit::SetParams { node, old, new } => Edit::SetParams { node, old: new, new: old },
            Edit::InsertMidi { node, index, id, clip } => Edit::RemoveMidi { node, index, id, clip },
            Edit::RemoveMidi { node, index, id, clip } => Edit::InsertMidi { node, index, id, clip },
            Edit::ReplaceMidi { id, old, new } => Edit::ReplaceMidi { id, old: new, new: old },
            Edit::SetLane { node, target, index, old, new } => Edit::SetLane { node, target, index, old: new, new: old },
            Edit::SetTiming { old, new } => Edit::SetTiming { old: new, new: old },
            Edit::SetChordTrack { old, new } => Edit::SetChordTrack { old: new, new: old },
            Edit::SetStructure { old, new } => Edit::SetStructure { old: new, new: old },
            Edit::SetLyrics { old, new } => Edit::SetLyrics { old: new, new: old },
            Edit::SetGroups { old, new } => Edit::SetGroups { old: new, new: old },
            Edit::SetReturnGroups { ret, old, new } => Edit::SetReturnGroups { ret, old: new, new: old },
            Edit::InsertNode { index, record } => Edit::RemoveNode { index, record },
            Edit::RemoveNode { index, record } => Edit::InsertNode { index, record },
            Edit::SetName { node, old, new } => Edit::SetName { node, old: new, new: old },
            Edit::SetColor { node, old, new } => Edit::SetColor { node, old: new, new: old },
            Edit::SetOutput { node, old, new } => Edit::SetOutput { node, old: new, new: old },
            Edit::SetSends { node, old, new } => Edit::SetSends { node, old: new, new: old },
            Edit::SetOrder { old, new } => Edit::SetOrder { old: new, new: old },
            Edit::SetPhys { node, old, new } => Edit::SetPhys { node, old: new, new: old },
            Edit::AddEffect { node, index, desc } => Edit::RemoveEffect { node, index, desc },
            Edit::RemoveEffect { node, index, desc } => Edit::AddEffect { node, index, desc },
            Edit::MoveEffect { node, from, to } => Edit::MoveEffect { node, from: to, to: from },
        }
    }
}

/// Une action annulable : plusieurs éditions appliquées dans l'ordre, défaites dans l'ordre inverse.
#[derive(Debug, Clone, Default)]
struct Transaction {
    label: String,
    edits: Vec<Edit>,
}

/// Courbe d'un paramètre de plugin lue dans un projet : le plugin (désigné par sa place dans la chaîne du nœud) est recréé par
/// l'appelant, qui rattache ensuite la courbe avec [`Session::attach_insert_lane`].
#[derive(Debug, Clone, PartialEq)]
pub struct PendingInsertLane {
    pub node: NodeId,
    pub chain_index: usize,
    pub param: u32,
    pub points: Vec<MPoint>,
    pub mode: daw_engine::AutoMode,
}

pub struct Session {
    pending_insert_lanes: Vec<PendingInsertLane>,
    graph: GraphDesc,
    /// Identifiants des clips, parallèles à `NodeDesc::clips` de chaque nœud (indexés par numéro de nœud).
    ids: Vec<Vec<ClipId>>,
    next_id: u64,
    /// Longueur (ms) du petit fondu enchaîné posé aux raccords entre prises (0 : aucun) ; voir `takes::smooth_seams`.
    seam_ms: u32,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// Transaction en cours de construction (groupe ouvert).
    open: Option<Transaction>,
    /// Fusion en cours (scripts) : indice de la pile d'annulation au début et libellé de l'action finale.
    merge: Option<(usize, String)>,
    /// Fichiers audio du projet : les données en mémoire et le chemin sous lequel elles sont enregistrées.
    assets: Vec<(Arc<AudioData>, String)>,
    /// Chemins (référencés en place) des fichiers introuvables à l'ouverture : leurs données sont du silence.
    missing: Vec<String>,
    sample_rate: u32,
    timing: Timing,
    /// Clips de notes en temps musical, par nœud (indexés par numéro de nœud). Font foi : `graph` en est dérivé.
    midi: Vec<Vec<(MidiClipId, MClip)>>,
    /// Courbes d'automation en temps musical, par nœud.
    lanes: Vec<Vec<MLane>>,
    next_midi_id: u64,
    /// Couleur d'affichage de chaque nœud (texte `#rrggbb`, vide = couleur par défaut), parallèle à `graph.nodes`.
    colors: Vec<String>,
    /// Ordre d'affichage des pistes et bus (tous les nœuds sauf la sortie principale).
    order: Vec<NodeId>,
    /// Entrée de chaque piste (audio : quel(s) canal(aux) ; ou MIDI), et écoute directe : parallèle à `graph.nodes`.
    inputs: Vec<nodes::NodeInput>,
    /// Augmente à chaque ajout ou retrait de nœud : les numéros de nœuds ont pu changer.
    structure_version: u64,
    /// Piste d'accords (information pour les plugins) et son numéro de révision, qui change à chaque modification.
    chords: chords::ChordTrack,
    chord_revision: u32,
    /// Carte de structure du morceau (intro, couplet, refrain...), indépendante des accords.
    structure: Vec<structure::Section>,
    /// Paroles (syllabes en temps musical).
    lyrics: Vec<lyrics::Syllable>,
    /// Groupes de pistes (identifiants stables) et groupe de chaque nœud (0 = aucun) : parallèle à `graph.nodes`.
    groups: Vec<groups::Group>,
    node_group: Vec<u32>,
    next_group_id: u32,
    /// Groupes propres de chaque retour (parallèle à `graph.nodes`) : `Some` remplace ceux du projet dans son mix.
    ret_groups: Vec<Option<groups::GroupState>>,
    /// Jeton secret de l'adresse de la tablette de chaque retour (parallèle à `graph.nodes`, vide : pas encore donné). Enregistré avec le
    /// projet : l'adresse d'un retour ne change plus d'une session à l'autre.
    monitor_tokens: Vec<String>,
    /// Rôle de la télécommande de chaque retour (0 : le musicien règle son retour seulement ; 1 : aussi le transport, les sections et le pré-roll).
    monitor_roles: Vec<u8>,
    /// Filtre d'entrée MIDI de chaque piste (parallèle à `graph.nodes`) : liste de règles, vide = tout accepter.
    midi_filters: Vec<Vec<midi_filter::MidiRule>>,
}

impl Session {
    pub fn new(graph: GraphDesc) -> Session {
        let timing = Timing { tempo: TempoMap::new(Tempo::bpm(120).expect("120 > 0")), meters: MeterMap::new(Meter::new(4, 4).expect("4/4")) };
        Session::with_timing(graph, DEFAULT_SAMPLE_RATE, timing)
    }

    /// Session à 120 bpm en 4/4 pour la fréquence d'échantillonnage donnée.
    pub fn new_at(graph: GraphDesc, sample_rate: u32) -> Session {
        let timing = Timing { tempo: TempoMap::new(Tempo::bpm(120).expect("120 > 0")), meters: MeterMap::new(Meter::new(4, 4).expect("4/4")) };
        Session::with_timing(graph, sample_rate, timing)
    }

    /// Session avec fréquence et cartes données. Les clips de notes et courbes que `graph` contient déjà (en
    /// échantillons) sont convertis **exactement** en temps musical : ils suivront ensuite les changements de tempo.
    pub fn with_timing(graph: GraphDesc, sample_rate: u32, timing: Timing) -> Session {
        let mut next_id = 0;
        let ids = graph
            .nodes
            .iter()
            .map(|n| {
                n.clips
                    .iter()
                    .map(|_| {
                        next_id += 1;
                        ClipId(next_id - 1)
                    })
                    .collect()
            })
            .collect();
        let mut next_midi_id = 0;
        let mut midi = Vec::new();
        let mut lanes = Vec::new();
        for n in &graph.nodes {
            let mut cs = Vec::new();
            for c in &n.midi_clips {
                if let Some(m) = music::clip_from_engine(c, &timing.tempo, sample_rate) {
                    cs.push((MidiClipId(next_midi_id), m));
                    next_midi_id += 1;
                }
            }
            midi.push(cs);
            lanes.push(n.automation.iter().filter_map(|l| music::lane_from_engine(l, &timing.tempo, sample_rate)).collect());
        }
        let colors = vec![String::new(); graph.nodes.len()];
        let order = (1..graph.nodes.len() as u32).map(NodeId).collect();
        let inputs = vec![nodes::NodeInput::default(); graph.nodes.len()];
        let node_group = vec![0u32; graph.nodes.len()];
        let node_group_len = graph.nodes.len();
        let mut s = Session { seam_ms: 10, pending_insert_lanes: Vec::new(), graph, ids, next_id, undo: Vec::new(), redo: Vec::new(), open: None, merge: None, assets: Vec::new(), missing: Vec::new(), sample_rate, timing, midi, lanes, next_midi_id, colors, order, inputs, structure_version: 0, chords: chords::ChordTrack::default(), chord_revision: 0, structure: Vec::new(), lyrics: Vec::new(), groups: Vec::new(), node_group, next_group_id: 0, ret_groups: vec![None; node_group_len], monitor_tokens: vec![String::new(); node_group_len], monitor_roles: vec![0; node_group_len], midi_filters: vec![Vec::new(); node_group_len] };
        let _ = s.sync_all(); // identique à l'entrée (conversion exacte) ; sinon le graphe reste tel quel
        s
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn tempo_map(&self) -> &TempoMap {
        &self.timing.tempo
    }
    pub fn meter_map(&self) -> &MeterMap {
        &self.timing.meters
    }
    pub fn midi_clip_ids(&self, node: NodeId) -> Vec<MidiClipId> {
        self.midi.get(node.0 as usize).map_or_else(Vec::new, |v| v.iter().map(|(i, _)| *i).collect())
    }
    pub fn midi_clip(&self, id: MidiClipId) -> Option<&MClip> {
        let (n, i) = self.locate_midi(id)?;
        Some(&self.midi[n][i].1)
    }
    /// Courbes de paramètres de plugins lues dans le projet et pas encore rattachées (voir [`PendingInsertLane`]) ; vide ensuite.
    pub fn take_pending_insert_lanes(&mut self) -> Vec<PendingInsertLane> {
        std::mem::take(&mut self.pending_insert_lanes)
    }

    /// Rattache (sans historique) la courbe d'un paramètre du plugin `insert` : pour un projet qu'on ouvre. Remplace une courbe
    /// existante du même paramètre.
    pub fn attach_insert_lane(&mut self, node: NodeId, insert: u32, param: u32, points: Vec<MPoint>, mode: daw_engine::AutoMode) -> Result<(), SessionError> {
        let n = node.0 as usize;
        let target = AutoTarget::InsertParam { insert, param };
        let lane = MLane { target, points, mode };
        if !lane.valid() || !self.graph.nodes.get(n).ok_or(SessionError::UnknownNode(node))?.inserts.iter().any(|i| i.id == insert) {
            return Err(SessionError::BadLane);
        }
        let list = &mut self.lanes[n];
        match list.iter().position(|l| l.target == target) {
            Some(i) => list[i] = lane,
            None => list.push(lane),
        }
        self.sync_node(n)
    }

    /// Mode d'une courbe existante, **sans historique** (le mode « de lecture » que l'écriture d'automation impose le temps d'un
    /// geste). Rend `false` si la courbe n'existe pas.
    pub fn set_lane_mode_untracked(&mut self, node: NodeId, target: AutoTarget, mode: daw_engine::AutoMode) -> bool {
        let n = node.0 as usize;
        match self.lanes.get_mut(n).and_then(|v| v.iter_mut().find(|l| l.target == target)) {
            Some(l) if l.mode == mode => true,
            Some(l) => {
                l.mode = mode;
                self.sync_node(n).is_ok()
            }
            None => false,
        }
    }

    /// Toutes les courbes d'automation d'un nœud.
    pub fn lanes(&self, node: NodeId) -> &[MLane] {
        self.lanes.get(node.0 as usize).map_or(&[], Vec::as_slice)
    }
    pub fn lane(&self, node: NodeId, target: AutoTarget) -> Option<&MLane> {
        self.lanes.get(node.0 as usize)?.iter().find(|l| l.target == target)
    }
    fn locate_midi(&self, id: MidiClipId) -> Option<(usize, usize)> {
        self.midi.iter().enumerate().find_map(|(n, v)| v.iter().position(|(x, _)| *x == id).map(|i| (n, i)))
    }

    /// Recalcule, pour un nœud, les clips de notes et courbes du moteur à partir du temps musical.
    fn sync_node(&mut self, n: usize) -> Result<(), SessionError> {
        let (tm, sr) = (&self.timing.tempo, self.sample_rate);
        let clips = self.midi[n].iter().map(|(_, c)| music::clip_to_engine(c, tm, sr)).collect::<Option<Vec<_>>>().ok_or(SessionError::TimeOverflow)?;
        // Une courbe dont la cible a disparu (envoi supprimé, plugin retiré) reste dans la session mais n'est pas donnée au moteur :
        // si la cible revient (annulation), la courbe revient avec elle.
        let node = &self.graph.nodes[n];
        let alive = |l: &MLane| match l.target {
            AutoTarget::SendGain { to } => node.sends.iter().any(|s| s.to == to),
            AutoTarget::InsertParam { insert, .. } => node.inserts.iter().any(|i| i.id == insert),
            _ => true,
        };
        let lanes = self.lanes[n].iter().filter(|l| alive(l)).map(|l| music::lane_to_engine(l, tm, sr)).collect::<Option<Vec<_>>>().ok_or(SessionError::TimeOverflow)?;
        self.graph.nodes[n].midi_clips = clips;
        self.graph.nodes[n].automation = lanes;
        Ok(())
    }

    /// Carte des temps du moteur (tempo et chiffrage en échantillons), pour le transport donné aux plugins.
    fn rebuild_time_map(&mut self) {
        let (tm, mm, sr) = (&self.timing.tempo, &self.timing.meters, self.sample_rate);
        let tempo: Vec<daw_engine::TempoSeg> = tm
            .changes()
            .iter()
            .filter_map(|(pos, t)| Some(daw_engine::TempoSeg { start_sample: tm.to_samples(*pos, sr)?, start_beat: pos.as_f64() * 4.0, bpm: t.as_f64() }))
            .collect();
        let meters: Vec<daw_engine::MeterSeg> = mm
            .changes()
            .iter()
            .filter_map(|(bar, m)| Some(daw_engine::MeterSeg { start_beat: mm.bar_start(*bar)?.as_f64() * 4.0, start_bar: *bar + 1, num: m.num() as u16, den: m.den() as u16 }))
            .collect();
        if !tempo.is_empty() && !meters.is_empty() {
            self.graph.time_map = Arc::new(daw_engine::TimeMap { tempo, meters });
        }
    }

    /// L'instantané de la piste d'accords du moteur (voir `chords::ChordTrack::snapshot`).
    fn rebuild_chords(&mut self) {
        let snap = self.chords.snapshot(&self.graph.time_map.meters, self.chord_revision);
        self.graph.chords = Arc::new(snap);
    }

    fn sync_all(&mut self) -> Result<(), SessionError> {
        self.rebuild_time_map();
        self.rebuild_chords();
        for n in 0..self.graph.nodes.len() {
            self.sync_node(n)?;
        }
        Ok(())
    }

    /// Déclare un fichier audio du projet (chemin relatif au dossier du projet, de préférence). Les clips créés sur ces
    /// données sont sauvegardés comme références à ce chemin. Redéclarer les mêmes données change le chemin.
    pub fn register_asset(&mut self, path: &str, data: Arc<AudioData>) -> Arc<AudioData> {
        match self.assets.iter_mut().find(|(d, _)| Arc::ptr_eq(d, &data)) {
            Some(a) => a.1 = path.to_owned(),
            None => self.assets.push((data.clone(), path.to_owned())),
        }
        data
    }

    /// Chemin sous lequel des données sont enregistrées (`None` : pas déclarées).
    pub fn asset_path(&self, data: &Arc<AudioData>) -> Option<&str> {
        self.assets.iter().find(|(d, _)| Arc::ptr_eq(d, data)).map(|(_, p)| p.as_str())
    }

    /// Fichiers référencés en place qui étaient introuvables à l'ouverture (chemins).
    pub fn missing_assets(&self) -> &[String] {
        &self.missing
    }

    /// Les données de ce clip remplacent-elles un fichier introuvable ?
    pub fn is_missing(&self, data: &Arc<AudioData>) -> bool {
        self.asset_path(data).is_some_and(|p| self.missing.iter().any(|m| m == p))
    }

    /// Fichiers audio déclarés : les données en mémoire et le chemin sous lequel elles sont enregistrées.
    pub fn assets(&self) -> &[(Arc<AudioData>, String)] {
        &self.assets
    }

    /// Données audio utilisées par des clips mais pas déclarées (prises, fichiers importés) : sans chemin, le projet ne peut
    /// pas les enregistrer. Chacune une seule fois, dans l'ordre des nœuds et des clips.
    pub fn unregistered_audio(&self) -> Vec<Arc<AudioData>> {
        let mut out: Vec<Arc<AudioData>> = Vec::new();
        for n in &self.graph.nodes {
            for c in &n.clips {
                let known = self.assets.iter().any(|(d, _)| Arc::ptr_eq(d, &c.data)) || out.iter().any(|d| Arc::ptr_eq(d, &c.data));
                if !known {
                    out.push(Arc::clone(&c.data));
                }
            }
        }
        out
    }

    /// Ajoute un effet inséré sur un nœud, **hors historique** (comme le chargement d'un projet : on ne l'annule pas).
    pub fn add_insert(&mut self, node: NodeId, name: &str, factory: daw_engine::InsertFactory) -> Option<daw_engine::InsertId> {
        self.graph.add_insert(node, name, factory)
    }

    /// Le graphe à donner au moteur (`EngineHandle::load_graph`).
    pub fn graph(&self) -> &GraphDesc {
        &self.graph
    }

    pub fn clip_ids(&self, node: NodeId) -> &[ClipId] {
        self.ids.get(node.0 as usize).map_or(&[], |v| v)
    }

    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        let (n, i) = self.locate(id)?;
        Some(&self.graph.nodes[n].clips[i])
    }

    fn locate(&self, id: ClipId) -> Option<(usize, usize)> {
        self.ids.iter().enumerate().find_map(|(n, v)| v.iter().position(|&x| x == id).map(|i| (n, i)))
    }

    fn fresh_id(&mut self) -> ClipId {
        self.next_id += 1;
        ClipId(self.next_id - 1)
    }

    /// Longueur du fondu des raccords entre prises, en millisecondes (0 : pas de raccord automatique).
    pub fn set_seam_ms(&mut self, ms: u32) {
        self.seam_ms = ms;
    }

    /// La piste d'accords.
    pub fn chord_track(&self) -> &chords::ChordTrack {
        &self.chords
    }

    /// La carte de structure (sections triées par début).
    pub fn structure(&self) -> &[structure::Section] {
        &self.structure
    }

    /// Remplace la carte de structure (une action annulable). Refusée sans trace si elle est invalide.
    pub fn set_structure(&mut self, label: &str, mut list: Vec<structure::Section>) -> Result<(), SessionError> {
        structure::normalize(&mut list);
        structure::validate(&list).map_err(SessionError::BadStructure)?;
        if list == self.structure {
            return Ok(());
        }
        let old = self.structure.clone();
        self.push(label, Edit::SetStructure { old, new: list })
    }

    /// Les paroles (syllabes triées par instant puis par couplet).
    pub fn lyrics(&self) -> &[lyrics::Syllable] {
        &self.lyrics
    }

    /// Remplace les paroles (une action annulable). Refusées sans trace si elles sont invalides.
    pub fn set_lyrics(&mut self, label: &str, mut list: Vec<lyrics::Syllable>) -> Result<(), SessionError> {
        lyrics::normalize(&mut list);
        lyrics::validate(&list).map_err(SessionError::BadLyrics)?;
        if list == self.lyrics {
            return Ok(());
        }
        let old = self.lyrics.clone();
        self.push(label, Edit::SetLyrics { old, new: list })
    }

    /// Numéro qui change à chaque modification de la piste d'accords (annulations comprises).
    pub fn chord_revision(&self) -> u32 {
        self.chord_revision
    }

    /// Remplace la piste d'accords (une action annulable). Refusée sans trace si elle est invalide.
    pub fn set_chord_track(&mut self, label: &str, mut track: chords::ChordTrack) -> Result<(), SessionError> {
        track.normalize();
        track.validate().map_err(|e| SessionError::BadChordTrack(e.to_string()))?;
        if track == self.chords {
            return Ok(());
        }
        let old = Box::new(self.chords.clone());
        self.push(label, Edit::SetChordTrack { old, new: Box::new(track) })
    }

    /// Modifie la piste d'accords avec `f` (copie, modification, validation, une action annulable).
    pub fn edit_chord_track(&mut self, label: &str, f: impl FnOnce(&mut chords::ChordTrack)) -> Result<(), SessionError> {
        let mut t = self.chords.clone();
        f(&mut t);
        self.set_chord_track(label, t)
    }

    /// Importe un fichier `flibuste-carte` : les voies présentes remplacent celles de la piste, les absentes ne sont pas touchées.
    /// Rend ce qui a été ignoré. Une seule action annulable (« Importer une carte »).
    pub fn import_carte(&mut self, text: &str) -> Result<chords::ImportReport, SessionError> {
        let (track, report) = chords::ChordTrack::from_carte_json(&self.chords, text).map_err(|e| SessionError::BadChordTrack(e.to_string()))?;
        self.set_chord_track("Importer une carte", track)?;
        Ok(report)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// Intitulé de la prochaine action à annuler / rétablir (pour les menus).
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|t| t.label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|t| t.label.as_str())
    }

    // ---- application ------------------------------------------------------------------------------------------

    fn apply(&mut self, e: &Edit) -> Result<(), SessionError> {
        match e {
            Edit::InsertNode { .. } | Edit::RemoveNode { .. } | Edit::SetName { .. } | Edit::SetColor { .. } | Edit::SetOutput { .. } | Edit::SetSends { .. } | Edit::SetOrder { .. } | Edit::SetPhys { .. } => return self.apply_node_edit(e),
            Edit::AddEffect { .. } | Edit::RemoveEffect { .. } | Edit::MoveEffect { .. } => return self.apply_effect_edit(e),
            Edit::InsertClip { node, index, id, clip } => {
                let n = node.0 as usize;
                let nd = self.graph.nodes.get_mut(n).ok_or(SessionError::UnknownNode(*node))?;
                if *index > nd.clips.len() {
                    return Err(SessionError::UnknownNode(*node));
                }
                nd.clips.insert(*index, clip.clone());
                self.ids[n].insert(*index, *id);
            }
            Edit::RemoveClip { id, .. } => {
                let (n, i) = self.locate(*id).ok_or(SessionError::UnknownClip(*id))?;
                self.graph.nodes[n].clips.remove(i);
                self.ids[n].remove(i);
            }
            Edit::ReplaceClip { id, new, .. } => {
                let (n, i) = self.locate(*id).ok_or(SessionError::UnknownClip(*id))?;
                self.graph.nodes[n].clips[i] = new.clone();
            }
            Edit::SetParams { node, new, .. } => {
                let nd = self.graph.nodes.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(*node))?;
                nd.params = new.clone();
            }
            Edit::InsertMidi { node, index, id, clip } => {
                let n = node.0 as usize;
                let list = self.midi.get_mut(n).ok_or(SessionError::UnknownNode(*node))?;
                list.insert((*index).min(list.len()), (*id, clip.clone()));
                self.sync_node(n)?;
            }
            Edit::RemoveMidi { id, .. } => {
                let (n, i) = self.locate_midi(*id).ok_or(SessionError::UnknownMidiClip(*id))?;
                self.midi[n].remove(i);
                self.sync_node(n)?;
            }
            Edit::ReplaceMidi { id, new, .. } => {
                let (n, i) = self.locate_midi(*id).ok_or(SessionError::UnknownMidiClip(*id))?;
                self.midi[n][i].1 = new.clone();
                self.sync_node(n)?;
            }
            Edit::SetLane { node, target, index, new, .. } => {
                let n = node.0 as usize;
                let list = self.lanes.get_mut(n).ok_or(SessionError::UnknownNode(*node))?;
                list.retain(|l| l.target != *target);
                if let Some(l) = new {
                    list.insert((*index).min(list.len()), l.clone());
                }
                self.sync_node(n)?;
            }
            Edit::SetTiming { new, .. } => {
                self.timing = new.clone();
                self.sync_all()?;
            }
            Edit::SetGroups { .. } | Edit::SetReturnGroups { .. } => self.apply_groups_edit(e),
            Edit::SetStructure { new, .. } => {
                self.structure = new.clone();
                self.chord_revision = self.chord_revision.wrapping_add(1); // la page relit chords + structure ensemble
            }
            Edit::SetLyrics { new, .. } => {
                self.lyrics = new.clone();
                self.chord_revision = self.chord_revision.wrapping_add(1); // la page relit chords + structure + paroles ensemble
            }
            Edit::SetChordTrack { new, .. } => {
                self.chords = (**new).clone();
                self.chord_revision = self.chord_revision.wrapping_add(1);
                self.rebuild_chords();
            }
        }
        Ok(())
    }

    /// Applique une édition et l'inscrit dans la transaction en cours (ou dans une transaction à elle seule).
    fn push(&mut self, label: &str, e: Edit) -> Result<(), SessionError> {
        if let Err(err) = self.apply(&e) {
            if err == SessionError::TimeOverflow {
                let _ = self.apply(&e.flip()); // l'édition avait modifié les données avant d'échouer : on la retire
            }
            return Err(err);
        }
        self.redo.clear();
        match self.open.as_mut() {
            Some(t) => t.edits.push(e),
            None => self.undo.push(Transaction { label: label.to_owned(), edits: vec![e] }),
        }
        Ok(())
    }

    /// Regroupe les éditions de `f` en une seule action annulable. Si `f` échoue, tout ce qu'elle avait fait est
    /// défait et rien n'est inscrit dans l'historique.
    pub fn group<T>(&mut self, label: &str, f: impl FnOnce(&mut Session) -> Result<T, SessionError>) -> Result<T, SessionError> {
        if self.open.is_some() {
            // Groupe imbriqué : il se fond dans le groupe extérieur.
            return f(self);
        }
        self.open = Some(Transaction { label: label.to_owned(), edits: Vec::new() });
        let r = f(self);
        let t = self.open.take().unwrap_or_default();
        match r {
            Ok(v) => {
                if !t.edits.is_empty() {
                    self.undo.push(t);
                }
                Ok(v)
            }
            Err(e) => {
                for ed in t.edits.iter().rev() {
                    // Les inverses d'éditions qui viennent de réussir réussissent.
                    let _ = self.apply(&ed.flip());
                }
                Err(e)
            }
        }
    }

    /// Début d'une fusion : tout ce qui s'inscrit ensuite dans l'historique devient, à `merge_end`, une seule action
    /// annulable (un script lancé = un pas d'annulation). Refusé si une fusion est déjà ouverte.
    pub fn merge_begin(&mut self, label: &str) -> bool {
        if self.merge.is_some() {
            return false;
        }
        self.merge = Some((self.undo.len(), label.to_owned()));
        true
    }

    /// Une fusion est-elle ouverte ?
    pub fn merge_open(&self) -> bool {
        self.merge.is_some()
    }

    /// Fin de la fusion : les actions faites depuis `merge_begin` n'en forment plus qu'une. Rend leur nombre (0 : rien à fusionner).
    pub fn merge_end(&mut self) -> usize {
        let Some((start, label)) = self.merge.take() else { return 0 };
        let start = start.min(self.undo.len());
        let count = self.undo.len() - start;
        if count > 0 {
            let edits: Vec<Edit> = self.undo.drain(start..).flat_map(|t| t.edits).collect();
            self.undo.push(Transaction { label, edits });
        }
        count
    }

    /// Abandon de la fusion : tout ce qui a été fait depuis `merge_begin` est défait et oublié (ni annulation ni rétablissement).
    pub fn merge_abort(&mut self) -> usize {
        let Some((start, _)) = self.merge.take() else { return 0 };
        let start = start.min(self.undo.len());
        let mut n = 0;
        while self.undo.len() > start {
            let t = self.undo.pop().unwrap_or_default();
            for e in t.edits.iter().rev() {
                let _ = self.apply(&e.flip());
            }
            n += 1;
        }
        self.redo.clear();
        n
    }

    pub fn undo(&mut self) -> bool {
        let Some(t) = self.undo.pop() else { return false };
        for e in t.edits.iter().rev() {
            let _ = self.apply(&e.flip());
        }
        self.redo.push(t);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(t) = self.redo.pop() else { return false };
        for e in &t.edits {
            let _ = self.apply(e);
        }
        self.undo.push(t);
        true
    }

    // ---- actions de l'utilisateur ------------------------------------------------------------------------------

    pub fn add_clip(&mut self, node: NodeId, clip: Clip) -> Result<ClipId, SessionError> {
        let index = self.graph.nodes.get(node.0 as usize).ok_or(SessionError::UnknownNode(node))?.clips.len();
        let id = self.fresh_id();
        self.push("Ajouter un clip", Edit::InsertClip { node, index, id, clip })?;
        Ok(id)
    }

    pub fn remove_clip(&mut self, id: ClipId) -> Result<(), SessionError> {
        let (n, index) = self.locate(id).ok_or(SessionError::UnknownClip(id))?;
        let clip = self.graph.nodes[n].clips[index].clone();
        self.push("Supprimer un clip", Edit::RemoveClip { node: NodeId(n as u32), index, id, clip })
    }

    fn replace(&mut self, label: &str, id: ClipId, f: impl FnOnce(&Clip) -> Result<Clip, EditError>) -> Result<(), SessionError> {
        let old = self.clip(id).ok_or(SessionError::UnknownClip(id))?.clone();
        let new = f(&old)?;
        self.push(label, Edit::ReplaceClip { id, old, new })
    }

    pub fn move_clip(&mut self, id: ClipId, start: u64) -> Result<(), SessionError> {
        self.replace("Déplacer un clip", id, |c| Ok(c.moved(start)))
    }

    pub fn trim_start(&mut self, id: ClipId, new_start: u64) -> Result<(), SessionError> {
        self.replace("Rogner le début", id, |c| c.trim_start(new_start))
    }

    pub fn trim_end(&mut self, id: ClipId, new_end: u64) -> Result<(), SessionError> {
        self.replace("Rogner la fin", id, |c| c.trim_end(new_end))
    }

    /// Remplace un clip par une version modifiée (gain, fondus, ...). Le clip doit rester valide.
    pub fn set_clip(&mut self, id: ClipId, clip: Clip) -> Result<(), SessionError> {
        self.replace("Modifier un clip", id, |_| Ok(clip))
    }

    /// Découpe : le clip d'origine garde son identifiant (partie gauche), la partie droite en reçoit un nouveau,
    /// inséré juste après. Une seule action annulable.
    pub fn split(&mut self, id: ClipId, at: u64) -> Result<(ClipId, ClipId), SessionError> {
        let (n, index) = self.locate(id).ok_or(SessionError::UnknownClip(id))?;
        let old = self.graph.nodes[n].clips[index].clone();
        let (left, right) = old.split(at)?;
        let rid = self.fresh_id();
        self.group("Découper", |s| {
            s.push("Découper", Edit::ReplaceClip { id, old, new: left })?;
            s.push("Découper", Edit::InsertClip { node: NodeId(n as u32), index: index + 1, id: rid, clip: right })
        })?;
        Ok((id, rid))
    }

    /// Fondu enchaîné entre `a` (le premier) et `b` (qui doit chevaucher la fin de `a`).
    pub fn crossfade(&mut self, a: ClipId, b: ClipId, shape: FadeShape) -> Result<(), SessionError> {
        let ca = self.clip(a).ok_or(SessionError::UnknownClip(a))?.clone();
        let cb = self.clip(b).ok_or(SessionError::UnknownClip(b))?.clone();
        let (na, nb) = crossfade(&ca, &cb, shape)?;
        self.group("Fondu enchaîné", |s| {
            s.push("Fondu enchaîné", Edit::ReplaceClip { id: a, old: ca, new: na })?;
            s.push("Fondu enchaîné", Edit::ReplaceClip { id: b, old: cb, new: nb })
        })
    }

    /// Longueur du fondu enchaîné entre `a` (qui sort) et `b` (qui entre) : rogne les bords des deux clips, contenu en place.
    /// Une seule action annulable ; voir `xfade`.
    pub fn set_crossfade_length(&mut self, a: ClipId, b: ClipId, len: u64, anchor: xfade::Anchor) -> Result<(), SessionError> {
        let ca = self.clip(a).ok_or(SessionError::UnknownClip(a))?.clone();
        let cb = self.clip(b).ok_or(SessionError::UnknownClip(b))?.clone();
        let (na, nb) = xfade::resize(&ca, &cb, len, anchor)?;
        self.group("Longueur du fondu enchaîné", |s| {
            s.push("Longueur du fondu enchaîné", Edit::ReplaceClip { id: a, old: ca, new: na })?;
            s.push("Longueur du fondu enchaîné", Edit::ReplaceClip { id: b, old: cb, new: nb })
        })
    }

    /// Réglage d'état de mixage **sans historique** (fader, panoramique, muet, solo tirés en direct) : garde le graphe de la
    /// session à jour — il est rechargé dans le moteur après chaque édition — sans polluer l'annulation de gestes continus.
    pub fn set_params_untracked(&mut self, node: NodeId, params: NodeParams) -> Result<(), SessionError> {
        self.graph.nodes.get_mut(node.0 as usize).ok_or(SessionError::UnknownNode(node))?.params = params;
        Ok(())
    }

    /// Défait la dernière action **et l'oublie** (elle ne reste pas dans la pile de rétablissement) : pour une édition
    /// que le moteur a refusée après coup.
    pub fn rollback_last(&mut self) -> bool {
        let done = self.undo();
        if done {
            self.redo.pop();
        }
        done
    }

    pub fn set_params(&mut self, node: NodeId, params: NodeParams) -> Result<(), SessionError> {
        let old = self.graph.nodes.get(node.0 as usize).ok_or(SessionError::UnknownNode(node))?.params.clone();
        self.push("Régler une piste", Edit::SetParams { node, old, new: params })
    }
}

// ---- temps musical : clips de notes, automation, tempo -----------------------------------------------------------

impl Session {
    pub fn add_midi_clip(&mut self, node: NodeId, clip: MClip) -> Result<MidiClipId, SessionError> {
        if !clip.valid() {
            return Err(SessionError::BadMidi);
        }
        let mut clip = clip;
        clip.sort();
        let index = self.midi.get(node.0 as usize).ok_or(SessionError::UnknownNode(node))?.len();
        let id = MidiClipId(self.next_midi_id);
        self.next_midi_id += 1;
        self.push("Ajouter un clip de notes", Edit::InsertMidi { node, index, id, clip })?;
        Ok(id)
    }

    pub fn remove_midi_clip(&mut self, id: MidiClipId) -> Result<(), SessionError> {
        let (n, index) = self.locate_midi(id).ok_or(SessionError::UnknownMidiClip(id))?;
        let clip = self.midi[n][index].1.clone();
        self.push("Supprimer un clip de notes", Edit::RemoveMidi { node: NodeId(n as u32), index, id, clip })
    }

    fn replace_midi(&mut self, label: &str, id: MidiClipId, f: impl FnOnce(&MClip) -> Result<MClip, SessionError>) -> Result<(), SessionError> {
        let old = self.midi_clip(id).ok_or(SessionError::UnknownMidiClip(id))?.clone();
        let mut new = f(&old)?;
        if !new.valid() {
            return Err(SessionError::BadMidi);
        }
        new.sort();
        self.push(label, Edit::ReplaceMidi { id, old, new })
    }

    pub fn move_midi_clip(&mut self, id: MidiClipId, start: Frac) -> Result<(), SessionError> {
        self.replace_midi("Déplacer un clip de notes", id, |c| Ok(MClip { start, ..c.clone() }))
    }

    /// Change la longueur du clip ; les notes qui ne tiendraient plus (début au-delà de la nouvelle fin) font refuser.
    pub fn resize_midi_clip(&mut self, id: MidiClipId, len: Frac) -> Result<(), SessionError> {
        self.replace_midi("Redimensionner un clip de notes", id, |c| Ok(MClip { len, ..c.clone() }))
    }

    /// Ajoute une note (relative au clip) ; retourne son indice dans la liste triée.
    pub fn add_note(&mut self, id: MidiClipId, note: MNote) -> Result<usize, SessionError> {
        self.replace_midi("Ajouter une note", id, |c| {
            let mut c = c.clone();
            // Une note ajoutée doit démarrer dans la partie visible du clip.
            if note.start < c.offset || c.offset.checked_add(c.len).is_none_or(|end| note.start >= end) {
                return Err(SessionError::BadMidi);
            }
            c.notes.push(note);
            Ok(c)
        })?;
        Ok(self.note_index(id, &note))
    }

    pub fn remove_note(&mut self, id: MidiClipId, index: usize) -> Result<(), SessionError> {
        self.replace_midi("Supprimer une note", id, |c| {
            let mut c = c.clone();
            if index >= c.notes.len() {
                return Err(SessionError::BadNoteIndex(index));
            }
            c.notes.remove(index);
            Ok(c)
        })
    }

    /// Modifie la note `index` (début, durée, touche, vélocité...) ; retourne son nouvel indice après tri.
    pub fn edit_note(&mut self, id: MidiClipId, index: usize, f: impl FnOnce(MNote) -> MNote) -> Result<usize, SessionError> {
        let mut edited = None;
        self.replace_midi("Modifier une note", id, |c| {
            let mut c = c.clone();
            let n = c.notes.get_mut(index).ok_or(SessionError::BadNoteIndex(index))?;
            *n = f(*n);
            edited = Some(*n);
            Ok(c)
        })?;
        Ok(self.note_index(id, &edited.expect("renseigné ci-dessus")))
    }

    pub fn move_note(&mut self, id: MidiClipId, index: usize, start: Frac, key: u8) -> Result<usize, SessionError> {
        self.edit_note(id, index, |n| MNote { start, key, ..n })
    }

    pub fn resize_note(&mut self, id: MidiClipId, index: usize, len: Frac) -> Result<usize, SessionError> {
        self.edit_note(id, index, |n| MNote { len, ..n })
    }

    /// Modifie plusieurs notes **en une seule action annulable** (les indices sont ceux d'avant l'action). Rend le nouvel indice de
    /// chaque note modifiée, dans l'ordre de `edits`. Une note dont le début sortirait de la partie visible du clip fait tout refuser.
    pub fn edit_notes(&mut self, id: MidiClipId, label: &str, edits: &[(usize, NoteEdit)]) -> Result<Vec<usize>, SessionError> {
        let mut tags: Vec<usize> = Vec::new();
        self.replace_midi(label, id, |c| {
            let mut c = c.clone();
            let end = c.offset.checked_add(c.len).ok_or(SessionError::TimeOverflow)?;
            let mut touched = vec![false; c.notes.len()];
            for (i, e) in edits {
                let n = c.notes.get_mut(*i).ok_or(SessionError::BadNoteIndex(*i))?;
                if touched[*i] {
                    return Err(SessionError::BadNoteIndex(*i));
                }
                touched[*i] = true;
                *n = MNote { start: e.start.unwrap_or(n.start), len: e.len.unwrap_or(n.len), key: e.key.unwrap_or(n.key), velocity: e.velocity.unwrap_or(n.velocity), channel: e.channel.unwrap_or(n.channel), voice: e.voice.unwrap_or(n.voice) };
                if n.channel > 15 || !(1..=4).contains(&n.voice) || n.key > 127 || !(n.velocity > 0.0 && n.velocity <= 1.0) {
                    return Err(SessionError::BadMidi);
                }
                if n.start < c.offset || n.start >= end {
                    return Err(SessionError::BadMidi);
                }
            }
            // Même tri (stable) que `MClip::sort` : on sait où chaque note modifiée atterrit.
            let mut order: Vec<usize> = (0..c.notes.len()).collect();
            order.sort_by(|&a, &b| {
                let (x, y) = (&c.notes[a], &c.notes[b]);
                x.start.cmp(&y.start).then(x.key.cmp(&y.key)).then(x.channel.cmp(&y.channel))
            });
            tags = edits.iter().map(|(i, _)| order.iter().position(|o| o == i).unwrap_or(0)).collect();
            Ok(c)
        })?;
        Ok(tags)
    }

    /// Ajoute plusieurs notes en une action ; rend leur indice final, dans l'ordre donné.
    pub fn add_notes(&mut self, id: MidiClipId, notes: &[MNote]) -> Result<Vec<usize>, SessionError> {
        self.replace_midi("Ajouter des notes", id, |c| {
            let mut c = c.clone();
            let end = c.offset.checked_add(c.len).ok_or(SessionError::TimeOverflow)?;
            for n in notes {
                if n.start < c.offset || n.start >= end {
                    return Err(SessionError::BadMidi);
                }
                c.notes.push(*n);
            }
            Ok(c)
        })?;
        Ok(notes.iter().map(|n| self.note_index(id, n)).collect())
    }

    /// Supprime plusieurs notes (indices d'avant l'action) en une action annulable.
    pub fn remove_notes(&mut self, id: MidiClipId, indices: &[usize]) -> Result<(), SessionError> {
        self.replace_midi("Supprimer des notes", id, |c| {
            let mut c = c.clone();
            let mut sorted = indices.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            if sorted.last().is_some_and(|&i| i >= c.notes.len()) {
                return Err(SessionError::BadNoteIndex(*sorted.last().expect("non vide")));
            }
            for &i in sorted.iter().rev() {
                c.notes.remove(i);
            }
            Ok(c)
        })
    }

    /// Aimante le début de toutes les notes sur la grille `grid` (en positions **absolues** : la grille suit la ligne
    /// de temps, pas le début du clip). Les durées sont conservées. Une note qui sortirait du clip reste en place.
    pub fn quantize_notes(&mut self, id: MidiClipId, grid: Frac) -> Result<(), SessionError> {
        self.replace_midi("Quantifier", id, |c| {
            let mut c = c.clone();
            for n in &mut c.notes {
                let abs = c.start.checked_add(n.start).ok_or(SessionError::TimeOverflow)?;
                let snapped = abs.snap_to(grid).ok_or(SessionError::TimeOverflow)?;
                if let Some(rel) = snapped.checked_sub(c.start) {
                    if rel < c.len {
                        n.start = rel;
                    }
                }
            }
            Ok(c)
        })
    }

    fn note_index(&self, id: MidiClipId, note: &MNote) -> usize {
        self.midi_clip(id).and_then(|c| c.notes.iter().position(|n| n == note)).unwrap_or(0)
    }

    /// Pose (ou remplace) la courbe d'automation d'un nœud pour sa cible.
    pub fn set_lane(&mut self, node: NodeId, lane: MLane) -> Result<(), SessionError> {
        self.set_lane_as("Modifier l'automation", node, lane)
    }

    /// Comme `set_lane`, avec le libellé de l'action d'annulation.
    pub fn set_lane_as(&mut self, label: &str, node: NodeId, lane: MLane) -> Result<(), SessionError> {
        if !lane.valid() {
            return Err(SessionError::BadLane);
        }
        // Une courbe se pose sur une cible qui existe (un envoi, un plugin de ce nœud) ; elle survit ensuite à sa disparition.
        let nd = self.graph.nodes.get(node.0 as usize).ok_or(SessionError::UnknownNode(node))?;
        let exists = match lane.target {
            AutoTarget::SendGain { to } => nd.sends.iter().any(|s| s.to == to),
            AutoTarget::InsertParam { insert, .. } => nd.inserts.iter().any(|i| i.id == insert),
            _ => true,
        };
        if !exists {
            return Err(SessionError::BadLane);
        }
        let list = self.lanes.get(node.0 as usize).ok_or(SessionError::UnknownNode(node))?;
        let (index, old) = match list.iter().position(|l| l.target == lane.target) {
            Some(i) => (i, Some(list[i].clone())),
            None => (list.len(), None),
        };
        let target = lane.target;
        self.push(label, Edit::SetLane { node, target, index, old, new: Some(lane) })
    }

    pub fn remove_lane(&mut self, node: NodeId, target: AutoTarget) -> Result<(), SessionError> {
        let list = self.lanes.get(node.0 as usize).ok_or(SessionError::UnknownNode(node))?;
        let i = list.iter().position(|l| l.target == target).ok_or(SessionError::BadLane)?;
        let old = Some(list[i].clone());
        self.push("Supprimer l'automation", Edit::SetLane { node, target, index: i, old, new: None })
    }

    fn set_timing(&mut self, label: &str, f: impl FnOnce(&mut Timing)) -> Result<(), SessionError> {
        let old = self.timing.clone();
        let mut new = old.clone();
        f(&mut new);
        if new == old {
            return Ok(());
        }
        self.push(label, Edit::SetTiming { old, new })
    }

    /// Pose un tempo à la position `at` : les clips de notes et courbes en temps musical **suivent** (leurs
    /// échantillons sont recalculés) ; les clips audio, eux, ne bougent pas.
    pub fn set_tempo(&mut self, at: Frac, tempo: Tempo) -> Result<(), SessionError> {
        self.set_timing("Changer le tempo", |t| t.tempo.set_tempo(at, tempo))
    }

    pub fn remove_tempo(&mut self, at: Frac) -> Result<(), SessionError> {
        if at == Frac::ZERO || !self.timing.tempo.changes().iter().any(|(p, _)| *p == at) {
            return Err(SessionError::Edit(EditError::OutOfRange));
        }
        self.set_timing("Supprimer un changement de tempo", |t| {
            let _ = t.tempo.remove_tempo(at);
        })
    }

    pub fn remove_meter(&mut self, bar: u32) -> Result<(), SessionError> {
        if !self.timing.meters.changes().iter().any(|(b, _)| *b == bar) || bar == 0 {
            return Err(SessionError::Edit(EditError::OutOfRange));
        }
        self.set_timing("Supprimer un changement de chiffrage", |t| {
            t.meters.remove_meter(bar);
        })
    }

    pub fn set_meter(&mut self, bar: u32, meter: Meter) -> Result<(), SessionError> {
        self.set_timing("Changer le chiffrage", |t| t.meters.set_meter(bar, meter))
    }
}

// ---- enregistrement MIDI ---------------------------------------------------------------------------------------

/// Ce qu'on change dans une note (rien = inchangé).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NoteEdit {
    pub start: Option<Frac>,
    pub len: Option<Frac>,
    pub key: Option<u8>,
    pub velocity: Option<f32>,
    /// Canal MIDI (0 à 15).
    pub channel: Option<u8>,
    /// Voix d'affichage (1 à 4).
    pub voice: Option<u8>,
}

/// Ce qu'une prise MIDI a recueilli (voir `EngineHandle::start_midi_record`).
#[derive(Debug, Clone, PartialEq)]
pub struct MidiTakeData {
    /// Position de la ligne de temps au début et à la fin de la prise (échantillons).
    pub start_pos: u64,
    pub stop_pos: u64,
    pub events: Vec<RecordedMidi>,
}

impl Session {
    /// Transforme une prise MIDI en clip de notes sur la piste `node` (une seule action annulable).
    ///
    /// - Les positions sont converties en temps musical avec la carte des tempos **puis arrondies au tick** (960 par
    ///   noire) : une note enregistrée tombe sur la grille fine de la ligne de temps, sans fractions à dénominateur
    ///   énorme. Précision : ½ tick (0,26 ms à 120 bpm).
    /// - `latency_comp` (échantillons) est retiré de chaque position : latence d'entrée à compenser. Une note qui
    ///   deviendrait antérieure au début du clip est ramenée à ce début.
    /// - Une note encore tenue à la fin de la prise est terminée à cet instant ; un début de note sur une touche déjà
    ///   tenue termine la précédente ; une fin sans début est ignorée ; une durée nulle devient 1 tick.
    /// - Une prise sans aucune note ne crée rien (`Ok(None)`).
    pub fn add_recorded_take(&mut self, node: NodeId, take: &MidiTakeData, latency_comp: u64) -> Result<Option<MidiClipId>, SessionError> {
        let Some(clip) = self.recorded_clip(take, latency_comp)? else { return Ok(None) };
        self.add_midi_clip(node, clip).map(Some)
    }

    /// Le clip de notes d'une prise MIDI (voir `add_recorded_take`), sans le poser ; `None` pour une prise vide.
    pub(crate) fn recorded_clip(&self, take: &MidiTakeData, latency_comp: u64) -> Result<Option<MClip>, SessionError> {
        let (tm, sr) = (&self.timing.tempo, self.sample_rate);
        let tick = |sample: u64| -> Result<u64, SessionError> {
            let pos = tm.to_frac(sample.saturating_sub(latency_comp), sr).ok_or(SessionError::TimeOverflow)?;
            TempoMap::to_ticks(pos, RECORD_PPQ).ok_or(SessionError::TimeOverflow)
        };
        let t0 = tick(take.start_pos)?;
        let t_end = tick(take.stop_pos)?.max(t0);
        let events = take.events.iter().map(|e| Ok((tick(e.pos)?, e.msg))).collect::<Result<Vec<_>, SessionError>>()?;
        music::build_clip(&events, t0, t_end, RECORD_PPQ).ok_or(SessionError::TimeOverflow)
    }

    /// Ajoute un événement de contrôle (contrôleur, molette, pression) à un clip ; retourne son indice dans la liste triée.
    pub fn add_event(&mut self, id: MidiClipId, event: MEvent) -> Result<usize, SessionError> {
        self.replace_midi("Ajouter un événement", id, |c| {
            let mut c = c.clone();
            // Comme pour une note : l'événement ajouté doit tomber dans la partie visible du clip.
            if event.start < c.offset || c.offset.checked_add(c.len).is_none_or(|end| event.start >= end) {
                return Err(SessionError::BadMidi);
            }
            c.events.push(event);
            Ok(c)
        })?;
        Ok(self.midi_clip(id).and_then(|c| c.events.iter().rposition(|e| *e == event)).unwrap_or(0))
    }

    /// Remplace d'un coup les événements d'une « voie » (un type de message, un contrôleur) **dans la partie visible du clip** : une seule
    /// action annulable (courbe de modulation dessinée, pédale…). Les événements de la voie cachés par le rognage sont conservés ; ceux des
    /// autres voies ne bougent pas ; les nouveaux doivent tomber dans la partie visible.
    pub fn replace_events(&mut self, label: &str, id: MidiClipId, is_lane: impl Fn(&MEvent) -> bool, new: &[MEvent]) -> Result<(), SessionError> {
        self.replace_midi(label, id, |c| {
            let mut c = c.clone();
            let end = c.offset.checked_add(c.len).ok_or(SessionError::TimeOverflow)?;
            if new.iter().any(|e| e.start < c.offset || e.start >= end) {
                return Err(SessionError::BadMidi);
            }
            c.events.retain(|e| !(is_lane(e) && e.start >= c.offset && e.start < end));
            c.events.extend_from_slice(new);
            Ok(c)
        })
    }

    pub fn remove_event(&mut self, id: MidiClipId, index: usize) -> Result<(), SessionError> {
        self.replace_midi("Supprimer un événement", id, |c| {
            let mut c = c.clone();
            if index >= c.events.len() {
                return Err(SessionError::BadNoteIndex(index));
            }
            c.events.remove(index);
            Ok(c)
        })
    }
}

// ---- clips entre pistes, fenêtre et découpe des clips de notes -----------------------------------------------------

impl Session {
    fn require_track(&self, node: NodeId) -> Result<usize, SessionError> {
        let n = node.0 as usize;
        match self.graph.nodes.get(n) {
            None => Err(SessionError::UnknownNode(node)),
            Some(nd) if matches!(nd.kind, daw_engine::NodeKind::Track(_)) => Ok(n),
            Some(_) => Err(SessionError::NotATrack(node)),
        }
    }

    /// Déplace un clip audio à `start` **sur la piste `node`** (la même ou une autre). Le clip garde son identifiant.
    /// Une seule action annulable.
    pub fn move_clip_to(&mut self, id: ClipId, node: NodeId, start: u64) -> Result<(), SessionError> {
        let target = self.require_track(node)?;
        let (n, index) = self.locate(id).ok_or(SessionError::UnknownClip(id))?;
        if n == target {
            return self.move_clip(id, start);
        }
        let clip = self.graph.nodes[n].clips[index].clone();
        let moved = clip.moved(start);
        let at = self.graph.nodes[target].clips.len();
        self.group("Déplacer un clip", |s| {
            s.push("Déplacer un clip", Edit::RemoveClip { node: NodeId(n as u32), index, id, clip })?;
            s.push("Déplacer un clip", Edit::InsertClip { node, index: at, id, clip: moved })
        })
    }

    /// Comme `move_clip_to`, pour un clip de notes (position musicale).
    pub fn move_midi_clip_to(&mut self, id: MidiClipId, node: NodeId, start: Frac) -> Result<(), SessionError> {
        let target = self.require_track(node)?;
        let (n, index) = self.locate_midi(id).ok_or(SessionError::UnknownMidiClip(id))?;
        if n == target {
            return self.move_midi_clip(id, start);
        }
        let clip = self.midi[n][index].1.clone();
        let moved = MClip { start, ..clip.clone() };
        let at = self.midi[target].len();
        self.group("Déplacer un clip de notes", |s| {
            s.push("Déplacer un clip de notes", Edit::RemoveMidi { node: NodeId(n as u32), index, id, clip })?;
            s.push("Déplacer un clip de notes", Edit::InsertMidi { node, index: at, id, clip: moved })
        })
    }

    /// Fenêtre visible d'un clip de notes : nouveau début, longueur et décalage dans la matière (rogner sans rien perdre).
    /// Les fondus sont ramenés à la nouvelle longueur.
    pub fn set_midi_window(&mut self, id: MidiClipId, start: Frac, len: Frac, offset: Frac) -> Result<(), SessionError> {
        self.replace_midi("Rogner un clip de notes", id, |c| {
            let mut c = c.clone();
            c.start = start;
            c.len = len;
            c.offset = offset;
            c.fade_in = c.fade_in.min(len);
            c.fade_out = c.fade_out.min(len);
            Ok(c)
        })
    }

    /// Fondus de vélocité d'un clip de notes (bornés à sa longueur).
    pub fn set_midi_fades(&mut self, id: MidiClipId, fade_in: Option<Frac>, fade_out: Option<Frac>) -> Result<(), SessionError> {
        self.replace_midi("Modifier un clip de notes", id, |c| {
            let mut c = c.clone();
            if let Some(f) = fade_in {
                c.fade_in = f.min(c.len);
            }
            if let Some(f) = fade_out {
                c.fade_out = f.min(c.len);
            }
            Ok(c)
        })
    }

    /// Découpe un clip de notes en `at` (strictement à l'intérieur). **Non destructive** : les deux moitiés gardent toute la
    /// matière et n'en montrent chacune que sa part ; une note qui commence dans la moitié gauche et dépasse la coupure est
    /// coupée net (la moitié droite ne la redéclenche pas). Le clip d'origine garde son identifiant (moitié gauche).
    pub fn split_midi_clip(&mut self, id: MidiClipId, at: Frac) -> Result<(MidiClipId, MidiClipId), SessionError> {
        let (n, index) = self.locate_midi(id).ok_or(SessionError::UnknownMidiClip(id))?;
        let old = self.midi[n][index].1.clone();
        let end = old.start.checked_add(old.len).ok_or(SessionError::TimeOverflow)?;
        if at <= old.start || at >= end {
            return Err(SessionError::Edit(EditError::OutOfRange));
        }
        let d = at.checked_sub(old.start).ok_or(SessionError::TimeOverflow)?;
        let left = MClip { len: d, fade_out: Frac::ZERO, fade_in: old.fade_in.min(d), ..old.clone() };
        let right = MClip { start: at, offset: old.offset.checked_add(d).ok_or(SessionError::TimeOverflow)?, len: old.len.checked_sub(d).ok_or(SessionError::TimeOverflow)?, fade_in: Frac::ZERO, fade_out: old.fade_out, ..old.clone() };
        let rid = MidiClipId(self.next_midi_id);
        self.next_midi_id += 1;
        let node = NodeId(n as u32);
        self.group("Découper", |s| {
            s.push("Découper", Edit::ReplaceMidi { id, old, new: left })?;
            s.push("Découper", Edit::InsertMidi { node, index: index + 1, id: rid, clip: right })
        })?;
        Ok((id, rid))
    }
}
