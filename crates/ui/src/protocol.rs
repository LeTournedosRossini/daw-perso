//! Messages du protocole (voir `docs/protocole-ui.md`). Les noms de champs sont ceux du JSON.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportAction {
    Play,
    Stop,
    Toggle,
}

fn default_tail() -> f64 {
    5.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportScope {
    Project,
    Loop,
    Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportMode {
    Mix,
    Stems,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Wav16,
    Wav24,
    Wav32f,
    Flac16,
    Flac24,
}

/// Commande (page → moteur).
/// Un retour de monitoring tel que la page le voit : son adresse (secrète) sur le réseau local et son code QR.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MonitorReturn {
    pub node: u32,
    pub name: String,
    pub url: String,
    pub qr: String,
    /// Rôle de la télécommande de ce retour (0 : retour seulement, 1 : retour + transport).
    pub role: u8,
    /// Les droits donnés à cette adresse ont expiré (voir `remote.elevated_hours` des préférences) : elle ne règle plus que son retour.
    pub expired: bool,
}

/// Une section de la carte de structure, telle que la page la lit et l'écrit (positions en fractions de ronde `[n, d]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionDto {
    pub at: [u64; 2],
    pub dur: [u64; 2],
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

/// Une syllabe des paroles (position en fraction de ronde `[n, d]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LyricDto {
    pub at: [u64; 2],
    pub text: String,
    #[serde(default)]
    pub hyphen: bool,
    #[serde(default = "first_verse")]
    pub verse: u8,
    /// Fin du trait de prolongation (mélisme), fraction de ronde ; absent : pas de trait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<[u64; 2]>,
}

