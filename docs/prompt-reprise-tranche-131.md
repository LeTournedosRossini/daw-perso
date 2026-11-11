# Prompt de reprise — projet DAW (11 novembre 2026, fin de la tranche 130)

Colle ce texte au début d'une nouvelle conversation Claude Code ouverte dans `~/Bureau/DAW perso`.

---

Tu reprends le développement de mon DAW écrit de zéro (Rust pour le moteur, interface web dans Tauri ; cible principale Windows avec une Behringer UMC 1820 en ASIO, puis Linux ; je travaille en français). **Lis d'abord `CLAUDE.md` en entier** : c'est le document de référence (règles non négociables du fil audio, décisions datées, journal détaillé de chaque tranche jusqu'à la 130, ce qui est vérifié et ce qui ne l'est pas). Puis `docs/protocole-ui.md` (protocole entre la page et l'hôte) et `docs/hote-sans-interface.md`.

## Règles de travail (à respecter)
- Réponds en français ; dis honnêtement ce qui est testé et ce qui ne l'est pas ; pas de `git push` ni de `gh workflow run` sans mon accord explicite ; aucune installation de logiciel sans mon accord ; ne touche pas aux plugins tiers (j'utilise seulement les miens, éditeur « Le Tournedos Rossini »).
- Chaque fonctionnalité arrive avec ses tests ; tu m'exposes les compromis avant les choix d'architecture ; tu peux (et tu dois) me poser des questions quand un choix est le mien ; une question d'opinion de ma part n'est pas une demande de coder.
- Linux : 2 cœurs maximum (`taskset -c 0,1 cargo test --workspace --no-fail-fast -j2`, long : le lancer en arrière-plan) ; tests de la page : `cd crates/ui/web && node --test tests/*.test.mjs` ; aperçu de la page : `preview_start` « interface-web » (port 8791), avec `?debug` pour `window.__daw.engine` (vider le cache des modules avec `fetch(url, {cache:'reload'})` puis recharger).
- **Windows (machine `neptune-win`, SSH, shell `cmd` : jamais de `;` dans une ligne `ssh`)** : envoyer les sources par `tar --exclude=target --exclude=node_modules --exclude=.git -cf - crates Cargo.toml Cargo.lock | ssh neptune-win "cd /d C:\Users\lcj\daw-build && tar -xf -"` ; vérifier la compilation seule avec `check_app.cmd` ; compiler l'application avec `ssh neptune-win 'cd /d C:\Users\lcj\daw-build && call rebuild.cmd'` (≈ 5 min ; **l'application doit être fermée sur Windows** : vérifier avec `tasklist /fi "imagename eq daw-app.exe"` avant, sinon l'édition de liens échoue après 5 minutes ; je lance parfois la commande moi-même dans mon terminal) ; puis vérifier la date de `crates\app\target\release\daw-app.exe` et « Finished » dans `build_app.log`. Le journal de l'application Windows est `%TEMP%\daw-app.log` (lisible par `ssh neptune-win 'type %TEMP%\daw-app.log'`). Une session SSH n'a pas de bureau : ce qui s'affiche, c'est moi qui l'essaie.

## Où on en est (tranches 120 à 130, 11 novembre)
Marqueurs d'étirement (lissage, aimantation générale, détection d'attaques), hôte sans interface `daw-headless`, sorties multiples des plugins (Neptune : 31 sorties) qui suivent le muet et le solo de leur piste (le fader de la piste ne les atténue pas : choix de Jean), écoute de contrôle AFL des bus et retours (casque à la place du S), cache disque du balayage des plugins, **fenêtre de plugin unique** (une fenêtre Tauri par plugin : `plugin.html`, `js/plugin-window.js`, `js/ui/plugin-view.js` ; liste des plugins de la piste à gauche avec ↑ ↓ ✕ ; bande avec bypass vert/orange, nom, sorties, bouton « Interface » à deux états ; interface native logée dessous par `crates/app/src/win32_window.rs`, `DockedSystem`, dont la zone est mesurée par la page via `set_plugin_area`), vue des sorties multiples sur toute la hauteur, et **pistes : moins de menu** (menu ⋮ court ; bouton fx toujours visible, qui ouvre le sélecteur sur une piste sans plugin ; appui long ou clic droit sur ● ou chevron ▾ pour les réglages d'entrée ; routage « → Bus 1 » cliquable).
Tout est committé en local (dernier commit des fonctionnalités : `11f4133`). **L'application Windows est recompilée avec la tranche 130** (exécutable du 11/11 à 18:12) : **je ne l'ai pas encore essayée.**

## À faire en premier
1. Me demander ce que j'ai constaté en essayant la tranche 130 sous Windows : bouton fx sur une piste vide, appui long sur ● (et le chevron ▾ : visible, bien placé dans les en-têtes étroits ?), clic sur le routage « → », menu ⋮ raccourci. Corriger ce qui ne va pas.
2. S'il reste un recouvrement de l'interface d'un plugin sur la bande ou la liste de la fenêtre de plugin, lire les deux lignes « fenêtre de plugin (1) / (2) » du journal Windows : elles donnent tous les rectangles réels.

## Idées en attente (à me proposer, pas à faire sans mon accord)
- Changer le plugin d'un emplacement depuis la bande (le nom n'est cliquable que pour un emplacement vide) ; mémoire de la taille des fenêtres de plugin (seule la position l'est) ; fenêtres de plugin sous Linux (X11) et macOS.
- Un bouton « un bus par sortie » pour les plugins multi-sorties (Neptune), sortie MIDI des plugins, écoute AFL sur la tablette.
- Fabrication d'ensembles d'échantillons (format de chargement de Neptune + SFZ), puis scripts JS dans la page.
- Surface ESP32 de la télécommande (après la tablette) ; chiffrement de la télécommande ; autres éléments « pas fait » listés tranche par tranche dans `CLAUDE.md`.
- Validation matérielle toujours non faite : écoute réelle (l'assistant n'a jamais rien écouté), macOS.
