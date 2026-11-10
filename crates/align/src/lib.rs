//! Caler une prise (le « double ») sur une autre (le « guide »), sans changer la hauteur : enveloppe sûre de la bibliothèque C++
//! `flibuste-align` (`native/`, interface `align_c.h`).
//!
//! Hors temps réel : **à n'appeler que depuis un fil de travail**. Aucun état global ; un [`Guide`] peut servir à plusieurs analyses en
//! parallèle sur des fils différents ; une même [`Analysis`] ne se touche que d'un fil à la fois. Tampons audio **entrelacés**.
//!
//! Tout le `unsafe` (appels C) est ici, regroupé et commenté ; l'interface publique est sûre.

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

mod ffi {
    use super::*;

    #[repr(C)]
    pub struct RawGuide {
        _p: [u8; 0],
    }
    #[repr(C)]
    pub struct RawAnalysis {
        _p: [u8; 0],
    }
    #[repr(C)]
    #[derive(Default)]
    pub struct RenderInfo {
        pub engine_used: c_int,
        pub fell_back: c_int,
    }
    pub type ProgressFn = Option<unsafe extern "C" fn(c_double, *mut c_void) -> c_int>;

    extern "C" {
        pub fn align_c_api_version() -> c_int;
        pub fn align_version() -> *const c_char;
        pub fn align_last_error() -> *const c_char;
        pub fn align_free(p: *mut c_void);
        pub fn align_settings_default_json() -> *const c_char;
        pub fn align_settings_validate(json: *const c_char) -> c_int;
        pub fn align_settings_merge_general(dst: *const c_char, src: *const c_char, out: *mut *mut c_char) -> c_int;
        pub fn align_guide_create(data: *const f32, frames: usize, channels: c_int, rate: c_double, settings: *const c_char, progress: ProgressFn, user: *mut c_void, out: *mut *mut RawGuide) -> c_int;
        pub fn align_guide_free(g: *mut RawGuide);
        pub fn align_analyse(g: *const RawGuide, data: *const f32, frames: usize, channels: c_int, rate: c_double, settings: *const c_char, progress: ProgressFn, user: *mut c_void, out: *mut *mut RawAnalysis) -> c_int;
        pub fn align_analysis_free(a: *mut RawAnalysis);
        pub fn align_render(a: *const RawAnalysis, data: *const f32, frames: usize, channels: c_int, rate: c_double, settings: *const c_char, progress: ProgressFn, user: *mut c_void, out: *mut *mut f32, out_frames: *mut usize, info: *mut RenderInfo) -> c_int;
        pub fn align_warp_render(data: *const f32, frames: usize, channels: c_int, rate: c_double, keys: *const c_double, key_count: usize, settings: *const c_char, progress: ProgressFn, user: *mut c_void, out: *mut *mut f32, out_frames: *mut usize, info: *mut RenderInfo) -> c_int;
        pub fn align_analysis_serialize(a: *const RawAnalysis, out: *mut *mut u8, size: *mut usize) -> c_int;
        pub fn align_analysis_deserialize(bytes: *const u8, size: usize, out: *mut *mut RawAnalysis) -> c_int;
        pub fn align_analysis_hop_seconds(a: *const RawAnalysis) -> c_double;
        pub fn align_analysis_sample_rate(a: *const RawAnalysis) -> c_double;
        pub fn align_analysis_length_frames(a: *const RawAnalysis) -> usize;
        pub fn align_analysis_get_path(a: *const RawAnalysis, out: *mut c_double, capacity: usize, count: *mut usize) -> c_int;
        pub fn align_analysis_get_consonants(a: *const RawAnalysis, out: *mut c_double, capacity: usize, count: *mut usize) -> c_int;
        pub fn align_analysis_set_consonants(a: *mut RawAnalysis, pairs: *const c_double, count: usize) -> c_int;
        pub fn align_applied_shift(a: *const RawAnalysis, settings: *const c_char, out: *mut c_double, capacity: usize, count: *mut usize) -> c_int;
        pub fn align_suggest_strength(a: *const RawAnalysis, threshold_ms: c_double, out: *mut c_double, capacity: usize, count: *mut usize) -> c_int;
    }
}

/// Pourquoi un appel a échoué.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlignError {
    /// Argument invalide (hors limites, fréquences différentes, mode d'analyse différent de celui du guide...).
    Argument(String),
    /// Annulé par le rappel de progression : aucun résultat.
    Cancelled,
    /// Échec interne de la bibliothèque (mémoire, calcul).
    Failed(String),
    /// JSON de réglages ou octets d'analyse invalides.
    Format(String),
}

