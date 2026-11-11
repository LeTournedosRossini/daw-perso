// @name Exporter les notes en CSV
// @description Écrit les notes du clip de notes sélectionné dans un fichier .csv (début et durée en noires, touche, vélocité, canal).
// @permissions files
const [clip] = await daw.selection({ kind: "notes" });
if (clip === undefined) throw new Error("Sélectionne d'abord un clip de notes");
const { notes } = await daw.notes.get(clip);
const noires = (f) => (f[0] * 4) / f[1];
const lignes = ["debut_noires;duree_noires;touche;velocite;canal"];
for (const n of notes) lignes.push([noires(n.start), noires(n.len), n.key, n.velocity.toFixed(3), n.channel].join(";"));
const path = await daw.fs.pickSave({ defaultName: "notes.csv", extensions: ["csv"], title: "Exporter les notes" });
if (!path) return "annulé";
await daw.fs.writeText(path, lignes.join("\n") + "\n");
daw.log(notes.length + " notes écrites dans " + path);
