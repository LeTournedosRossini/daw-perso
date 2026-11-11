# Scripts (lots 1 et 2, 11 novembre 2026)

Un script est un fichier JavaScript ; il tourne dans un **Web Worker** (aucun accès à l'interface) et ne pilote le DAW que par les **commandes du protocole** (`docs/protocole-ui.md`). **Tout ce qu'un script fait forme une seule action annulable** (« Script : nom »). Une erreur, un arrêt ou un dépassement de délai défont tout.

## Écrire un script

Le fichier est le corps d'une fonction asynchrone. En-tête facultatif (lignes de commentaires au début) :

```js
// @name Pistes en majuscules
// @description Met en majuscules le nom de chaque piste.
// @shortcut Ctrl+Alt+U
const { project } = await daw.state();
for (const node of project.nodes) {
  if (node.id !== 0) await daw.send({ type: "rename_track", node: node.id, name: node.name.toUpperCase() });
}
```

| `daw.…` | effet |
|---|---|
| `await daw.state()` | copie en lecture seule : `project` (pistes, clips, notes…), `position`, `playing`, `loop`, `selection` (ids des clips), `history` |
| `await daw.send(commande)` | envoie une commande du protocole et attend qu'elle soit traitée ; rend le nouvel état ; **lève une erreur** si le moteur la refuse (un script peut l'attraper avec `try`) |
| `daw.log(...)` | écrit dans la console |
| `await daw.sleep(ms)` | attend (5 s au plus par appel) |

### Aides pour les notes et les clips (lot 2)

Toutes sont construites sur `daw.state()` / `daw.send()` (rien d'autre) et se défont avec le reste du script. Les positions et durées de notes sont des fractions de ronde `[n, d]` depuis le début de la matière du clip ; un **tick** vaut 1/3 840 de ronde (960 par noire).

| aide | effet |
|---|---|
| `await daw.selection({ kind? })` | ids des clips sélectionnés (`kind` : `"notes"` ou `"audio"`) |
| `await daw.clips.list({ kind?, node?, selected? })` | clips du projet, chacun avec sa piste (`node`) |
| `await daw.clips.move(ids, delta, nodeDelta?)` / `split(ids, at)` / `remove(ids)` / `duplicate(ids, delta?, nodeDelta?)` | commandes `move_clips`, `split_clips`, `delete_clips`, `duplicate_clips` |
| `await daw.notes.get(clip)` | `{ start, len, offset, notes: [{ index, start, len, key, channel, velocity, voice }] }` |
| `await daw.notes.transpose(clip, demiTons, { indices? })` | transpose ; une note qui sortirait de 0-127 reste en place ; rend le nombre de notes modifiées |
| `await daw.notes.scaleVelocity(clip, facteur, { indices? })` / `setVelocity(clip, v, { indices? })` | vélocité (0 < v ≤ 1) |
| `await daw.notes.humanize(clip, { timing, velocity, seed, indices? })` | décale chaque début d'au plus `timing` ticks et la vélocité d'au plus `velocity`, **reproductible** (même `seed`, même résultat) ; un début ne passe jamais avant 0 |
| `await daw.notes.shift(clip, ticks, { indices? })` | décale les débuts ; une note qui passerait avant 0 reste en place |
| `await daw.notes.quantize(clip, [1, 16])` | aimante les débuts sur la grille (positions absolues) |
| `await daw.notes.add(clip, [{ start, len, key, velocity }])` / `remove(clip, indices)` | ajoute / supprime des notes (indices de `notes.get`) |
| `daw.frac.ticks(t)` / `toTicks(f)` / `add(a, b)` / `value(f)` | calculs sur les fractions |

Une aide qui n'a rien à changer n'envoie aucune commande. Après une modification, les indices des notes peuvent changer (elles sont retriées) : relis avec `notes.get` avant la modification suivante. Exemples fournis : « Transposer +1 octave » (`Ctrl+Alt+T`) et « Humaniser la sélection ».

**Raccourcis** : `Ctrl+Alt+<lettre ou 1-9>` ou `Alt+Maj+<lettre ou 1-9>` seulement (les raccourcis du DAW restent intacts).

## Où sont les scripts

- Application : dossier `…/DAW/scripts/*.js` (`%LOCALAPPDATA%\DAW\scripts` sous Windows ; `~/.local/share/DAW/scripts` sous Linux). Deux exemples y sont déposés la première fois.
- Navigateur (aperçu) : stockage local, avec les mêmes exemples.
- Menu **Scripts ▾** (liste, raccourcis, « Console… », « Actualiser la liste ») ; la **console** (taper, `Ctrl+Entrée` pour exécuter, « Enregistrer » dans le dossier).

## Limites et sécurité

- **Interdits aux scripts** : `begin_group` / `end_group` / `abort_group` / `undo` / `redo` (gérés par l'exécuteur), le cycle de vie du projet (`new_project`, `open_project`, `save_project`, `convert_project`), tout ce qui ouvre le réseau (`start_monitor`, `set_monitor_role`, `renew_monitor_address`…) et `set_prefs`.
- **Bornes** : 30 s par script, 5 000 commandes, 10 s par commande, 500 lignes de journal. Le bouton « Script « … » » de la barre (ou **Arrêter** dans la console) l'interrompt et défait ce qu'il a fait.
- **Réseau** : `fetch`, `XMLHttpRequest`, `WebSocket`, les Workers imbriqués… sont retirés du Worker. C'est **au mieux** : ce n'est pas un bac à sable prouvé, et la page n'a pas de politique CSP. **N'exécute que des scripts que tu as lus.**
- **Fichiers (lot 4, pas fait)** : un script qui déclare `// @permissions files` est refusé pour l'instant.
- Pendant qu'un script tourne, une action faite à la main dans l'interface s'ajoute au même pas d'annulation. Les raccourcis d'annulation sont refusés pendant le script.

## Dans le code

`crates/ui/web/js/script-runner.js` (Worker, API, limites), `script-api.js` (aides notes et clips, recopiées dans le Worker), `script-store.js` (dossier ou stockage local), `ui/scripts-ui.js` (menu, console) ; côté Rust : `Session::merge_begin/merge_end/merge_abort`, commandes `begin_group` / `end_group` / `abort_group` de l'hôte, commandes Tauri `scripts_list` et `script_save` (`crates/app/src/main.rs`, nom de fichier simple `.js`, jamais de chemin).
