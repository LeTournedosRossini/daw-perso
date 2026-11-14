// Gamme et accord « en vigueur » à un instant : calculs purs (sans DOM), testés avec `node --test`.
// Ils lisent la piste d'accords de l'hôte (`track.scales`, `track.chords`) ; positions absolues en rondes (flottant).
// Règles : plusieurs gammes qui se chevauchent → la plus récemment commencée gagne ; aucune gamme → rien n'est contraint ;
// en mode « accord », les notes de l'accord comptent comme jouables même hors gamme.

import { parsePitch } from "./chord-model.js";

const frac = (f) => (Array.isArray(f) ? f[0] / f[1] : Number(f) || 0);
const EPS = 1e-9;
const covers = (e, w) => frac(e.at) - EPS <= w && w < frac(e.at) + frac(e.dur) - EPS;

export const SCALE_MODES = [
  { id: "off", label: "Libre" },
  { id: "scale", label: "Gamme" },
  { id: "chord", label: "Accord" },
];

/** Gamme en vigueur (la plus récemment commencée parmi celles qui couvrent `whole`) ; `null` s'il n'y en a pas. */
export function scaleAt(track, whole) {
  let best = null;
  for (const s of track?.scales ?? []) if (covers(s, whole) && (!best || frac(s.at) >= frac(best.at))) best = s;
  return best;
}

/** Classes de hauteur (0-11) d'une gamme, ou `null` si elle est illisible. */
export function scalePcs(s) {
  const r = parsePitch(s?.root);
  if (!r || !s.intervals?.length) return null;
  return new Set(s.intervals.map((i) => (((r.pc + i) % 12) + 12) % 12));
}

/** Intervalles d'une qualité d'accord : identifiant du catalogue de l'hôte, ou qualité libre `[0,4,7,13]` ; `null` si inconnue. */
export function qualityIntervals(quality, catalog, track) {
  const q = String(quality ?? "");
  const own = track?.qualities?.[q];
  if (Array.isArray(own)) return own;
  const hit = catalog?.qualities?.find(([id]) => id === q);
  if (hit) return hit[1];
  const m = /^\[(\s*\d+\s*(,\s*\d+\s*)*)\]$/.exec(q);
  return m ? m[1].split(",").map((x) => Number(x)) : null;
}

/** Accord en vigueur (hors « pas d'accord ») avec ses classes de hauteur (basse comprise) ; `null` s'il n'y en a pas. */
export function chordAt(track, whole, catalog) {
  const c = (track?.chords ?? []).find((x) => covers(x, whole));
  if (!c || c.nc) return null;
  const r = parsePitch(c.root);
  const iv = qualityIntervals(c.quality, catalog, track);
  if (!r || !iv?.length) return null;
  const pcs = new Set(iv.map((i) => (((r.pc + i) % 12) + 12) % 12));
  const b = c.bass ? parsePitch(c.bass) : null;
  if (b) pcs.add(b.pc);
  return { pcs, root: r.pc };
}

/** Ce qu'il faut savoir à l'instant `whole` : classes autorisées selon le mode (`null` = tout), gamme, accord et fondamentale. */
export function toneAt(track, whole, mode, catalog) {
  const s = scaleAt(track, whole);
  const sp = s ? scalePcs(s) : null;
  const ch = chordAt(track, whole, catalog);
  let allowed = null;
  if (mode === "scale") allowed = sp;
  else if (mode === "chord") allowed = ch ? ch.pcs : null;
  return { allowed, scale: sp, chord: ch?.pcs ?? null, root: s ? parsePitch(s.root)?.pc ?? null : null };
}

/** Touche autorisée la plus proche de `key` (à égalité : la plus grave) ; `key` si tout est permis ; bornée à 0-127. */
export function snapKey(key, allowed) {
  if (!allowed?.size) return key;
  for (let d = 0; d <= 12; d++) {
    for (const k of d === 0 ? [key] : [key - d, key + d]) if (k >= 0 && k <= 127 && allowed.has(k % 12)) return k;
  }
  return key;
}

/** Touche autorisée suivante dans le sens `dir` (+1 / −1) ; `key` si rien n'est plus loin. */
export function stepKey(key, dir, allowed) {
  for (let k = key + dir; k >= 0 && k <= 127; k += dir) if (!allowed?.size || allowed.has(k % 12)) return k;
  return key;
}

/** Instants (rondes absolues, croissants) où la gamme ou l'accord changent dans `[from, to]`, bornes comprises. */
export function breakpoints(track, from, to) {
  const pts = new Set([from]);
  for (const e of [...(track?.scales ?? []), ...(track?.chords ?? [])]) {
    for (const p of [frac(e.at), frac(e.at) + frac(e.dur)]) if (p > from && p < to) pts.add(p);
  }
  return [...pts].sort((a, b) => a - b);
}

/**
 * Modifications de touche qui ramènent des notes à la gamme (mode « scale ») ou à l'accord (« chord ») en vigueur à leur début.
 * `notes` : `[{ index, start, key }]` ; `absWhole(note)` : début absolu en rondes. Les notes déjà permises, ou sans gamme / accord à leur
 * début, ne changent pas. Rend `[{ index, key }]`.
 */
export function fitEdits(notes, absWhole, track, mode, catalog) {
  const edits = [];
  for (const n of notes) {
    const allowed = toneAt(track, absWhole(n), mode, catalog).allowed;
    const key = snapKey(n.key, allowed);
    if (key !== n.key) edits.push({ index: n.index, key });
  }
  return edits;
}

/** Nombre de notes hors de la gamme en vigueur à leur début (les notes sans gamme à cet endroit ne comptent pas). */
export function outOfScale(notes, absWhole, track, catalog) {
  let count = 0;
  for (const n of notes) {
    const sc = toneAt(track, absWhole(n), "scale", catalog).allowed;
    if (sc && !sc.has(n.key % 12)) count++;
  }
  return count;
}
