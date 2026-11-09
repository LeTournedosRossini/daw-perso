// Partition « en blocs par section » : calculs purs, sans DOM. Un clip de notes dont la partie visible traverse plusieurs sections de la forme
// est montré au module de Scorsaire en plusieurs morceaux, un par section (le DAW ne coupe rien : c'est de l'affichage). Chaque morceau garde les
// positions d'origine « depuis le début de la matière » : une note posée dedans arrive à la bonne place dans le vrai clip ; seuls les indices
// de notes changent (ceux de la liste réduite du morceau), `indexMap` les ramène à ceux du clip.

import { fadd, fsub, fcmp, fmax, fmin } from "./notes-model.js";
import { categoryOf } from "./section-model.js";

/**
 * Les morceaux d'un clip : `d` = événement `notes` (start, len, offset…), `structure` = sections [{ at, dur, … }] (triées ou non, sans chevauchement).
 * Rend [{ from, to, sec }] dans l'ordre du temps : `sec` = indice de la section dans `structure`, ou -1 pour ce qui est hors de toute section.
 * Un clip sans section dans sa fenêtre donne un seul morceau (`sec` -1) qui est tout le clip.
 */
export function splitBlocks(d, structure) {
  const S = d.start;
  const E = fadd(d.start, d.len);
  const secs = (structure ?? []).map((s, i) => ({ i, at: s.at, end: fadd(s.at, s.dur) })).filter((s) => fcmp(s.end, S) > 0 && fcmp(s.at, E) < 0).sort((a, b) => fcmp(a.at, b.at));
  const out = [];
  let cursor = S;
  for (const s of secs) {
    const from = fmax(s.at, cursor);
    if (fcmp(from, cursor) > 0) out.push({ from: cursor, to: from, sec: -1 });
    const to = fmin(s.end, E);
    if (fcmp(to, from) > 0) out.push({ from, to, sec: s.i });
    cursor = fmax(cursor, to);
  }
  if (fcmp(cursor, E) < 0) out.push({ from: cursor, to: E, sec: -1 });
  return out;
}

/** Le clip réduit à un morceau : mêmes positions de matière, fenêtre (start, len, offset) rognée, seules les notes qui y commencent ; `indexMap[i]` = indice dans le clip entier. */
export function blockData(d, b) {
  const offset = fadd(d.offset, fsub(b.from, d.start));
  const end = fadd(offset, fsub(b.to, b.from));
  const notes = [];
  const indexMap = [];
  d.notes.forEach((n, i) => {
    if (fcmp(n.start, offset) >= 0 && fcmp(n.start, end) < 0) {
      notes.push(n);
      indexMap.push(i);
    }
  });
  return { data: { ...d, start: b.from, len: fsub(b.to, b.from), offset, notes, selection: [] }, indexMap };
}

/** Ce que le module reçoit comme `clip.section` pour la section `sec` de la forme (la catégorie se déduit du nom si elle manque). */
export function sectionOf(sec) {
  return { name: sec.name, category: categoryOf(sec), ...(sec.color ? { color: sec.color } : {}) };
}

/**
 * Ordre d'affichage des morceaux de plusieurs clips : section après section (dans le temps), et dans une section les pistes dans l'ordre du projet ;
 * les morceaux qui sont hors de toute section se rangent à leur début. Ainsi les morceaux d'une même section se suivent et le module les met sous un seul titre.
 * `blocks` : [{ nodeIndex, from, secAt (début de la section, ou null) }] → les mêmes, triés.
 */
export function orderBlocks(blocks) {
  const t = (b) => b.secAt ?? b.from;
  return [...blocks].sort((a, b) => fcmp(t(a), t(b)) || a.nodeIndex - b.nodeIndex || fcmp(a.from, b.from));
}

/** Les indices de morceau → indices du clip entier (ceux que l'adaptateur et le protocole attendent). */
export const toClipIndices = (indices, indexMap) => (indices ?? []).map((i) => indexMap[i]).filter((i) => i !== undefined);

/** L'inverse : indices de clip → indices du morceau (ceux qui n'y sont pas sont ignorés). */
export function toBlockIndices(indices, indexMap) {
  const at = new Map(indexMap.map((orig, i) => [orig, i]));
  return (indices ?? []).map((i) => at.get(i)).filter((i) => i !== undefined);
}
