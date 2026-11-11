// @name Rapport du projet
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
