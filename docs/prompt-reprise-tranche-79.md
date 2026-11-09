Reprise du projet DAW (Rust + interface web dans Tauri), dossier /home/jean/Bureau/DAW perso. Lis d'abord CLAUDE.md : c'est le document de référence (règles de travail §14, journal des tranches §15 à la fin, dernière tranche = 78) ; le protocole page ↔ hôte est dans docs/protocole-ui.md. Je m'appelle Jean, je travaille en français.

## Où on en est (9 novembre 2026)
Tout est fait, vérifié, compilé pour Windows (daw-app.exe du 09/11 à 09:15) et **essayé par moi avec succès** : partition (voix, tonalités, armure, aperçu du glisser), voie « Tonalités », voies de contrôle du piano roll. Tests : Linux et Windows **107 résultats « ok », aucun échec** (`cargo test --workspace --no-fail-fast`, derniers passages avant les tranches 77-78 qui n'ont changé que de la page), **242 tests Node** pour la page.

Dernières tranches (74 à 78) :
- 74 : partition avec **plusieurs clips empilés** (une portée par clip, réglages de vue par clip).
- 75 : module de Scorsaire mis à jour ; **champ « voix » 1 à 4 d'une note** (affichage seulement, ignoré par l'audio, enregistré dans le projet) ; boutons de voix.
- 76 : **voie « Tonalités »** (4ᵉ voie épinglée : accords, tonalités, gammes, structure) ; correction : `meters[].bar` envoyé au module à partir de 1.
- 77 : touche K, bouton **♪ Partition** du bandeau, `keyChanges` (changements d'armure dans un clip), **voies de contrôle du piano roll** (`set_events` : modulation, volume, expression, pédale, molette, pression, contrôleur libre ; dessin, déplacement, Suppr).
- 78 : **l'armure de la partition est la piste des tonalités** : le choix « Armure » pose/remplace la tonalité au début de la mesure de la tête de lecture (comme K), ✕ la retire ; module de Scorsaire avec **aperçu pendant le glisser** (la note sonne à chaque demi-ton, note fantôme avec son nom et sa mesure).

La partition vient du **module de Scorsaire** copié sans modification dans `crates/ui/web/js/vendor/scorsaire-score-view/` (ORIGINE.txt donne la version). **Le dépôt de Scorsaire (`~/Bureau/Scorsair/…`) est en lecture seule : ne rien y modifier.** Les échanges avec leur conversation se font par fichiers dans `~/Bureau/FlibusteSuite/echanges/messages/` (le module livré est dans `echanges/scorsaire-score-view/`) ; je colle moi-même leurs réponses et mes messages.

## Ce que je veux faire ensuite (dans cet ordre)
1. **Fenêtre de partition** (`js/ui/score-window.js`) : elle est fixée en haut à gauche et seulement redimensionnable. Je veux (a) pouvoir la **déplacer** (par son titre, souris et doigt), (b) un bouton **plein écran** (et retour), (c) la **fenêtre ancrée sous la piste**, comme le piano roll ancré (`piano-roll.js` : lignes `roll` / `roll-cont` de `orderedRows`), avec le même composant. Le module a `setStaffScale`, etc. ; la taille de portée et les réglages restent ceux de la fenêtre.
2. **Rendu « en blocs » par sections** : intro, puis couplet 1 en dessous, puis refrain… à partir de la **carte de structure** (sections avec catégorie, `state.structure`) : chaque section devient un bloc de mesures empilé ; il faut afficher **plusieurs clips / pistes par bloc**. C'est à demander à Scorsaire (leurs conteneurs de systèmes / `blockband.js`) : écrire un message à leur conversation avec l'API voulue avant de coder.
3. **Paroles** (idée de Jean, à discuter avant de coder) : une **piste « Paroles »** dans les voies épinglées de la timeline (avec accords, tonalités, gammes, structure), dont le texte s'affiche **sous les notes de la partition**. Mon avis (à confirmer avec lui) : les paroles vivent dans le DAW (la vérité), en **syllabes positionnées en fractions de ronde** `{ at, text, hyphen (la syllabe continue), verse? }`, saisies dans la voie (double-clic, comme les accords) et dans la partition ; la partition les affiche sous la note la plus proche. Il faut demander à Scorsaire ce que leur module sait dessiner (leur modèle a des paroles : `layout/lyrics.js`, `gridlyrics.js`) : une API du genre `setLyrics([{ start, text, hyphen }])` et, pour la saisie, une commande `lyric`. Un message pour eux est déjà écrit dans `echanges/messages/2026-11-09_1030_de-daw_pour-scorsaire-paroles-et-fenetre.md` (à coller par Jean).
4. **Voies de contrôle du piano roll** : copier-coller de points ; amincissement automatique d'une courbe trop dense (aujourd'hui un point par case de la grille) ; pression **par touche** (`poly`, lue mais non affichée) ; changement de canal d'un point.
5. **GitHub Actions** : Jean part à midi et veut lancer le workflow ensuite. Écrire le workflow (Linux, Windows, macOS ; macOS jamais compilé : la couche CoreAudio n'existe pas, mais `core-types`, `engine`, `session`, `offline-render` doivent passer, **y compris l'empreinte de rendu de référence sur ARM**). **Aucun `git push` ni `gh workflow run` sans mon accord explicite.**

