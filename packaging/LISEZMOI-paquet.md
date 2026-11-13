# DAW — version de test

Station de travail audio écrite de zéro (moteur Rust, interface web dans Tauri). **Version de test, non signée, sans garantie** : elle a été
écrite et vérifiée surtout sous Windows (carte UMC 1820, ASIO). Les autres systèmes sont moins éprouvés.

## Lancer

- **Windows** : décompresser le dossier, lancer `daw-app.exe`. Nécessite WebView2 (déjà présent sous Windows 11). Pilote audio : ASIO si un pilote
  « UMC » est installé (réglable dans Réglages), sinon l'application démarre sans son.
- **macOS (Apple Silicon)** : glisser `DAW.app` dans Applications. L'application n'est ni signée par un compte Apple ni notarisée : au premier
  lancement, **clic droit sur l'icône puis « Ouvrir »**, ou dans un terminal : `xattr -cr /Applications/DAW.app`. Le son passe par CoreAudio
  (le micro demande une autorisation). **Jamais essayée sur un vrai Mac par l'auteur** : les retours sont précieux.
- **Linux** : `./lancer.sh`. Nécessite WebKitGTK 4.1 (`libwebkit2gtk-4.1-0`) ; le son passe par JACK (`pw-jack` sous PipeWire).

## Plugins fournis (dossier `plugins`)

21 plugins CLAP de l'auteur : Neptune (instrument), AnalogSynth, FM6Op, Orgamon, Compresseur, Egaliseur, Delai, Choeur, Microdecale, Convolution,
LaFleche, Fournaise, Trident, Digue, Moussaillon, Diapason, Mirador, Carte, Arpege, Sillage, LowLatConv. **Sous macOS, Neptune et Sillage sont arm64
seulement** (pas de Mac Intel). Neptune et Orgamon se lancent muets tant qu'on ne leur donne pas d'échantillons.

## Licences

- Le DAW : **GNU GPL v3** (fichier `LICENSE`, composants tiers dans `THIRD-PARTY-LICENSES.md`). Sources : https://github.com/LeTournedosRossini/daw-perso
- Plugins : Neptune et ses 19 plugins et Sillage en **AGPL v3** ; LowLatConv en GPL v3 ou ultérieure. Ce sont des programmes séparés chargés par
  l'interface CLAP. Sources : dépôts `flibuste-neptune`, `flibuste-align` et `LowLatConv` du même compte GitHub.
- Windows : le backend ASIO utilise le SDK ASIO de Steinberg sous sa double licence (GPLv3 choisie).
