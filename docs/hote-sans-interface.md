# Hôte sans interface (`daw-headless`)

Le moteur et l'hôte du DAW sans fenêtre ni carte son, pilotés par le **même protocole JSON que la page** (`docs/protocole-ui.md`), une ligne par commande sur l'entrée standard, une ligne par événement sur la sortie standard. Sert aux scripts, aux bancs d'essai de plugins et aux tests de bout en bout.

```
cargo build -p daw-headless
target/debug/daw-headless [--rate 48000] [--block 64] [--demo] [--all]
```

- Un projet vide au départ (`--demo` : le projet de démonstration). Le moteur ne tourne **que** quand on l'avance : l'audio est calculé hors temps réel, donc de façon déterministe.
- Chaque ligne d'entrée peut porter un `"id"`, renvoyé dans l'accusé `{"type":"ack","id":…,"error":null|"message"}` qui termine la réponse (`error` : le premier événement d'erreur).
- Commandes propres à l'outil : `{"type":"advance","frames":N}` (fait avancer le moteur de N échantillons ; renvoie ensuite `position` et `meters`), `{"type":"wait_for","event":"import","timeout_ms":60000}` (attend la fin d'une tâche de fond : import, export, étirement, calage, conversion, balayage `plugins`), `{"type":"quit"}`.
- Les événements `position` et `meters` ne sortent que par `advance` (tous avec `--all`).

**Client Python** : `tools/daw_headless.py` (`Daw().send(commande)`, `advance`, `wait_for`, `last`). **Banc d'essai de plugin** : `tools/plugin_bench.py NomDuPlugin [--params]` charge le plugin, joue une note, mesure le niveau et vérifie que la queue s'éteint.

Limites : pas d'entrée audio ni MIDI matérielle, pas de fenêtres de plugins, un seul client, pas de temps réel (le moteur n'avance pas tout seul).