fn first_verse() -> u8 {
    1
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    GetProject,
    /// Demande les préférences (événement `prefs`).
    GetPrefs,
    /// Enregistre les préférences (toutes données à chaque fois). Le métronome s'applique tout de suite ; le pilote, le tampon et les
    /// fils de calcul au prochain démarrage de l'application (`restart_needed` dans la réponse).
    SetPrefs { prefs: crate::prefs::Prefs },
    Transport { action: TransportAction },
    Seek { pos: u64 },
    Loop { start: Option<u64>, end: Option<u64> },
    /// Allume ou éteint le **punch** : allumé, `record` enregistre exactement dans la zone (la bande bleue de la règle, la même que la
    /// boucle) au lieu de refuser quand une boucle existe. La lecture part de `début − pré-roll` (après le pré-compte), la prise
    /// s'arrête toute seule à la fin de la zone (au dernier échantillon près) et la lecture continue ; la boucle est suspendue pendant
    /// la prise puis rétablie. Enregistré avec le projet. Refusé pendant une prise.
    SetPunch { enabled: bool },
    /// La piste d'accords (accords, gammes, tonalités, motifs, mesures) : événement `chords`.
    GetChords,
    /// Retour de monitoring : le bus `node` sort sur la carte son (`output`) au lieu d'alimenter la sortie principale ; `null` : il revient à
    /// la sortie principale. Une action annulable. Refusé si ce n'est pas un bus, si les canaux sont ceux de la sortie principale,
    /// déjà pris, ou au-delà de ce que le pilote a ouvert.
    SetReturnOutput { node: u32, output: Option<ReturnOutput> },
    /// Dictionnaires de la saisie d'accords et de gammes : événement `chord_catalog` (aussi envoyé avec `get_project`).
    GetChordCatalog,
    /// Remplace la piste d'accords par `track` (objet au format `flibuste-carte`) : une action annulable.
    SetChords { track: serde_json::Value, label: Option<String> },
    /// Démarre le serveur web local des retours de monitoring (une page secrète par retour, avec son code QR) : événement `monitor`.
    /// `port` : port souhaité (défaut 8780, le premier libre parmi les 20 suivants) ; `local_only` : n'écoute que cette machine (essais).
    StartMonitor { port: Option<u16>, local_only: Option<bool> },
    /// Arrête le serveur des retours (les adresses et les codes QR cessent de fonctionner).
    StopMonitor,
    /// Donne une adresse neuve au retour `node` (l'ancienne cesse de fonctionner) : les adresses des retours sont gardées dans le projet.
    RenewMonitorAddress { node: u32 },
    /// Demande le journal des commandes reçues par le serveur des retours (réponse : événement `monitor_log`).
    GetMonitorLog,
    /// Rôle de la télécommande d'un retour : 0 = son retour seulement, 1 = aussi le transport, les sections et le pré-roll.
    SetMonitorRole { node: u32, role: u8 },
    /// Remplace la carte de structure (sections nommées : intro, couplet, refrain...) : une action annulable, refusée en entier si invalide.
    SetStructure { sections: Vec<SectionDto>, label: Option<String> },
    /// Remplace les paroles (syllabes en temps musical, couplets 1 à 8) : une action annulable, refusée en entier si invalide.
    SetLyrics { syllables: Vec<LyricDto>, label: Option<String> },
    /// Importe un fichier `flibuste-carte` : les voies présentes remplacent celles de la piste, les absentes ne sont pas touchées.
    ImportChords { path: String },
    /// Écrit la piste d'accords dans un fichier `flibuste-carte`.
    ExportChords { path: String },
    /// Enregistrement en boucle : la boucle (bande bleue) continue de tourner pendant la prise, chaque passage donne une prise du groupe.
    SetLoopRecord { enabled: bool },
    /// Réglages d'une piste ou d'un bus. `monitor` : écoute directe de l'entrée quand la piste est armée. `invert` : polarité
    /// inversée (signal × −1), pour vérifier un alignement par annulation de phase.
    SetNode { node: u32, gain: Option<f32>, pan: Option<f32>, mute: Option<bool>, solo: Option<bool>, arm: Option<bool>, monitor: Option<bool>, invert: Option<bool> },
    GetPeaks { clip: u64, from: u64, to: u64, columns: usize },
    /// Déplace un clip (audio ou de notes) : nouveau début en échantillons, et éventuellement une autre piste (`node`).
    MoveClip { clip: u64, start: u64, node: Option<u32> },
    /// Active ou désactive les fondus enchaînés automatiques quand deux clips d'une piste se chevauchent.
    SetAutoCrossfade { enabled: bool },
    /// Rogne un clip audio : nouveau début et/ou nouvelle fin absolus (échantillons).
    TrimClip { clip: u64, start: Option<u64>, end: Option<u64> },
    /// Découpe un clip audio en `at` (strictement à l'intérieur).
    SplitClip { clip: u64, at: u64 },
    DeleteClip { clip: u64 },
    /// Rend active la prise `clip` (les prises du même groupe qui la recouvrent deviennent muettes) : une action annulable.
    SetActiveTake { clip: u64 },
    /// Composer : active seulement la plage `[start, end)` (échantillons) de la prise `clip` ; elle est coupée aux bords de la plage.
    ActivateTakeRange { clip: u64, start: u64, end: u64 },
    /// Coupe ou rétablit un clip audio.
    SetClipMuted { clip: u64, muted: bool },
    /// Décale plusieurs clips (audio et de notes mêlés) du même nombre d'échantillons (négatif : vers la gauche ; le groupe
    /// s'arrête à 0) et, avec `node_delta`, du même nombre de **pistes** dans l'ordre d'affichage (borné pour que tous restent sur une
    /// piste ; ni bus ni sortie principale). Une seule action annulable.
    MoveClips { clips: Vec<u64>, delta: i64, node_delta: Option<i32> },
    /// Découpe en `at` tous les clips désignés qui contiennent cette position (les autres sont ignorés ; erreur si aucun ne la
    /// contient). Une seule action annulable « Découper » ; réponse `selection` : les deux moitiés de chaque clip.
    SplitClips { clips: Vec<u64>, at: u64 },
    /// Supprime plusieurs clips en une seule action annulable.
    DeleteClips { clips: Vec<u64> },
    /// Copie des clips dans le presse-papiers de l'hôte (hors historique) ; réponse `clipboard`.
    CopyClips { clips: Vec<u64> },
    /// Colle le presse-papiers : le plus ancien des clips copiés démarre en `at` (échantillons), les autres gardent leurs écarts.
    /// Chaque clip va sur sa piste d'origine ; avec `node`, la plus haute des pistes copiées vient sur cette piste et les autres gardent
    /// leur écart (refusé s'il manque des pistes). Réponse `selection`.
    PasteClips { at: u64, node: Option<u32> },
    /// Duplique des clips ; la copie est décalée de `delta` échantillons (par défaut : juste après le groupe). Réponse `selection`.
    DuplicateClips { clips: Vec<u64>, delta: Option<i64>, node_delta: Option<i32> },
    /// Longueur des fondus d'un clip audio, en échantillons.
    /// Longueur (échantillons) et/ou courbe des fondus d'un clip.
    SetClipFades { clip: u64, fade_in: Option<u64>, fade_out: Option<u64>, curve_in: Option<CurveSpec>, curve_out: Option<CurveSpec> },
    /// Courbes des deux fondus d'un fondu enchaîné : fondu de sortie du clip `clip_out`, fondu d'entrée du clip `clip_in` (un seul pas d'annulation).
    SetCrossfade { clip_out: u64, clip_in: u64, curve_out: Option<CurveSpec>, curve_in: Option<CurveSpec> },
    /// Longueur (échantillons) du fondu enchaîné entre `clip_out` et `clip_in`, avec ce qui reste en place : `center`, `start`
    /// (début du fondu) ou `end` (fin du fondu). Rogne les bords des clips sans déplacer leur contenu ; un seul pas d'annulation.
    SetCrossfadeLength { clip_out: u64, clip_in: u64, length: u64, anchor: String },
    /// Aperçu sonore d'un fondu : boucle du moteur sur la zone du fondu plus une marge de chaque côté (`margin_ms`), lecture
    /// lancée, piste du fondu seule si `solo`. Fondu enchaîné : `clip_out` et `clip_in` ; fondu seul : `clip` et `side`
    /// (`in` ou `out`). Voir `StopPreview`.
    PreviewFade { clip_out: Option<u64>, clip_in: Option<u64>, clip: Option<u64>, side: Option<String>, margin_ms: u64, solo: bool },
    /// Nouveau projet : une sortie principale et une piste stéréo, sans clip. Remplace le projet ouvert (la page a déjà
    /// proposé d'enregistrer si besoin).
    NewProject,
    /// Ouvre le projet du dossier `path` (« Nom.daw »).
    OpenProject {
        path: String,
        /// Reprendre la sauvegarde automatique du dossier au lieu du projet enregistré.
        #[serde(default)]
        recover: bool,
    },
    /// Supprime la sauvegarde automatique du dossier `path` (refus de la reprise).
    DiscardAutosave { path: String },
    /// Enregistre le projet. `path` : dossier choisi (« Enregistrer sous ») ; sans `path`, le dossier du projet ouvert.
    SaveProject { path: Option<String> },
    /// Demande l'inventaire des plugins CLAP installés. Le balayage se fait en arrière-plan la première fois ; la réponse est
    /// l'événement `plugins` (d'abord avec `scanning: true` et une liste vide, puis la liste complète).
    ListPlugins,
    /// Ajoute un plugin à la fin de la chaîne de la piste ou du bus `node` (annulable). `file` : nom du fichier du plugin (pour
    /// le retrouver vite) ; facultatif.
    AddPlugin { node: u32, clap_id: String, file: Option<String> },
    /// Retire un plugin (annulable : il revient avec son état).
    RemovePlugin { insert: u32 },
    /// Place le plugin juste avant `before` dans la chaîne de sa piste (à la fin sans `before`).
    MovePlugin { insert: u32, before: Option<u32> },
    /// Source de la chaîne latérale d'un plugin (`null` : aucune) : le signal de cette piste ou de ce bus, avant son fader, va au second
    /// port d'entrée du plugin (un compresseur qui réagit à la grosse caisse, par exemple). Hors historique, enregistrée avec le projet.
    SetInsertSidechain { insert: u32, source: Option<u32> },
    /// Contourne (`true`) ou réactive un plugin, sans clic et sans changer la latence. Hors historique.
    SetPluginBypass { insert: u32, bypass: bool },
    /// Demande la liste des paramètres d'un plugin : réponse `plugin_params`.
    GetPluginParams { insert: u32 },
    /// La banque personnelle de pages de contrôle (réponse : événement `control_pages`).
    GetControlPages,
    /// Définit la page de contrôle d'un plugin dans la banque (16 paramètres au plus, sans paramètre : supprime la page). La banque est un fichier de
    /// l'utilisateur, valable pour tous les projets.
    SetControlPage { plugin_id: String, name: String, params: Vec<crate::control_pages::PageParam> },
    /// Les paramètres de la page de contrôle d'un plugin du projet, avec leurs valeurs et leurs textes (réponse : événement `insert_page`).
    GetInsertPage { insert: u32 },
    /// Règle un paramètre d'un plugin (valeur dans l'unité du plugin). Hors historique ; réponse `plugin_param`.
    SetPluginParam { insert: u32, param: u32, value: f64 },
    /// Texte que le plugin affiche pour `value` (sans rien changer) : pour montrer une valeur automatisée.
    GetPluginParamText { insert: u32, param: u32, value: f64 },
    /// Ajoute une piste (`kind` : `mono` ou `stereo`), un bus (`bus`) ou un **retour de monitoring** (`return`) à la fin. Une piste ou
    /// un bus part vers la sortie principale ; un retour n'y va jamais (muet tant qu'il n'a pas de sortie physique). `output` (bus ou
    /// retour seulement) donne tout de suite la sortie sur la carte son : le tout est **un seul** pas d'annulation. Sans `name` :
    /// « Piste N », « Bus N » ou « Retour N ».
    AddTrack { kind: String, name: Option<String>, #[serde(default)] output: Option<ReturnOutput> },
    /// Supprime une piste ou un bus avec ses clips, son automation et ses effets. Les nœuds qui y étaient dirigés repartent vers la
    /// sortie principale, ses envois sont supprimés. Annulable : tout revient, plugins avec leur état.
    DeleteTrack { node: u32 },
    RenameTrack { node: u32, name: String },
    /// Couleur d'affichage, `#rrggbb`.
    SetTrackColor { node: u32, color: String },
    /// Sortie d'une piste ou d'un bus : `0` = sortie principale, ou un bus. Refusé si cela fait une boucle.
    SetTrackOutput { node: u32, output: u32 },
    /// Place la piste `node` juste avant la piste `before` dans l'ordre d'affichage (à la fin si `before` est absent).
    MoveTrack { node: u32, before: Option<u32> },
    /// Ce qu'une piste enregistre : un canal d'entrée du pilote (`channels: [3]`), une paire (`[3, 4]`, stéréo) — numérotés à
    /// partir de 1 comme sur la face avant —, ou le MIDI (`kind: "midi"`). Hors historique ; refusé pendant un enregistrement.
    SetTrackInput { node: u32, input: InputSpec },
    /// Filtre d'entrée MIDI d'une piste : une liste de règles (appareil, canaux) ; un message passe s'il correspond à au moins une règle,
    /// liste vide = tout passe. `device` vide : n'importe quel appareil ; `channels` vide : tous les canaux (sinon des numéros de 1 à 16).
    /// Hors historique, enregistré avec le projet.
    SetTrackMidiFilter { node: u32, rules: Vec<MidiRuleSpec> },
    /// Dynamique d'entrée d'une piste (porte + compresseur sur le signal qui entre, avant l'écoute et, si `record`, avant la prise). Hors
    /// historique, enregistrée avec le projet.
    SetTrackDynamics { node: u32, dynamics: DynamicsSpec },
    /// Courbe de réponse de la vélocité des débuts de note d'une piste (entre les notes, en direct ou des clips, et ses plugins ;
    /// l'enregistrement reste brut). Hors historique, enregistrée avec le projet.
    SetTrackVelocityCurve { node: u32, curve: VelocityCurveSpec },
    /// Enregistre : lance la lecture à partir de la position courante et enregistre toutes les pistes armées. Refusé sans
    /// piste armée, sans dossier de projet (« Enregistrer sous » d'abord), ou si une boucle est active.
    Record,
    /// Arrête l'enregistrement **et la lecture** ; les prises deviennent des clips (une seule action annulable) dès que le
    /// moteur a fini d'écrire (événement `recording` puis `project`).
    StopRecord,
    /// Change le tempo à partir de la mesure `at_bar` (1 = le début du projet) : `bpm` en noires par minute, de 20 à 400, trois
    /// décimales au plus. Les notes MIDI et l'automation suivent (elles sont en temps musical), pas les clips audio. Annulable.
    SetTempo { bpm: f64, at_bar: u32 },
    /// Change le chiffrage à partir de la mesure `at_bar` : `num` temps de `1/den` de ronde (`den` : 1, 2, 4, 8, 16 ou 32). Annulable.
    SetMeter { num: u32, den: u32, at_bar: u32 },
    /// Pose la courbe d'automation de `node` pour `target` (remplace la précédente ; sans point : la supprime). `mode` : `read`,
    /// `trim` (le fader s'ajoute à la courbe) ou `off` ; absent : on garde celui de la courbe existante (`read` pour une neuve).
    /// Positions en fractions de ronde `[n, d]` strictement croissantes ; valeurs : gain et niveau d'envoi de 0 à 4 (linéaire),
    /// panoramique de −1 à 1, paramètre de plugin dans son unité. Annulable (une action par geste).
    SetLane { node: u32, target: LaneTarget, mode: Option<String>, points: Vec<LanePoint>, label: Option<String> },
    /// Arme un paramètre pour l'écriture de l'automation : `mode` = `touch`, `latch`, `write` ou `none` (désarmer). Pendant la
    /// lecture, ses gestes s'écrivent dans la courbe (une action annulable à la fin). Armer met la courbe existante en mode lecture.
    /// Les armements ne sont pas enregistrés dans le projet.
    SetWriteMode { node: u32, target: LaneTarget, mode: String },
    /// L'utilisateur prend (`active`) ou lâche le réglage d'un paramètre (curseur, fader, bouton) : pour le mode Toucher.
    Gesture { node: u32, target: LaneTarget, active: bool },
    /// Change seulement le mode d'une courbe existante. Annulable.
    SetLaneMode { node: u32, target: LaneTarget, mode: String },
    /// Ajoute un envoi de `node` vers `to` (un bus ou la sortie principale `0`), **en plus** de sa sortie : niveau 1, après le fader.
    /// Une seule fois par destination. Annulable.
    AddSend { node: u32, to: u32 },
    /// Supprime l'envoi de `node` vers `to`. Annulable.
    RemoveSend { node: u32, to: u32 },
    /// Règle un envoi : `gain` (0 à 2, linéaire ; hors historique, geste continu) et/ou `pre` (pris avant le fader : annulable).
    /// `mute` / `solo` : coupe ou met en solo l'envoi **dans sa destination seulement** (hors historique) ; si un envoi vers un nœud
    /// est en solo, seuls les envois en solo y sont entendus.
    SetSend { node: u32, to: u32, gain: Option<f64>, pre: Option<bool>, mute: Option<bool>, solo: Option<bool> },
    /// Crée un groupe de pistes (nom, couleur facultative `#rrggbb`, membres). Un nœud n'est que dans un groupe : il quitte l'ancien.
    /// Annulable.
    /// `ret` : le retour dont c'est un groupe **propre** (il doit en avoir : `customize_return_groups`) ; sans, un groupe du projet.
    CreateGroup { name: String, color: Option<String>, nodes: Vec<u32>, ret: Option<u32> },
    /// Modifie un groupe : nom, couleur et/ou liste complète des membres. Annulable.
    EditGroup { group: u32, name: Option<String>, color: Option<String>, nodes: Option<Vec<u32>>, ret: Option<u32> },
    /// Supprime un groupe (ses membres restent, sans groupe). Annulable.
    DeleteGroup { group: u32, ret: Option<u32> },
    /// Donne au retour ses **propres groupes** : une copie de ceux du projet (à modifier), qui les **remplace** dans son mix. Les
    /// décalages de niveau des groupes du projet vers ce retour sont d'abord remis à 0. Annulable.
    CustomizeReturnGroups { ret: u32 },
    /// Ramène le retour aux groupes du projet (ses groupes propres sont oubliés ; décalages remis à 0 d'abord). Annulable.
    ResetReturnGroups { ret: u32 },
    /// Donne au retour `ret` une **copie des groupes de l'autre retour** `from` (les siens, ou ceux du projet qu'il suit) comme groupes
    /// propres, en remplaçant ceux qu'il avait. Décalages de niveau remis à 0. Annulable.
    CopyReturnGroups { ret: u32, from: u32 },
    /// Agit sur tous les membres d'un groupe d'un coup (hors historique). Sans `to` : mute / solo / niveau des pistes ; avec `to` :
    /// sur leurs envois vers ce nœud (mix d'un retour). `delta_db` : décalage de niveau commun, écarts conservés.
    SetGroup { group: u32, mute: Option<bool>, solo: Option<bool>, delta_db: Option<f64>, to: Option<u32> },
    /// Décalage de niveau **absolu** d'un groupe (−24 à +24 dB, 0 = rien) : le curseur du groupe reste à sa place. L'hôte applique la
    /// différence avec le décalage actuel à tous les membres (écarts conservés) et mémorise la valeur ; la remettre à 0 rend les niveaux
    /// d'avant (sauf aux bornes des faders). `to` : sur les envois vers ce retour. Hors historique, non enregistré dans le projet.
    SetGroupOffset { group: u32, db: f64, to: Option<u32> },
    /// Demande le détail des notes d'un clip de notes (`clip` : identifiant du protocole, ≥ 1 000 000) : événement `notes`.
    GetNotes { clip: u64 },
    /// Ajoute des notes (début et durée en fractions de ronde `[n, d]`, **depuis le début de la matière du clip**) : une action.
    AddNotes { clip: u64, notes: Vec<NoteSpec> },
    /// Modifie des notes (indices de l'événement `notes` le plus récent) en **une seule action annulable** ; `label` nomme l'action.
    EditNotes { clip: u64, edits: Vec<NoteEditSpec>, label: Option<String> },
    /// Remplace les événements de contrôle d'**une voie** d'un clip de notes (`kind` : `cc` avec `controller` 0 à 127, `bend` ou `pressure`)
    /// dans la partie visible du clip : une seule action annulable. Positions en fractions de ronde depuis le début de la matière.
    SetEvents { clip: u64, kind: String, controller: Option<u8>, events: Vec<EventSpec>, label: Option<String> },
    /// Supprime des notes (indices de l'événement `notes`) en une action.
    DeleteNotes { clip: u64, indices: Vec<usize> },
    /// Aimante le début des notes du clip sur la grille `grid` (fraction de ronde `[n, d]`, positions absolues). Annulable.
    QuantizeNotes { clip: u64, grid: [u64; 2] },
    /// Fait sonner (`on`) ou taire une note sur une piste, pour l'écoute pendant l'édition. Hors historique.
    PreviewNote { node: u32, key: u8, velocity: f64, on: bool },
    /// Supprime le changement de tempo posé à la position `pos` (fraction de ronde `[n, d]`, celle du marqueur dans `project.tempo_map`).
    /// Le tempo initial ne se supprime pas. Annulable.
    RemoveTempo { pos: [u64; 2] },
    /// Supprime le changement de chiffrage posé à la mesure `at_bar` (≥ 2). Annulable.
    RemoveMeter { at_bar: u32 },
    /// Réglages du métronome (tous donnés à chaque fois) : `enabled` allume le clic ; `only_recording` ne le fait sonner que pendant
    /// un enregistrement ; `volume` de 0 à 1,5 ; `count_in_bars` : mesures de pré-compte (0 à 2) avant un enregistrement ;
    /// `pre_roll` : secondes de lecture avant le point d'enregistrement (0 à 10) ; `post_roll` : secondes après l'arrêt demandé
    /// (0 à 10). Pas annulable ni enregistré dans le projet.
    SetMetronome {
        enabled: bool,
        only_recording: bool,
        volume: f64,
        count_in_bars: u32,
        pre_roll: f64,
        post_roll: f64,
        /// Pré-roll en mesures (0 à 8), utilisé quand `pre_roll_unit` vaut `bars` : la lecture part du début de la mesure située
        /// `pre_roll_bars` mesures avant celle qui contient le point d'enregistrement (tempo et chiffrage du projet).
        #[serde(default)]
        pre_roll_bars: u32,
        /// Unité du pré-roll : `seconds` (par défaut, champ `pre_roll`) ou `bars` (champ `pre_roll_bars`).
        #[serde(default)]
        pre_roll_unit: crate::prefs::PreRollUnit,
    },
    /// Panique : coupe toutes les notes de toutes les pistes (fins de notes, pédale, molette, « toutes les notes » sur les 16
    /// canaux, une fin de note pour chaque touche de chaque canal). Ne touche ni à la lecture ni à la position.
    Panic,
    /// Arrête l'aperçu et rend à l'utilisateur sa lecture, sa position, sa boucle et ses solos.
    StopPreview,
    /// Exporte le mixage ou les stems en fichiers (WAV ou FLAC), sur un fil de travail : la lecture reste possible. Progression par
    /// l'événement `export`. `scope` : tout le projet, la boucle ou `[start, end[` (échantillons). `dir` : dossier de destination
    /// (par défaut `export/` dans le dossier du projet) ; `name` : début du nom des fichiers (par défaut le nom du projet).
    /// `tail_secs` : queue maximale après la fin de la zone (0 à 60, défaut 5), coupée dès que le son est sous −90 dB.
    /// `normalize_db` : pic cible (−30 à 0), mixage seulement. `dither` : par défaut activé en 16 bits seulement.
    Export {
        scope: ExportScope,
        start: Option<u64>,
        end: Option<u64>,
        mode: ExportMode,
        /// Stems : nœuds à exporter (par défaut toutes les pistes et tous les bus).
        nodes: Option<Vec<u32>>,
        format: ExportFormat,
        dither: Option<bool>,
        #[serde(default = "default_tail")]
        tail_secs: f64,
        normalize_db: Option<f64>,
        dir: Option<String>,
        name: Option<String>,
    },
    /// Lit les en-têtes de fichiers audio (sans les décoder) : format, fréquence, canaux, durée. Réponse : `audio_info`. La page s'en sert
    /// pour proposer un choix aux fichiers de plus de deux canaux et pour dire qu'une conversion de fréquence aura lieu.
    ProbeAudio { files: Vec<String> },
    /// Importe des fichiers audio (WAV, FLAC, MP3) sur un fil de travail : décodage, conversion à la fréquence du projet, copie dans
    /// `audio/` (ou `reference: true` : le fichier reste en place, possible seulement s'il est déjà à la fréquence du projet, en mono
    /// ou stéréo). Le clip du premier fichier va sur la piste `node` (si donnée), les autres fichiers et tous les fichiers sans `node`
    /// sur de **nouvelles pistes** ; `pos` : position de départ (échantillons, défaut : la tête de lecture). Plus de deux canaux :
    /// `multichannel` = `split` (une piste mono par canal) ou `stereo` (mixage : canaux pairs à gauche, impairs à droite). Une seule
    /// action annulable « Importer de l'audio ». Réponse : événements `import`.
    ImportAudio {
        files: Vec<String>,
        node: Option<u32>,
        pos: Option<u64>,
        #[serde(default)]
        reference: bool,
        multichannel: Option<String>,
    },
    /// Ouvre l'**interface native** du plugin `insert` dans une fenêtre à part (réglages à la souris, avec la vraie interface du plugin).
    /// Réponse : événement `plugin_ui`. Refusé si la plateforme n'a pas de fenêtres de plugins ou si le plugin n'a pas d'interface
    /// pour elle ; déjà ouverte : la fenêtre passe au premier plan. L'interface se ferme seule si le plugin est retiré ou si le
    /// projet change.
    OpenPluginUi { insert: u32 },
    /// Ferme l'interface native du plugin `insert` (sans effet si elle n'est pas ouverte).
    ClosePluginUi { insert: u32 },
    /// Crée une **copie convertie** du projet `path` (un dossier « Nom.daw ») à la fréquence d'échantillonnage du moteur, dans un nouveau
    /// dossier voisin « Nom (48000 Hz).daw » (numéroté s'il existe), puis l'ouvre. L'original n'est **jamais modifié**. À envoyer en
    /// réponse à l'événement `rate_mismatch` (la page confirme avant). Sur un fil de travail : événements `convert`. Refusé pendant
    /// un enregistrement, un import ou une autre conversion.
    ConvertProject { path: String },
    /// Annule la conversion en cours (le nouveau dossier n'est pas créé).
    CancelConvert,
    /// Annule l'import en cours (rien n'est ajouté au projet ; les fichiers déjà copiés dans `audio/` sont retirés).
    CancelImport,
    /// **Caler des prises sur une autre** (bibliothèque `daw-align`) : le clip `guide` fixe la fenêtre de temps et le rythme ; chaque clip
    /// de `doubles` est lu sur cette fenêtre, analysé puis recalé sans changer la hauteur. Le résultat de chaque double est **une prise de
    /// plus** dans le groupe de prises du double (la nouvelle est active, l'original reste dessous), le tout en **une seule action annulable**
    /// « Caler des prises ». `settings` : fiche de réglages (JSON de `daw-align`), la même pour tous (défaut : réglages par défaut).
    /// Travail sur un fil, avec progression et annulation : événements `align`.
    AlignClips { guide: u64, #[serde(default)] guide_more: Vec<u64>, doubles: Vec<u64>, settings: Option<String> },
    /// **Refaire le calage** de prises déjà calées : une nouvelle prise de plus par clip de `clips` (qui doit être une prise calée).
    /// `settings` : nouvelle fiche (pour tous les clips ; sinon chacun garde la sienne). `reanalyse` : refaire l'analyse (nécessaire si le
    /// mode d'analyse, le serrage ou l'écart maximal changent) ; sinon l'analyse enregistrée sert, et seul le rendu est refait (rapide).
    /// `consonants` (un seul clip) : consonnes modifiées à la main, paires début-fin en secondes dans le double.
    /// `general_only` : `settings` ne donne que son bloc **général**, copié dans la fiche propre de chaque clip (sa courbe de force reste la sienne) :
    /// c'est le copier-coller des réglages.
    AlignRedo { clips: Vec<u64>, settings: Option<String>, #[serde(default)] reanalyse: bool, consonants: Option<Vec<[f64; 2]>>, #[serde(default)] general_only: bool },
    CancelAlign,
    /// **Écoute de calage** (sans toucher à la prise active ni à l'historique) : boucle sur la fenêtre du clip `guide`, en solo les pistes
    /// concernées, et seuls les clips voulus audibles sur la piste `node` du double. `what` : `guide`, `original` (le double d'avant le
    /// calage), `aligned` (le double recalé : `aligned` désigne la prise calée à entendre), `guide_original`, `guide_aligned`. Rappeler la commande
    /// avec un autre `what` change l'écoute sans coupure. Tout est rendu à l'arrêt (`stop_listen`), à la première autre commande de
    /// modification, ou quand l'utilisateur reprend le transport. Réponse : événement `listen`.
    ListenAlign { guide: u64, #[serde(default)] guide_more: Vec<u64>, node: u32, aligned: Option<u64>, what: String },
    StopListen,
    /// **Marqueurs d'étirement** : remplace les marqueurs d'un clip audio. `markers` : les marqueurs **intérieurs**, `[position dans le clip,
    /// position dans la source du clip]`, tous deux en échantillons depuis le début du clip (le début et la fin du clip sont implicites et ne
    /// bougent pas) ; strictement croissants dans les deux colonnes, jamais au bord. L'audio entre deux marqueurs est étiré ou comprimé **sans
    /// changer la hauteur** (rapport de 25 % à 400 %, tronçon de source d'au moins 10 ms) ; la longueur du clip ne change pas. `engine` :
    /// `signalsmith` (défaut), `rubberband_r2` ou `rubberband_r3` ; absent : celui du clip. **Sans marqueur**, un clip étiré retrouve sa source
    /// (aucun rendu). Le rendu a lieu sur un fil de travail (événement `stretch`) ; **une seule action annulable** à la fin (« Étirer l'audio »
    /// / « Retirer l'étirement »). Refusé : enregistrement, calage ou étirement en cours, clip inconnu, marqueurs invalides.
    SetStretch { clip: u64, markers: Vec<[u64; 2]>, #[serde(default)] engine: Option<String> },
    /// Annule l'étirement en cours (rien n'est posé).
    CancelStretch,
    /// Détails du calage d'une prise calée (pour l'écran de réglage) : chemin, décalage appliqué, consonnes, force suggérée. Réponse :
    /// événement `align_detail`.
    GetAlign { clip: u64, settings: Option<String>, #[serde(default)] threshold_ms: Option<f64> },
    /// Copier-coller des réglages : la fiche `dst` avec le bloc **général** de `src` (la courbe de force ne se copie jamais). Réponse :
    /// événement `align_settings`.
    MergeAlignSettings { dst: String, src: String },
    /// Annule l'export en cours (les fichiers déjà terminés sont conservés ; aucun fichier partiel ne reste).
    CancelExport,
    Undo,
    Redo,
}

impl Command {
    /// Nom du type de commande (pour les messages d'erreur).
    pub fn name(&self) -> &'static str {
        match self {
            Command::GetProject => "get_project",
            Command::GetPrefs => "get_prefs",
            Command::SetPrefs { .. } => "set_prefs",
            Command::MoveClip { .. } => "move_clip",
            Command::SetAutoCrossfade { .. } => "set_auto_crossfade",
            Command::TrimClip { .. } => "trim_clip",
            Command::SplitClip { .. } => "split_clip",
            Command::DeleteClip { .. } => "delete_clip",
            Command::SetActiveTake { .. } => "set_active_take",
            Command::ActivateTakeRange { .. } => "activate_take_range",
            Command::SetClipMuted { .. } => "set_clip_muted",
            Command::MoveClips { .. } => "move_clips",
            Command::SplitClips { .. } => "split_clips",
            Command::DeleteClips { .. } => "delete_clips",
            Command::CopyClips { .. } => "copy_clips",
            Command::PasteClips { .. } => "paste_clips",
            Command::DuplicateClips { .. } => "duplicate_clips",
            Command::SetClipFades { .. } => "set_clip_fades",
            Command::SetCrossfade { .. } => "set_crossfade",
            Command::SetCrossfadeLength { .. } => "set_crossfade_length",
            Command::PreviewFade { .. } => "preview_fade",
            Command::StopPreview => "stop_preview",
            Command::Panic => "panic",
            Command::ListPlugins => "list_plugins",
            Command::AddPlugin { .. } => "add_plugin",
            Command::RemovePlugin { .. } => "remove_plugin",
            Command::MovePlugin { .. } => "move_plugin",
            Command::SetPluginBypass { .. } => "set_plugin_bypass",
            Command::SetInsertSidechain { .. } => "set_insert_sidechain",
            Command::GetPluginParams { .. } => "get_plugin_params",
            Command::GetControlPages => "get_control_pages",
            Command::SetControlPage { .. } => "set_control_page",
            Command::GetInsertPage { .. } => "get_insert_page",
            Command::SetPluginParam { .. } => "set_plugin_param",
            Command::GetPluginParamText { .. } => "get_plugin_param_text",
            Command::AddTrack { .. } => "add_track",
            Command::DeleteTrack { .. } => "delete_track",
            Command::RenameTrack { .. } => "rename_track",
            Command::SetTrackColor { .. } => "set_track_color",
            Command::SetTrackOutput { .. } => "set_track_output",
            Command::MoveTrack { .. } => "move_track",
            Command::SetTempo { .. } => "set_tempo",
            Command::SetMeter { .. } => "set_meter",
            Command::SetMetronome { .. } => "set_metronome",
            Command::RemoveTempo { .. } => "remove_tempo",
            Command::SetLane { .. } => "set_lane",
            Command::SetLaneMode { .. } => "set_lane_mode",
            Command::SetWriteMode { .. } => "set_write_mode",
            Command::Gesture { .. } => "gesture",
            Command::AddSend { .. } => "add_send",
            Command::RemoveSend { .. } => "remove_send",
            Command::SetSend { .. } => "set_send",
            Command::CreateGroup { .. } => "create_group",
            Command::EditGroup { .. } => "edit_group",
            Command::DeleteGroup { .. } => "delete_group",
            Command::CustomizeReturnGroups { .. } => "customize_return_groups",
            Command::ResetReturnGroups { .. } => "reset_return_groups",
            Command::CopyReturnGroups { .. } => "copy_return_groups",
            Command::SetGroup { .. } => "set_group",
            Command::SetGroupOffset { .. } => "set_group_offset",
            Command::GetNotes { .. } => "get_notes",
            Command::AddNotes { .. } => "add_notes",
            Command::SetEvents { .. } => "set_events",
            Command::EditNotes { .. } => "edit_notes",
            Command::DeleteNotes { .. } => "delete_notes",
            Command::QuantizeNotes { .. } => "quantize_notes",
            Command::PreviewNote { .. } => "preview_note",
            Command::RemoveMeter { .. } => "remove_meter",
            Command::SetTrackInput { .. } => "set_track_input",
            Command::SetTrackMidiFilter { .. } => "set_track_midi_filter",
            Command::SetTrackDynamics { .. } => "set_track_dynamics",
            Command::SetTrackVelocityCurve { .. } => "set_track_velocity_curve",
            Command::Record => "record",
            Command::StopRecord => "stop_record",
            Command::NewProject => "new_project",
            Command::OpenProject { .. } => "open_project",
            Command::DiscardAutosave { .. } => "discard_autosave",
            Command::SaveProject { .. } => "save_project",
            Command::OpenPluginUi { .. } => "open_plugin_ui",
            Command::ClosePluginUi { .. } => "close_plugin_ui",
            Command::ConvertProject { .. } => "convert_project",
            Command::CancelConvert => "cancel_convert",
            Command::ProbeAudio { .. } => "probe_audio",
            Command::ImportAudio { .. } => "import_audio",
            Command::CancelImport => "cancel_import",
            Command::AlignClips { .. } => "align_clips",
            Command::AlignRedo { .. } => "align_redo",
            Command::CancelAlign => "cancel_align",
            Command::ListenAlign { .. } => "listen_align",
            Command::StopListen => "stop_listen",
            Command::SetStretch { .. } => "set_stretch",
            Command::CancelStretch => "cancel_stretch",
            Command::GetAlign { .. } => "get_align",
            Command::MergeAlignSettings { .. } => "merge_align_settings",
            Command::Export { .. } => "export",
            Command::CancelExport => "cancel_export",
            Command::Undo => "undo",
            Command::Redo => "redo",
            Command::Transport { .. } => "transport",
            Command::Seek { .. } => "seek",
            Command::Loop { .. } => "loop",
            Command::SetPunch { .. } => "set_punch",
            Command::GetChords => "get_chords",
            Command::SetReturnOutput { .. } => "set_return_output",
            Command::GetChordCatalog => "get_chord_catalog",
            Command::SetChords { .. } => "set_chords",
            Command::SetStructure { .. } => "set_structure",
            Command::SetLyrics { .. } => "set_lyrics",
            Command::StartMonitor { .. } => "start_monitor",
            Command::StopMonitor => "stop_monitor",
            Command::RenewMonitorAddress { .. } => "renew_monitor_address",
            Command::SetMonitorRole { .. } => "set_monitor_role",
            Command::GetMonitorLog => "get_monitor_log",
            Command::ImportChords { .. } => "import_chords",
            Command::ExportChords { .. } => "export_chords",
            Command::SetLoopRecord { .. } => "set_loop_record",
            Command::SetNode { .. } => "set_node",
            Command::GetPeaks { .. } => "get_peaks",
        }
    }

    /// Mêmes règles que `validateCommand` côté page : une commande invalide est refusée avec un message.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Command::MoveClips { clips, .. } | Command::SplitClips { clips, .. } | Command::DeleteClips { clips } | Command::CopyClips { clips } | Command::DuplicateClips { clips, .. } => {
                if clips.is_empty() || clips.len() > 10_000 {
                    return Err(format!("{} : de 1 à 10 000 clips", self.name()));
                }
                Ok(())
            }
            Command::Loop { start, end } => match (start, end) {
                (None, None) => Ok(()),
                (Some(s), Some(e)) if s < e => Ok(()),
                _ => Err("loop : start < end, entiers ≥ 0 (ou null pour supprimer)".into()),
            },
            Command::SetNode { gain, pan, mute, solo, arm, monitor, invert, .. } => {
                if gain.is_none() && pan.is_none() && mute.is_none() && solo.is_none() && arm.is_none() && monitor.is_none() && invert.is_none() {
                    return Err("set_node : rien à régler".into());
                }
                if gain.is_some_and(|g| !g.is_finite() || g < 0.0) {
                    return Err("set_node : gain doit être un nombre ≥ 0".into());
                }
                if pan.is_some_and(|p| !p.is_finite() || !(-1.0..=1.0).contains(&p)) {
                    return Err("set_node : pan doit être entre -1 et 1".into());
                }
                Ok(())
            }
            Command::GetPeaks { from, to, columns, .. } => {
                if from >= to {
                    return Err("get_peaks : clip, from < to".into());
                }
                if !(1..=8192).contains(columns) {
                    return Err("get_peaks : columns entre 1 et 8192".into());
                }
                Ok(())
            }
            Command::ActivateTakeRange { start, end, .. } if start >= end => Err("activate_take_range : start < end".into()),
            Command::TrimClip { start, end, .. } => match (start, end) {
                (None, None) => Err("trim_clip : start et/ou end".into()),
                (Some(s), Some(e)) if s >= e => Err("trim_clip : start < end".into()),
                _ => Ok(()),
            },
            Command::SetClipFades { fade_in, fade_out, curve_in, curve_out, .. } => {
                if fade_in.is_none() && fade_out.is_none() && curve_in.is_none() && curve_out.is_none() {
                    return Err("set_clip_fades : fade_in, fade_out, curve_in et/ou curve_out".into());
                }
                check_curve(curve_in)?;
                check_curve(curve_out)
            }
            Command::SetCrossfade { clip_out, clip_in, curve_out, curve_in } => {
                if clip_out == clip_in {
                    return Err("set_crossfade : deux clips différents".into());
                }
                if curve_out.is_none() && curve_in.is_none() {
                    return Err("set_crossfade : curve_out et/ou curve_in".into());
                }
                check_curve(curve_in)?;
                check_curve(curve_out)
            }
            Command::SetTempo { bpm, at_bar } => {
                if !(20.0..=400.0).contains(bpm) {
                    return Err("set_tempo : bpm entre 20 et 400".into());
                }
                if *at_bar == 0 {
                    return Err("set_tempo : at_bar ≥ 1 (1 = le début du projet)".into());
                }
                Ok(())
            }
            Command::SetLane { target, mode, points, .. } => {
                if mode.as_deref().is_some_and(|m| !matches!(m, "read" | "trim" | "off")) {
                    return Err("set_lane : mode read, trim ou off".into());
                }
                let (lo, hi) = match target {
                    LaneTarget::Gain | LaneTarget::Send { .. } => (0.0, 4.0),
                    LaneTarget::Pan => (-1.0, 1.0),
                    LaneTarget::Insert { .. } => (f64::MIN, f64::MAX),
                };
                let mut last: Option<f64> = None;
                for p in points {
                    let pos = p.pos[0] as f64 / p.pos[1].max(1) as f64;
                    if p.pos[1] == 0 || !p.value.is_finite() || p.value < lo || p.value > hi || last.is_some_and(|l| pos <= l) || !matches!(p.curve.as_str(), "linear" | "step") {
                        return Err("set_lane : points à positions strictement croissantes (fractions valides), valeurs dans la plage de la cible, courbe linear ou step".into());
                    }
                    last = Some(pos);
                }
                Ok(())
            }
            Command::SetWriteMode { mode, .. } if !matches!(mode.as_str(), "none" | "touch" | "latch" | "write") => Err("set_write_mode : none, touch, latch ou write".into()),
            Command::SetLaneMode { mode, .. } if !matches!(mode.as_str(), "read" | "trim" | "off") => Err("set_lane_mode : read, trim ou off".into()),
            Command::AddSend { node, to } | Command::RemoveSend { node, to } if node == to => Err("un nœud ne s'envoie pas à lui-même".into()),
            Command::AddSend { node, .. } | Command::RemoveSend { node, .. } if *node == 0 => Err("la sortie principale n'a pas d'envoi".into()),
            Command::CreateGroup { name, color, nodes, .. } => group_fields_ok(Some(name), color.as_deref(), Some(nodes)),
            Command::EditGroup { group, name, color, nodes, .. } => {
                if *group == 0 {
                    return Err("edit_group : groupe inconnu".into());
                }
                group_fields_ok(name.as_ref(), color.as_deref(), nodes.as_ref())
            }
            Command::DeleteGroup { group, .. } if *group == 0 => Err("delete_group : groupe inconnu".into()),
            Command::SetGroup { group, mute, solo, delta_db, .. } => {
                if *group == 0 {
                    return Err("set_group : groupe inconnu".into());
                }
                if mute.is_none() && solo.is_none() && delta_db.is_none() {
                    return Err("set_group : mute, solo et/ou delta_db".into());
                }
                if delta_db.is_some_and(|d| !d.is_finite() || !(-96.0..=96.0).contains(&d)) {
                    return Err("set_group : delta_db entre -96 et 96".into());
                }
                Ok(())
            }
            Command::SetGroupOffset { group, db, .. } => {
                if *group == 0 {
                    return Err("set_group_offset : groupe inconnu".into());
                }
                if !db.is_finite() || !(-24.0..=24.0).contains(db) {
                    return Err("set_group_offset : db entre -24 et 24".into());
                }
                Ok(())
            }
            Command::SetSend { node, gain, pre, mute, solo, .. } => {
                if *node == 0 {
                    return Err("set_send : la sortie principale n'a pas d'envoi".into());
                }
                if gain.is_none() && pre.is_none() && mute.is_none() && solo.is_none() {
                    return Err("set_send : gain, pre, mute et/ou solo".into());
                }
                if gain.is_some_and(|g| !(0.0..=2.0).contains(&g)) {
                    return Err("set_send : gain entre 0 et 2".into());
                }
                Ok(())
            }
            Command::SetEvents { kind, controller, events, .. } => {
                let (lo, hi) = match kind.as_str() {
                    "cc" | "pressure" => (0, 127),
                    "bend" => (-8192, 8191),
                    _ => return Err("set_events : kind = cc, bend ou pressure".into()),
                };
                if (kind == "cc") != controller.is_some() || controller.is_some_and(|c| c > 127) {
                    return Err("set_events : `controller` (0 à 127) est requis pour cc, et seulement pour cc".into());
                }
                if events.len() > 4096 || events.iter().any(|e| e.start[1] == 0 || e.channel > 15 || e.value < lo || e.value > hi) {
                    return Err(format!("set_events : au plus 4096 points, fraction valide, canal < 16, valeur de {lo} à {hi}"));
                }
                Ok(())
            }
            Command::AddNotes { notes, .. } => {
                if notes.is_empty() || notes.iter().any(|n| !n.valid()) {
                    return Err("add_notes : au moins une note, début et durée en fractions valides (durée > 0), touche < 128, vélocité dans ]0, 1]".into());
                }
                Ok(())
            }
            Command::EditNotes { edits, .. } => {
                if edits.is_empty() || edits.iter().any(|e| !e.valid()) {
                    return Err("edit_notes : au moins une modification, fractions valides (durée > 0), touche < 128, canal < 16, vélocité dans ]0, 1]".into());
                }
                Ok(())
            }
            Command::DeleteNotes { indices, .. } if indices.is_empty() => Err("delete_notes : aucune note".into()),
            Command::QuantizeNotes { grid, .. } if grid[0] == 0 || grid[1] == 0 => Err("quantize_notes : grille invalide".into()),
            Command::PreviewNote { key, velocity, .. } => {
                if *key > 127 || !(*velocity > 0.0 && *velocity <= 1.0) {
                    return Err("preview_note : touche < 128, vélocité dans ]0, 1]".into());
                }
                Ok(())
            }
            Command::RemoveTempo { pos } => {
                if pos[1] == 0 {
                    return Err("remove_tempo : dénominateur nul".into());
                }
                Ok(())
            }
            Command::RemoveMeter { at_bar } => {
                if *at_bar < 2 {
                    return Err("remove_meter : at_bar ≥ 2 (le chiffrage initial ne se supprime pas)".into());
                }
                Ok(())
            }
            Command::SetPrefs { prefs } => prefs.validate().map_err(|e| format!("set_prefs : {e}")),
            Command::SetMetronome { volume, count_in_bars, pre_roll, post_roll, pre_roll_bars, .. } => {
                if !(0.0..=1.5).contains(volume) || *count_in_bars > 2 || !(0.0..=10.0).contains(pre_roll) || !(0.0..=10.0).contains(post_roll) || *pre_roll_bars > 8 {
                    return Err("set_metronome : volume 0 à 1,5, pré-compte 0 à 2 mesures, pré-roll 0 à 10 s ou 0 à 8 mesures, post-roll 0 à 10 s".into());
                }
                Ok(())
            }
            Command::SetMeter { num, den, at_bar } => {
                if !(1..=32).contains(num) || !matches!(den, 1 | 2 | 4 | 8 | 16 | 32) {
                    return Err("set_meter : num de 1 à 32, den parmi 1, 2, 4, 8, 16, 32".into());
                }
                if *at_bar == 0 {
                    return Err("set_meter : at_bar ≥ 1 (1 = le début du projet)".into());
                }
                Ok(())
            }
            Command::SetTrackDynamics { node, dynamics } => {
                if *node == 0 {
                    return Err("set_track_dynamics : la sortie principale n'a pas d'entrée".into());
                }
                if !dynamics.to_engine().is_valid() {
                    return Err("set_track_dynamics : porte −90 à −20 dB, seuil −60 à 0 dB, rapport 1 à 20, attaque 0,1 à 100 ms, relâchement 10 à 2 000 ms, compensation 0 à 24 dB".into());
                }
                Ok(())
            }
            Command::SetTrackVelocityCurve { node, curve } => {
                if *node == 0 {
                    return Err("set_track_velocity_curve : la sortie principale n'a pas d'entrée".into());
                }
                if !curve.to_engine().is_valid() {
                    return Err("set_track_velocity_curve : courbure −1000 à 1000, adoucissement 0 à 1000, vélocités minimale et maximale de 1 à 127, vélocité fixe 0 à 127".into());
                }
                Ok(())
            }
            Command::SetTrackMidiFilter { node, rules } => {
                if *node == 0 {
                    return Err("set_track_midi_filter : la sortie principale n'a pas d'entrée".into());
                }
                if rules.len() > 16 {
                    return Err("set_track_midi_filter : 16 filtres au plus".into());
                }
                for r in rules {
                    if r.device.chars().count() > 120 || r.channels.iter().any(|c| !(1..=16).contains(c)) {
                        return Err("set_track_midi_filter : appareil de 120 caractères au plus, canaux de 1 à 16".into());
                    }
                }
                Ok(())
            }
            Command::SetTrackInput { node, input } => {
                if *node == 0 {
                    return Err("set_track_input : la sortie principale n'a pas d'entrée".into());
                }
                match input.kind.as_str() {
                    "midi" => Ok(()),
                    "audio" if (1..=2).contains(&input.channels.len()) && input.channels.iter().all(|c| (1..=64).contains(c)) => Ok(()),
                    "audio" => Err("set_track_input : audio demande 1 ou 2 canaux, numérotés de 1 à 64".into()),
                    other => Err(format!("set_track_input : kind « {other} » inconnu (audio ou midi)")),
                }
            }
            Command::AddPlugin { clap_id, .. } if clap_id.trim().is_empty() => Err("add_plugin : clap_id vide".into()),
            Command::GetPluginParamText { value, .. } if !value.is_finite() => Err("get_plugin_param_text : value doit être un nombre fini".into()),
            Command::SetControlPage { plugin_id, name, params } => crate::control_pages::validate(&crate::control_pages::PageEntry { plugin_id: plugin_id.clone(), name: name.clone(), params: params.clone() }).map_err(|e| format!("set_control_page : {e}")),
            Command::SetPluginParam { value, .. } if !value.is_finite() => Err("set_plugin_param : value doit être un nombre fini".into()),
            Command::AddTrack { kind, name, output } => {
                if !matches!(kind.as_str(), "mono" | "stereo" | "bus" | "return") {
                    return Err(format!("add_track : kind « {kind} » inconnu (mono, stereo, bus ou return)"));
                }
                if output.is_some() && !matches!(kind.as_str(), "bus" | "return") {
                    return Err("add_track : une sortie physique ne se donne qu'à un bus ou à un retour".into());
                }
                match name {
                    Some(n) => check_name(n, "add_track"),
                    None => Ok(()),
                }
            }
            Command::DeleteTrack { node } | Command::MoveTrack { node, .. } if *node == 0 => Err("la sortie principale ne se supprime ni ne se déplace".into()),
            Command::RenameTrack { node, name } => {
                if *node == 0 {
                    return Err("rename_track : la sortie principale ne se renomme pas".into());
                }
                check_name(name, "rename_track")
            }
            Command::SetTrackColor { node, color } => {
                if *node == 0 {
                    return Err("set_track_color : la sortie principale n'a pas de couleur à choisir".into());
                }
                let ok = color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit());
                if ok { Ok(()) } else { Err(format!("set_track_color : « {color} » n'est pas une couleur #rrggbb")) }
            }
            Command::SetTrackOutput { node, .. } if *node == 0 => Err("set_track_output : la sortie principale n'a pas de sortie".into()),
            Command::ConvertProject { path } if path.trim().is_empty() => Err("convert_project : path vide".into()),
            Command::DiscardAutosave { path } if path.trim().is_empty() => Err("discard_autosave : path vide".into()),
            Command::OpenProject { path, .. } => {
                if path.trim().is_empty() {
                    return Err("open_project : path vide".into());
                }
                Ok(())
            }
            Command::AlignClips { guide, guide_more, doubles, .. } => {
                if doubles.is_empty() || doubles.len() > 32 || guide_more.len() > 32 || doubles.contains(guide) || guide_more.iter().any(|g| doubles.contains(g)) {
                    return Err("align_clips : de 1 à 32 doubles, différents du guide".into());
                }
                Ok(())
            }
            Command::AlignRedo { clips, consonants, .. } => {
                if clips.is_empty() || clips.len() > 32 {
                    return Err("align_redo : de 1 à 32 clips".into());
                }
                if let Some(c) = consonants {
                    if clips.len() != 1 || c.iter().any(|p| !(p[0].is_finite() && p[1].is_finite() && p[0] >= 0.0 && p[1] > p[0])) {
                        return Err("align_redo : consonnes pour un seul clip, paires (début, fin) en secondes, fin > début".into());
                    }
                }
                Ok(())
            }
            Command::SetStretch { markers, engine, .. } => {
                if markers.len() > crate::host::MAX_STRETCH_MARKERS {
                    return Err(format!("set_stretch : {} marqueurs au plus", crate::host::MAX_STRETCH_MARKERS));
                }
                let mut prev = [0u64, 0u64];
                for m in markers {
                    if m[0] <= prev[0] || m[1] <= prev[1] {
                        return Err("set_stretch : marqueurs strictement croissants (position dans le clip et dans la source, au-delà de 0)".into());
                    }
                    prev = *m;
                }
                match engine.as_deref() {
                    None | Some("signalsmith") | Some("rubberband_r2") | Some("rubberband_r3") => Ok(()),
                    Some(_) => Err("set_stretch : engine = signalsmith, rubberband_r2 ou rubberband_r3".into()),
                }
            }
            Command::ListenAlign { what, aligned, .. } => {
                if !["guide", "original", "aligned", "guide_original", "guide_aligned"].contains(&what.as_str()) {
                    return Err("listen_align : what = guide, original, aligned, guide_original ou guide_aligned".into());
                }
                if what.ends_with("aligned") && aligned.is_none() {
                    return Err("listen_align : `aligned` (la prise calée) est requis pour écouter le double recalé".into());
                }
                Ok(())
            }
            Command::GetAlign { threshold_ms: Some(t), .. } if !(t.is_finite() && *t > 0.0 && *t <= 5000.0) => Err("get_align : seuil de 0 à 5 000 ms".into()),
            Command::ProbeAudio { files } | Command::ImportAudio { files, .. } if files.is_empty() || files.len() > 64 || files.iter().any(|f| f.trim().is_empty()) => Err("fichiers : de 1 à 64 chemins non vides".into()),
            Command::ImportAudio { node, multichannel, .. } => {
                if *node == Some(0) {
                    return Err("import_audio : la sortie principale ne reçoit pas de clip".into());
                }
                if multichannel.as_deref().is_some_and(|m| !matches!(m, "split" | "stereo")) {
                    return Err("import_audio : multichannel = split ou stereo".into());
                }
                Ok(())
            }
            Command::Export { scope, start, end, nodes, tail_secs, normalize_db, dir, name, mode, .. } => {
                if *scope == ExportScope::Range && !matches!((start, end), (Some(s), Some(e)) if s < e) {
                    return Err("export : scope « range » demande start < end (échantillons)".into());
                }
                if !tail_secs.is_finite() || !(0.0..=60.0).contains(tail_secs) {
                    return Err("export : tail_secs entre 0 et 60".into());
                }
                if normalize_db.is_some_and(|n| !n.is_finite() || !(-30.0..=0.0).contains(&n)) {
                    return Err("export : normalize_db entre −30 et 0 dBFS".into());
                }
                if *mode == ExportMode::Stems && normalize_db.is_some() {
                    return Err("export : la normalisation ne s'applique qu'au mixage (les stems gardent leur équilibre)".into());
                }
                if nodes.as_ref().is_some_and(|n| n.is_empty() || n.contains(&0)) {
                    return Err("export : nodes = au moins une piste ou un bus (pas la sortie principale)".into());
                }
                if dir.as_ref().is_some_and(|d| d.trim().is_empty()) || name.as_ref().is_some_and(|n| n.trim().is_empty()) {
                    return Err("export : dir et name ne peuvent pas être vides".into());
                }
                Ok(())
            }
            Command::SaveProject { path: Some(p) } if p.trim().is_empty() => Err("save_project : path vide".into()),
            Command::PreviewFade { clip_out, clip_in, clip, side, margin_ms, .. } => {
                if *margin_ms > MAX_PREVIEW_MARGIN_MS {
                    return Err(format!("preview_fade : margin_ms ≤ {MAX_PREVIEW_MARGIN_MS}"));
                }
                match (clip_out, clip_in, clip, side.as_deref()) {
                    (Some(a), Some(b), None, None) if a != b => Ok(()),
                    (Some(_), Some(_), None, None) => Err("preview_fade : deux clips différents".into()),
                    (None, None, Some(_), Some("in" | "out")) => Ok(()),
                    _ => Err("preview_fade : clip_out + clip_in (fondu enchaîné) ou clip + side (« in » ou « out »)".into()),
                }
            }
            Command::SetCrossfadeLength { clip_out, clip_in, length, anchor } => {
                if clip_out == clip_in {
                    return Err("set_crossfade_length : deux clips différents".into());
                }
                if *length < 1 {
                    return Err("set_crossfade_length : length ≥ 1 échantillon".into());
                }
                if !matches!(anchor.as_str(), "center" | "start" | "end") {
                    return Err(format!("set_crossfade_length : anchor inconnu « {anchor} » (center, start ou end)"));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Nom de piste : ni vide ni plus de 60 caractères.
fn check_name(name: &str, command: &str) -> Result<(), String> {
    let t = name.trim();
    if t.is_empty() || t.chars().count() > 60 {
        Err(format!("{command} : nom de 1 à 60 caractères"))
    } else {
        Ok(())
    }
}

/// Marge maximale (de chaque côté du fondu) de l'aperçu sonore.
pub const MAX_PREVIEW_MARGIN_MS: u64 = 10_000;

/// Courbe d'un fondu : forme de base (`linear`, `equal_power`), courbure (−1000 concave … +1000 convexe) et adoucissement
/// en S (0 … 1000), en millièmes. Voir `engine::audio::Fade`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CurveSpec {
    pub shape: String,
    #[serde(default)]
    pub bend: i16,
    #[serde(default)]
    pub smooth: u16,
}

impl CurveSpec {
    pub fn check(&self) -> Result<(), String> {
        if daw_session::project::shape_from_name(&self.shape).is_none() {
            return Err(format!("forme de fondu inconnue : {}", self.shape));
        }
        if !(-1000..=1000).contains(&self.bend) || self.smooth > 1000 {
            return Err("courbe de fondu hors plage : bend de −1000 à 1000, smooth de 0 à 1000".into());
        }
        Ok(())
    }
}

impl From<&daw_engine::Fade> for CurveSpec {
    fn from(f: &daw_engine::Fade) -> CurveSpec {
        CurveSpec { shape: daw_session::project::shape_name(f.shape).into(), bend: f.bend, smooth: f.smooth }
    }
}

fn check_curve(c: &Option<CurveSpec>) -> Result<(), String> {
    c.as_ref().map_or(Ok(()), CurveSpec::check)
}

/// Un point d'une voie de contrôle (modulation, pédale, molette…) : `value` 0 à 127, ou −8192 à 8191 pour la molette (`bend`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventSpec {
    pub start: [u64; 2],
    #[serde(default)]
    pub channel: u8,
    pub value: i32,
}

/// Événement de contrôle d'un clip tel que la page le lit : `kind` = `cc` (`controller`), `bend`, `pressure` ou `poly` (`key`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventDetail {
    pub start: [u64; 2],
    pub channel: u8,
    pub kind: String,
    pub controller: Option<u8>,
    pub key: Option<u8>,
    pub value: i32,
}

/// Une note à ajouter : `start` et `len` en fractions de ronde `[n, d]` (début depuis le début de la matière du clip).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NoteSpec {
    pub start: [u64; 2],
    pub len: [u64; 2],
    pub key: u8,
    pub velocity: f64,
    /// Voix d'affichage (1 à 4) ; absente = 1.
    #[serde(default)]
    pub voice: Option<u8>,
}

impl NoteSpec {
    fn valid(&self) -> bool {
        self.start[1] != 0 && self.len[1] != 0 && self.len[0] != 0 && self.key < 128 && self.velocity > 0.0 && self.velocity <= 1.0
    }
}

/// Modification d'une note (`index` de l'événement `notes`) ; ce qui manque ne change pas.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NoteEditSpec {
    pub index: usize,
    pub start: Option<[u64; 2]>,
    pub len: Option<[u64; 2]>,
    pub key: Option<u8>,
    pub velocity: Option<f64>,
    /// Canal MIDI (0 à 15).
    pub channel: Option<u8>,
    /// Voix d'affichage (1 à 4).
    #[serde(default)]
    pub voice: Option<u8>,
}

impl NoteEditSpec {
    fn valid(&self) -> bool {
        self.start.is_none_or(|s| s[1] != 0) && self.len.is_none_or(|l| l[1] != 0 && l[0] != 0) && self.key.is_none_or(|k| k < 128) && self.channel.is_none_or(|c| c < 16) && self.voice.is_none_or(|v| (1..=4).contains(&v)) && self.velocity.is_none_or(|v| v > 0.0 && v <= 1.0)
    }
}

/// Une note telle que le piano roll la montre : positions **exactes** et indice dans la liste du clip.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteDetail {
    pub start: [u64; 2],
    pub len: [u64; 2],
    pub key: u8,
    pub velocity: f32,
    pub channel: u8,
    pub voice: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TempoChange {
    pub pos: [u64; 2],
    pub num: u32,
    pub den: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeterChange {
    pub bar: u32,
    pub num: u32,
    pub den: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteInfo {
    pub start: u64,
    pub len: u64,
    pub key: u8,
    pub velocity: f32,
}

/// Sorties de la carte son d'un retour : premier canal (à partir de 1 ; la sortie principale est 1-2, donc au moins 3), une paire ou un
/// canal, et somme mono (un canal seul est toujours en somme mono).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnOutput {
    pub first: u8,
    pub stereo: bool,
    pub mono: bool,
}

/// Une gamme du catalogue de Scorsaire.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScaleInfo {
    pub id: String,
    pub fr: String,
    pub en: String,
    pub intervals: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipInfo {
    /// `source_len` : longueur des données audio du fichier (borne du rognage).
    /// `missing` : fichier référencé en place introuvable à l'ouverture (le clip joue du silence).
    Audio { id: u64, name: String, start: u64, len: u64, offset: u64, source_len: u64, gain: f32, fade_in: u64, fade_out: u64, fade_in_curve: CurveSpec, fade_out_curve: CurveSpec, missing: bool,
        /// Clip muet (prise inactive ou clip coupé) : conservé mais jamais joué. `take_group` : groupe de prises (0 = aucun).
        muted: bool, take_group: u32,
        /// Prise calée sur un guide (ses réglages de calage sont enregistrés : `get_align`, `align_redo`).
        aligned: bool,
        /// Clip étiré par des marqueurs (voir `set_stretch`) ; `null` : clip ordinaire (la page peut en poser).
        stretch: Option<StretchInfo> },
    /// `offset`, `fade_in`, `fade_out` : en échantillons (le décalage de la fenêtre visible dans la matière, les fondus de
    /// vélocité) ; `source_len` : pratiquement illimité (un clip de notes peut être étendu à droite à volonté).
    Midi { id: u64, name: String, start: u64, len: u64, offset: u64, source_len: u64, fade_in: u64, fade_out: u64, notes: Vec<NoteInfo>, muted: bool, take_group: u32 },
}

/// Ce qu'une piste enregistre, tel que l'interface l'affiche : `kind` `audio` (`channels` : 1 ou 2 numéros d'entrées, à partir de 1)
/// ou `midi`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct InputSpec {
    pub kind: String,
    #[serde(default)]
    pub channels: Vec<u32>,
}

/// Un plugin dans la chaîne d'une piste.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InsertInfo {
    /// Identifiant stable du plugin (jamais réutilisé dans un projet).
    pub id: u32,
    pub name: String,
    pub vendor: String,
    pub bypass: bool,
    /// Le plugin a un second port d'entrée audio (chaîne latérale) : on peut lui donner une source.
    pub has_sidechain: bool,
    /// Nœud (piste ou bus) dont le signal, avant son fader, alimente la chaîne latérale du plugin ; `null` : aucune.
    pub sidechain: Option<u32>,
    /// Latence annoncée par le plugin, en échantillons.
    pub latency: u32,
    /// Le plugin a une interface native qu'on peut ouvrir ici (`open_plugin_ui`).
    pub gui: bool,
    /// Son interface native est ouverte en ce moment.
    pub ui_open: bool,
}

