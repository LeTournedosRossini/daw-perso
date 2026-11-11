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
];
