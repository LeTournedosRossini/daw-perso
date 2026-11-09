# scorsaire-score-view : la partition d'un clip de notes, pour le DAW

Module ES autonome (un seul fichier, aucune dépendance, aucun réseau, aucun son, aucune police distante). Il dessine les notes d'un clip MIDI en partition et dit ce que l'utilisateur veut y changer ; **il ne garde aucune donnée** : le DAW exécute la commande et renvoie le clip.

- **Tiré de** : Scorsaire, branche `module-vue-partition-daw`, sur la base du commit `b9bf05b`. Commits du module : `5684d43`, `1569cde`, `e9e4707`.
- **Poids** : `scorsaire-score-view.js` = **957 Ko bruts, 339 Ko gzip**. Aucune police : les symboles sont les contours de Bravura embarqués dans le fichier.
- **Licences** : GPLv3 ou ultérieure (`licences/Scorsaire-GPLv3.txt`) ; contours de glyphes de la police Bravura (SIL OFL, `licences/Bravura-OFL.txt`). La police Barlow Condensed n'est pas utilisée ici.
- **Essai** : servir ce dossier (`python3 -m http.server`) et ouvrir `index.html`. La page simule un DAW minimal : les notes y vivent, la vue envoie des commandes, la page les exécute et renvoie le clip. C'est la page d'acceptation.

## API

```js
import { createScoreView } from './scorsaire-score-view.js';
const view = createScoreView(container, { onEdit, onPlayheadSeek, onError, locale: 'fr' | 'en' });
view.setTiming({ meters: [{ bar: 1, num: 4, den: 4 }] });   // barres de mesure (la mesure `bar` à partir de 1)
view.setClips([clip, …]);      // une portée (ou un système) par clip, dans l'ordre d'affichage
view.updateClip(clip);         // remplace UN clip (ajouté à la fin s'il n'est pas encore affiché)
view.setPlayhead([n, d] | null);   // position dans le projet, en fraction de ronde
view.setStaffScale(1.0);       // 0,5 à 3 (l'espace de portée vaut 8 px à 1,0)
view.setSelection(clipId, [indices]);
view.setSections([{ start, len, name, category?, color? }]);   // nom écrit au-dessus de la mesure de début
view.setTool('edit' | 'seek'); // 'seek' : un tap appelle onPlayheadSeek au lieu d'écrire
view.setEntryVoice(1);          // voix d'affichage (1 à 4) des notes ajoutées d'un tap : `add` part avec `voice`
view.getSelection(clipId);     // les indices sélectionnés maintenant, dans l'ordre de votre liste ([] si aucun)
view.setEditGrid([n, d] | null); // grille des GESTES (add, move, resize, flèches, fantômes, seek), indépendante de l'écriture ; null = celle de `presentation.grid`
view.setHoverGhost(true);       // note fantôme sous la souris (ou un stylet), jamais au doigt (défaut : oui)
view.setMeasureNumbers(true);  // numéro de mesure du projet au début de chaque système (défaut : oui)
view.setNewNoteLength([n, d]); // durée d'une note ajoutée d'un tap (1/4 par défaut ; suit le dernier `resize`)
view.resize();                 // redessine à la taille actuelle du conteneur (voir « Taille du conteneur »)
view.destroy();
```

`clip` : `{ id, name, start, len, offset, notes: [{ start, len, key, velocity, channel }], selection?: [indices], presentation: { staff: 'treble' | 'bass' | 'grand', grid: [1,16], keyFifths: -7…7 } }`, exactement les champs de votre événement `notes`. Les fractions sont `[n, d]`. Les notes doivent être dans **votre ordre trié** (début, touche, canal) : les indices des commandes sont ceux de cette liste.

### Grille d'écriture et grille d'édition
`presentation.grid` règle **l'écriture** (arrondi des débuts et des durées affichés). La **grille d'édition** règle seulement l'aimantation des gestes : `add`, `move` (souris et flèches ← →), `resize` (Alt + ← →), fantômes (glisser et survol) et `onPlayheadSeek`. Priorité : `presentation.editGrid` du clip, puis `view.setEditGrid`, puis `presentation.grid`. Changer la grille d'édition ne redessine rien. Les lignes de la grille d'édition sont comptées depuis le début de chaque mesure (comme celles de l'écriture) ; les positions envoyées par `onEdit` restent depuis le début de la matière et exactes en fractions.

