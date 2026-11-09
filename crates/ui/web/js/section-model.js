// Catégories des sections de la forme (intro, couplet, refrain…) : les mêmes identifiants et les mêmes couleurs que Scorsaire
// (`src/model/sections.js`), pour que les deux logiciels montrent la même chose. Le nom d'une section reste libre ; la catégorie est un champ séparé.

export const CATEGORIES = ["intro", "verse", "preChorus", "chorus", "bridge", "solo", "interlude", "outro", "coda", "letter", "custom"];

/** Noms proposés (français), avec leur catégorie. */
export const SECTION_NAMES = [
  ["Intro", "intro"],
  ["Couplet", "verse"],
  ["Pré-refrain", "preChorus"],
  ["Refrain", "chorus"],
  ["Pont", "bridge"],
  ["Solo", "solo"],
  ["Interlude", "interlude"],
  ["Outro", "outro"],
  ["Coda", "coda"],
];

export const CATEGORY_LABELS = { intro: "Intro", verse: "Couplet", preChorus: "Pré-refrain", chorus: "Refrain", bridge: "Pont", solo: "Solo", interlude: "Interlude", outro: "Outro", coda: "Coda", letter: "Lettre (A, B, C…)", custom: "Autre" };

/** Couleurs douces par catégorie (elles sont derrière la musique). */
export const CATEGORY_COLORS = {
  intro: "#b8c7e6", verse: "#bfe3c0", preChorus: "#e6dcb8", chorus: "#f2c4c4", bridge: "#d9c4ec", solo: "#f7d9b0", interlude: "#c9e6e6", outro: "#cfd3d8", coda: "#cfd3d8", custom: "#e0e0e0",
};
/** Huit couleurs qui tournent pour une section « lettre » selon sa lettre. */
export const LETTER_COLORS = ["#c4d7f2", "#f2d7c4", "#d4f2c4", "#f2c4e6", "#f2f0c4", "#c4f2ea", "#dcc4f2", "#f2c4c4"];

const strip = (s) => String(s).trim().toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "");

/** Catégorie qu'un nom suggère : « Couplet 1 » → verse, « refrain » → chorus, « B » → letter ; sinon `custom`. */
export function suggestCategory(name) {
  const k = strip(name);
  if (/^[a-h]$/.test(k)) return "letter";
  const base = k.replace(/[\s\-_]*\d+\s*$/, "");
  const hit = SECTION_NAMES.find(([n]) => strip(n) === base || strip(n) === k);
  return hit ? hit[1] : "custom";
}

/** Catégorie d'une section : celle qu'elle porte, sinon celle que son nom suggère. */
export const categoryOf = (sec) => (CATEGORIES.includes(sec?.category) ? sec.category : suggestCategory(sec?.name ?? ""));

/** Couleur d'une section : la sienne, sinon celle de sa catégorie (une lettre prend la couleur de sa lettre). */
export function sectionColor(sec) {
  if (sec.color) return sec.color;
  const cat = categoryOf(sec);
  if (cat === "letter") {
    const n = String(sec.name ?? "A").trim().toUpperCase().charCodeAt(0) - 65;
    return LETTER_COLORS[(((Number.isFinite(n) ? n : 0) % 8) + 8) % 8];
  }
  return CATEGORY_COLORS[cat] ?? CATEGORY_COLORS.custom;
}