/// Un plugin installé, tel que l'inventaire le décrit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PluginListing {
    pub clap_id: String,
    pub name: String,
    pub vendor: String,
    /// Nom du fichier du plugin.
    pub file: String,
    pub description: String,
    pub features: Vec<String>,
}

/// Un paramètre d'un plugin avec sa valeur courante et son texte (celui que le plugin affiche lui-même).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParamEntry {
    pub id: u32,
    pub name: String,
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub value: f64,
    pub text: String,
}

impl ClipInfo {
    /// Identifiant du clip (audio ou notes).
    pub fn id(&self) -> u64 {
        match self {
            ClipInfo::Audio { id, .. } | ClipInfo::Midi { id, .. } => *id,
        }
    }
}

/// Ce qu'une courbe d'automation commande.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LaneTarget {
    Gain,
    Pan,
    /// Niveau de l'envoi du nœud vers `to` (`0` = sortie principale).
    Send { to: u32 },
    /// Paramètre `param` du plugin `insert`.
    Insert { insert: u32, param: u32 },
}

/// Un point de courbe : position en fraction de ronde `[n, d]`, valeur, façon de rejoindre le point suivant (`linear` ou `step`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LanePoint {
    pub pos: [u64; 2],
    pub value: f64,
    pub curve: String,
}

