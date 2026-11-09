/*
 * Extension d'hôte CLAP « fr.flibuste.chord-track/1 » : la piste d'accords de l'hôte (DAW).
 *
 * Un plugin la demande avec  host->get_extension(host, FR_EXT_CHORD_TRACK). Si elle répond, elle fait FOI, même vide : le plugin ne
 * lit alors PAS la mémoire partagée (et un plugin « écrivain » cesse de publier). Si elle ne répond pas (autre hôte), repli sur la
 * mémoire partagée habituelle.
 *
 * Règles
 *  - Les positions sont en NOIRES (double), la même unité que `song_pos_beats` du transport CLAP (clap_event_transport).
 *    Débuts inclus, fins exclues : début <= t < début + longueur.
 *  - On peut interroger N'IMPORTE QUELLE position (chaque pas d'un arpégiateur, plusieurs dizaines par bloc) : recherche dichotomique,
 *    sans verrou ni allocation.
 *  - Les fonctions ne répondent QUE PENDANT `process()` (fil audio). Appelées ailleurs (interface, fil principal), elles répondent
 *    « rien » (faux / 0) : l'instantané appartient au graphe audio et n'existe pour le plugin que le temps d'un traitement. Un
 *    plugin qui veut dessiner la carte dans son interface garde la dernière lecture faite dans `process()`.
 *  - `revision()` change à chaque modification de la piste (annulations comprises) : à comparer pour invalider un cache.
 *  - Hauteurs : classes de hauteur 0 à 11 (do = 0). `intervals` : RELATIFS à la fondamentale (bit i = i demi-tons), basse exclue ; pour un
 *    accord, sur DEUX OCTAVES (i de 0 à 23 : une neuvième est à 14, pas à 2 : `add9` n'est pas `sus2`) ; un lecteur qui veut des classes de
 *    hauteur plie lui-même (i % 12).
 */
#ifndef FR_FLIBUSTE_CHORD_TRACK_H
#define FR_FLIBUSTE_CHORD_TRACK_H

#include <stdbool.h>
#include <stdint.h>
#include <clap/clap.h>

#define FR_EXT_CHORD_TRACK "fr.flibuste.chord-track/1"

/* Natures d'éléments. */
enum {
  FR_KIND_CHORD   = 0,
  FR_KIND_KEY     = 1,
  FR_KIND_SCALE   = 2,
  FR_KIND_PATTERN = 3,
  FR_KIND_MEASURE = 4,
};

/* fr_item_t.flags */
enum {
  FR_FLAG_NC    = 1, /* accord : « pas d'accord » (un trou) ; l'élément est quand même rendu, pour connaître sa durée */
  FR_FLAG_SHARP = 2, /* la fondamentale s'écrit avec un dièse (sinon un bémol) : écriture propre à chaque élément */
  FR_FLAG_MINOR = 4, /* tonalité mineure */
};

#define FR_NO_BASS 255
#define FR_FOREVER 1.0e12 /* longueur « jusqu'au marqueur suivant » du dernier marqueur de tonalité */

/*
 * Un élément. Disposition mémoire : 56 octets, alignement 8 (offsets : 0, 8, 16, 17, 18, 19, 20 (4 octets), 24, 28 ; 4 octets de remplissage à la fin).
 *
 *            start_beats  length_beats  root                 bass         intervals             index                name
 *  accord    début        durée         fondamentale 0-11    basse 0-11   intervalles sur 2     rang dans la voie    « Dm7 », « G7/B », « N.C. »,
 *                                                            ou 255       octaves (0-23)                            « C[0,4,7,13] » (qualité libre)
 *  tonalité  début du     jusqu'au      tonique 0-11         255          gamme majeure ou      rang du marqueur     « C major »
 *            marqueur     marqueur                                         mineure naturelle
 *            suivant      suivant                                          (`fifths` : −7 à 7)
 *  gamme     début        durée         fondamentale 0-11    255          notes de la gamme     rang dans la voie    nom affiché
 *  motif     début        durée         EMPLACEMENT 1 à 16   255          0                     rang dans la voie    nom du motif
 *  mesure    début        durée         NUMÉRATEUR           DÉNOMINATEUR 0                     rang depuis 0        (vide)
 */
typedef struct fr_item {
  double   start_beats;
  double   length_beats;
  uint8_t  root;
  uint8_t  bass;
  uint8_t  flags;
  int8_t   fifths;
  uint32_t intervals; /* bit i = i demi-tons au-dessus de la fondamentale, i de 0 à 23 ; gamme et tonalité : 0 à 11 ; motif et mesure : 0 */
  uint32_t index;
  char     name[24]; /* UTF-8, terminé par un octet nul */
} fr_item_t;

typedef struct fr_flibuste_chord_track {
  /* Numéro qui change à chaque modification de la piste. */
  uint32_t (*revision)(const clap_host_t *host);

  /* Nombre d'éléments de cette nature (tonalités : marqueurs + la tonalité globale s'il y en a une ; mesures : ceux de la règle du
   * morceau, 0 si le morceau n'en a pas et que les mesures se déduisent des chiffrages du projet). */
  uint32_t (*count)(const clap_host_t *host, uint32_t kind);

  /* L'élément de cette nature qui COUVRE `beat`. Faux si aucun. Gammes : celui dont le début est le plus récent. Tonalités : le
   * marqueur précédent, sinon la tonalité globale. Mesures : celle de la règle du morceau (anacrouse comprise), sinon celle que
   * donnent les chiffrages du projet. */
  bool (*lookup)(const clap_host_t *host, double beat, uint32_t kind, fr_item_t *out);

  /* Tous les éléments qui couvrent `beat`, DÉBUT LE PLUS RÉCENT D'ABORD (au plus `max`, et au plus 8). Utile pour les gammes qui se
   * chevauchent ; pour les autres natures, 0 ou 1 élément. Rend le nombre écrit. Règle commune : un lecteur qui n'en veut qu'une
   * prend la première. */
  uint32_t (*lookup_all)(const clap_host_t *host, double beat, uint32_t kind, fr_item_t *out, uint32_t max);

  /* Premier élément de cette nature qui COMMENCE STRICTEMENT APRÈS `beat`. Faux s'il n'y en a plus. */
  bool (*next_after)(const clap_host_t *host, double beat, uint32_t kind, fr_item_t *out);

  /* Rang (depuis 0) de l'élément de cette nature qui couvre `beat` (mesures : numéro de la mesure). Faux si aucun. */
  bool (*index_at)(const clap_host_t *host, double beat, uint32_t kind, uint32_t *out_index);
} fr_flibuste_chord_track_t;

/*
 * Qualités : identifiants du dictionnaire de Scorsaire (`M7`, `o7`, `m7b5`, `69`, `add9`...), dans le fichier d'échange et dans `name`.
 * Une qualité libre `[0,4,7,13]` s'affiche `C[0,4,7,13]`.
 *
 * Équivalences avec SharedChordMap::Reader (Carte) :
 *   lookup (beat, kind, out)   ->  lookup
 *   next (beat, kind, out)     ->  next_after
 *   indexAt (beat, Measure)    ->  index_at (beat, FR_KIND_MEASURE, ...)
 *   count (kind)               ->  count
 *   connected() / sequence()   ->  (toujours « connecté » quand l'extension répond) / revision()
 */

#endif
