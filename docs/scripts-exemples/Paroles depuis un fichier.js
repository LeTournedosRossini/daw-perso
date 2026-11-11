// @name Paroles depuis un fichier
// @description Choisis un .txt de paroles : les mots sont posés sur la voie Paroles, un par noire, à partir de la tête de lecture (couplet 1). « ma-man » = deux syllabes liées. Remplace les paroles du couplet 1 sur la zone couverte.
// @permissions files
const path = await daw.fs.pickFile({ extensions: ["txt"], title: "Fichier de paroles" });
if (!path) return "annulé";
const texte = await daw.fs.readText(path);
const { position, lyrics, project } = await daw.state();

// Une syllabe par mot ; « ma-man » donne « ma » (liée) puis « man ». Une noire = 1/4 de ronde.
const sylls = [];
for (const mot of texte.split(/\s+/).filter(Boolean)) {
  const parts = mot.split("-").filter(Boolean);
  parts.forEach((t, i) => sylls.push({ text: t, hyphen: i < parts.length - 1 }));
}
if (!sylls.length) throw new Error("le fichier ne contient pas de mots");

// Position de départ : la tête de lecture, ramenée à la noire (fraction de ronde exacte).
const bpm = project.tempo_map[0].num / project.tempo_map[0].den; // premier tempo
const spq = (project.sample_rate * 60) / bpm; // échantillons par noire
const start = Math.round(position / spq); // en noires
const nouvelles = sylls.map((s, i) => ({ at: daw.frac.add([start + i, 4], [0, 1]), text: s.text, ...(s.hyphen ? { hyphen: true } : {}), verse: 1 }));

// On garde tout ce qui est hors de la zone couverte (et les autres couplets).
const debut = start / 4, fin = (start + sylls.length) / 4;
const gardees = lyrics.filter((l) => (l.verse ?? 1) !== 1 || l.at[0] / l.at[1] < debut || l.at[0] / l.at[1] >= fin);
await daw.send({ type: "set_lyrics", syllables: [...gardees, ...nouvelles], label: "Paroles depuis un fichier" });
daw.log(sylls.length + " syllabes posées à partir de la noire " + start);
