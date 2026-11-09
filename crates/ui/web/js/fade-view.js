// Calculs de la vue « formes d'onde » de l'éditeur de fondu (sans dessin : testables seuls) : fenêtre de temps, déplacement
// des poignées, gain entendu à chaque instant. Les bornes sont celles de `crossfade.js`.

import { crossfadeBounds, crossfadeEdges } from "./crossfade.js";
import { fadeGain } from "./fades.js";

/** Fenêtre de temps `{ start, end }` (échantillons) autour de la zone `[zs, ze]` du fondu : une fois et demie sa longueur de chaque côté, au moins 0,3 s. */
export function fitWindow(zs, ze, sampleRate) {
  const pad = Math.max((ze - zs) * 1.5, 0.3 * sampleRate);
  return { start: Math.max(0, Math.round(zs - pad)), end: Math.round(ze + pad) };
}

/** Faut-il refaire la fenêtre ? Oui si la zone sort de sa partie centrale (10 % à 90 %) ou si elle y est devenue minuscule. */
export function windowFits(win, zs, ze) {
  const w = win.end - win.start;
  return zs >= win.start + 0.1 * w && ze <= win.end - 0.1 * w && ze - zs >= 0.04 * w;
}

/**
 * Poignée d'un fondu enchaîné déplacée à l'échantillon `sample`. `handle` : `start` (début de la zone = début du second clip :
 * la fin du premier ne bouge pas, ancrage `end`) ou `end` (fin de la zone = fin du premier clip : le début du second ne bouge
 * pas, ancrage `start`). Retourne `{ length, anchor, bStart, aEnd }`, la longueur bornée par la matière disponible.
 */
export function crossfadeHandleDrag(a, b, handle, sample) {
  const anchor = handle === "start" ? "end" : "start";
  const bounds = crossfadeBounds(a, b, anchor);
  if (!bounds) return null;
  const raw = handle === "start" ? a.start + a.len - sample : sample - b.start;
  const length = Math.max(bounds.min, Math.min(bounds.max, Math.round(raw)));
  return { length, anchor, ...pick(crossfadeEdges(a, b, length, anchor)) };
}
const pick = ({ bStart, aEnd }) => ({ bStart, aEnd });

/**
 * Poignée du haut d'un fondu enchaîné, tirée verticalement : le milieu du fondu reste en place (ancrage `center`) et chaque
 * pixel monté élargit la zone d'un pixel de chaque côté (`spp` échantillons par pixel), chaque pixel descendu la rétrécit.
 * `startLength` : longueur au début du geste ; `dy` : pixels montés (négatif = descendu).
 */
export function crossfadeCenterDrag(a, b, startLength, dy, spp) {
  const bounds = crossfadeBounds(a, b, "center");
  if (!bounds) return null;
  const length = Math.max(bounds.min, Math.min(bounds.max, Math.round(startLength + 2 * dy * spp)));
  return { length, anchor: "center", ...pick(crossfadeEdges(a, b, length, "center")) };
}

/** Longueur d'un fondu seul (`in` ou `out`) quand sa poignée est déplacée à `sample` : entre 0 et la longueur du clip. */
export function singleHandleDrag(clip, side, sample) {
  const raw = side === "in" ? sample - clip.start : clip.start + clip.len - sample;
  return Math.max(0, Math.min(clip.len, Math.round(raw)));
}

/** Gain appliqué à l'échantillon `t` par le fondu de sortie (`role` `out`) ou d'entrée (`in`) qui occupe `[zs, ze]`. */
export function fadeGainAt(role, curve, zs, ze, t) {
  if (ze <= zs) return 1;
  const p = Math.max(0, Math.min(1, (t - zs) / (ze - zs)));
  return fadeGain(curve, role === "out" ? 1 - p : p);
}
