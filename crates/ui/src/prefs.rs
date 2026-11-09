//! Préférences de l'application : réglages de la machine (pilote audio, tampon, calcul parallèle, calcul en avance) et du
//! métronome. Un fichier JSON dans les données de l'utilisateur, valable pour tous les projets (le pilote et le tampon
//! dépendent de la machine, pas du projet). Les variables d'environnement `DAW_ASIO_DRIVER`, `DAW_BUFFER`, `DAW_THREADS`,
//! `DAW_AHEAD` et `DAW_AHEAD_THREADS` restent prioritaires (essais, scripts) : la fenêtre des réglages le signale.
//!
//! Tout ce qui touche au moteur audio (pilote, tampon, fils) ne s'applique qu'au prochain démarrage de l'application : le projet
//! ouvert vit dans le moteur qui tourne. Ce module ne fait que lire, valider, résoudre et écrire ; il ne dépend d'aucun pilote.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Version du fichier (pour d'éventuelles migrations futures).
pub const PREFS_VERSION: u32 = 1;

pub const DEFAULT_DRIVER: &str = "UMC";
pub const DEFAULT_BUFFER: u32 = 64;

/// Unité du pré-roll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreRollUnit {
    #[default]
    Seconds,
    Bars,
}

/// Réglages du métronome et de l'enregistrement (mêmes champs que la commande `set_metronome`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetroPrefs {
    pub enabled: bool,
    pub only_recording: bool,
    pub volume: f64,
    pub count_in_bars: u32,
    pub pre_roll: f64,
    pub post_roll: f64,
    pub pre_roll_bars: u32,
    pub pre_roll_unit: PreRollUnit,
}

impl Default for MetroPrefs {
    fn default() -> Self {
        MetroPrefs { enabled: false, only_recording: false, volume: 0.8, count_in_bars: 0, pre_roll: 0.0, post_roll: 0.0, pre_roll_bars: 0, pre_roll_unit: PreRollUnit::Seconds }
    }
}

impl MetroPrefs {
    fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.5).contains(&self.volume) || self.count_in_bars > 2 || !(0.0..=10.0).contains(&self.pre_roll) || !(0.0..=10.0).contains(&self.post_roll) || self.pre_roll_bars > 8 {
            return Err("métronome : volume 0 à 1,5, pré-compte 0 à 2 mesures, pré-roll 0 à 10 s ou 0 à 8 mesures, post-roll 0 à 10 s".into());
        }
        Ok(())
    }
}

/// Arrêt automatique du transport (désactivé par défaut) : protège d'une lecture ou d'une prise oubliée, par exemple quand le réseau d'une
/// télécommande tombe en plein enregistrement. Après `after_end_secs` secondes passées la fin du dernier élément du projet ; pendant une prise,
/// après `recording_secs` secondes passées la fin du dernier élément **et** le début de la prise (rien derrière).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoStopPrefs {
    pub enabled: bool,
    pub after_end_secs: u32,
    pub recording_secs: u32,
}

impl Default for AutoStopPrefs {
    fn default() -> Self {
        AutoStopPrefs { enabled: false, after_end_secs: 120, recording_secs: 300 }
    }
}

impl AutoStopPrefs {
    fn validate(&self) -> Result<(), String> {
        if !(10..=3600).contains(&self.after_end_secs) || !(10..=3600).contains(&self.recording_secs) {
            return Err("arrêt automatique : de 10 à 3 600 secondes".into());
        }
        Ok(())
    }
}

/// Comportement du transport : le pré-roll s'applique-t-il à la lecture normale (en plus de l'enregistrement, où il sert toujours), et la
/// tête revient-elle à son point de départ quand on arrête la lecture (sinon elle reste où elle est : pause).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransportPrefs {
    pub play_pre_roll: bool,
    pub return_on_stop: bool,
}

impl Default for TransportPrefs {
    fn default() -> Self {
        TransportPrefs { play_pre_roll: false, return_on_stop: true }
    }
}

