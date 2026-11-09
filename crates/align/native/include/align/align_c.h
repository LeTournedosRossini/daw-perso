/*
 * flibuste-align : interface C pour caler une doublure (« double ») sur une prise de référence (« guide »), sans changer la hauteur.
 *
 * Utilisation type (hors temps réel, depuis un fil de travail ; jamais depuis le fil audio) :
 *   1. align_guide_create        analyse du guide (une fois ; réutilisable pour N doubles)
 *   2. align_analyse             chemin d'alignement + consonnes d'UN double
 *   3. align_render              le double recalé (même nombre de voies, durée du guide) ; refaisable avec d'autres réglages sans refaire 2.
 *   4. align_analysis_serialize  octets à enregistrer avec la prise ; align_analysis_deserialize pour refaire le rendu plus tard.
 *
 * Règles
 *  - Aucun état global : plusieurs appels en parallèle sur des fils différents. Une même poignée ne doit pas être utilisée par deux fils en même temps
 *    (align_guide peut en revanche être LU par plusieurs align_analyse en parallèle : il n'est jamais modifié après sa création).
 *  - Tampons audio : flottants ENTRELACÉS (échantillon 0 voie 0, échantillon 0 voie 1, ...). `frames` = nombre d'échantillons par voie.
 *  - Mémoire : tout ce que la bibliothèque rend (tampons, octets, textes) est libéré par align_free ; les poignées par leur fonction *_free.
 *  - Les fonctions rendent ALIGN_OK (0) ou un code d'erreur ; align_last_error() donne un message lisible (propre au fil, valable jusqu'au prochain appel).
 *    Aucune exception ne traverse cette interface.
 *  - Progression / annulation : `progress(fraction, user)` reçoit un avancement de 0 à 1 (croissant) ; rendre une valeur non nulle ANNULE : la fonction
 *    rend ALIGN_ERR_CANCELLED, sans fuite et sans résultat. `progress` peut être NULL.
 *  - Réglages : texte JSON (format « flibuste-align-settings », voir align_settings_default_json). NULL = réglages par défaut. Le bloc « general » se copie
 *    d'un double à l'autre ; le bloc « local » (courbe de force) jamais.
 *  - Même fréquence d'échantillonnage pour le guide et le double (sinon ALIGN_ERR_ARGUMENT) : rééchantillonner avant.
 *  - Octets de l'analyse : petit-boutiste (toutes les machines visées le sont).
 */
#ifndef FLIBUSTE_ALIGN_C_H
#define FLIBUSTE_ALIGN_C_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32) && defined(ALIGN_SHARED)
#  ifdef ALIGN_BUILDING
#    define ALIGN_API __declspec(dllexport)
#  else
#    define ALIGN_API __declspec(dllimport)
#  endif
#else
#  define ALIGN_API
#endif

#define ALIGN_C_API_VERSION 1

enum
{
    ALIGN_OK = 0,
    ALIGN_ERR_ARGUMENT = 1,  /* argument absent, hors limites, fréquences différentes... */
    ALIGN_ERR_CANCELLED = 2, /* annulé par le rappel de progression */
    ALIGN_ERR_FAILED = 3,    /* échec interne (mémoire, calcul) */
    ALIGN_ERR_FORMAT = 4     /* JSON ou octets d'analyse invalides */
};

typedef struct align_guide align_guide;       /* analyse du guide, réutilisable pour N doubles */
typedef struct align_analysis align_analysis; /* chemin d'alignement + consonnes d'un double */

/* Rappel de progression : rendre non nul pour annuler. */
typedef int (*align_progress_fn)(double fraction, void* user);

typedef struct align_render_info
{
    int engine_used; /* 0 Signalsmith, 1 Rubber Band R2, 2 Rubber Band R3 */
    int fell_back;   /* 1 si le moteur demandé a échoué et que Signalsmith l'a remplacé */
} align_render_info;

ALIGN_API int align_c_api_version(void);   /* ALIGN_C_API_VERSION de la bibliothèque compilée */
ALIGN_API const char* align_version(void); /* « 0.1.0 » */
ALIGN_API const char* align_last_error(void);
ALIGN_API void align_free(void* p);        /* libère un tampon, des octets ou un texte rendus par la bibliothèque */

