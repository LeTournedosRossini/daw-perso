// @name Test événements (lecture seule)
// @description Reste à l'écoute et numérote chaque événement dans la console (lecture, arrêt, sélection, projet, fin de prise). Ne modifie rien.
let n = 0;
for (const type of ["play", "stop", "selection", "project", "recording-end"]) {
  daw.on(type, (e) => daw.log("#" + ++n + " " + type + " " + JSON.stringify(e)));
}
daw.log("à l'écoute : lance la lecture, arrête-la, sélectionne des clips, modifie une piste…");
