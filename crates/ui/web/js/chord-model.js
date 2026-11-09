// Piste d'accords : calculs purs (sans DOM), testés avec `node --test`. Format = `flibuste-carte` : positions et durées en fractions
// de ronde `[n, d]`, hauteurs en lettres anglaises (« C », « F# », « Bb »), qualités par identifiant du dictionnaire de Scorsaire.

import { fnorm, fadd, fsub, fcmp, fnum } from "./notes-model.js";

const SHARP = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const FLAT = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
const BASES = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 };

/** « Bb », « F# », « Ebb » → { pc, sharp } (écriture conservée) ; `null` si ce n'est pas une note. */
export function parsePitch(text) {
  const m = /^([A-Ga-g])([#b♯♭]*)$/.exec(String(text ?? "").trim());
  if (!m) return null;
  let pc = BASES[m[1].toUpperCase()];
  let sharp = false;
  for (const c of m[2]) {
    if (c === "#" || c === "♯") (pc += 1), (sharp = true);
    else (pc -= 1), (sharp = false);
  }
  return { pc: ((pc % 12) + 12) % 12, sharp };
}

export const pitchName = ({ pc, sharp }) => (sharp ? SHARP : FLAT)[((pc % 12) + 12) % 12];

/** Écriture normalisée d'une note saisie (« bb » → « Bb »). */
export function normalizePitch(text) {
  const p = parsePitch(text);
  if (!p) return null;
  const t = String(text).trim();
  return t[0].toUpperCase() + t.slice(1).replace(/♯/g, "#").replace(/♭/g, "b");
}

/**
 * Texte d'un accord → { nc: true } ou { root, quality, bass } ; `null` si illisible. `Dm7`, `G7/B`, `Bb`, `C[0,4,7,13]`, `N.C.`.
 * La qualité reste telle que saisie : c'est l'hôte qui la ramène à l'identifiant de Scorsaire (alias) ou la refuse.
 */
export function parseChordText(text) {
  const t = String(text ?? "").trim();
  if (/^(n\.?\s?c\.?|%)$/i.test(t)) return { nc: true };
  const m = /^([A-Ga-g][#b♯♭]*)(.*)$/.exec(t);
  if (!m) return null;
  const root = normalizePitch(m[1]);
  let rest = m[2].trim();
  let bass = null;
  const slash = rest.lastIndexOf("/");
  if (slash >= 0 && parsePitch(rest.slice(slash + 1))) {
    bass = normalizePitch(rest.slice(slash + 1));
    rest = rest.slice(0, slash).trim();
  }
  return { root, quality: rest, bass };
}

/** Texte affiché d'un accord de la carte. */
export function chordLabel(c) {
  if (c.nc) return "N.C.";
  return `${c.root}${c.quality}${c.bass ? `/${c.bass}` : ""}`;
}

/** Texte d'une tonalité (« C maj », « D min »). */
export const keyLabel = (k) => `${k.root ?? "?"} ${k.mode === "minor" ? "min" : "maj"}`;

/** Nom d'une gamme de la carte. */
export const scaleLabel = (s) => `${s.root} ${s.name || s.scale || ""}`.trim();

/** Rang de couche (0, 1, 2...) de chaque gamme pour les dessiner sans qu'elles se recouvrent à l'écran (les chevauchements en piles). */
export function scaleLayers(scales) {
  const ends = [];
  return scales.map((s) => {
    const start = fnum(s.at);
    let layer = ends.findIndex((e) => e <= start + 1e-12);
    if (layer < 0) layer = ends.length;
    ends[layer] = start + fnum(s.dur);
    return layer;
  });
}

/** Position d'un début à partir d'une position absolue (rondes, flottant) aimantée à la grille `g` ; `null` : libre (1/3 840). */
export function snapWhole(whole, g) {
  const grid = g ?? [1, 3840];
  const k = Math.max(0, Math.round((whole * grid[1]) / grid[0]));
  return fnorm([k * grid[0], grid[1]]);
}

const sortedBy = (list) => [...list].sort((a, b) => fcmp(a.at, b.at));

/** Nouvelle voie avec l'élément `i` déplacé à `start` (durée conservée). */
export function moveItem(list, i, start) {
  return sortedBy(list.map((x, k) => (k === i ? { ...x, at: start } : x)));
}

/** Nouvelle voie avec le début (`edge: "start"`) ou la fin (`"end"`) de l'élément `i` déplacé en `pos` ; `null` si la durée deviendrait nulle. */
export function resizeItem(list, i, edge, pos) {
  const x = list[i];
  const end = fadd(x.at, x.dur);
  const [start, newEnd] = edge === "start" ? [pos, end] : [x.at, pos];
  if (fcmp(newEnd, start) <= 0) return null;
  return sortedBy(list.map((y, k) => (k === i ? { ...y, at: start, dur: fsub(newEnd, start) } : y)));
}

/** Durée d'un accord posé en `start` : une ronde, ou jusqu'au prochain accord s'il est plus proche. `null` : `start` tombe dans un accord. */
export function newChordLength(chords, start, bar = [1, 1]) {
  let len = bar;
  for (const c of chords) {
    const s = c.at;
    const e = fadd(c.at, c.dur);
    if (fcmp(start, s) >= 0 && fcmp(start, e) < 0) return null;
    if (fcmp(s, start) > 0 && fcmp(fsub(s, start), len) < 0) len = fsub(s, start);
  }
  return len;
}

/** Durée d'une mesure à la position `whole` (rondes) d'après la carte des chiffrages (`MeterMap`) : fraction `[num, den]`. */
export function barLength(meters, whole) {
  if (!meters) return [1, 1];
  const { bar } = meters.locate(Math.max(0, whole));
  const { num, den } = meters.meterAtBar(bar);
  return [num, den];
}

/** Les 12 notes de la fenêtre de saisie : un bouton pour une note naturelle, deux (dièse puis bémol) pour une touche noire. */
export const ROOT_CELLS = [
  { pc: 0, names: ["C"] },
  { pc: 1, names: ["C#", "Db"] },
  { pc: 2, names: ["D"] },
  { pc: 3, names: ["D#", "Eb"] },
  { pc: 4, names: ["E"] },
  { pc: 5, names: ["F"] },
  { pc: 6, names: ["F#", "Gb"] },
  { pc: 7, names: ["G"] },
  { pc: 8, names: ["G#", "Ab"] },
  { pc: 9, names: ["A"] },
  { pc: 10, names: ["A#", "Bb"] },
  { pc: 11, names: ["B"] },
];

/** Texte d'une note pour l'affichage (♯ et ♭ typographiques). */
export const prettyPitch = (name) => (name ? name.replace("#", "♯").replace(/b$/, "♭").replace(/(?<=.)b/, "♭") : "");

/** Brouillon de la fenêtre de saisie ← texte d'un accord (ou `null` s'il est illisible) : { nc, root, quality, bass }. */
export function draftFromText(text) {
  const p = parseChordText(text);
  if (!p) return null;
  return p.nc ? { nc: true, root: null, quality: "", bass: null } : { nc: false, root: p.root, quality: p.quality, bass: p.bass ?? null };
}

/** Texte d'un accord ← brouillon (celui que reçoit `parseChordText`). Sans fondamentale : chaîne vide. */
export function textFromDraft(d) {
  if (d.nc) return "N.C.";
  if (!d.root) return "";
  return chordLabel({ root: d.root, quality: d.quality ?? "", bass: d.bass ?? null });
}

/** Notes d'un accord, écrites avec l'orthographe de la fondamentale : `["D", "F", "A", "C"]` ; la basse (si elle n'en fait pas partie) en tête. */
export function chordNotes(root, intervals, bass = null) {
  const r = parsePitch(root);
  if (!r || !intervals?.length) return [];
  const names = (r.sharp ? SHARP : FLAT);
  const out = [...new Set(intervals.map((i) => names[(((r.pc + i) % 12) + 12) % 12]))];
  const b = bass ? parsePitch(bass) : null;
  if (b) {
    const bn = bass;
    return out.includes(bn) ? out : [bn, ...out];
  }
  return out;
}

/** Résout une qualité saisie avec le catalogue de l'hôte : identifiant exact, alias, ou qualité libre `[0,4,7,13]` ; sinon `null`. */
export function resolveQuality(text, catalog) {
  const t = String(text ?? "").trim();
  if (!catalog) return t;
  if (catalog.qualities.some(([id]) => id === t)) return t;
  const alias = catalog.aliases.find(([a]) => a === t);
  if (alias) return alias[1];
  const m = /^\[(.*)\]$/.exec(t);
  if (m) {
    const nums = m[1].split(",").map((x) => Number(x.trim()));
    if (nums.length && nums.every((n) => Number.isInteger(n) && n >= 0 && n <= 23)) return `[${[...new Set(nums)].sort((a, b) => a - b).join(",")}]`;
  }
  return null;
}

// ---- tonalités (piste des tonalités : armure du morceau, fournie à la partition) ---------------------------------------------

const MAJOR_ROOTS = ["Cb", "Gb", "Db", "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#"]; // −7 … +7 quintes
const MINOR_ROOTS = ["Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#", "G#", "D#", "A#"];

/** Texte d'une armure en quintes : 0 → « aucune altération », 2 → « 2 dièses », −1 → « 1 bémol ». */
export const fifthsText = (n) => (n === 0 ? "aucune altération" : `${Math.abs(n)} ${n > 0 ? (n > 1 ? "dièses" : "dièse") : (n < -1 ? "bémols" : "bémol")}`);

/** Les 30 tonalités proposées (15 majeures, 15 mineures), de 7 bémols à 7 dièses. */
export const KEY_CHOICES = [false, true].flatMap((minor) =>
  Array.from({ length: 15 }, (_, i) => {
    const fifths = i - 7;
    const root = (minor ? MINOR_ROOTS : MAJOR_ROOTS)[i];
    return { id: `${root}${minor ? "m" : ""}`, fifths, minor, root, label: `${root} ${minor ? "mineur" : "majeur"} — ${fifthsText(fifths)}` };
  }),
);

/** Entrée de la piste d'une tonalité choisie (même forme que `keys` du fichier `flibuste-carte`). */
export const keyEntry = (choice, at) => ({ at, root: choice.root, mode: choice.minor ? "minor" : "major", fifths: choice.fifths });

/** Le choix qui correspond à une tonalité de la piste (`{ root, mode, fifths }`), ou `null`. */
export const choiceOf = (k) => KEY_CHOICES.find((c) => c.fifths === k?.fifths && c.minor === (k?.mode === "minor")) ?? KEY_CHOICES.find((c) => c.fifths === k?.fifths && !c.minor) ?? null;

/** Armure en vigueur à la position `whole` (rondes) : dernière tonalité posée avant ou à cette position, sinon la tonalité du morceau, sinon 0. */
export function fifthsAt(track, whole) {
  const keys = [...(track?.keys ?? [])].sort((a, b) => a.at[0] / a.at[1] - b.at[0] / b.at[1]);
  let cur = track?.globalKey?.fifths ?? 0;
  for (const k of keys) {
    if (k.at[0] / k.at[1] <= whole + 1e-9) cur = k.fifths ?? cur;
    else break;
  }
  return cur;
}

/** Les changements de tonalité strictement après `from` et avant `to` (rondes) : `[{ whole, fifths }]`. */
export function keyChangesWithin(track, from, to) {
  return (track?.keys ?? [])
    .map((k) => ({ whole: k.at[0] / k.at[1], fifths: k.fifths }))
    .filter((k) => k.whole > from + 1e-9 && k.whole < to - 1e-9)
    .sort((a, b) => a.whole - b.whole);
}

/** Texte d'une tonalité de la piste : « D majeur · 2 dièses ». */
export function keyName(k) {
  const c = choiceOf(k);
  return c ? `${c.root} ${c.minor ? "mineur" : "majeur"} · ${fifthsText(c.fifths)}` : keyLabel(k);
}

/** La tonalité en vigueur à la position `whole` (la dernière posée avant ou à cette position, sinon la tonalité du morceau, sinon `null`). */
export function keyAt(track, whole) {
  const keys = [...(track?.keys ?? [])].sort((a, b) => a.at[0] / a.at[1] - b.at[0] / b.at[1]);
  let cur = track?.globalKey ?? null;
  for (const k of keys) {
    if (k.at[0] / k.at[1] <= whole + 1e-9) cur = k;
    else break;
  }
  return cur;
}

/** Le marqueur posé exactement à `whole`, ou `null`. */
export const markerAt = (track, whole) => (track?.keys ?? []).find((k) => Math.abs(k.at[0] / k.at[1] - whole) < 1e-9) ?? null;
