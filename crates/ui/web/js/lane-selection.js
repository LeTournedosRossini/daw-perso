// Sélection multiple, copier / couper / coller et duplication des éléments des voies d'accords (accords, gammes, tonalités,
// sections de la structure, syllabes). Calculs purs sur des listes d'éléments `{ at, dur?, … }` en fractions de ronde `[n, d]`.

import { fnum, fadd, fsub, fcmp } from "./notes-model.js";

const gcd = (a, b) => (b ? gcd(b, a % b) : a);
const norm = ([n, d]) => {
  const k = gcd(Math.abs(n), Math.abs(d)) || 1;
  return [n / k, d / k];
};
const add = (a, b) => norm(fadd(a, b));
const sub = (a, b) => norm(fsub(a, b));

/** Genres dont les éléments ne peuvent pas se chevaucher dans la voie (l'hôte refuserait). */
export const NO_OVERLAP = new Set(["chord", "section"]);

/** Fin d'un élément : début + durée, ou le trait de prolongation d'une syllabe, ou son début. */
export const endOf = (it) => (it.dur ? add(it.at, it.dur) : it.end ?? it.at);

/** Indices des blocs dont l'étendue horizontale rencontre `[xa, xb]` (pixels). `blocks` : `{ index, x0, x1 }`. */
export function indicesInRect(blocks, xa, xb) {
  const lo = Math.min(xa, xb);
  const hi = Math.max(xa, xb);
  return blocks.filter((b) => b.x1 >= lo && b.x0 <= hi).map((b) => b.index);
}

/** Ajoute ou retire `index` de la sélection `set` (Maj / Ctrl + clic) ; rend un nouvel ensemble. */
export function toggled(set, index) {
  const s = new Set(set);
  if (!s.delete(index)) s.add(index);
  return s;
}

/** Copie des éléments choisis : `{ items (triés par début), origin (début du premier), span (jusqu'à la fin du dernier) }`. */
export function copySelection(list, indices) {
  const items = [...indices].filter((i) => list[i]).map((i) => structuredClone(list[i])).sort((a, b) => fcmp(a.at, b.at));
  if (!items.length) return null;
  const origin = items[0].at;
  const last = items.reduce((m, it) => (fcmp(endOf(it), m) > 0 ? endOf(it) : m), origin);
  return { items, origin, span: sub(last, origin) };
}

/**
 * Colle `clip` avec son premier élément en `at`. Pour les genres sans chevauchement, ce que les copies recouvrent est retiré ; pour les
 * tonalités, celles posées au même instant sont remplacées ; pour les syllabes, celles du même couplet au même instant (déjà fait par
 * l'appelant). `verse` : couplet des syllabes collées. Rend `{ list, added }` (la liste triée et le nombre d'éléments collés).
 */
export function pasteSelection(list, clip, at, kind, verse = null) {
  const shift = sub(at, clip.origin);
  const moved = clip.items.map((it) => {
    const copy = { ...structuredClone(it), at: add(it.at, shift) };
    if (it.end) copy.end = add(it.end, shift);
    if (verse != null && kind === "lyric") copy.verse = verse;
    return copy;
  });
  let kept = list;
  if (NO_OVERLAP.has(kind)) {
    kept = list.filter((x) => !moved.some((m) => fcmp(x.at, add(m.at, m.dur)) < 0 && fcmp(m.at, add(x.at, x.dur)) < 0));
  } else if (kind === "key") {
    kept = list.filter((x) => !moved.some((m) => fcmp(x.at, m.at) === 0));
  } else if (kind === "lyric") {
    kept = list.filter((x) => !moved.some((m) => (m.verse ?? 1) === (x.verse ?? 1) && fcmp(x.at, m.at) === 0));
  }
  return { list: [...kept, ...moved].sort((a, b) => fcmp(a.at, b.at) || (a.verse ?? 1) - (b.verse ?? 1)), added: moved.length };
}

/** Où se pose une duplication : juste après le groupe (genres avec durée), sinon une ronde plus loin (tonalité) ou une noire (syllabe). */
export function duplicateAt(clip, kind) {
  const base = add(clip.origin, clip.span);
  if (kind === "key") return add(base, [1, 1]);
  if (kind === "lyric") return add(base, [1, 4]);
  return base;
}

/** Plus petit décalage permis : le groupe ne passe pas avant 0. `delta` en rondes (flottant) ; rend le décalage borné. */
export function clampDelta(list, indices, delta) {
  const first = Math.min(...[...indices].map((i) => fnum(list[i].at)));
  return Math.max(delta, -first) || 0;
}

/** Déplace les éléments `indices` de `shift` (fraction signée) ; rend la nouvelle liste triée. */
export function moveGroup(list, indices, shift) {
  const set = new Set(indices);
  return list
    .map((x, i) => (set.has(i) ? { ...x, at: add(x.at, shift), ...(x.end ? { end: add(x.end, shift) } : {}) } : x))
    .sort((a, b) => fcmp(a.at, b.at) || (a.verse ?? 1) - (b.verse ?? 1));
}

/** Retire les éléments `indices` ; rend la nouvelle liste. */
export function removeGroup(list, indices) {
  const set = new Set(indices);
  return list.filter((_, i) => !set.has(i));
}
