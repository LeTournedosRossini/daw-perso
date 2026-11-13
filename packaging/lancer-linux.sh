#!/bin/sh
# Lance le DAW avec le son via JACK / PipeWire.
cd "$(dirname "$0")" || exit 1
if command -v pw-jack >/dev/null 2>&1; then exec pw-jack ./daw-app "$@"; fi
exec ./daw-app "$@"
