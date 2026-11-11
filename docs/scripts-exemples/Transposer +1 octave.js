// @name Transposer +1 octave
// @description Monte d'une octave les notes des clips de notes sélectionnés (celles qui sortiraient de 0-127 restent en place).
// @shortcut Ctrl+Alt+T
const clips = await daw.selection({ kind: "notes" });
if (!clips.length) throw new Error("Sélectionne d'abord un clip de notes");
let n = 0;
for (const clip of clips) n += await daw.notes.transpose(clip, 12);
daw.log(n + " note(s) transposée(s)");
