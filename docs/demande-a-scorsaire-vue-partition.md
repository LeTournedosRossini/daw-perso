# DAW → Scorsaire : demande d'un module « partition d'un clip de notes » (voir et éditer)

- **De** : daw (par Jean)
- **Pour** : scorsaire
- **Date** : 2026-11-08 23:30
- **Répond à** : `scorsaire-/docs/note-conception-scorsaire-dans-le-daw.md` (5 novembre) et à la discussion du 8 novembre avec Jean
- **Ce que j'attends** : (1) un avis sur la faisabilité et sur les points marqués « question » ; (2) un module livré comme décrit en section 3, dans ce dossier d'échange, avec le commit de Scorsaire dont il est tiré. Rien n'est codé côté DAW pour l'instant. Le dépôt de Scorsaire reste en lecture seule pour nous : nous n'y construisons rien.

## 1. Ce qu'on veut (premier lot)

**Voir et éditer les notes d'un clip MIDI sur une partition**, dans une fenêtre du DAW qui est une **autre vue d'édition du même clip que le piano roll**. Le DAW est la vérité : la partition est une interprétation des notes, jamais une copie. Un clic sur la partition **ajoute une note dans le clip** (par une commande du DAW), il n'y a pas de document Scorsaire à enregistrer.

Usage normal : un clip de 4 à 8 mesures, parfois plusieurs clips de plusieurs pistes empilés (une portée par clip), rarement un morceau entier. Pas de panneaux, pas de fichiers `.scorsaire`, pas de moteur de son : le DAW joue lui-même.

Premier lot : une portée par clip (clé de sol, de fa, ou deux portées pour le piano), **une seule voix**, hauteur / début / durée / ajout / suppression / déplacement. Hors premier lot : voix 2 à 4, tablature, batterie, accords générés, swing, rendu « en blocs » par sections (mais voir section 5, nous voulons que l'API ne l'interdise pas).

## 2. Ce que le DAW envoie (format exact de nos événements)

Temps en **fractions exactes de ronde** `[n, d]` (comme chez vous), tempo rationnel, chiffrage par mesure. Un clip de notes du DAW (`MClip`) garde toute sa matière et n'en montre qu'une **fenêtre** : il faut n'afficher **que la partie visible**.

Notre événement `notes` pour un clip :

```json
{ "type": "notes", "clip": 1000012,
  "start": [4, 1], "len": [2, 1], "offset": [0, 1],
  "notes": [ { "start": [1, 4], "len": [1, 8], "key": 64, "velocity": 0.8, "channel": 0 } ],
  "selection": [0, 2] }
```

