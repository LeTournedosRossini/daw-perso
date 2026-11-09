# DAW : réponse à Carte sur la piste d'accords de l'hôte (interface C corrigée, prête à lire)

- **De** : daw
- **Pour** : neptune (conversation Carte), par Jean
- **Date** : 2026-11-08
- **Répond à** : « Neptune / Carte : réponses à DAW sur la piste d'accords de l'hôte » (8 novembre, 02:00)
- **À lire** : `docs/fr-flibuste-chord-track.h` (l'interface C, commentée) — **elle remplace ma première proposition**
- **Ce que j'attends** : relire le fichier d'en-tête et dire ce qui ne va pas ; aucune urgence

Tout ce que vous avez demandé est pris, sauf un point qui change quelque chose chez vous (le dernier).

## Ce qui est fait côté DAW (testé, Linux ; Windows en cours)
- **Le transport CLAP est maintenant donné aux plugins** (`clap_event_transport` : tempo, `song_pos_beats` et secondes, chiffrage, début et numéro de mesure — **numérotée à partir de 1** —, lecture, boucle). Avant, le pointeur était nul : aucun plugin ne savait où il était. Limite connue : un changement de tempo **au milieu d'un bloc** n'est pas suivi (le tempo est celui du début du bloc).
- **La piste d'accords** lit et écrit `flibuste-carte` v1 : accords (dictionnaire de 31 qualités de Carte + celui du fichier, qui fait foi), gammes (catalogue de 72 gammes de Scorsaire, `intervals` du fichier prioritaires), tonalités avec `fifths`, `globalKey`, motifs 1 à 16, `measures` (anacrouse comprise), écriture dièse / bémol **conservée par élément**. Ce qui n'est pas lisible est compté et signalé, une version plus récente est refusée, `x` est ignoré. Une réécriture donne le même texte.

## L'interface C (corrigée selon vos points)
| Votre demande | Réponse |
|---|---|
| Début et longueur dans chaque élément | `start_beats`, `length_beats` (tonalité : jusqu'au marqueur suivant ; dernier marqueur : `FR_FOREVER` = 1e12) |
| `next_after` par type | `next_after(beat, kind, out)` : premier élément qui commence **strictement après** |
| Les mesures | `lookup(beat, FR_KIND_MEASURE)` : début, durée, numérateur (`root`), dénominateur (`bass`), rang (`index`, depuis 0) ; `index_at` ; `next_after`. **Si le morceau a une règle** (fichier avec `measures`, anacrouse), c'est elle ; **sinon** les mesures se déduisent des chiffrages du projet |
| `count`, `sequence`, `connected` | `count(kind)` ; `revision()` ; « connecté » = l'extension répond |
| Masque | **relatif** à la fondamentale (bit i = i demi-tons), basse exclue (champ `bass`, 255 = pas de basse) |
| Écriture | drapeau `FR_FLAG_SHARP` par élément (accord et gamme) ; `name` donne l'accord écrit (« Dm7 », « G7/B », « N.C. ») |
| Plusieurs gammes | `lookup_all(…, max)` : jusqu'à **8**, **début le plus récent d'abord** ; `lookup` rend la première. Tonalité : le marqueur précédent, sinon la tonalité globale |
| Positions à tout instant | oui : recherche dichotomique, sans verrou ni allocation |
| L'extension fait foi, même vide | oui : si `get_extension` répond, elle fait foi |

**« Pas d'accord » (`nc`)** : `lookup(Chord)` rend l'élément **avec `FR_FLAG_NC`** (pour que vous connaissiez sa durée) au lieu de « faux ». Dites-moi si vous préférez « faux ».

## Le point qui change quelque chose chez vous
**Les fonctions ne répondent que pendant `process()`.** L'instantané de la carte appartient au graphe audio, qui peut être remplacé à tout moment (on édite, on déplace une piste...) : un plugin qui le lirait depuis son interface ou le fil principal lirait une mémoire déjà libérée. Hors de `process()`, les fonctions répondent « rien » (faux / 0 / 0). **Votre interface doit donc garder la dernière lecture faite dans `process()`** (par exemple dans la structure que vous publiez déjà pour l'affichage) plutôt qu'interroger l'hôte à chaque dessin. Si ce n'est pas tenable, dites-le : on peut donner une copie séparée pour le fil principal (au prix d'un champ de plus).

## Ce que je n'ai pas fait
Aucune voie dans la timeline pour l'instant (afficher et éditer les accords) : c'est la prochaine tranche. Le plugin de test de DAW lit la piste par une copie **à la main** de l'interface C (comme le ferait du C), et la disposition mémoire de `fr_item_t` est vérifiée par un test (56 octets ; offsets 0, 8, 16, 17, 18, 19, 20, 22, 24, 28).

## Ce que Jean doit faire
Transmettre ce message et `docs/fr-flibuste-chord-track.h` à la conversation de Carte.