/// Une courbe d'automation d'un nœud.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LaneInfo {
    pub target: LaneTarget,
    pub mode: String,
    pub points: Vec<LanePoint>,
}

/// Un paramètre armé pour l'écriture de l'automation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WriteArmInfo {
    pub node: u32,
    pub target: LaneTarget,
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SendInfo {
    pub to: u32,
    pub gain: f32,
    pub pre: bool,
    /// Envoi coupé / en solo dans sa destination.
    pub mute: bool,
    pub solo: bool,
}

/// Courbe de réponse de la vélocité d'une piste (voir `engine::VelocityCurve`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VelocityCurveSpec {
    pub enabled: bool,
    #[serde(default)]
    pub bend: i16,
    #[serde(default)]
    pub smooth: u16,
    pub min: u8,
    pub max: u8,
    /// Vélocité fixe (1 à 127) ; 0 : aucune.
    #[serde(default)]
    pub fixed: u8,
}

impl VelocityCurveSpec {
    pub fn to_engine(self) -> daw_engine::VelocityCurve {
        daw_engine::VelocityCurve { enabled: self.enabled, bend: self.bend, smooth: self.smooth, min: self.min, max: self.max, fixed: self.fixed }
    }

    pub fn from_engine(c: daw_engine::VelocityCurve) -> VelocityCurveSpec {
        VelocityCurveSpec { enabled: c.enabled, bend: c.bend, smooth: c.smooth, min: c.min, max: c.max, fixed: c.fixed }
    }
}

