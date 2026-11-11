// @name Pistes en majuscules
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
