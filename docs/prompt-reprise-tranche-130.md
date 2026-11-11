# Prompt de reprise — projet DAW (11 novembre 2026, fin de la tranche 129)

Colle ce texte au début d'une nouvelle conversation Claude Code ouverte dans `~/Bureau/DAW perso`.

---

Tu reprends le développement de mon DAW écrit de zéro (Rust pour le moteur, interface web dans Tauri ; cible principale Windows avec une Behringer UMC 1820 en ASIO, puis Linux ; je travaille en français). **Lis d'abord `CLAUDE.md` en entier** : c'est le document de référence (règles non négociables du fil audio, décisions datées, journal détaillé de chaque tranche jusqu'à la 129, ce qui est vérifié et ce qui ne l'est pas). Puis `docs/protocole-ui.md` (protocole entre la page et l'hôte) et `docs/hote-sans-interface.md`.

## Règles de travail (à respecter)
- Réponds en français ; dis honnêtement ce qui est testé et ce qui ne l'est pas ; pas de `git push` ni de `gh workflow run` sans mon accord explicite ; aucune installation de logiciel sans mon accord ; ne touche pas aux plugins tiers (j'utilise seulement les miens, éditeur « Le Tournedos Rossini »).
- Chaque fonctionnalité arrive avec ses tests ; je décris les compromis avant les choix d'architecture ; tu peux me poser des questions.
- Linux : 2 cœurs maximum (`taskset -c 0,1 cargo test --workspace --no-fail-fast -j2`, long : le lancer en arrière-plan) ; tests de la page : `cd crates/ui/web && node --test tests/*.test.mjs` ; aperçu de la page : `preview_start` « interface-web » (port 8791), avec `?debug` pour `window.__daw.engine`.
- **Windows (machine `neptune-win`, SSH, shell `cmd` : jamais de `;` dans une ligne `ssh`)** : envoyer les sources par `tar --exclude=target --exclude=node_modules --exclude=.git -cf - crates Cargo.toml Cargo.lock | ssh neptune-win "cd /d C:\Users\lcj\daw-build && tar -xf -"`, vérifier compilation seule avec `check_app.cmd`, compiler l'application avec `ssh neptune-win 'cd /d C:\Users\lcj\daw-build && call rebuild.cmd'` (≈ 5 min ; **l'application doit être fermée sur Windows**, sinon l'édition de liens échoue) puis vérifier la date de `crates\app\target\release\daw-app.exe` et « Finished » dans `build_app.log`. Le journal de l'application Windows est `%TEMP%\daw-app.log` (lisible par `ssh neptune-win 'type %TEMP%\daw-app.log'`). Une session SSH n'a pas de bureau : ce qui s'affiche, c'est moi qui l'essaie.

## Où on en est (tranches 120 à 129, 11 novembre)
Marqueurs d'étirement (lissage, aimantation générale, détection d'attaques), hôte sans interface `daw-headless`, sorties multiples des plugins (Neptune : 31 sorties) avec muet/solo suivis, **écoute de contrôle AFL des bus et retours** (casque à la place du S), cache disque du balayage des plugins, et surtout **la fenêtre de plugin unique** : une fenêtre Tauri par plugin (`crates/ui/web/plugin.html`, `js/plugin-window.js`, `js/ui/plugin-view.js`) avec la liste des plugins de la piste à gauche (↑ ↓ ✕, changement de plugin dans la même fenêtre), une bande (bypass vert/orange, nom, sorties, bouton « Interface » à deux états) et la zone de l'interface native logée dessous (`crates/app/src/win32_window.rs`, `DockedSystem` : fenêtre Win32 sans cadre propriétaire de la fenêtre Tauri, replacée par la pompe ; la page mesure l'origine de sa zone et la rapporte par `set_plugin_area`). Le panneau « Plugins » de la page principale ne sert plus, dans l'application, que pour une piste sans plugin.

## À faire en premier
1. **Recompiler l'application Windows** : la correction de la vue des sorties multiples (liste libérée de son plafond de 168 px, plusieurs colonnes) est committée (`6f4ede8`) et les sources sont déjà sur Windows, mais **l'exécutable n'a pas été recompilé** (l'application était ouverte). Me demander de la fermer, puis `rebuild.cmd`.
2. Me demander ce que j'ai constaté avec la fenêtre de plugin sous Windows (alignement de la zone native sous la bande et à droite de la liste ; Neptune ne doit plus déborder) et, s'il reste un écart, lire les deux lignes « fenêtre de plugin (1) / (2) » du journal Windows : elles donnent tous les rectangles réels.

## Idées en attente (à me proposer, pas à faire sans mon accord)
- Changer le plugin d'un emplacement depuis la bande (le nom n'est cliquable que pour un emplacement vide) ; mémoire de la taille des fenêtres de plugin (seule la position l'est) ; fenêtres de plugin sous Linux (X11) et macOS.
- Un bouton « un bus par sortie » pour les plugins multi-sorties (Neptune), sortie MIDI des plugins, écoute AFL sur la tablette.
- Fabrication d'ensembles d'échantillons (format de chargement de Neptune + SFZ), puis scripts JS dans la page.
- Surface ESP32 de la télécommande (après la tablette) ; chiffrement de la télécommande ; autres éléments « pas fait » listés tranche par tranche dans `CLAUDE.md`.
- Validation matérielle toujours non faite : écoute réelle (aucune écoute n'a été faite par l'assistant), macOS.
