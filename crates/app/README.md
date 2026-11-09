# daw-app — application de bureau (Tauri)

Fenêtre Tauri qui affiche l'interface web de `crates/ui/web` et la relie au vrai moteur par le protocole JSON de
`docs/protocole-ui.md` (hôte : crate `daw-ui`).

**Espace de travail à part** (voir `exclude` dans le `Cargo.toml` racine) : Tauri tire beaucoup de dépendances et,
sous Linux, des bibliothèques système (WebKitGTK) qui ne sont pas installées ; `cargo test --workspace` ne doit pas en
dépendre. À compiler depuis ce dossier :

- **Windows** : `vcvars64.bat`, `set ASIO_SDK_DIR=…`, puis `cargo build --release` (WebView2 est fourni par Windows 11).
  Variables facultatives au lancement : `DAW_ASIO_DRIVER` (défaut `UMC`), `DAW_BUFFER` (défaut 64). Journal : `%TEMP%\daw-app.log`.
- **Linux / macOS** : non branché pour l'instant (la page s'ouvre mais le moteur tourne sans son).
