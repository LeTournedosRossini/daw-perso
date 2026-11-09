//! Effets insérés sur un nœud du graphe (typiquement des plugins).
//!
//! Le moteur ne connaît pas CLAP : il appelle un [`Insert`]. Les instances vivent côté audio dans le graphe
//! compilé ; elles sont **recréées par une usine** (`InsertFactory`) à la première compilation d'un identifiant, puis
//! **adoptées** d'un graphe au suivant (même identifiant) pour conserver leur état quand on modifie le routage.

use std::sync::Arc;

use crate::midi::MidiEvent;

/// Identifiant stable d'un effet inséré, unique dans la vie d'un `GraphDesc`.
pub type InsertId = u32;

/// Effet audio stéréo exécuté dans le callback : `process`, `set_param` et `reset` n'allouent pas, ne bloquent pas
/// et ne paniquent pas.
pub trait Insert: Send + 'static {
    /// Latence introduite, en échantillons (valeur lue à la création ; sert à la compensation de latence).
    fn latency(&self) -> u32;
    /// Traite `in_*` vers `out_*` (même longueur). Un instrument ignore l'entrée.
    fn process(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32]);
    /// Vrai si l'effet reçoit des notes (instrument ou effet à entrée de notes).
    fn accepts_notes(&self) -> bool {
        false
    }
    /// Comme `process`, avec les notes du bloc (triées par instant). N'est appelée que si `accepts_notes()`.
    fn process_with_notes(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32], _notes: &[MidiEvent]) {
        self.process(in_l, in_r, out_l, out_r);
    }
    /// Transport au début du prochain `process` (position en temps, tempo, chiffrage, boucle). Appelée avant chaque morceau de bloc ;
    /// sans effet par défaut.
    fn set_transport(&mut self, _t: &crate::TransportInfo, _sample_rate: u32) {}
    /// Vrai si l'effet a une entrée de chaîne latérale (un second port d'entrée audio : un compresseur commandé par un autre signal).
    fn has_sidechain(&self) -> bool {
        false
    }
    /// Signal de chaîne latérale du prochain `process` (même longueur que le bloc, aligné sur l'entrée de l'effet) ; l'effet le copie :
    /// les tampons appartiennent au moteur. Appelée avant `process`, seulement si `has_sidechain()`.
    fn set_sidechain(&mut self, _l: &[f32], _r: &[f32]) {}
    /// Piste d'accords valable **pour le prochain `process` seulement** : l'instantané appartient au graphe, qui peut disparaître
    /// ensuite ; une instance qui en garde l'adresse doit l'oublier à la fin de son traitement. Sans effet par défaut.
    fn set_chords(&mut self, _c: &crate::ChordSnapshot) {}
    /// Poignée de contrôle de cette instance, si elle en a une (appelée à la création, côté contrôle).
    fn control(&self) -> Option<Arc<dyn InsertControl>> {
        None
    }
    /// Change un paramètre au début du prochain bloc.
    fn set_param(&mut self, param: u32, value: f64);
    /// Vide les mémoires internes (déplacement de la tête de lecture).
    fn reset(&mut self);
}

/// Poignée de **contrôle** d'un effet, utilisable côté contrôle (fil principal) pendant que l'effet tourne dans le
/// callback : sauvegarde et restauration d'état, demande de redémarrage. Pour un plugin CLAP, ce sont les fonctions
/// « fil principal » de la spécification.
pub trait InsertControl: Send + Sync + 'static {
    fn as_any(&self) -> &dyn std::any::Any;
    /// État sérialisé de l'effet (`None` : l'effet n'a pas d'état).
    fn save_state(&self) -> Option<Vec<u8>>;
    /// Restaure un état ; `false` s'il est refusé.
    fn load_state(&self, data: &[u8]) -> bool;
    /// L'effet demande à être relancé (changement de latence, de ports…) : voir `EngineHandle::poll_restarts`.
    fn restart_requested(&self) -> bool;
}

/// Fabrique une instance prête à traiter pour (fréquence d'échantillonnage, plus grand bloc, état à restaurer).
/// L'état éventuel doit être appliqué **avant** l'activation de l'effet : c'est ce qui permet à un réglage de changer
/// sa latence au redémarrage. Appelée côté contrôle seulement, jamais dans le callback.
pub type InsertFactory = Arc<dyn Fn(u32, usize, Option<&[u8]>) -> Result<Box<dyn Insert>, String>>;

#[derive(Clone)]
pub struct InsertDesc {
    pub id: InsertId,
    pub name: String,
    pub factory: InsertFactory,
    /// Contourné : le signal passe sans traitement, retardé de la latence de l'effet pour rester aligné (contournement
    /// compensé). L'effet continue de tourner (il reçoit les notes, garde son état) : sa sortie est seulement mise de côté.
    pub bypass: bool,
    /// Source de la chaîne latérale : la piste ou le bus dont le signal (avant son fader) alimente le second port d'entrée de l'effet.
    /// `None` : pas de chaîne latérale (le port est omis).
    pub sidechain: Option<daw_core_types::NodeId>,
}

impl std::fmt::Debug for InsertDesc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InsertDesc").field("id", &self.id).field("name", &self.name).finish_non_exhaustive()
    }
}
