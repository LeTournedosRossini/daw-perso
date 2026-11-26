// Texte des paroles en strophes : lecture et écriture d'un texte à marqueurs (« [Couplet 1] », « [Refrain] »), et lien d'une strophe
// avec les cartes de la structure. Calculs purs, sans page.

import { CATEGORIES, suggestCategory } from "./section-model.js";

export const MAX_STROPHES = 256;
const strip = (s) => String(s).trim().toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "");
const baseOf = (name) => strip(name).replace(/[\s\-_]*\d+\s*$/, "");

/**
 * Lit un texte : une ligne `[Nom]` commence une strophe, les lignes qui suivent en sont les phrases (les lignes vides sont ignorées).
 * Du texte avant le premier marqueur forme une strophe « Couplet 1 ». Un nom est tronqué à 60 caractères.
 */
export function parseStrophesText(text) {
  const out = [];
  let cur = null;
  for (const raw of String(text ?? "").split(/\r?\n/)) {
    const line = raw.trim();
    const m = /^\[(.*)\]$/.exec(line);
    if (m) {
      cur = { name: m[1].trim().slice(0, 60) || `Strophe ${out.length + 1}`, lines: [] };
      out.push(cur);
    } else if (line) {
      if (!cur) {
        cur = { name: "Couplet 1", lines: [] };
        out.push(cur);
      }
      cur.lines.push(line.slice(0, 400));
    }
  }
  return out.slice(0, MAX_STROPHES);
}

/** Le texte à marqueurs d'une liste de strophes (l'inverse de `parseStrophesText`). */
export const strophesToText = (list) => (list ?? []).map((s) => `[${s.name}]\n${s.lines.join("\n")}`).join("\n\n");

/**
 * Les strophes lues d'un texte, avec la catégorie et le lien qu'avaient déjà les strophes de même nom (`previous`) ; une strophe neuve
 * prend la catégorie que son nom suggère.
 */
export function withMeta(parsed, previous = []) {
  const used = new Set();
  return parsed.map((p) => {
    const i = previous.findIndex((q, k) => !used.has(k) && strip(q.name) === strip(p.name));
    if (i >= 0) used.add(i);
    const old = i >= 0 ? previous[i] : null;
    const category = old?.category ?? suggestCategory(p.name);
    return { name: p.name, ...(CATEGORIES.includes(category) ? { category } : {}), lines: p.lines, ...(old?.link ? { link: old.link } : {}) };
  });
}

/**
 * Les sections de la structure auxquelles une strophe correspond : celles dont le nom est celui du lien (s'il y en a un), sinon celles de
 * même nom ; une strophe sans numéro (« Refrain ») correspond à toutes les sections de ce nom, numérotées (« Refrain 2 ») ou non.
 * `link === ""` : aucune section.
 */
export function sectionsFor(strophe, structure) {
  const want = strophe.link ?? strophe.name;
  if (strophe.link === "") return [];
  const k = strip(want);
  // Un nom sans numéro (« Refrain ») couvre toutes les sections de ce nom, numérotées ou non ; un nom numéroté (« Couplet 2 ») une seule.
  if (!/\d\s*$/.test(want)) return structure.filter((s) => baseOf(s.name) === k);
  return structure.filter((s) => strip(s.name) === k);
}

/** Les sections de la structure qu'aucune strophe ne couvre (pour le dire à l'utilisateur). */
export function uncoveredSections(strophes, structure) {
  const covered = new Set();
  for (const s of strophes) for (const sec of sectionsFor(s, structure)) covered.add(sec);
  return structure.filter((sec) => !covered.has(sec));
}

/** Le texte d'une strophe pour la réserve de paroles : ses phrases mises bout à bout. */
export const strophePlainText = (s) => s.lines.join(" ");

/** Commande `set_strophes` pour une liste ; les champs vides ne sont pas envoyés. */
export function strophesCommand(list, label) {
  return {
    type: "set_strophes",
    strophes: list.map((s) => ({ name: s.name, ...(s.category ? { category: s.category } : {}), lines: s.lines, ...(s.link != null ? { link: s.link } : {}) })),
    ...(label ? { label } : {}),
  };
}
