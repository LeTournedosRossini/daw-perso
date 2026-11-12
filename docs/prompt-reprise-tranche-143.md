# Prompt de reprise (12 novembre 2026)

Je reprends le projet DAW (Rust + interface web dans Tauri, Windows/UMC 1820 en priorité). Lis d'abord `CLAUDE.md` (document de référence, journal par tranches jusqu'à la 142, puis la note « CI du 12 novembre »). Je parle français.

État : tout est poussé sur GitHub (`main`), la CI manuelle (`gh workflow run ci.yml --ref main`) est verte sur Linux, Windows, macOS ARM et les tests Node. Dernier exécutable Windows validé : bandeau compact (tranche 142). Règles : aucun `git push` ni `gh workflow run` sans mon accord explicite ; les essais pour moi sur Windows sont livrés comme scripts dans `C:\Users\lcj\AppData\Local\DAW\scripts` ; après toute modification de `crates/app/src/main.rs`, lancer `check_app.cmd` sous Windows (le code ASIO n'est pas compilé sous Linux) ; ne jamais utiliser `;` dans une ligne `ssh` vers Windows.

Idées en attente (rien n'est commencé, demande-moi lequel) :
- palette de commandes (Ctrl+K) ;
- bouton « Restaurer les exemples » dans le menu Scripts ;
- bouton d'écoute (AFL) sur la tablette ;
- compiler `crates/app` en CI ;
- vérifier à l'écran / au doigt ce qui n'a été vu qu'avec le faux moteur (liste des « non vérifié » dans CLAUDE.md).
