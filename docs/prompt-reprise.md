Bonjour Claude. Je reprends le projet DAW perso (dossier `/home/jean/Bureau/DAW perso`) avec toi dans une nouvelle conversation. Nous travaillons en français, depuis plusieurs heures : le projet est bien avancé. **Avant toute chose, lis `CLAUDE.md` en entier** (sections 12 à 35 et les « tranches » à la fin : c'est le journal de toutes les décisions, mesures et limites), puis `docs/protocole-ui.md` (protocole entre l'interface web et le moteur). Ne refais pas ce qui y est déjà décidé.

## Où nous en sommes (5 novembre 2026, fin d'après-midi)

- **Moteur Rust** (workspace `crates/`) : audio ASIO/JACK, graphe de pistes et bus, CLAP (plugins « Le Tournedos Rossini » de Jean seulement), PDC, MIDI (notes, contrôleurs, molette, pressions, entrée matérielle), enregistrement, édition de clips non destructive avec annulation, automation, temps musical exact (fractions de ronde + carte des tempos), fichiers MIDI standard, projet JSON versionné (version 6 avec migrations), formes de fondu continues. 268 tests Rust passent sur Linux **et** Windows.
- **Interface web** (`crates/ui/web/`, JavaScript natif, aucune dépendance, tests `cd crates/ui/web && node --test tests/*.test.mjs` : 31 tests) dans une **fenêtre Tauri** (`crates/app`, espace de travail à part). Jean l'utilise sous Windows avec la Behringer UMC 1820 : le son sort, tout ce qu'il a essayé fonctionne (transport, faders, vumètres, édition de clips à la souris, annuler/rétablir, clips entre pistes, clips de notes rognables/découpables, fondus enchaînés automatiques, éditeur de courbes de fondu, taille de l'interface réglable, AnalogSynth sur la piste Claviers).
- Machines : Linux (ici, 2 cœurs maximum pour compiler) et Windows par SSH (alias `neptune-win`, utilisateur `lcj`, dossier de build `C:\Users\lcj\daw-build`). Une autre conversation (Neptune) utilise aussi ces machines : **toute compilation ou test passe par `~/.local/bin/claude-build linux|windows …`** (file d'attente).

## Ce que Jean vient de demander (à traiter dans cet ordre)

1. **Taille de l'interface : défaut à 70 %** (au lieu de 100 %) **et choix automatique selon l'écran** (taille de la fenêtre / de l'écran), avec réglage manuel qui prime et reste mémorisé. Voir `crates/ui/web/js/ui-scale.js` (zoom CSS sur la racine, `?scale=80` possible).
2. **Éditeur de fondu responsive** : si la fenêtre dépasse l'écran, mettre **le graphique à gauche et les réglages à droite** (mise en page en deux colonnes), au lieu d'une seule colonne verticale. Voir `js/ui/fade-editor.js` et la fin de `css/app.css`.
3. **Longueur du fondu enchaîné réglable dans l'éditeur** (aujourd'hui elle est imposée par la zone commune des deux clips : déplacer un clip la change). À concevoir : que veut dire « changer la longueur » (déplacer le début du second clip en gardant sa fin ? rogner ?) — propose les options avec leurs compromis avant de coder.
4. **Aperçu sonore des fondus** dans l'éditeur (écouter le fondu enchaîné). À concevoir : rendu hors ligne de la zone concernée (le rendu déterministe existe : crate `offline-render`) puis lecture par le moteur ; ne rien allouer dans le fil audio.

Ensuite, selon la priorité de Jean : **ouverture et sauvegarde de projet** (sans elles, rien de ce qu'on édite n'est conservé ; le format JSON v6 et `Session::to_json/from_json` existent, il manque l'interface et l'écriture atomique), **piano roll** (édition des notes à la souris ; le modèle d'édition existe dans `session`), automation et enregistrement dans l'interface (armement, entrée audio et MIDI), Linux/WebKitGTK (**demander l'accord de Jean avant d'installer des paquets**).

## Règles de travail (à respecter)

- Réponds en français ; dis honnêtement ce qui est testé et ce qui ne l'est pas (la plupart des choses audibles sont vérifiées hors ligne, pas à l'oreille).
- Chaque fonctionnalité arrive avec ses tests ; mets à jour `CLAUDE.md` (nouvelle sous-section datée) et `docs/protocole-ui.md` à chaque tranche.
- Règles du thread audio non négociables (§4 de `CLAUDE.md`) : aucune allocation ni verrou dans le callback ; le garde-fou d'allocation le vérifie.
- Aucun `git push`, aucune installation de logiciel ou de plugin sans l'accord de Jean. N'essaie que ses plugins. **Ne tue jamais de processus** sur la machine Windows (une tentative a été refusée : `msedgewebview2.exe` peut appartenir à d'autres applications).
- Aperçu de l'interface : serveur statique du projet (`.claude/launch.json`, **port 8791**, via `preview_start`) puis navigateur intégré ; vérifie visuellement chaque changement de la page. Faux moteur dans un navigateur ordinaire, vrai moteur dans la fenêtre Tauri.
- Compiler l'application Windows : envoyer **`crates/app` ET `crates/ui` (et `session`, `engine`… si modifiés)** par `tar | ssh`, **fermer l'application d'abord** (sinon « failed to remove file daw-app.exe »), faire `touch crates/app/src/main.rs`, puis `claude-build windows ssh neptune-win 'C:\Users\lcj\daw-build\build_app.cmd'` (≈ 3 min 30). Tests Windows : `all_win.cmd`. Jean lance l'application avec `Lancer-DAW.cmd` (Bureau Windows) et te dit ce qu'il voit et entend ; les mesures audio réelles se font dans sa fenêtre, pas par SSH.
- Quand une décision d'architecture a plusieurs options, expose-les avec leurs compromis avant de choisir.

Commence par lire `CLAUDE.md`, puis résume-moi en quelques lignes où nous en sommes et ta proposition pour le point 1.
