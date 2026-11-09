#!/bin/sh
# Lance l'application sous Linux avec le son : le moteur joue par JACK, que PipeWire fournit via `pw-jack`.
# Usage : tools/lancer-linux.sh [debug|release]   (par défaut : debug)
cd "$(dirname "$0")/../crates/app" || exit 1
exec pw-jack "./target/${1:-debug}/daw-app"
