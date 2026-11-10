# align-native : caler une doublure sur un guide (interface C)

Paquet autonome de sources pour DAW (à copier dans `crates/align/native/`). Généré par `tools/make_daw_bundle.py` du dépôt `flibuste-align` ; **ne pas modifier ici** : la source est dans ce dépôt.

## Contenu
- `include/align/align_c.h` : **l'interface C** (à lire en premier, tout y est commenté). Les autres en-têtes sont internes.
- `src/*.cpp`, `sources.txt` : les sources à compiler (+ `third_party/rubberband/single/RubberBandSingle.cpp`, déjà dans `sources.txt`).
- `third_party/` : Rubber Band (GPL v2 ou ultérieure), Signalsmith Stretch et Signalsmith Linear (MIT, en-têtes), dr_wav (domaine public, ne sert qu'à la démonstration).
- `demo/align_demo.c` : programme de démonstration **en C pur** (un exemple de chaque famille de fonctions) et **test de bout en bout**.
- `ref/` : `guide.wav` et `double.wav` (7 s, voix synthétiques, mono 44,1 kHz) + `onsets.txt` (vérité connue : temps du guide, temps du double, début de chaque note).
- `CMakeLists.txt` : construit la bibliothèque et la démonstration, pour vérifier le paquet seul (`ctest`). DAW n'en a pas besoin.

## Compiler depuis `build.rs` (crate `cc`)
- Fichiers : ceux de `sources.txt` ; en-têtes : `include/`, `third_party/` et `third_party/rubberband/`.
- C++17 avec **exceptions activées** (elles servent à l'annulation, jamais exposées au C) ; MSVC : `/std:c++17 /EHsc` et **`NOMINMAX`** (sinon Rubber Band ne compile pas) ; GCC/Clang : `-std=c++17`. Aucun `-D` autre. Aucune bibliothèque à lier (sous Linux, `-lpthread` si votre édition de liens l'exige pour `thread_local`).
- Esquisse :
```rust
cc::Build::new().cpp(true).std("c++17")
    .include("native/include").include("native/third_party").include("native/third_party/rubberband")
    .files(sources_txt_lines).define("NOMINMAX", None)   // NOMINMAX utile surtout pour MSVC
    .compile("flibuste_align");
```

## Règles de l'interface (détail dans `align_c.h`)
- **Hors temps réel, fil de travail uniquement.** Aucun état global ; plusieurs appels en parallèle sur des fils différents ; un `align_guide` peut être lu par plusieurs `align_analyse` en parallèle.
- **Tampons entrelacés** `float` (voie 0, voie 1, ... pour chaque échantillon) ; `frames` = échantillons par voie ; même fréquence pour guide et double (sinon `ALIGN_ERR_ARGUMENT`).
- **Mémoire** : ce que la bibliothèque rend (tampons, octets, textes) se libère par `align_free` ; les poignées par `align_guide_free` / `align_analysis_free`.
- **Erreurs** : codes `ALIGN_OK / ALIGN_ERR_ARGUMENT / _CANCELLED / _FAILED / _FORMAT` + `align_last_error()` (message en français, propre au fil). Aucune exception ne traverse le C.
- **Progression et annulation** : `int progress(double fraction, void* user)`, fraction croissante de 0 à 1 ; rendre non nul annule (`ALIGN_ERR_CANCELLED`, sortie mise à NULL, rien ne fuit).
- **Réglages = JSON** (`align_settings_default_json()` donne la fiche par défaut). Bloc `general` (mode d'analyse `same_pitch` / `harmony`, serrage, écart maximal, moteur `signalsmith` / `rubberband_r2` / `rubberband_r3`, force, protection des consonnes) : **se copie d'un double à l'autre** (`align_settings_merge_general`). Bloc `local` (courbe de force) : **jamais copié**. Les consonnes modifiées à la main vivent dans l'analyse (`align_analysis_set_consonants`), pas dans la fiche, et ne se copient jamais non plus.
- **Analyse sérialisable** (`align_analysis_serialize` / `_deserialize`, octets « ALN1 », petit-boutiste, environ 1,7 Ko par seconde de guide) : à enregistrer avec la prise ; le rendu refait à partir de ces octets est **identique au bit près** (graine fixe).
- **Mode d'analyse** : il fait partie du guide. Pour un double en mode « harmonies » il faut un guide créé avec ce mode (sinon `ALIGN_ERR_ARGUMENT`) : DAW garde au plus deux poignées de guide par lot.
- **Pour l'interface** : `align_analysis_get_path` (position du double à chaque point de 5 ms), `align_applied_shift` (décalage appliqué après force et courbe), `align_analysis_get_consonants` / `_set_consonants`, `align_suggest_strength` (courbe de force proposée).
- **Durée** : jusqu'à 3 minutes d'un coup (mesuré : 127 s de voix stéréo : analyse 7 s, 290 Mo ; rendu 3 s avec Signalsmith).

## Étirement par marqueurs (`align_warp_render`, interface version 2)
Sans guide ni analyse : une liste de couples (temps source, temps de sortie) en secondes, entrelacés, le premier (0, 0), le dernier (durée de la source, durée voulue). Chaque tronçon a son rapport sortie / source, borné à [0,25 ; 4] (sinon `ALIGN_ERR_ARGUMENT` avec le tronçon fautif dans `align_last_error()`) ; tronçon de moins de 10 ms de source refusé. Sortie : durée exacte à l'échantillon près, mêmes voies et même fréquence, retard du moteur compensé, mêmes entrées = mêmes échantillons (les trois moteurs, `general.engine` des réglages). `info->fell_back` dit si Rubber Band a été remplacé par Signalsmith.
Mesuré (Linux, 44,1 kHz, mono, voir `tests/test_c_api.cpp`) :
- **Instants** : pour des salves de bruit de 8 ms, Signalsmith les rend 1,9 à 2,8 ms après l'instant prévu (début de la salve) ; Rubber Band R2 : centre de la salve à 9 ms au plus ; R3 : jusqu'à 17 ms (il étale les attaques et ajoute un pré-écho). Ce n'est donc pas du ± 1 ms : le chiffre vaut pour les pistes tonales, pas pour des transitoires isolés.
- **Rapport exactement 1** : pas laissé intact par Signalsmith (écart maximal 0,015 pour un sinus de crête 0,5, soit environ -30 dB) ; Rubber Band R2 : 0,0007 ; R3 : 0,0000 (inchangé). Si l'inaltéré compte, ne pas rendre les tronçons à rapport 1 et copier la source.
- **Niveau** : un moteur peut dépasser la crête de l'entrée (sinus de 0,5 étiré par R2 : crête 0,8 ; salves de bruit : crête de 1,58 pour 0,8 en entrée avec Signalsmith). Prévoir un écrêtage ou une limite de crête côté DAW.
- **Durée** : aucune limite propre à cette fonction (celle de 3 minutes est celle du calage). Mesuré pour 5 minutes en stéréo à 48 kHz, quatre marqueurs : Signalsmith 12 s, Rubber Band R2 25 s, R3 89 s ; crête mémoire environ 510 Mo (Linux, un fil). Annulation propre à tout moment.
- Réglages propres à l'étirement : aucun pour ce premier lot (valeurs par défaut des moteurs).

## Test de bout en bout
`ctest` (ou `align_demo ref/guide.wav ref/double.wav ref/onsets.txt`) : le chemin doit retrouver les débuts de notes de `onsets.txt` (erreur moyenne sous 15 ms, pire sous 45 ms), la sortie a la durée du guide, l'analyse relue donne le même rendu, l'annulation est propre. Le même scénario, écrit côté Rust, est le test d'intégration attendu côté DAW.

## Licences
GPLv3 pour l'ensemble (`LICENSE-GPLv3`) ; Rubber Band : GPL v2 ou ultérieure ; Signalsmith Stretch / Linear : MIT ; dr_wav : domaine public / MIT-0.