/// Dynamique d'entrée d'une piste (voir `engine::InputDynamics`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DynamicsSpec {
    pub enabled: bool,
    pub gate_db: f32,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
    /// Écrire le signal traité dans la prise (sinon la prise reste brute).
    #[serde(default)]
    pub record: bool,
}

impl DynamicsSpec {
    pub fn to_engine(self) -> daw_engine::InputDynamics {
        daw_engine::InputDynamics { enabled: self.enabled, gate_db: self.gate_db, threshold_db: self.threshold_db, ratio: self.ratio, attack_ms: self.attack_ms, release_ms: self.release_ms, makeup_db: self.makeup_db, record: self.record }
    }

    pub fn from_engine(d: daw_engine::InputDynamics) -> DynamicsSpec {
        DynamicsSpec { enabled: d.enabled, gate_db: d.gate_db, threshold_db: d.threshold_db, ratio: d.ratio, attack_ms: d.attack_ms, release_ms: d.release_ms, makeup_db: d.makeup_db, record: d.record }
    }
}

/// Une règle du filtre d'entrée MIDI d'une piste.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiRuleSpec {
    /// Nom de l'appareil (vide : n'importe lequel).
    #[serde(default)]
    pub device: String,
    /// Canaux acceptés, de 1 à 16 (vide : tous).
    #[serde(default)]
    pub channels: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupInfo {
    pub id: u32,
    pub name: String,
    /// `#rrggbb` ou vide (la page choisit).
    pub color: String,
    /// Décalage de niveau actuel du groupe (dB) : un par destination où il n'est pas nul (`to` absent : les pistes elles-mêmes).
    pub offsets: Vec<GroupOffsetInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReturnGroupsInfo {
    pub groups: Vec<GroupInfo>,
    /// (nœud, groupe) de chaque membre.
    pub members: Vec<[u32; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupOffsetInfo {
    pub to: Option<u32>,
    pub db: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeInfo {
    pub id: u32,
    pub name: String,
    pub kind: String,
    /// Retour de monitoring (un bus d'un type à part : QR, onglet de mix, groupes propres) ; faux pour une piste et un bus ordinaire.
    pub is_return: bool,
    pub color: String,
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    /// Polarité inversée.
    pub invert: bool,
    pub arm: bool,
    /// Entrée de la piste (audio : canaux ; ou MIDI) et écoute directe quand elle est armée.
    pub input: InputSpec,
    pub monitor: bool,
    /// Filtre d'entrée MIDI de la piste (liste vide : tout passe).
    pub midi_filter: Vec<MidiRuleSpec>,
    /// Dynamique d'entrée de la piste.
    pub dynamics: DynamicsSpec,
    /// Courbe de réponse de la vélocité de la piste.
    pub velocity_curve: VelocityCurveSpec,
    /// Chaîne de plugins, dans l'ordre de traitement.
    pub inserts: Vec<InsertInfo>,
    /// Envois (en plus de la sortie), dans l'ordre : destination (`0` = sortie principale), niveau, avant le fader.
    pub sends: Vec<SendInfo>,
    /// Groupe du nœud (0 : aucun).
    pub group: u32,
    /// Courbes d'automation du nœud (celles dont la cible existe).
    pub automation: Vec<LaneInfo>,
    /// Sortie du nœud : `0` = sortie principale, ou le numéro d'un bus (`null` pour la sortie principale elle-même).
    pub output: Option<u32>,
    /// Retour de monitoring : sorties de la carte son d'un bus (numérotées à partir de 1 ; 1 et 2 sont la sortie principale).
    pub phys: Option<ReturnOutput>,
    /// Groupes **propres** de ce retour (ils remplacent ceux du projet dans son mix) ; absent : il suit ceux du projet.
    pub return_groups: Option<ReturnGroupsInfo>,
    pub clips: Vec<ClipInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectInfo {
    /// Nombre de canaux de sortie audio que le pilote a ouverts (2 : la sortie principale seule, pas de retour de monitoring possible).
    pub audio_outputs: u32,
    /// Nombre de canaux d'entrée audio que le pilote a ouverts (0 : pas d'entrée, on ne peut pas enregistrer l'audio).
    pub audio_inputs: u32,
    pub sample_rate: u32,
    pub tempo_map: Vec<TempoChange>,
    pub meters: Vec<MeterChange>,
    pub latency_samples: u32,
    /// Fondus enchaînés automatiques activés.
    pub auto_crossfade: bool,
    /// Paramètres armés pour l'écriture de l'automation (`touch`, `latch`, `write`).
    pub write_arms: Vec<WriteArmInfo>,
    /// Groupes de pistes du projet.
    pub groups: Vec<GroupInfo>,
    /// Appareils d'entrée MIDI que le système voit (pour les filtres d'entrée des pistes).
    pub midi_devices: Vec<String>,
    pub nodes: Vec<NodeInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LoopRange {
    pub start: u64,
    pub end: u64,
}

/// Événement (moteur → page).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Project(ProjectInfo),
    Position {
        pos: u64,
        playing: bool,
        #[serde(rename = "loop")]
        loop_range: Option<LoopRange>,
    },
    /// Crêtes linéaires `[gauche, droite]` par identifiant de nœud (clé en texte, comme tout objet JSON).
    Meters { levels: BTreeMap<String, [f32; 2]> },
    /// Témoin d'entrée MIDI : pour chaque piste qui a reçu un message récemment, les types reçus (bit 1 notes, 2 contrôleurs, 4 molette
    /// et pressions). Envoyé seulement quand l'ensemble change ; une piste absente est éteinte.
    MidiActivity { active: BTreeMap<String, u8> },
    /// Vélocité brute (1 à 127) de la dernière note jouée en direct sur une piste (aperçu de la courbe de vélocité).
    NoteVelocity { node: u32, input: u8 },
    Peaks { clip: u64, from: u64, to: u64, columns: Vec<[f32; 2]> },
    /// État de l'historique d'annulation (envoyé après chaque modification).
    /// Ce que contient le presse-papiers de l'hôte (réponse à `copy_clips`).
    Clipboard { count: usize },
    /// Clips à sélectionner dans la page (créés par un collage ou une duplication).
    Selection { clips: Vec<u64> },
    History { can_undo: bool, can_redo: bool, undo_label: Option<String>, redo_label: Option<String> },
    /// L'aperçu sonore d'un fondu démarre (`active`) ou se termine (arrêt demandé, fondu disparu, ou l'utilisateur a repris le transport).
    Preview { active: bool },
    /// Le projet ouvert : nom, dossier (`null` tant qu'il n'est pas enregistré), modifications non enregistrées, numéro de
    /// chargement (change à chaque ouverture ou nouveau projet : la page vide ses caches) et avertissements de l'ouverture
    /// ou de l'enregistrement (plugin introuvable...).
    /// Notes d'un clip (réponse à `get_notes`, et après toute modification de notes) : `start`, `len` (longueur visible) et `offset`
    /// (début de la partie visible dans la matière) du clip en fractions de ronde ; les notes dans l'ordre de leurs indices ;
    /// `selection` : indices des notes qui viennent d'être ajoutées ou modifiées (pour que la page garde sa sélection).
    Notes { clip: u64, start: [u64; 2], len: [u64; 2], offset: [u64; 2], notes: Vec<NoteDetail>, selection: Vec<usize>, events: Vec<EventDetail> },
    /// Ce qui a été enregistré **pour l'instant** d'un paramètre en cours d'écriture (aperçu en direct de la courbe) : `points` =
    /// `[échantillon, valeur]` à partir de l'indice `index` (les précédents ont déjà été envoyés) ; `done` : l'écriture est terminée
    /// (la courbe définitive arrive avec `project`).
    WriteLive { node: u32, target: LaneTarget, index: u32, points: Vec<[f64; 2]>, done: bool },
    /// De l'automation s'écrit en ce moment (`active`) : un paramètre armé enregistre les gestes de l'utilisateur.
    Writing { active: bool },
    /// Une sauvegarde automatique plus récente que le projet enregistré existe dans `path` (`untitled` : projet jamais enregistré,
    /// perdu lors d'une fermeture brutale) ; `modified` : secondes depuis 1970. La page propose la reprise (`open_project { recover }`)
    /// ou l'oubli (`discard_autosave`).
    Autosave { path: String, untitled: bool, modified: u64 },
    File { name: String, path: Option<String>, dirty: bool, generation: u64, warnings: Vec<String> },
    /// État de l'enregistrement : `active` pendant la prise ; `stopping` entre l'ordre d'arrêt et la fin de l'écriture sur le
    /// disque ; `overruns` : blocs perdus (le disque ne suit pas : la prise est abîmée) ; `warnings` à la fin.
    /// `start` : position (échantillons) où les prises commencent ; `nodes` : les pistes qui enregistrent : la page dessine un conteneur
    /// « en cours d'enregistrement » de `start` à la tête de lecture, sur ces pistes.
    Recording { active: bool, stopping: bool, overruns: u32, warnings: Vec<String>, start: u64, nodes: Vec<u32> },
    /// Punch allumé ou éteint, envoyé avec le projet et après chaque changement.
    Punch { enabled: bool },
    /// La piste d'accords au format `flibuste-carte` (`track`), avec son numéro de révision et, après un import, ce qui a été ignoré.
    /// État du serveur des retours : actif ou non, et pour chaque retour (bus qui sort sur la carte son) son adresse et son code QR (SVG).
    /// Journal des commandes reçues par le serveur des retours (les plus récentes en dernier).
    MonitorLog { entries: Vec<crate::monitor::JournalEntry> },
    Monitor { active: bool, port: u16, returns: Vec<MonitorReturn>, error: Option<String> },
    Chords { revision: u32, track: serde_json::Value, skipped: Vec<String>, structure: Vec<SectionDto>, lyrics: Vec<LyricDto> },
    /// Ce que la page doit connaître pour saisir des accords et des gammes : dictionnaire de qualités de Scorsaire (identifiant,
    /// demi-tons sur deux octaves), écritures acceptées (`maj7` → `M7`) et catalogue des 72 gammes. Envoyé avec `get_project`.
    ChordCatalog { qualities: Vec<(String, Vec<u8>)>, aliases: Vec<(String, String)>, scales: Vec<ScaleInfo> },
    /// Enregistrement en boucle allumé ou éteint (préférence de session, non enregistrée dans le projet).
    LoopRecord { enabled: bool },
    /// Réglages du métronome (mêmes champs que `set_metronome`), envoyés avec le projet et après chaque changement.
    Metronome { enabled: bool, only_recording: bool, volume: f64, count_in_bars: u32, pre_roll: f64, post_roll: f64, pre_roll_bars: u32, pre_roll_unit: crate::prefs::PreRollUnit },
    /// Préférences enregistrées, ce qui tourne maintenant (`running`, absent avant l'ouverture du moteur audio), et si un
    /// redémarrage est nécessaire pour que les réglages audio enregistrés soient ceux qui tournent. `path` : le fichier.
    Prefs { prefs: crate::prefs::Prefs, running: Option<crate::prefs::Runtime>, restart_needed: bool, path: Option<String> },
    /// Inventaire des plugins installés ; `scanning` : le balayage est en cours (la liste suivra).
    Plugins { plugins: Vec<PluginListing>, scanning: bool },
    PluginParams { insert: u32, params: Vec<ParamEntry> },
    /// La banque de pages de contrôle (toutes les pages définies sur cet ordinateur) ; `error` : le fichier n'a pas pu être lu ou écrit.
    ControlPages { entries: Vec<crate::control_pages::PageEntry>, error: Option<String> },
    /// La page de contrôle d'un plugin du projet : `configured` faux s'il n'en a pas dans la banque ; `missing` : les noms de la page que le plugin
    /// n'expose pas (ou plus).
    InsertPage { insert: u32, plugin_id: String, configured: bool, params: Vec<ParamEntry>, missing: Vec<String> },
    /// Valeur et texte d'un paramètre après `set_plugin_param`.
    PluginParam { insert: u32, param: u32, value: f64, text: String },
    /// Réponse à `get_plugin_param_text`.
    PluginParamText { insert: u32, param: u32, value: f64, text: String },
    /// Avancement de l'export. `active` faux = terminé, annulé ou en erreur ; `files` : fichiers déjà écrits. `progress` de 0 à 1
    /// sur l'ensemble des fichiers ; `current` : nom du fichier en cours ; `warnings` : par exemple écrêtage.
    Export { active: bool, progress: f64, done: u32, total: u32, current: String, files: Vec<ExportedFile>, warnings: Vec<String>, cancelled: bool, error: Option<String>, dir: String },
    /// L'interface native d'un plugin vient de s'ouvrir ou de se fermer (par l'utilisateur, par le plugin, ou parce que le plugin ou
    /// le projet a disparu).
    PluginUi { insert: u32, open: bool },
    /// `open_project` a trouvé un projet à une autre fréquence d'échantillonnage que le moteur : rien n'a changé. La page propose de
    /// le convertir (`convert_project { path }`).
    RateMismatch { path: String, project_rate: u32, engine_rate: u32 },
    /// Avancement de la conversion d'un projet. `active` faux = terminée (`path` : le nouveau dossier, qui vient d'être ouvert),
    /// annulée ou en échec.
    Convert { active: bool, progress: f64, done: u32, total: u32, current: String, path: Option<String>, warnings: Vec<String>, cancelled: bool, error: Option<String> },
    /// Réponse à `probe_audio`.
    AudioInfo { files: Vec<AudioFileInfo> },
    /// Avancement de l'import. `active` faux = terminé, annulé ou en échec ; `clips` : ce qui a été posé (à la fin).
    /// Calage de prises en cours ou terminé (voir `align_clips`). `clips` : les prises créées (à la fin).
    /// Rendu d'un étirement (`set_stretch`) : avancement, fin, annulation ou erreur. `fell_back` : Rubber Band a échoué, Signalsmith l'a remplacé.
    Stretch { active: bool, progress: f64, clip: u64, name: String, cancelled: bool, error: Option<String>, fell_back: bool },
    Align { active: bool, progress: f64, done: u32, total: u32, current: String, clips: Vec<AlignedClip>, warnings: Vec<String>, cancelled: bool, error: Option<String> },
    /// Détails du calage d'une prise calée. Temps en secondes ; le chemin donne, pour chaque point (`hop` secondes) de la fenêtre du guide,
    /// la position correspondante dans le double ; `shift` le décalage appliqué ; `consonants` les paires (début, fin) dans le double ;
    /// `suggest` des paires (temps, force) proposées ; `settings` la fiche de la prise.
    AlignDetail { clip: u64, name: String, guide: String, window_start: u64, window_len: u64, hop: f64, path: Vec<f64>, shift: Vec<f64>, consonants: Vec<[f64; 2]>, suggest: Vec<[f64; 2]>, settings: String },
    AlignSettings { settings: String },
    /// Écoute de calage active ou terminée (`what` : voir `listen_align`).
    Listen { active: bool, what: String },
    Import { active: bool, progress: f64, done: u32, total: u32, current: String, clips: Vec<ImportedClip>, warnings: Vec<String>, cancelled: bool, error: Option<String> },
    Error { command: String, message: String },
}

/// En-tête d'un fichier audio. `needs_conversion` : sa fréquence n'est pas celle du projet.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AudioFileInfo {
    pub path: String,
    pub name: String,
    pub format: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: Option<u64>,
    pub needs_conversion: bool,
    pub error: Option<String>,
}

/// Les marqueurs d'étirement d'un clip, tels que la page les montre.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StretchInfo {
    /// Moteur du dernier rendu.
    pub engine: String,
    /// Longueur, en échantillons, de la partie de la source que couvre le clip (égale à la longueur du clip si le rapport d'ensemble est 1).
    pub src_len: u64,
    /// Marqueurs intérieurs `[position dans le clip, position dans la source du clip]`, en échantillons depuis le début du clip.
    pub markers: Vec<[u64; 2]>,
}

/// Une prise calée créée par `align_clips` / `align_redo`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlignedClip {
    pub clip: u64,
    pub name: String,
    pub track: String,
    /// Moteur de rendu demandé non disponible : Signalsmith a pris le relais.
    pub fell_back: bool,
}

/// Un clip importé.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImportedClip {
    pub name: String,
    pub track: String,
    /// Converti à la fréquence du projet.
    pub converted: bool,
    /// Référencé en place (pas copié dans `audio/`).
    pub referenced: bool,
}

/// Un fichier exporté.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExportedFile {
    pub path: String,
    pub frames: u64,
    /// Pic avant normalisation, en dBFS (`None` : silence).
    pub peak_db: Option<f64>,
    /// Échantillons au-delà de la pleine échelle (écrêtés dans un fichier entier).
    pub clipped: u64,
    pub applied_gain_db: f64,
}

/// Champs d'un groupe : nom de 1 à 40 caractères, couleur `#rrggbb`, au plus 512 membres sans doublon.
fn group_fields_ok(name: Option<&String>, color: Option<&str>, nodes: Option<&Vec<u32>>) -> Result<(), String> {
    if let Some(n) = name {
        if n.trim().is_empty() || n.chars().count() > 40 {
            return Err("groupe : nom de 1 à 40 caractères".into());
        }
    }
    if let Some(c) = color {
        if !(c.is_empty() || (c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()))) {
            return Err("groupe : couleur au format #rrggbb".into());
        }
    }
    if let Some(list) = nodes {
        let mut sorted = list.clone();
        sorted.sort_unstable();
        if list.len() > 512 || sorted.windows(2).any(|w| w[0] == w[1]) || list.contains(&0) {
            return Err("groupe : membres sans doublon, sans la sortie principale".into());
        }
    }
    Ok(())
}