### Grille automatique (`presentation.grid: 'auto'`)
Le module choisit la grille d'écriture **mesure par mesure**, la plus grossière sur laquelle **tous les débuts de notes** de la mesure tombent, à **un tick près** (1/3 840 de ronde). Essayées dans l'ordre : 1/4, 1/8, 1/16, puis 1/6 + 1/8, 1/12 + 1/8, 1/12 + 1/16, 1/24 + 1/16 (une grille de triolet garde la ligne binaire voisine, donc une croche et un triolet cohabitent). Rien ne convient (prise jouée en direct) : **1/16 comme avant**, les positions sont arrondies et la pastille « arrondie » le dit ; aucune tolérance plus large pour « deviner » un triolet. Les **fins** de notes peuvent aussi tomber sur les lignes de 1/16 (une note courte garde sa durée). La grille d'édition par défaut d'un clip en `'auto'` est 1/16 (`setEditGrid` / `presentation.editGrid` la remplacent comme avant). Un clip qui change de grille d'une mesure à l'autre se redessine en entier comme tout clip modifié.

### Morceau qui commence au milieu d'une mesure (`clip.startInBar`)
`clip.startInBar` = `[n, d]` (fraction de ronde) : la position, **dans la mesure qui contient `clip.start`**, où le morceau commence. La première mesure dessinée est **raccourcie** de cette longueur (une anacrouse), **garde son numéro** (celui du projet) et ses temps (les groupes de croches et les n-olets restent alignés sur les temps de la mesure complète). Les notes et syllabes qui commencent avant la coupure sont ignorées ; celle qui la traverse commence à la coupure. Les positions envoyées par `onEdit` restent mesurées depuis le début de la matière, sans changement. Le module **ne déduit pas** `startInBar` de `clip.start` : sans ce champ, la première mesure est entière comme avant. Une valeur nulle, négative ou ≥ à la longueur de la mesure est ignorée.

### Paroles
`clip.lyrics` : des syllabes posées à un instant exact (`start`, depuis le début de la matière, comme les notes ; seules celles de la partie visible sont écrites), arrondies à la grille de leur mesure, alignées sur les colonnes des notes au même instant ; une syllabe sans note crée sa colonne. `verse` (1 à 8, 1 par défaut) = la ligne, empilées sous la dernière portée. `hyphen: true` écrit un tiret collé à la syllabe (« ma- »). `end` (même repère que `start`, strictement après) : le trait de prolongation d'un mélisme, de la fin de la syllabe jusqu'à cet instant (arrondi à la grille, coupé à la dernière mesure, un segment par système). `presentation.lyricsLine: true` réserve la ligne de paroles même sans syllabe (pour pouvoir taper la première). `view.setLyricVerse(n)` : le couplet (1 à 8) demandé par un tap sur une place vide.

**Saisie d'une syllabe** : un tap (sans glisser) dans la bande de paroles sous la dernière portée émet `lyricTap` (voir la table). Le module ne montre aucun champ de saisie : l'hôte ouvre le sien à `rect` et répond par `updateClip`. Un trait orange marque la place jusqu'au prochain `updateClip`.

### Sections et numéros de mesure
`clip.section: { name, category, color? }` : un titre coloré (couleur de la catégorie, ou la vôtre) au-dessus du clip ; **des clips consécutifs de même section** (même nom, même catégorie, même couleur) **restent sous un seul titre** (celui du premier). Le `name` du clip s'écrit toujours au-dessus de sa portée. Les numéros de mesure sont ceux du projet (la mesure 1 est la première du projet).

### Note fantôme au survol
Souris ou stylet au-dessus d'une portée (aucun bouton enfoncé) : une note translucide à la place arrondie à la grille où un clic écrirait, avec les lignes supplémentaires quand la hauteur est hors de la portée. Pas de fantôme au doigt, pas en mode `seek`, pas au-dessus d'une note existante (un clic la sélectionne), pas pendant un glisser (celui-ci a le sien, avec le nom de la note). `setHoverGhost(false)` le coupe. Les lignes supplémentaires apparaissent aussi sur le fantôme du glisser.

