// @name Rapport du projet en fichier
// @description Écrit un rapport texte (pistes, clips, durée) dans le dossier du projet (rapport.txt ; demande avant de remplacer un rapport existant).
// @permissions files
const dossier = await daw.fs.projectDir();
if (!dossier) throw new Error("Enregistre d'abord le projet (Ctrl+S) : il n'a pas encore de dossier");
const { project } = await daw.state();
const lignes = ["Rapport du projet", ""];
for (const n of project.nodes) lignes.push(n.name.padEnd(20) + n.clips.length + " clip(s)");
const chemin = daw.fs.join(dossier, "rapport.txt");
const existe = (await daw.fs.list(dossier)).some((e) => e.name === "rapport.txt");
await daw.fs.writeText(chemin, lignes.join("\n") + "\n", { overwrite: existe });
daw.log("rapport écrit : " + chemin);
