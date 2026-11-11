// @name Humaniser la sélection
// @description Décale un peu le début et la vélocité des notes des clips sélectionnés (reproductible : même graine, même résultat).
const clips = await daw.selection({ kind: "notes" });
if (!clips.length) throw new Error("Sélectionne d'abord un clip de notes");
let n = 0;
for (const clip of clips) n += await daw.notes.humanize(clip, { timing: 12, velocity: 0.08, seed: clip });
daw.log(n + " note(s) humanisée(s)");
