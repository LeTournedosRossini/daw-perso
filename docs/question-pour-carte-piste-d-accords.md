# Question de DAW pour la conversation de Carte : la piste d'accords de l'hôte

**Contexte.** DAW (notre hôte audio, écrit en Rust, hôte CLAP) va avoir une **piste d'accords intégrée** : une voie de la timeline qui porte les accords, les gammes (qui peuvent se chevaucher), les tonalités et les appels de motifs, **dans le format `flibuste-carte` version 1** (fractions de ronde `[n, d]`, `qualities`, `chords`, `scales` avec `intervals`, `keys`, `patterns`). Un logiciel qui a cette piste n'a **pas besoin du plugin Carte** ; Carte reste pour les logiciels qui ne l'ont pas. Les plugins de la suite (Arpège, etc.) liraient la piste **directement auprès de l'hôte**, sans mémoire partagée.

Comme CLAP n'a rien de standard pour ça, DAW définirait une **extension d'hôte maison** (identifiant provisoire `fr.flibuste.chord-track/1`), que le plugin obtient avec `host->get_extension(host, ...)`. Si l'extension n'existe pas (autre hôte), le plugin retombe sur sa mémoire partagée actuelle.

Je ne veux pas inventer une interface qui ne vous convient pas. **Voici ce que je propose ; dis-moi ce qui est faux ou manquant.**

## Proposition (C, lecture seule, sans verrou, sans allocation, appelable depuis `process`)

```c
typedef struct fr_flibuste_chord_track {
  // Numéro qui change à chaque modification de la piste (pour invalider un cache).
  uint32_t (*revision)(const clap_host_t *host);

  // Position en temps : noires (double), la même unité que song_pos_beats du transport CLAP.
  // Accord actif à la position. Retourne false si aucun (ou « pas d'accord »).
  bool (*chord_at)(const clap_host_t *host, double beat, fr_chord_t *out);

  // Gammes actives à la position (elles peuvent se chevaucher). Retourne le nombre écrit (≤ max).
  uint32_t (*scales_at)(const clap_host_t *host, double beat, fr_scale_t *out, uint32_t max);

  // Tonalité à la position (le marqueur précédent, sinon la tonalité globale).
  bool (*key_at)(const clap_host_t *host, double beat, fr_key_t *out);

  // Appel de motif actif (emplacement 1 à 16). 0 si aucun.
  uint32_t (*pattern_at)(const clap_host_t *host, double beat);

  // Prochain changement (accord, gamme, tonalité ou motif) strictement après la position ; false s'il n'y en a plus.
  bool (*next_change_after)(const clap_host_t *host, double beat, double *next_beat);
} fr_flibuste_chord_track_t;

typedef struct { uint8_t root_pc; uint8_t bass_pc; bool has_bass; bool nc;
                 uint16_t intervals_mask;      // bit i = intervalle de i demi-tons depuis la racine
                 char name[16]; } fr_chord_t; // « Dm7 »
typedef struct { uint8_t root_pc; uint16_t intervals_mask; char name[24]; } fr_scale_t;
typedef struct { int8_t fifths; uint8_t root_pc; bool minor; } fr_key_t;
```

Les hauteurs sont des **classes de hauteur 0 à 11** (do = 0). L'**écriture** (dièse ou bémol, `"F#"` ou `"Gb"`) se déduit de `fifths` de la tonalité, ou du `name` : dites-moi si vous avez besoin d'autre chose.

## Questions précises

1. **Unité de position.** Votre `SharedChordMap` se consulte « avec sa propre position de lecture (ppq) ». Les noires en `double` conviennent-elles ? DAW fournira aussi le transport CLAP (`clap_event_transport` : tempo, `song_pos_beats`, lecture, boucle), **que nous ne donnions pas encore aux plugins** — c'est corrigé en même temps.
2. **Interface actuelle.** Quelles sont **exactement** les requêtes de `SharedChordMap` que vos lecteurs utilisent aujourd'hui ? Je voudrais que l'extension ait la même forme, pour que la modification chez vous soit minime. Si c'est plus simple pour vous, donnez-moi le fichier `SharedChordMap.h` et la liste des appels.
3. **Besoins en plus.** Un lecteur a-t-il besoin de la **règle des mesures** (anacrouse, numéro de mesure, chiffrage), ou seulement des positions en noires ? L'arpégiateur a-t-il besoin de **regarder en avance** (plusieurs accords à venir), ou `next_change_after` suffit ?
4. **Chevauchement des gammes.** Combien de gammes simultanées au plus (ma limite serait 8) ? Et pour l'import, vous écrivez « la dernière posée l'emporte » : dans la piste de DAW, on garderait **toutes** les gammes qui se chevauchent, et c'est le lecteur qui choisit. D'accord ?
5. **Qualités et gammes.** Pour la saisie dans DAW, j'ai besoin du **dictionnaire de qualités par défaut de Carte** (identifiant, suffixe affiché, intervalles) et du catalogue de **72 gammes** (je le lirai dans `scales.json` de Scorsaire si c'est la source). Où est le dictionnaire de Carte ?
6. **Mesures.** Le fichier peut décrire une anacrouse et des mesures de durées quelconques. La carte de chiffrages de DAW ne change que **aux débuts de mesure**. Le cas de l'anacrouse est-il important pour vous (dans ce cas j'ajouterai une mesure initiale incomplète) ?
7. **Écriture.** Quand DAW a la piste, vos plugins en mode « écrivain » n'ont plus de raison d'écrire. Comment le plugin doit-il le savoir ? Mon idée : si `get_extension` répond, le rôle « écrivain » est grisé et le plugin lit l'hôte.

Je commence côté DAW par ce qui ne dépend pas de vos réponses : fournir le **transport** aux plugins, et le **modèle** de la piste (accords, gammes, tonalités, motifs, import et export `flibuste-carte`). L'interface C ci-dessus n'est figée qu'après votre retour.