/* ---- réglages (JSON) */
ALIGN_API const char* align_settings_default_json(void);                  /* fiche par défaut, texte constant (ne pas libérer) */
ALIGN_API int align_settings_validate(const char* json);                  /* ALIGN_OK ou ALIGN_ERR_FORMAT (+ message) */
/* Copier-coller des réglages généraux : *out_json (à libérer par align_free) = `dst_json` avec le bloc « general » de `src_json` ; « local » de `dst_json` intact. */
ALIGN_API int align_settings_merge_general(const char* dst_json, const char* src_json, char** out_json);

/* ---- guide (le mode d'analyse « same_pitch » / « harmony » vient de ses réglages ; les doubles doivent utiliser le même) */
ALIGN_API int align_guide_create(const float* interleaved, size_t frames, int channels, double sample_rate, const char* settings_json,
                                 align_progress_fn progress, void* user, align_guide** out);
ALIGN_API void align_guide_free(align_guide*);

/* ---- analyse d'un double : serrage et écart maximal viennent de `settings_json` ; mode d'analyse et fréquence doivent être ceux du guide. */
ALIGN_API int align_analyse(const align_guide* guide, const float* dub_interleaved, size_t frames, int channels, double sample_rate,
                            const char* settings_json, align_progress_fn progress, void* user, align_analysis** out);
ALIGN_API void align_analysis_free(align_analysis*);

/* ---- rendu : *out_interleaved (à libérer par align_free) = double recalé, `channels` voies, *out_frames = durée du guide.
 * Réglages utilisés : moteur, force, courbe de force, protection des consonnes. `info` peut être NULL. */
ALIGN_API int align_render(const align_analysis* analysis, const float* dub_interleaved, size_t frames, int channels, double sample_rate,
                           const char* settings_json, align_progress_fn progress, void* user, float** out_interleaved, size_t* out_frames,
                           align_render_info* info);

/* ---- enregistrer / relire l'analyse (avec la prise) : octets à libérer par align_free */
ALIGN_API int align_analysis_serialize(const align_analysis*, unsigned char** out_bytes, size_t* out_size);
ALIGN_API int align_analysis_deserialize(const unsigned char* bytes, size_t size, align_analysis** out);

/* ---- pour l'interface : tableaux copiés dans le tampon de l'appelant (capacité en éléments). Si `out` est NULL, seul *count est rempli.
 * Si la capacité est insuffisante : ALIGN_ERR_ARGUMENT et *count donne la taille nécessaire. */
ALIGN_API double align_analysis_hop_seconds(const align_analysis*);          /* durée entre deux points du chemin (0,005 s) */
ALIGN_API double align_analysis_sample_rate(const align_analysis*);
ALIGN_API size_t align_analysis_length_frames(const align_analysis*);        /* durée de la sortie = durée du guide, en échantillons */
ALIGN_API int align_analysis_get_path(const align_analysis*, double* out_seconds, size_t capacity, size_t* count);          /* position du double (s) à chaque point */
ALIGN_API int align_analysis_get_consonants(const align_analysis*, double* out_pairs, size_t capacity_pairs, size_t* count); /* (début s, fin s) dans le double */
ALIGN_API int align_analysis_set_consonants(align_analysis*, const double* pairs, size_t count);                           /* l'interface les modifie ; refaire le rendu ensuite */
/* Décalage appliqué (s) à chaque point du chemin, après force et courbe de force de `settings_json` (pour dessiner la courbe de décalage). */
ALIGN_API int align_applied_shift(const align_analysis*, const char* settings_json, double* out_seconds, size_t capacity, size_t* count);
/* Courbe de force proposée d'après les passages très décalés : paires (temps s, force 0 à 1). Liste vide = rien à proposer / force constante. */
ALIGN_API int align_suggest_strength(const align_analysis*, double threshold_ms, double* out_pairs, size_t capacity_pairs, size_t* count);

#ifdef __cplusplus
}
#endif

#endif /* FLIBUSTE_ALIGN_C_H */
