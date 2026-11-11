// @name Test réaction annulable
// @description Au démarrage de la lecture, ajoute « * » au nom de la première piste. Un seul Ctrl+Z défait la réaction ; le script continue d'écouter.
daw.on("play", async () => {
  const { project } = await daw.state();
  const piste = project.nodes.find((n) => n.id !== 0);
  if (!piste || piste.name.endsWith("*")) return;
  await daw.send({ type: "rename_track", node: piste.id, name: piste.name + "*" });
  daw.log("renommée : " + piste.name + "*  (Ctrl+Z pour défaire)");
});
daw.log("à l'écoute : lance la lecture, puis Ctrl+Z");
