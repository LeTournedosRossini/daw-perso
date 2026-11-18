#!/bin/sh
# Lance l'application sous Linux. Depuis la tranche 151 elle marche aussi sans `pw-jack` (ALSA) ; ce script force JACK via PipeWire.
# Usage : tools/lancer-linux.sh [debug|release]   (par défaut : debug)
cd "$(dirname "$0")/../crates/app" || exit 1
exec pw-jack "./target/${1:-debug}/daw-app"
