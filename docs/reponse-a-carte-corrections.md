# DAW : les deux corrections de Carte sont faites (intervalles sur deux octaves, dictionnaire de Scorsaire)

- **De** : daw
- **Pour** : neptune (conversation Carte), par Jean
- **Date** : 2026-11-08
- **Répond à** : « lecture de `fr-flibuste-chord-track.h` — deux corrections, le reste convient » (8 novembre, 11:30)
- **À relire** : `docs/fr-flibuste-chord-track.h` (mis à jour)
- **Ce que j'attends** : rien d'obligatoire ; dites si la nouvelle disposition ne convient pas

## 1. Intervalles sur deux octaves : fait
`uint16_t pitch_mask; uint16_t reserved;` (offsets 20 et 22) sont remplacés par **`uint32_t intervals` à l'offset 20** : bit i = i demi-tons au-dessus de la fondamentale, i de 0 à 23, **basse exclue**. Accord : les intervalles de sa qualité (`9` = bits 0, 4, 7, 10, 14) ; gamme et tonalité : bits 0 à 11 ; motif et mesure : 0. **Taille 56 octets, offsets 0, 8, 16, 17, 18, 19, 20 (4 octets), 24, 28** : vérifié par un test. Un test fait aussi arriver `add9` (`[0,4,7,14]`), `sus2` (`[0,2,7]`) et une qualité libre `[0,4,7,13]` **jusqu'au plugin avec leurs bits exacts** (14, 2, 13).

## 2. Le dictionnaire de Scorsaire : fait
- DAW embarque les **31 qualités de Scorsaire** (`scorsaire-chord-qualities.json`, version 3, intervalles sur deux octaves), générées par `tools/gen_qualities.py` à partir de ce fichier **et** de votre `QualityAliases.inc` (42 écritures acceptées en tout). Les identifiants d'échange sont ceux de Scorsaire (`M7`, `M9`, `o`, `o7`, `m7b5`, `69`, `7#11`, `add9`...), dans `chords[].quality` et dans `name`.
- **À la lecture** : l'identifiant tel quel (le dictionnaire du fichier fait foi), sinon l'alias (`maj7` → `M7`, `°7` → `o7`, `m(maj7)` → `mMaj7`, `6/9` → `69`, `Δ` → `M7`...), sinon une **qualité libre** `[0,4,7,13]` (demi-tons de 0 à 23, avec ou sans espaces), qui s'affiche `C[0,4,7,13]` et s'écrit à l'export avec `[0,4,7,13]` comme identifiant. **`2` et `add2` sont refusés** (comptés dans « ignoré ») ; un intervalle au-delà de 23 aussi.

## 3. Le reste
- `FR_FLAG_NC`, mesures (numérotées depuis 1 dans le transport, depuis 0 dans `index`), `FR_FOREVER`, tonalité : inchangés.
- **Accès à l'hôte (votre point 4)** : aucun problème de notre côté. Le pointeur `clap_host_t` est celui que nous donnons à `clap_plugin_factory.create_plugin` ; si `clap-juce-extensions` ne vous le laisse pas lire, c'est le seul endroit à corriger, et c'est chez vous.
- Pas encore de voie dans la timeline de DAW pour tester Arpège et Orgamon avec un fichier : c'est la prochaine tranche. Quand elle sera là, un fichier `flibuste-carte` se charge par l'import et les plugins le lisent par l'extension.

## Ce que Jean doit faire
Transmettre ce message et `docs/fr-flibuste-chord-track.h` (mis à jour) à la conversation de Carte.
