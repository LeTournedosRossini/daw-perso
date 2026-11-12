# Licences

Ce projet est distribué sous la **GNU General Public License v3.0 ou ultérieure** (voir `LICENSE`).

Il embarque ou utilise des composants tiers, tous compatibles avec la GPLv3 :

| Composant | Où | Licence |
|---|---|---|
| Rubber Band (étirement / calage) | `crates/align/native/third_party/rubberband` | GPL v2 ou ultérieure (`COPYING`) |
| Signalsmith Stretch / Linear | `crates/align/native/third_party/signalsmith-*` | MIT (`LICENSE.txt`) |
| Bibliothèque de calage `flibuste-align` | `crates/align/native` | GPLv3 (`LICENSE-GPLv3`) |
| Module de partition de Scorsaire | `crates/ui/web/js/vendor/scorsaire-score-view` | GPLv3 (`licences/Scorsaire-GPLv3.txt`) |
| Police musicale Bravura (contours) | idem | SIL Open Font License (`licences/Bravura-OFL.txt`) |
| Crates Rust (rtrb, serde, libloading, clap-sys, claxon, flacenc, tiny_http, qrcode, tauri…) | `Cargo.lock` | MIT, Apache-2.0, ISC, Zlib, Unlicense (sans condition gênante) |
| `symphonia` (lecture MP3) | `Cargo.lock` | MPL-2.0 (compatible GPLv3) |

**Non inclus dans le dépôt** : le SDK ASIO de Steinberg (double licence Steinberg ASIO ou GPLv3, au choix de qui compile ; à télécharger séparément, voir `crates/audio-io/build.rs`) et les plugins CLAP de l'auteur.