impl std::fmt::Display for AlignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlignError::Cancelled => write!(f, "calage annulé"),
            AlignError::Argument(m) | AlignError::Failed(m) | AlignError::Format(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for AlignError {}

/// Message de la dernière erreur du fil courant (valable jusqu'au prochain appel de la bibliothèque sur ce fil).
fn last_error() -> String {
    // SAFETY: `align_last_error` rend un texte C terminé par NUL, propre au fil, valable jusqu'au prochain appel : copié tout de suite.
    unsafe {
        let p = ffi::align_last_error();
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

fn check(code: c_int) -> Result<(), AlignError> {
    match code {
        0 => Ok(()),
        1 => Err(AlignError::Argument(last_error())),
        2 => Err(AlignError::Cancelled),
        4 => Err(AlignError::Format(last_error())),
        _ => Err(AlignError::Failed(last_error())),
    }
}

fn c_settings(json: Option<&str>) -> Result<Option<CString>, AlignError> {
    json.map(|j| CString::new(j).map_err(|_| AlignError::Format("réglages : octet nul dans le texte".into()))).transpose()
}

fn ptr_of(c: &Option<CString>) -> *const c_char {
    c.as_ref().map_or(ptr::null(), |s| s.as_ptr())
}

/// Rappel de progression : reçoit la fraction (0 à 1, croissante) ; rendre `true` annule l'appel.
pub type Progress<'a> = &'a mut dyn FnMut(f64) -> bool;

/// Passerelle vers le rappel Rust : un panic ne traverse jamais le C (il annule l'appel).
unsafe extern "C" fn trampoline(fraction: c_double, user: *mut c_void) -> c_int {
    // SAFETY: `user` est le `&mut Progress` posé par `with_progress` pour la durée de l'appel, jamais utilisé ailleurs.
    let cb = &mut *(user as *mut Progress<'_>);
    match catch_unwind(AssertUnwindSafe(|| cb(fraction))) {
        Ok(false) => 0,
        _ => 1,
    }
}

/// Appelle `f(progress, user)` avec le rappel de `progress` (ou sans rappel si `None`).
fn with_progress<T>(progress: Option<Progress<'_>>, f: impl FnOnce(ffi::ProgressFn, *mut c_void) -> T) -> T {
    match progress {
        Some(mut p) => {
            let user = &mut p as *mut Progress<'_> as *mut c_void;
            f(Some(trampoline), user)
        }
        None => f(None, ptr::null_mut()),
    }
}

/// Version de la bibliothèque C++ (« 0.1.0 ») et de son interface C.
pub fn library_version() -> (String, i32) {
    // SAFETY: `align_version` rend un texte constant terminé par NUL ; `align_c_api_version` n'a pas d'argument.
    unsafe { (CStr::from_ptr(ffi::align_version()).to_string_lossy().into_owned(), ffi::align_c_api_version()) }
}

/// Fiche de réglages par défaut (JSON : blocs `general` et `local`).
pub fn default_settings_json() -> String {
    // SAFETY: texte constant, terminé par NUL, à ne pas libérer.
    unsafe { CStr::from_ptr(ffi::align_settings_default_json()).to_string_lossy().into_owned() }
}

/// Vérifie une fiche de réglages (JSON).
pub fn validate_settings(json: &str) -> Result<(), AlignError> {
    let c = CString::new(json).map_err(|_| AlignError::Format("réglages : octet nul dans le texte".into()))?;
    // SAFETY: `c` est un texte C valide pendant l'appel.
    check(unsafe { ffi::align_settings_validate(c.as_ptr()) })
}

/// Copier-coller des réglages généraux : `dst` avec le bloc `general` de `src` ; le bloc `local` de `dst` reste intact.
pub fn merge_general(dst: &str, src: &str) -> Result<String, AlignError> {
    let (d, s) = (CString::new(dst), CString::new(src));
    let (Ok(d), Ok(s)) = (d, s) else { return Err(AlignError::Format("réglages : octet nul dans le texte".into())) };
    let mut out: *mut c_char = ptr::null_mut();
    // SAFETY: textes C valides ; `out` reçoit un texte alloué par la bibliothèque, copié puis libéré par `align_free`.
    unsafe {
        check(ffi::align_settings_merge_general(d.as_ptr(), s.as_ptr(), &mut out))?;
        let text = CStr::from_ptr(out).to_string_lossy().into_owned();
        ffi::align_free(out as *mut c_void);
        Ok(text)
    }
}

/// Audio entrelacé à analyser ou à rendre.
#[derive(Clone, Copy)]
pub struct Audio<'a> {
    pub samples: &'a [f32],
    pub channels: usize,
    pub sample_rate: f64,
}

impl Audio<'_> {
    fn frames(&self) -> Result<usize, AlignError> {
        if self.channels == 0 || self.channels > 16 || self.samples.len() % self.channels != 0 {
            return Err(AlignError::Argument("audio : 1 à 16 voies et un nombre d'échantillons multiple du nombre de voies".into()));
        }
        Ok(self.samples.len() / self.channels)
    }
}

/// Analyse du guide, réutilisable pour N doubles (jamais modifiée après sa création).
pub struct Guide {
    raw: *mut ffi::RawGuide,
}

// SAFETY: la bibliothèque garantit qu'un guide n'est jamais modifié après sa création et peut être lu par plusieurs analyses en parallèle.
unsafe impl Send for Guide {}
unsafe impl Sync for Guide {}

impl Guide {
    /// Analyse le guide. `settings` : fiche JSON (`None` = défaut) ; son mode d'analyse fait partie du guide.
    pub fn new(audio: Audio<'_>, settings: Option<&str>, progress: Option<Progress<'_>>) -> Result<Guide, AlignError> {
        let frames = audio.frames()?;
        let s = c_settings(settings)?;
        let mut raw = ptr::null_mut();
        // SAFETY: les tampons et le texte vivent pendant l'appel ; `raw` reçoit une poignée libérée par `Drop`.
        let code = with_progress(progress, |p, user| unsafe { ffi::align_guide_create(audio.samples.as_ptr(), frames, audio.channels as c_int, audio.sample_rate, ptr_of(&s), p, user, &mut raw) });
        check(code)?;
        Ok(Guide { raw })
    }

    /// Analyse un double : chemin d'alignement et consonnes. Même fréquence que le guide ; même mode d'analyse.
    pub fn analyse(&self, dub: Audio<'_>, settings: Option<&str>, progress: Option<Progress<'_>>) -> Result<Analysis, AlignError> {
        let frames = dub.frames()?;
        let s = c_settings(settings)?;
        let mut raw = ptr::null_mut();
        // SAFETY: `self.raw` est une poignée valide (jamais modifiée) ; tampons et texte valides pendant l'appel.
        let code = with_progress(progress, |p, user| unsafe { ffi::align_analyse(self.raw, dub.samples.as_ptr(), frames, dub.channels as c_int, dub.sample_rate, ptr_of(&s), p, user, &mut raw) });
        check(code)?;
        Ok(Analysis { raw })
    }
}

impl Drop for Guide {
    fn drop(&mut self) {
        // SAFETY: poignée rendue par `align_guide_create`, libérée une seule fois.
        unsafe { ffi::align_guide_free(self.raw) }
    }
}

/// Moteur qui a réellement servi au rendu, et si un repli sur Signalsmith a eu lieu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderInfo {
    /// 0 Signalsmith, 1 Rubber Band R2, 2 Rubber Band R3.
    pub engine_used: i32,
    pub fell_back: bool,
}

/// Chemin d'alignement + consonnes d'un double (l'analyse est lente, le rendu rapide : on peut refaire le rendu avec d'autres réglages).
pub struct Analysis {
    raw: *mut ffi::RawAnalysis,
}

// SAFETY: une analyse peut changer de fil ; la bibliothèque interdit seulement son usage simultané par deux fils (`&mut` ou propriétaire unique ici ;
// les méthodes en `&self` ne font que lire et ne sont pas `Sync`).
unsafe impl Send for Analysis {}

impl Analysis {
    /// Le double recalé (mêmes voies, durée du guide), entrelacé. Réglages utilisés : moteur, force, courbe de force, protection des consonnes.
    pub fn render(&self, dub: Audio<'_>, settings: Option<&str>, progress: Option<Progress<'_>>) -> Result<(Vec<f32>, RenderInfo), AlignError> {
        let frames = dub.frames()?;
        let s = c_settings(settings)?;
        let (mut out, mut out_frames) = (ptr::null_mut::<f32>(), 0usize);
        let mut info = ffi::RenderInfo::default();
        // SAFETY: poignée valide ; tampons valides pendant l'appel ; la sortie est copiée dans un `Vec` puis libérée par `align_free`.
        let code = with_progress(progress, |p, user| unsafe { ffi::align_render(self.raw, dub.samples.as_ptr(), frames, dub.channels as c_int, dub.sample_rate, ptr_of(&s), p, user, &mut out, &mut out_frames, &mut info) });
        check(code)?;
        // SAFETY: en cas de succès la bibliothèque a rendu `out_frames × channels` flottants valides.
        let data = unsafe {
            let v = std::slice::from_raw_parts(out, out_frames * dub.channels).to_vec();
            ffi::align_free(out as *mut c_void);
            v
        };
        Ok((data, RenderInfo { engine_used: info.engine_used, fell_back: info.fell_back != 0 }))
    }

    /// Octets à enregistrer avec la prise (petit-boutiste, « ALN1 »).
    pub fn to_bytes(&self) -> Result<Vec<u8>, AlignError> {
        let (mut out, mut size) = (ptr::null_mut::<u8>(), 0usize);
        // SAFETY: poignée valide ; les octets rendus sont copiés puis libérés par `align_free`.
        unsafe {
            check(ffi::align_analysis_serialize(self.raw, &mut out, &mut size))?;
            let v = std::slice::from_raw_parts(out, size).to_vec();
            ffi::align_free(out as *mut c_void);
            Ok(v)
        }
    }

    /// Relit une analyse enregistrée : le rendu refait à partir d'elle est identique au bit près.
    pub fn from_bytes(bytes: &[u8]) -> Result<Analysis, AlignError> {
        let mut raw = ptr::null_mut();
        // SAFETY: octets valides pendant l'appel ; la poignée rendue est libérée par `Drop`.
        check(unsafe { ffi::align_analysis_deserialize(bytes.as_ptr(), bytes.len(), &mut raw) })?;
        Ok(Analysis { raw })
    }

    /// Durée entre deux points du chemin (secondes).
    pub fn hop_seconds(&self) -> f64 {
        // SAFETY: poignée valide.
        unsafe { ffi::align_analysis_hop_seconds(self.raw) }
    }

    pub fn sample_rate(&self) -> f64 {
        // SAFETY: poignée valide.
        unsafe { ffi::align_analysis_sample_rate(self.raw) }
    }

    /// Durée de la sortie = durée du guide, en échantillons.
    pub fn length_frames(&self) -> usize {
        // SAFETY: poignée valide.
        unsafe { ffi::align_analysis_length_frames(self.raw) }
    }

    /// Lit un tableau de `f64` rendu par la bibliothèque en deux temps (taille, puis copie), `per` nombres par élément.
    fn read(&self, fetch: impl Fn(*mut c_double, usize, *mut usize) -> c_int, per: usize) -> Result<Vec<f64>, AlignError> {
        let mut count = 0usize;
        check(fetch(ptr::null_mut(), 0, &mut count))?;
        let mut v = vec![0.0f64; count * per];
        let cap = count;
        check(fetch(v.as_mut_ptr(), cap, &mut count))?;
        v.truncate(count * per);
        Ok(v)
    }

    /// Position du double (secondes) à chaque point de 5 ms du guide.
    pub fn path(&self) -> Result<Vec<f64>, AlignError> {
        // SAFETY: poignée valide ; le tampon fourni a la capacité annoncée.
        self.read(|o, c, n| unsafe { ffi::align_analysis_get_path(self.raw, o, c, n) }, 1)
    }

    /// Consonnes du double : paires (début, fin) en secondes.
    pub fn consonants(&self) -> Result<Vec<(f64, f64)>, AlignError> {
        // SAFETY: poignée valide ; capacité en paires, tampon de 2 × capacité.
        let v = self.read(|o, c, n| unsafe { ffi::align_analysis_get_consonants(self.raw, o, c, n) }, 2)?;
        Ok(v.chunks_exact(2).map(|p| (p[0], p[1])).collect())
    }

    /// Remplace les consonnes (paires début-fin en secondes) ; refaire le rendu ensuite.
    pub fn set_consonants(&mut self, pairs: &[(f64, f64)]) -> Result<(), AlignError> {
        let flat: Vec<f64> = pairs.iter().flat_map(|p| [p.0, p.1]).collect();
        // SAFETY: poignée valide, accès exclusif (`&mut self`) ; tableau de `pairs.len()` paires.
        check(unsafe { ffi::align_analysis_set_consonants(self.raw, flat.as_ptr(), pairs.len()) })
    }

    /// Décalage appliqué (secondes) à chaque point, après force et courbe de force de la fiche.
    pub fn applied_shift(&self, settings: Option<&str>) -> Result<Vec<f64>, AlignError> {
        let s = c_settings(settings)?;
        // SAFETY: poignée valide ; texte valide pendant l'appel ; tampon de la capacité annoncée.
        self.read(|o, c, n| unsafe { ffi::align_applied_shift(self.raw, ptr_of(&s), o, c, n) }, 1)
    }

    /// Courbe de force proposée d'après les passages décalés de plus de `threshold_ms` : paires (temps s, force 0 à 1).
    pub fn suggest_strength(&self, threshold_ms: f64) -> Result<Vec<(f64, f64)>, AlignError> {
        // SAFETY: poignée valide ; capacité en paires.
        let v = self.read(|o, c, n| unsafe { ffi::align_suggest_strength(self.raw, threshold_ms, o, c, n) }, 2)?;
        Ok(v.chunks_exact(2).map(|p| (p[0], p[1])).collect())
    }
}

impl Drop for Analysis {
    fn drop(&mut self) {
        // SAFETY: poignée rendue par la bibliothèque, libérée une seule fois.
        unsafe { ffi::align_analysis_free(self.raw) }
    }
}

/// Un marqueur d'étirement : l'instant `source` (en secondes depuis le début de la source) se retrouve à l'instant `output` de la sortie.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpKey {
    pub source: f64,
    pub output: f64,
}

