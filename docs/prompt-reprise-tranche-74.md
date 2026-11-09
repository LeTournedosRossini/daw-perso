Reprise du projet DAW (Rust + interface web dans Tauri), dossier /home/jean/Bureau/DAW perso. Lis d'abord CLAUDE.md : c'est le document de référence (règles de travail §14, journal des tranches §15 à la fin, dernière tranche = 73) ; le protocole page ↔ hôte est dans docs/protocole-ui.md. Je m'appelle Jean, je travaille en français.

## Où on en est (9 novembre 2026)
Tout est fait, vérifié et compilé pour Windows (daw-app.exe du 09/11 à 00:03). Tests : Linux et Windows **107 résultats « ok », aucun échec** (`cargo test --workspace --no-fail-fast`), **233 tests Node** pour la page. Format de projet : version 20 (session) ; les champs ajoutés depuis sont facultatifs (catégorie des sections, morceaux multiples des fiches de calage).

Dernières tranches (71 à 73), essayées par moi avec succès sur Windows :
- 71 : calage de plusieurs clips d'une piste lus comme une seule bande (double et guide), trous signalés ; un plugin ajouté ouvre son interface.
- 72 : catégorie des sections de la forme (les 11 identifiants de Scorsaire, leurs couleurs) ; adaptateur `js/score-adapter.js`.
- 73 : **fenêtre « Partition »** (`js/ui/score-window.js`, module de Scorsaire copié sans modification dans `js/vendor/scorsaire-score-view/`, commit 5684d43) : voir et éditer les notes d'un clip (touche N ou bouton « ♪ Partition » du piano roll), portée / armure / grille / taille de portée, tête de lecture, sections. Une seule portée par clip, une voix.

## Pas encore vérifié
- Le **doigt sur tablette** dans la partition (glisser vertical d'une note, deux doigts = défiler et zoomer) ; le cas d'un deuxième doigt qui se pose pendant un geste (le module peut garder un geste en cours : voir `docs/` / message à Scorsaire du 9 novembre 02:00).
- Les limites annoncées par Scorsaire : triolets, altérations en cours de mesure, changement d'armure ou de chiffrage dans un clip.
- Le calage avec un double stéréo, le mode « harmonie », une prise de 3 minutes ; la chaîne latérale entendue ; macOS.

## Ce qu'il reste à faire (propositions — dis-moi lesquelles on prend)
1. **Partition, suite** : (a) plusieurs clips empilés (une portée par clip, un système par piste) — le module les gère, la fenêtre n'en montre qu'un ; (b) fenêtre **ancrée sous la piste** comme le piano roll ; (c) **voix d'affichage 2 à 4** : champ `voice` dans la note (affichage seulement, ignoré par le moteur ; format de projet + migration sans transformation, `edit_notes { voice }`), à demander d'abord à Scorsaire (commande `voice` dans `onEdit`) ; (d) armure mémorisée par clip ; (e) **rendu « en blocs » par sections** (intro, couplet 1 en dessous, refrain…) sur plusieurs clips ; (f) plus tard : tablature, batterie, accords générés (réécriture de `comping.js` en Rust avec fichiers de cas de test), swing.
2. **CI GitHub Actions** : le quota devait revenir la semaine du 9 novembre 2026 (§12). Écrire le workflow (Linux, Windows, macOS ; macOS jamais compilé, y compris l'empreinte de rendu de référence sur ARM). **Aucun `git push` ni `gh workflow run` sans mon accord.**
3. **Plugins de Jean** : Neptune a un pic de calcul à chaque début de note (8 instances au même échantillon : 1,8 ms à 64 échantillons) ; Mirador a de rares appels de plusieurs ms. À signaler dans leurs conversations si ça gêne.
4. **MIDI** : courbe de vélocité par appareil ; canal d'une note créée au piano roll ; enregistrer la vélocité après courbe.
5. **Calage** : nettoyage des fiches dont la prise a disparu ; zoom vertical du tracé ; gains et fondus des clips d'une bande.
6. **Moteur** : mode isolé des plugins (étape « 3b », §18), sortie séparée du clic du métronome.
7. **Étape 7 du plan** : macOS (CoreAudio) et finition multiplateforme ; étape 8 : PWA (basse priorité, §8).

## Règles de travail (voir CLAUDE.md §14)
- Dire honnêtement ce qui est testé et ce qui ne l'est pas ; chaque fonctionnalité avec ses tests ; ajouter une tranche numérotée (74, 75…) à la fin de CLAUDE.md à chaque lot.
- Aucun git push, aucune installation, aucune dépendance ajoutée sans mon accord. N'essayer que mes plugins (éditeur « Le Tournedos Rossini »).
- **Le dépôt de Scorsaire (`~/Bureau/Scorsair/…`) est en lecture seule** : ne rien y modifier ; les demandes passent par `FlibusteSuite/echanges/messages/` et le module livré est copié tel quel (ne pas le modifier dans `js/vendor/`).
- Linux : `. ~/.cargo/env` puis `taskset -c 0,1 cargo ... -j2` (2 cœurs au plus).
- Windows (alias SSH neptune-win, dossier C:\Users\lcj\daw-build, **jamais de « ; » dans une ligne ssh** : le shell est cmd) : envoyer les sources avec
  tar --exclude=target --exclude=node_modules -cf - Cargo.toml Cargo.lock CLAUDE.md crates docs tools tests | ssh neptune-win 'cd /d C:\Users\lcj\daw-build && tar -xf -'
  puis `ssh neptune-win 'cd /d C:\Users\lcj\daw-build && call go2.cmd'` (tests, daw-cli.exe, application : ~10 min ; l'EXIT= affiché est trompeur : vérifier all2.log, le nombre de « test result: ok » égal à celui de Linux, 0 « FAILED », CARGO_EXIT=0) ; pour la page seule : `call rebuild.cmd` (~5 min). **Avant de compiler l'application, vérifier avec `tasklist /fi "imagename eq daw-app.exe"` qu'elle n'est pas ouverte ; si elle l'est, me demander de la fermer, ne jamais tuer le processus.** Vérifier la date de crates\app\target\release\daw-app.exe avant de dire « recompilé ».
- Aperçu de la page : serveur statique `interface-web` (.claude/launch.json, port 8791, `?debug&scale=100` ; en mode debug `window.__daw` expose le moteur, le piano roll, la timeline et `scoreWindow`) ; faux moteur crates/ui/web/js/mock-engine.js à tenir à jour avec chaque commande. Tests de la page : `cd crates/ui/web && node --test tests/*.test.mjs`. Le navigateur met les modules en cache : fetch(url, {cache:'reload'}) puis recharger.
- Les simulations de gestes tactiles dans le navigateur ne remplacent pas un essai sur mon écran : pour tout geste tactile, m'indiquer ce qu'il faut essayer.
- Quand je dois faire quelque chose, préciser si c'est sur la machine locale (Linux) ou sur la machine Windows, avec la commande complète.

Commence par lire CLAUDE.md (la fin : tranches 71 à 73), puis dis-moi en quelques lignes ce que tu as compris et ce que tu me proposes de faire en premier, avant de coder.