/// Sécurité de la télécommande : un droit « transport » ou « régie » donné à une adresse expire au bout de `elevated_hours` heures (0 : jamais) ;
/// l'adresse règle alors de nouveau son seul retour jusqu'à ce qu'on renouvelle les droits (fenêtre du QR). Le compte repart à chaque démarrage du
/// serveur des retours, à chaque changement de droits et à chaque nouvelle adresse.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemotePrefs {
    pub elevated_hours: u32,
}

impl Default for RemotePrefs {
    fn default() -> Self {
        RemotePrefs { elevated_hours: 12 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub version: u32,
    /// Nom du pilote ASIO ; `None` : `UMC`.
    pub driver: Option<String>,
    /// Taille du tampon du pilote (échantillons) ; `None` : 64.
    pub buffer: Option<u32>,
    /// Fils de calcul des pistes, fil audio compris (1 : un seul cœur).
    pub threads: u32,
    /// Avance du calcul des pistes de lecture, en millisecondes (0 : désactivé).
    pub ahead_ms: u32,
    /// Fils de rendu du calcul en avance.
    pub ahead_threads: u32,
    pub metronome: MetroPrefs,
    pub auto_stop: AutoStopPrefs,
    pub transport: TransportPrefs,
    pub remote: RemotePrefs,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { version: PREFS_VERSION, driver: None, buffer: None, threads: 1, ahead_ms: 0, ahead_threads: 2, metronome: MetroPrefs::default(), auto_stop: AutoStopPrefs::default(), transport: TransportPrefs::default(), remote: RemotePrefs::default() }
    }
}

impl Prefs {
    /// Réglages d'un premier démarrage (aucun fichier de préférences), selon le nombre de cœurs : à partir de 4 cœurs, calcul en
    /// avance de 50 ms avec 2 fils de rendu et un seul fil de calcul (3 cœurs occupés, mesuré propre à 32 et 16 échantillons sur la
    /// UMC) ; en dessous, pas d'avance (les fils se gêneraient). Le reste est la valeur par défaut.
    pub fn recommended(cores: u32) -> Prefs {
        if cores >= 4 {
            Prefs { ahead_ms: 50, ahead_threads: 2, threads: 1, ..Prefs::default() }
        } else {
            Prefs::default()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Some(d) = &self.driver {
            if d.trim().is_empty() || d.len() > 200 {
                return Err("pilote : nom vide ou trop long".into());
            }
        }
        if let Some(b) = self.buffer {
            if !(8..=2048).contains(&b) {
                return Err("tampon : de 8 à 2048 échantillons".into());
            }
        }
        if !(1..=16).contains(&self.threads) {
            return Err("fils de calcul : de 1 à 16".into());
        }
        if self.ahead_ms != 0 && !(10..=500).contains(&self.ahead_ms) {
            return Err("avance : 0 (désactivée) ou de 10 à 500 ms".into());
        }
        if !(1..=8).contains(&self.ahead_threads) {
            return Err("fils de rendu : de 1 à 8".into());
        }
        if self.remote.elevated_hours > 168 {
            return Err("droits de la télécommande : de 0 (illimité) à 168 heures".into());
        }
        self.metronome.validate()?;
        self.auto_stop.validate()
    }
}

/// Deux noms désignent-ils le même pilote ? Le pilote s'ouvre par un morceau de son nom (« UMC » pour « UMC ASIO Driver »), sans
/// tenir compte de la casse : un nom contenu dans l'autre suffit.
pub fn same_driver(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
}

/// Ce que l'application utilise vraiment pour le moteur audio, une fois les variables d'environnement prises en compte.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Effective {
    pub driver: String,
    pub buffer: u32,
    pub threads: u32,
    pub ahead_ms: u32,
    pub ahead_threads: u32,
    /// Réglages imposés par une variable d'environnement (noms des champs : « driver », « buffer »…).
    pub forced: Vec<String>,
}

/// Résout les réglages : variable d'environnement si elle est valide, sinon préférence, sinon valeur par défaut. `env` donne la
/// valeur d'une variable (testable sans toucher à l'environnement du processus).
pub fn resolve(p: &Prefs, env: &dyn Fn(&str) -> Option<String>) -> Effective {
    let mut forced = Vec::new();
    let num = |name: &str, field: &str, forced: &mut Vec<String>| -> Option<u32> {
        let v = env(name)?.trim().parse::<u32>().ok()?;
        forced.push(field.into());
        Some(v)
    };
    let driver = match env("DAW_ASIO_DRIVER").filter(|s| !s.trim().is_empty()) {
        Some(d) => {
            forced.push("driver".into());
            d
        }
        None => p.driver.clone().unwrap_or_else(|| DEFAULT_DRIVER.into()),
    };
    let buffer = num("DAW_BUFFER", "buffer", &mut forced).unwrap_or_else(|| p.buffer.unwrap_or(DEFAULT_BUFFER));
    let threads = num("DAW_THREADS", "threads", &mut forced).unwrap_or(p.threads).max(1);
    let ahead_ms = num("DAW_AHEAD", "ahead_ms", &mut forced).unwrap_or(p.ahead_ms);
    let ahead_threads = num("DAW_AHEAD_THREADS", "ahead_threads", &mut forced).unwrap_or(p.ahead_threads).max(1);
    Effective { driver, buffer, threads, ahead_ms, ahead_threads, forced }
}

/// Ce qui tourne maintenant, pour la fenêtre des réglages (renseigné par l'application une fois le pilote ouvert).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Runtime {
    pub effective: Effective,
    pub sample_rate: u32,
    pub inputs: u32,
    /// Pilotes ASIO installés (vide : pas de pilote branché sur cette plateforme).
    pub drivers: Vec<String>,
    /// Cœurs logiques de la machine.
    pub cores: u32,
    /// Le pilote est ouvert (sinon le moteur tourne sans son).
    pub audio: bool,
}