## Pas encore vérifié / à garder en tête
- Tactile : le menu natif d'un choix d'armure au doigt ; le fantôme de glisser sur la tablette (taille, couleur) ; plusieurs portées au doigt.
- Triolets, altérations en cours de mesure : limites du module de Scorsaire.
- Le son : AnalogSynth n'interprète ni la modulation ni la pression (la molette seule) ; FM6Op tient la pédale ; Orgamon réagit à tout.
- Le calage avec un double stéréo, le mode « harmonie », une prise de 3 minutes ; la chaîne latérale entendue ; macOS.

## Autres suites possibles (si je les demande)
Plugins : pic de calcul de Neptune à chaque début de note (1,8 ms à 64 échantillons pour 8 instances au même échantillon), Mirador (rares appels de plusieurs ms) — à signaler dans leurs conversations. Calage : nettoyage des fiches dont la prise a disparu, zoom vertical du tracé. Moteur : mode isolé des plugins (étape « 3b », §18), sortie séparée du clic. Étape 7 : macOS (CoreAudio) ; étape 8 : PWA (basse priorité, §8).

## Règles de travail (voir CLAUDE.md §14)
- Dire honnêtement ce qui est testé et ce qui ne l'est pas ; chaque fonctionnalité avec ses tests ; ajouter une tranche numérotée (79, 80…) à la fin de CLAUDE.md à chaque lot.
- Aucun git push, aucune installation, aucune dépendance ajoutée sans mon accord. N'essayer que mes plugins (éditeur « Le Tournedos Rossini »).
- Linux : `. ~/.cargo/env` puis `taskset -c 0,1 cargo ... -j2` (2 cœurs au plus).
- Windows (alias SSH neptune-win, dossier C:\Users\lcj\daw-build, **jamais de « ; » dans une ligne ssh** : le shell est cmd) : envoyer les sources avec
  tar --exclude=target --exclude=node_modules -cf - Cargo.toml Cargo.lock CLAUDE.md crates docs tools tests | ssh neptune-win 'cd /d C:\Users\lcj\daw-build && tar -xf -'
  puis `ssh neptune-win 'cd /d C:\Users\lcj\daw-build && call go2.cmd'` (tests, daw-cli.exe, application : ~10 min ; l'EXIT= affiché est trompeur : vérifier all2.log, le nombre de « test result: ok » égal à celui de Linux, 0 « FAILED », CARGO_EXIT=0) ; pour la page seule : `call rebuild.cmd` (~5 min). **Avant de compiler l'application, vérifier avec `tasklist /fi "imagename eq daw-app.exe"` qu'elle n'est pas ouverte ; si elle l'est, me demander de la fermer, ne jamais tuer le processus.** Vérifier la date de crates\app\target\release\daw-app.exe avant de dire « recompilé ». Lancer ces commandes **en arrière-plan** (`run_in_background`) et attendre la notification : ne pas bloquer sur une attente longue.
- Aperçu de la page : serveur statique `interface-web` (.claude/launch.json, port 8791, `?debug&scale=100` ; en mode debug `window.__daw` expose le moteur, le piano roll, la timeline et `scoreWindow`) ; faux moteur crates/ui/web/js/mock-engine.js à tenir à jour avec chaque commande. Tests de la page : `cd crates/ui/web && node --test tests/*.test.mjs`. Le navigateur met les modules en cache : fetch(url, {cache:'reload'}) puis recharger. Dans l'aperçu, les clics de la souris utilisent le repère 800 × 600 de la capture.
- Les simulations de gestes tactiles dans le navigateur ne remplacent pas un essai sur mon écran : pour tout geste tactile, m'indiquer ce qu'il faut essayer.
- Quand je dois faire quelque chose, préciser si c'est sur la machine locale (Linux) ou sur la machine Windows, avec la commande complète.

Commence par lire CLAUDE.md (la fin : tranches 74 à 78), puis dis-moi en quelques lignes ce que tu as compris et ce que tu me proposes de faire en premier, avant de coder.
