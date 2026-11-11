// Scripts d'exemple : déposés dans le dossier des scripts au premier lancement (et proposés dans un navigateur sans dossier).
// Un script est le corps d'une fonction asynchrone ; il dispose de `daw` (voir docs/scripts.md).

export const EXAMPLE_SCRIPTS = [
  {
    file: "Pistes en majuscules.js",
    source: `// @name Pistes en majuscules
// @description Met en majuscules le nom de chaque piste et bus (un seul Ctrl+Z défait tout).
// @shortcut Ctrl+Alt+U
const { project } = await daw.state();
let n = 0;
for (const node of project.nodes) {
  if (node.id === 0) continue; // la sortie principale garde son nom
  const up = node.name.toUpperCase();
  if (up !== node.name) {
    await daw.send({ type: "rename_track", node: node.id, name: up });
    n++;
  }
}
daw.log(n + " nom(s) modifié(s)");
`,
  },
  {
    file: "Rapport du projet.js",
    source: `// @name Rapport du projet
// @description Affiche dans la console le nombre de pistes, de clips et la durée occupée (lecture seule).
const { project } = await daw.state();
let clips = 0;
let end = 0;
for (const node of project.nodes) {
  for (const c of node.clips) {
    clips++;
    end = Math.max(end, c.start + c.len);
  }
}
daw.log(project.nodes.length + " nœuds, " + clips + " clips");
daw.log("durée occupée : " + (end / project.sample_rate).toFixed(1) + " s");
`,
  },
  {
    file: "Transposer +1 octave.js",
    source: `// @name Transposer +1 octave
// @description Monte d'une octave les notes des clips de notes sélectionnés (celles qui sortiraient de 0-127 restent en place).
// @shortcut Ctrl+Alt+T
const clips = await daw.selection({ kind: "notes" });
if (!clips.length) throw new Error("Sélectionne d'abord un clip de notes");
let n = 0;
for (const clip of clips) n += await daw.notes.transpose(clip, 12);
daw.log(n + " note(s) transposée(s)");
`,
  },
  {
    file: "Humaniser la sélection.js",
    source: `// @name Humaniser la sélection
// @description Décale un peu le début et la vélocité des notes des clips sélectionnés (reproductible : même graine, même résultat).
const clips = await daw.selection({ kind: "notes" });
if (!clips.length) throw new Error("Sélectionne d'abord un clip de notes");
let n = 0;
for (const clip of clips) n += await daw.notes.humanize(clip, { timing: 12, velocity: 0.08, seed: clip });
daw.log(n + " note(s) humanisée(s)");
`,
  },
  {
    file: "Annonce la lecture.js",
    source: `// @name Annonce la lecture
// @description Reste à l'écoute : écrit dans la console quand la lecture démarre ou s'arrête, et quand la sélection change.
daw.on("play", (e) => daw.log("lecture depuis " + e.position + " échantillons"));
daw.on("stop", (e) => daw.log("arrêt à " + e.position + " échantillons"));
daw.on("selection", (e) => daw.log(e.clips.length + " clip(s) sélectionné(s)"));
`,
  },
];