/// Rapport sortie / source permis pour un tronçon (la bibliothèque le refuse au-delà).
pub const WARP_RATIO_MIN: f64 = 0.25;
pub const WARP_RATIO_MAX: f64 = 4.0;
/// Durée minimale de source d'un tronçon, en secondes (la bibliothèque refuse moins).
pub const WARP_MIN_SEGMENT: f64 = 0.010;

/// Étire `audio` selon des marqueurs, **sans changer la hauteur** (interface C version 2, sans guide ni analyse).
///
/// `keys` : au moins deux marqueurs, le premier à (0, 0), le dernier à (durée de la source, durée voulue) ; temps strictement croissants dans les
/// deux colonnes ; entre deux marqueurs le rapport est constant, dans [`WARP_RATIO_MIN`, `WARP_RATIO_MAX`] ; chaque tronçon d'au moins
/// [`WARP_MIN_SEGMENT`] de source. `settings` : fiche JSON dont seul `general.engine` est lu (`signalsmith`, `rubberband_r2`, `rubberband_r3` ;
/// `None` = Signalsmith). Rend `round(dernier instant de sortie × fréquence)` échantillons par voie, alignés sur la ligne de temps de sortie.
/// Déterministe. **Hors temps réel : fil de travail seulement.**
pub fn warp_render(audio: Audio<'_>, keys: &[WarpKey], settings: Option<&str>, progress: Option<Progress<'_>>) -> Result<(Vec<f32>, RenderInfo), AlignError> {
    let frames = audio.frames()?;
    let s = c_settings(settings)?;
    let flat: Vec<f64> = keys.iter().flat_map(|k| [k.source, k.output]).collect();
    let (mut out, mut out_frames) = (ptr::null_mut::<f32>(), 0usize);
    let mut info = ffi::RenderInfo::default();
    // SAFETY: tampons, marqueurs et texte valides pendant l'appel ; la sortie est copiée dans un `Vec` puis libérée par `align_free`.
    let code = with_progress(progress, |p, user| unsafe {
        ffi::align_warp_render(audio.samples.as_ptr(), frames, audio.channels as c_int, audio.sample_rate, flat.as_ptr(), keys.len(), ptr_of(&s), p, user, &mut out, &mut out_frames, &mut info)
    });
    check(code)?;
    // SAFETY: en cas de succès la bibliothèque a rendu `out_frames × channels` flottants valides.
    let data = unsafe {
        let v = std::slice::from_raw_parts(out, out_frames * audio.channels).to_vec();
        ffi::align_free(out as *mut c_void);
        v
    };
    Ok((data, RenderInfo { engine_used: info.engine_used, fell_back: info.fell_back != 0 }))
}
