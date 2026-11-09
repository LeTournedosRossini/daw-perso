// Longueur d'un fondu enchaîné : mêmes règles que `session::xfade` (Rust). La longueur est la zone où `a` (qui sort) et
// `b` (qui entre) se chevauchent ; la changer rogne les bords des deux clips, leur contenu ne bouge pas.
// Ancrages : `center` (le milieu du fondu reste en place), `start` (début du fondu = début de b), `end` (fin = fin de a).

export const ANCHORS = ["center", "start", "end"];

/** Vrai si « a puis b » se chevauchent comme le fondu automatique l'exige. */
export function isCrossfadePair(a, b) {
  return a.start < b.start && b.start < a.start + a.len && a.start + a.len <= b.start + b.len;
}

/** Longueurs possibles `{ min, max }` (échantillons) selon l'ancrage, ou `null` si les clips ne forment pas une paire. */
export function crossfadeBounds(a, b, anchor) {
  if (!isCrossfadePair(a, b)) return null;
  const s = b.start;
  const e = a.start + a.len;
  const tailA = a.source_len - a.offset - a.len; // données après la fin de a
  const lo = Math.max(a.start + 1, s - b.offset); // plus petit début de b possible
  const hi = Math.min(b.start + b.len, e + tailA); // plus grande fin de a possible
  let max;
  if (anchor === "start") max = hi - s;
  else if (anchor === "end") max = e - lo;
  else max = Math.min(s + e - 2 * lo, 2 * hi + 1 - s - e);
  return { min: 1, max: Math.max(1, max) };
}

/** Début de b et fin de a pour une longueur donnée. */
export function crossfadeEdges(a, b, len, anchor) {
  const s = b.start;
  const e = a.start + a.len;
  if (anchor === "start") return { bStart: s, aEnd: s + len };
  if (anchor === "end") return { bStart: e - len, aEnd: e };
  const bStart = Math.floor((s + e - len) / 2);
  return { bStart, aEnd: bStart + len };
}
