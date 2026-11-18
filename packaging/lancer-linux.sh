#!/bin/sh
# Lance le DAW. Le son passe par JACK s'il est joignable (`pw-jack` sous PipeWire), sinon par ALSA (le périphérique « default » marche
# sous PipeWire sans `pw-jack`). Choix du système et du périphérique : Réglages.
cd "$(dirname "$0")" || exit 1
if command -v pw-jack >/dev/null 2>&1; then exec pw-jack ./daw-app "$@"; fi
exec ./daw-app "$@"