### Armure
`keyFifths` est l'armure du début ; `keyChanges` (facultatif) change l'armure à partir de la mesure qui contient `start` (position comptée depuis le début de la matière, comme les notes) et jusqu'au changement suivant. Un `start` en cours de mesure prend effet à la mesure SUIVANTE. Chaque mesure écrit ses notes d'après son armure (la hauteur d'un tap aussi). Le dessin montre la nouvelle armure (avec les bécarres qui annulent l'ancienne) au changement.

### Aperçu pendant un glisser
Pendant un glisser (une note déplacée ou une note qu'on pose), la vue dessine une note translucide à la place arrondie à la grille, avec son nom et sa mesure (« Fa♯4 · m. 5 ») ; elle disparaît au relâchement. Une note posée d'un tap prend la position et la hauteur où le doigt se lève (celle du fantôme).

### Taille du conteneur
Le conteneur est observé (`ResizeObserver`, redessin 30 ms après la dernière variation) : déplacer, agrandir, plein écran ou ancrer la fenêtre redessine seul, la sélection est gardée. Un conteneur masqué (largeur 0) n'est pas redessiné avant d'être montré à nouveau. `view.resize()` force le redessin (utile après un changement que le navigateur ne signale pas, comme la propriété CSS `zoom`). Non vérifié dans un navigateur actif : dans le panneau de test que nous avons, aucun `ResizeObserver` ne se déclenche (même le nôtre) ; `resize()`, lui, a été vérifié.

### Erreurs
`onError({ clip, message })` : un clip que le modèle ne sait pas écrire n'empêche pas les autres ; sa place montre le message (rouge) et l'hôte en est informé.

### Triolets et n-olets
Une grille non binaire (`presentation.grid` = 1/12, 1/6, 1/20…) est écrite en VRAIS n-olets : les temps dont des débuts ou fins de notes ne tombent pas sur une ligne binaire deviennent des triolets (3:2), quintolets, etc., avec leur crochet et leur chiffre. Trois notes d'un tiers de temps = un triolet de croches ; trois notes d'un tiers de demi-mesure (deux temps) = un triolet de noires. Un trou dans un n-olet est un silence du n-olet. Les positions binaires restent exactes à côté (une croche et un triolet dans la même mesure) : le module arrondit chaque position à la ligne la plus proche entre la grille donnée et sa grille binaire voisine. Une note qui dépasse son n-olet est coupée et liée. Les duolets (6/8) ne sont pas traités. Si les positions d'un temps ne tombent sur aucune division usuelle, elles sont arrondies au triolet (la pastille « arrondie » le dit).

### Nom du clip
`clip.hideName: true` (ou `name: ""`) : pas de ligne de nom sous le titre de section.

### Ce que la vue montre
Seules les notes dont le début est dans `[offset, offset + len)` de la matière, à `start − offset + note.start` dans le projet. **Voix d'affichage** : chaque note peut porter `voice` (1 à 4 ; absent ou invalide = 1). Chaque voix a ses propres durées : une note s'arrête au début de la note suivante *de sa voix* (une basse tenue en voix 2 n'est pas coupée par la mélodie). Les hampes se séparent comme dans Scorsaire. Les silences d'une voix 2 à 4 s'écrivent là où la voix n'a rien, à l'intérieur de ses mesures. Une voix sans note n'est pas dessinée. Portée `grand` : elle envoie les touches ≥ 60 sur la clé de sol et les autres sur la clé de fa. Début et fin sont arrondis aux lignes de la grille **comptées depuis le début de la mesure qui contient la note** (un clip qui commence entre deux lignes s'écrit donc en mesures entières ; pour des mesures qui commencent sur la grille du projet, c'est la grille absolue) ; une note arrondie porte un petit point d'avertissement. Un accord = les notes de même début arrondi, de durée médiane, coupée au début de l'accord suivant ; ce qui passe une barre de mesure est coupé et lié ; les silences sont complétés. Les barres de mesure viennent de `meters` (jamais d'un chiffrage unique).

### `onEdit(command)`
Toutes les commandes portent `clip` (l'identifiant). **Toutes les positions sont comptées depuis le début de la matière du clip** (comme votre événement `notes`) et déjà aimantées à la grille de la vue (grille absolue du projet : `start − offset` est pris en compte).

| `type` | Contenu | Geste |
|---|---|---|
| `add` | `start`, `len`, `key`, `voice` (celle de `setEntryVoice`) | tap sur une portée vide (la hauteur vient de la ligne, avec l'armure choisie) |
| `move` | `indices`, `dstart`, `dkey` | glisser une note sélectionnée ; flèches ← → (une case de grille) |
| `pitch` | `indices`, `dkey` | flèches ↑ ↓ (demi-ton ; Maj = octave) |
| `resize` | `index`, `len` | Alt + ← / → (une case de grille) |
| `delete` | `indices` | Suppr. / Retour arrière |
| `select` | `indices` | tap sur une note (Maj / Ctrl : ajoute ou retire), Échap : vide |
| `voice` | `indices`, `voice` (1 à 4) | touches 1 à 4 avec une sélection (sans Ctrl / Alt / Méta) |
| `lyricTap` | `start` (depuis le début de la matière), `verse`, `text` (la syllabe touchée, ou `null` sur une place vide), `hyphen`, `end` (si la syllabe en a un), `rect` `{ left, top, width, height }` en pixels de la fenêtre : où poser le champ de saisie | tap sur une syllabe (la modifier) ou sur une place vide de la bande de paroles (position = colonne de la grille d'édition la plus proche) ; ne fait pas d'ajout de note |
| `preview` | `key`, `on` | `true` à l'appui sur une note ou sur la portée ; **pendant un glisser, `true` avec la nouvelle touche à chaque changement de touche seulement** (sans `false` entre deux : l'hôte coupe la note précédente) ; `false` au relâchement (avec la dernière touche émise) |

`onPlayheadSeek([n, d])` : seulement avec `setTool('seek')`, position du projet arrondie à la grille.

**Sélection** : les indices suivent VOTRE liste triée, la vue ne suppose jamais qu'un indice reçu plus tôt désigne encore la même note. Quand les données d'un clip changent, la sélection est effacée sauf si le clip arrive avec `selection` (ou si vous appelez `setSelection`). Un geste en cours est abandonné si le clip change.

**Doigts** : un deuxième doigt posé pendant un geste d'un doigt, une perte de capture ou un `pointercancel` abandonnent le geste sans aucune commande (seul `preview: false` est émis). Vous n'avez pas besoin d'émettre de `pointercancel`.

**Couleurs des sections** : `section-colors.json` (catégories, couleur par catégorie, huit couleurs de lettre), produit par `tools/export-section-colors.mjs`.

## Mesuré (rien d'autre n'est affirmé)
- Clip de 8 mesures, 32 notes, sous Node 22 : mise en page ~3 ms après un changement, 7 à 10 ms sans le cache, SVG 0,3 ms.
- Dans Chromium (page de démonstration, une portée piano, 7 notes) : `updateClip` 6,5 ms en moyenne (reconstruction complète du clip, mise en page, SVG et insertion), 29 ms la première fois.
- **Limite connue** : ce premier jet reconstruit le document interne du clip à chaque `updateClip` ; le cache de mise en page par mesure de Scorsaire n'est PAS encore utilisé. À 6,5 ms pour 8 mesures ce n'est pas gênant ; une écriture par différence viendra si un clip plus long le demande.
- Essayé dans Chromium avec la souris : ajout, sélection, glisser (temps et hauteur), clavier (↑ ↓ Alt+→ Suppr.), page agrandie par la propriété CSS `zoom` à 150 % (la conversion des clics passe par le rapport taille affichée / taille de mise en page). Tests Node : `tests/scoreview-plan.test.js` (9 tests : barres, fenêtre visible, accords, coupe et liaison, arrondi, piano à deux portées, correspondance avec les indices).

## Pas vérifié
- Le doigt sur une vraie tablette (les événements sont des `pointer*`, avec `touch-action: pan-y` sur la vue : un glisser vertical fait défiler la page).
- Les zooms 60 à 175 % autres que 150 % ; plusieurs clips empilés (le code les gère, la démonstration n'en montre qu'un).
- Les altérations posées plus tôt dans la même mesure (un tap utilise l'armure seule, pas les altérations de la mesure).
- Les changements de chiffrage dans un clip (ils viennent de `meters`, par mesure) : testés sur un seul changement dans les tests Node, pas à l'écran. L'armure change maintenant par `keyChanges` (voir plus haut).
- Les durées qui ne sont pas des multiples d'une grille binaire (triolets) : première version, non traités.