- `start` : position du clip dans le projet (fraction de ronde) ; `len` : longueur **visible** ; `offset` : début de la partie visible dans la matière.
- Position d'une note : depuis le **début de la matière** du clip. Position dans le projet = `start − offset + note.start`. Une note n'est affichée que si elle tombe dans `[offset, offset + len)`.
- `key` : numéro MIDI 0 à 127 ; `velocity` : ]0, 1] ; `channel` : 0 à 15 (le canal ne se voit pas sur la partition).
- Les notes sont **triées** (début, touche, canal) et adressées par leur **indice** dans cette liste. Il n'y a pas d'identifiant de note : après chaque commande nous renvoyons le clip complet et `selection` (les indices des notes qui viennent d'être touchées).
- Carte des tempos et des chiffrages, fournies à l'ouverture : `tempo_map` (liste de `{ pos:[n,d], num, den }` : le tempo vaut `num/den` noires par minute, rationnel exact), `meters` (liste de `{ bar, num, den }`, la mesure `bar` à partir de 1). Pas d'armure dans le DAW pour l'instant : le module reçoit une armure en quintes (−7 à 7) **choisie dans la fenêtre** (réglage de la vue, pas dans le MIDI).

## 3. Ce qu'on demande : un module JavaScript autonome

Un fichier ES (ou un fichier + ses polices), sans dépendance, pas de panneaux, utilisable dans une page qui n'a ni bundler ni npm. Poids à indiquer (brut et gzip). API proposée (à discuter) :

```js
const view = createScoreView(container, { onEdit, onPlayheadSeek, locale: 'fr' });
view.setClips([                       // une portée (ou un système) par clip, dans l'ordre d'affichage
  { id: 1000012, name: 'Arpège', start: [4,1], len: [2,1], offset: [0,1],
    notes: [ { start: [1,4], len: [1,8], key: 64, velocity: 0.8 } ],
    selection: [0, 2],
    presentation: { staff: 'treble' | 'bass' | 'grand', grid: [1,16], keyFifths: 0 } } ]);
view.setTiming({ tempoMap, meters });   // pour numéroter les mesures, placer les barres, afficher le chiffrage
view.setPlayhead(fractionOfWhole | null);   // appelé ~30 fois par seconde, Scorsaire dessine le curseur
view.setStaffScale(1.0);                // taille de portée, réglée par l'utilisateur dans la fenêtre
view.setSelection(clipId, [indices]);
view.destroy();
```

Événement `onEdit(command)` : **le module ne modifie jamais ses données**, il n'a pas de mémoire propre. Il dit seulement ce que l'utilisateur veut faire ; le DAW exécute (annulation du DAW), renvoie le clip, et la vue se redessine (la mise en page incrémentale de Scorsaire doit réutiliser ce qui n'a pas changé). Commandes, toutes avec `clip` :

| Commande | Contenu |
|---|---|
| `add` | `{ start:[n,d], len:[n,d], key }` (début déjà placé sur la grille, durée par défaut de la grille ou de la dernière note) |
| `move` | `{ indices, dstart:[n,d], dkey }` (déplacement relatif d'un groupe) |
| `resize` | `{ index, len:[n,d] }` |
| `pitch` | `{ indices, dkey }` ou `{ indices, key }` |
| `delete` | `{ indices }` |
| `select` | `{ indices }` (pour que le DAW garde la sélection) |
| `preview` | `{ key, on }` (le DAW fait sonner la note pendant le clic) |

Entrée : **souris et doigt** (ce sera surtout une tablette), hauteur correctement orthographiée d'après l'armure choisie, position aimantée à la grille de la vue. L'utilisateur règle aussi la **taille de portée** dans la fenêtre.

## 4. Règles de lecture MIDI → partition (premier lot)

1. **Grille** : un réglage de vue (par clip), `grid` en fraction de ronde (1/16 par défaut). Le DAW garde les positions réelles ; la partition **arrondit à la grille** (case la plus proche) pour l'écriture. Une note qui a été arrondie peut être signalée très discrètement.
2. **Accord** : les notes dont le début arrondi est le même.
3. **Durée affichée** = la valeur musicale la plus proche de la durée réelle (une note qui tombe entre deux valeurs est affichée à la valeur la plus proche). Dans la voix unique, la durée affichée s'arrête au début de la note suivante (le MIDI joué a des chevauchements).
4. **Barres de mesure** : une note qui dépasse la barre est coupée et liée ; silences complétés ; altérations d'après l'armure ; ligatures par temps (déjà dans `placeChord` et vos règles de découpage).
5. **Édition** : si l'utilisateur change la durée d'une note sur la partition, **c'est la durée affichée qui est écrite** (le DAW la reçoit dans `resize`) ; toute durée qu'on peut entrer est affichable puisque ce sont des durées musicales.
6. Repères de renvoi, reprises, volta : **sans objet** (tout est linéaire). On ne dessine pas de marques de répétition.

## 5. Sections de la forme (intro, couplet…) : à harmoniser

Le DAW a depuis le 8 novembre une **carte de structure** (voie « Structure » de la timeline) : sections `{ start:[n,d], len:[n,d], name, color }` sans chevauchement, avec trous permis, enregistrées dans le projet. Votre `model/sections.js` est plus complet (catégories, couleurs par catégorie). Proposition que le DAW adopte de son côté :

- on **garde la représentation du DAW** (début et durée explicites) et on **ajoute une catégorie** à chaque section : `intro, verse, preChorus, chorus, bridge, solo, interlude, outro, coda, letter, custom` (vos identifiants) ;
- **couleurs par défaut = vos `SECTION_COLORS` et `LETTER_COLORS`**, pour que les deux logiciels montrent la même chose ;
- votre « dure jusqu'à la section suivante » devient une vue calculée côté DAW.

**Question** : cette liste et cette palette sont-elles figées ? Un nom en français (« Couplet 1 ») détermine-t-il la catégorie chez vous, ou la catégorie est-elle un champ séparé ? Dans l'API, prévoir de pouvoir recevoir ces sections (pour dessiner le nom au-dessus de la mesure) ; le rendu « en blocs » (intro, puis couplet 1 en dessous, puis refrain) viendra plus tard, sur plusieurs clips.

## 6. Contraintes pour le module

- **Licence** : GPLv3 comme Scorsaire ; le DAW est lui-même sous GPL (Rubber Band). Indiquer les licences des polices embarquées (Bravura, Barlow Condensed, OFL) et les citer dans un fichier à part.
- **Aucune dépendance réseau**, pas de police distante, pas de son (donc pas de SpessaSynth, pas de `sounds/`).
- **Poids** : le bundle `dist/note` pèse 7,9 Mo avec les sons ; ici seulement rendu + entrée : dites-nous ce que ça donne sans le moteur de son et sans la bibliothèque d'icônes.
- **Zoom de l'interface du DAW** : la page est agrandie par la propriété CSS `zoom` (60 à 175 %). Le module doit mesurer ses entrées souris et doigt en conséquence (rapport entre taille affichée et taille de mise en page).
- **Performance** : redessiner un clip de 8 mesures à chaque commande sans à-coup ; la partition ne doit jamais ralentir la lecture (tout est dans l'interface, pas dans le moteur audio).

## 7. Ce que le DAW fera de son côté

- fenêtre (flottante d'abord, puis ancrée comme le piano roll), taille de portée réglable et mémorisée, choix clé / armure / grille ;
- traduction des `onEdit` en nos commandes existantes (`add_notes`, `edit_notes`, `delete_notes`, `preview_note`), toutes annulables ;
- envoi de la position de lecture ;
- voix d'affichage plus tard : un champ `voice` dans la note (affichage seulement, ignoré par le moteur audio), pas d'identifiant de note.

## 8. Livraison

Dépôt d'échange : ce dossier, un sous-dossier `scorsaire-score-view/` avec le module, un `LISEZMOI` (API exacte, poids, version de Scorsaire d'où il est tiré, licences) et une page de démonstration qui l'utilise avec un clip de notes en JSON comme ci-dessus (c'est elle qui servira de test d'acceptation côté DAW). Nous copions ensuite le module dans le DAW avec son numéro de commit (comme pour la bibliothèque de calage).

Merci. Questions résumées : (1) l'API de la section 3 vous convient-elle ? (2) section 5 : catégories et palette figées ? (3) poids et faisabilité d'un module sans son ni icônes ? (4) l'incrémental fonctionne-t-il quand une seule note change dans un clip ?
