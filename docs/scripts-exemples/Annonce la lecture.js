// @name Annonce la lecture
// @description Reste à l'écoute : écrit dans la console quand la lecture démarre ou s'arrête, et quand la sélection change.
daw.on("play", (e) => daw.log("lecture depuis " + e.position + " échantillons"));
daw.on("stop", (e) => daw.log("arrêt à " + e.position + " échantillons"));
daw.on("selection", (e) => daw.log(e.clips.length + " clip(s) sélectionné(s)"));