/// Lit le fichier ; absent ou abîmé, on repart de `defaults` (avec un message pour le second cas). Un fichier partiel complète ses
/// champs manquants par les valeurs par défaut du crate (`Prefs::default()`), pas par `defaults` : ce que l'utilisateur a enregistré
/// ne change jamais d'un démarrage à l'autre.
pub fn load(path: &Path, defaults: &Prefs) -> (Prefs, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (defaults.clone(), None),
        Err(e) => return (defaults.clone(), Some(format!("préférences illisibles ({e}) : valeurs par défaut"))),
    };
    match serde_json::from_str::<Prefs>(&text) {
        Ok(p) if p.version > PREFS_VERSION => (defaults.clone(), Some(format!("préférences d'une version plus récente ({}) : valeurs par défaut", p.version))),
        Ok(p) => match p.validate() {
            Ok(()) => (p, None),
            Err(e) => (defaults.clone(), Some(format!("préférences invalides ({e}) : valeurs par défaut"))),
        },
        Err(e) => (defaults.clone(), Some(format!("préférences abîmées ({e}) : valeurs par défaut"))),
    }
}

/// Écrit le fichier de façon atomique (à côté, puis renommé).
pub fn save(path: &Path, p: &Prefs) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("dossier des préférences : {e}"))?;
    }
    let text = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("écriture des préférences : {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("écriture des préférences : {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_are_valid_and_resolve_to_the_historical_values() {
        let p = Prefs::default();
        p.validate().unwrap();
        let e = resolve(&p, &no_env);
        assert_eq!((e.driver.as_str(), e.buffer, e.threads, e.ahead_ms, e.ahead_threads), ("UMC", 64, 1, 0, 2));
        assert!(e.forced.is_empty());
    }

    #[test]
    fn a_first_start_uses_the_lookahead_only_with_four_cores_or_more() {
        for cores in [1, 2, 3] {
            assert_eq!(Prefs::recommended(cores), Prefs::default());
        }
        for cores in [4, 6, 16] {
            let p = Prefs::recommended(cores);
            assert_eq!((p.ahead_ms, p.ahead_threads, p.threads), (50, 2, 1));
            p.validate().unwrap();
            // 1 fil audio + 2 fils de rendu : toujours moins que les cœurs.
            assert!(1 + p.ahead_threads < cores);
        }
    }

    #[test]
    fn a_driver_is_designated_by_part_of_its_name() {
        assert!(same_driver("UMC", "UMC ASIO Driver"));
        assert!(same_driver("umc asio driver", "UMC"));
        assert!(!same_driver("ASIO4ALL v2", "UMC ASIO Driver"));
        assert!(!same_driver("", "UMC"));
    }

    #[test]
    fn environment_wins_and_is_reported_but_garbage_is_ignored() {
        let p = Prefs { buffer: Some(128), threads: 2, ..Prefs::default() };
        let env = |k: &str| match k {
            "DAW_BUFFER" => Some("32".to_string()),
            "DAW_AHEAD" => Some("100".to_string()),
            "DAW_THREADS" => Some("beaucoup".to_string()),
            "DAW_ASIO_DRIVER" => Some("  ".to_string()),
            _ => None,
        };
        let e = resolve(&p, &env);
        assert_eq!((e.buffer, e.ahead_ms, e.threads), (32, 100, 2));
        assert_eq!(e.driver, "UMC");
        assert_eq!(e.forced, vec!["buffer".to_string(), "ahead_ms".to_string()]);
    }

    #[test]
    fn validation_rejects_out_of_range_values() {
        let ok = Prefs::default();
        for bad in [
            Prefs { buffer: Some(4), ..ok.clone() },
            Prefs { buffer: Some(4096), ..ok.clone() },
            Prefs { threads: 0, ..ok.clone() },
            Prefs { threads: 17, ..ok.clone() },
            Prefs { ahead_ms: 5, ..ok.clone() },
            Prefs { ahead_ms: 900, ..ok.clone() },
            Prefs { ahead_threads: 0, ..ok.clone() },
            Prefs { driver: Some(" ".into()), ..ok.clone() },
            Prefs { metronome: MetroPrefs { volume: 2.0, ..MetroPrefs::default() }, ..ok.clone() },
            Prefs { metronome: MetroPrefs { count_in_bars: 3, ..MetroPrefs::default() }, ..ok.clone() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        Prefs { ahead_ms: 50, buffer: Some(32), threads: 3, ..ok }.validate().unwrap();
    }

    #[test]
    fn save_then_load_round_trips_and_partial_files_get_defaults() {
        let dir = std::env::temp_dir().join(format!("daw-prefs-{}", std::process::id()));
        let path = dir.join("sub").join("preferences.json");
        let p = Prefs { driver: Some("ASIO4ALL v2".into()), buffer: Some(32), threads: 3, ahead_ms: 50, ahead_threads: 2, metronome: MetroPrefs { enabled: true, volume: 1.0, count_in_bars: 1, ..MetroPrefs::default() }, ..Prefs::default() };
        save(&path, &p).unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        assert_eq!(load(&path, &Prefs::default()), (p, None));
        std::fs::write(&path, r#"{"buffer": 128}"#).unwrap();
        let (q, w) = load(&path, &Prefs::recommended(8));
        assert_eq!((q.buffer, q.threads, q.ahead_ms, w), (Some(128), 1, 0, None), "un fichier partiel garde les valeurs du crate, pas les recommandations");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_missing_newer_and_invalid_files_fall_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("daw-prefs-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.json");
        // Fichier absent : les valeurs données (ici celles d'un premier démarrage sur 8 cœurs).
        assert_eq!(load(&path, &Prefs::recommended(8)), (Prefs::recommended(8), None));
        for (text, expect_warning) in [("pas du json", true), (r#"{"version": 99}"#, true), (r#"{"threads": 0}"#, true), (r#"{"buffer": 64}"#, false)] {
            std::fs::write(&path, text).unwrap();
            let (p, w) = load(&path, &Prefs::recommended(8));
            assert_eq!(w.is_some(), expect_warning, "{text}");
            if expect_warning {
                assert_eq!(p, Prefs::recommended(8));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
