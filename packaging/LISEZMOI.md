# Livrer des plugins avec l'application

Principe : **l'application cherche ses plugins CLAP dans un dossier `plugins` qui lui est propre**, en plus des dossiers du système
(`default_search_dirs` dans `crates/clap-host/src/lib.rs`). Le paquet de livraison n'a donc qu'à y déposer les plugins.

| Système | Où l'application cherche ses plugins livrés |
|---|---|
| macOS | `DAW.app/Contents/Resources/plugins/` (et `Contents/MacOS/plugins/`) |
| Windows, Linux | `plugins/` à côté de l'exécutable |

Sous macOS un `.clap` est un **dossier** (`Neptune.clap/Contents/MacOS/Neptune`) : l'hôte charge l'exécutable qu'il contient
(`CFBundleExecutable` de `Info.plist`) et passe le chemin du bundle à `init`, comme l'exige la spécification.

## D'où viennent les plugins

`plugins.json` liste des dépôts GitHub dont la **dernière Release** (ou un tag précis) contient une archive par système. Les dépôts des plugins
(`flibuste-neptune`, `LowLatConv`, `flibuste-align`) construisent déjà ces archives (`Neptune-macOS.tar.gz`, `…-Linux.tar.gz`, `…-Windows.zip`) ;
une Release est créée quand on pousse un tag `v*` dans le dépôt du plugin (`git tag v0.0.2 && git push --tags`). **Aucune Release n'existe encore** :
tant qu'il n'y en a pas, rien n'est téléchargeable.

```bash
python3 packaging/fetch_plugins.py --os macos --dest DOSSIER_PLUGINS              # depuis les Releases (outil gh)
python3 packaging/fetch_plugins.py --os macos --dest DOSSIER_PLUGINS --local DOSSIER_D_ARCHIVES   # archives déjà sur le disque
```

Ajouter un plugin = l'ajouter à une Release d'un dépôt déjà listé, ou ajouter un dépôt à `plugins.json`. Les archives sont reconnues par leur nom
(`mac`, `linux`, `win`, sans tenir compte de la casse) ; tout `.clap` qu'elles contiennent est copié.

## À savoir pour macOS

- Les plugins doivent avoir **la même architecture que l'application** (arm64 sur les exécuteurs `macos-latest`). `LowLatConv` est universel,
  `Neptune` (archives locales) est arm64 seulement.
- L'application sera livrée signée « ad hoc » (gratuit, sans compte Apple) en **une seule passe sur tout le paquet, plugins compris**, après la
  copie des plugins : une signature faite avant serait invalidée par l'ajout de fichiers.
- Non notarisé : au premier lancement, clic droit puis « Ouvrir » (ou `xattr -cr DAW.app`).
