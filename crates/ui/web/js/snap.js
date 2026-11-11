// Réglage général de l'aimantation (magnétisme) de la timeline : une seule grille pour tout ce qui se pose ou se déplace à la souris
// (clips, rognage, boucle, marqueurs d'étirement, points d'automation, dépôt de fichiers) et, avec une grille choisie, la tête de lecture.
//
// Modes : `off` (rien), `auto` (la ligne de grille visible la plus proche, à 8 px près : le comportement historique), puis une grille
// **musicale fixe** : `bar` (mesures), `beat` (temps du chiffrage), `1/8`, `1/16`, `1/32` (croche, double, triple croche). Une grille fixe
// aimante toujours, quel que soit le zoom. Maj / Alt contournent ponctuellement (comme avant). Le choix est mémorisé.

import { snapSample as snapVisible } from "./time.js";

export const SNAP_MODES = [
  ["auto", "Auto (grille visible)"],
  ["bar", "Mesures"],
  ["beat", "Temps"],
  ["1/8", "Croches (1/8)"],
  ["1/16", "Doubles croches (1/16)"],
  ["1/32", "Triples croches (1/32)"],
  ["off", "Désactivée"],
];
const KEY = "daw-snap";
const NOTE = { "1/8": 1 / 8, "1/16": 1 / 16, "1/32": 1 / 32 };

let mode = "auto";
try {
  const saved = localStorage.getItem(KEY);
  if (SNAP_MODES.some(([m]) => m === saved)) mode = saved;
} catch {
  // Pas de mémorisation : le choix vaut pour cette session.
}
const listeners = new Set();

export const getSnapMode = () => mode;
export const isFixedGrid = (m = mode) => m !== "auto" && m !== "off";
export function onSnapChange(fn) {
  listeners.add(fn);
}
export function setSnapMode(m) {
  if (!SNAP_MODES.some(([k]) => k === m) || m === mode) return;
  mode = m;
  try {
    localStorage.setItem(KEY, m);
  } catch {
    // voir plus haut
  }
  for (const f of listeners) f(m);
}

/** Échantillon de la ligne de la grille fixe `m` la plus proche de `sample` (chiffrages et tempos variables pris en compte). */
export function gridSnap(tempo, meters, sample, m) {
  const whole = Math.max(0, tempo.wholeAt(sample));
  const { bar, within } = meters.locate(whole);
  const bs = meters.barStart(bar);
  const be = meters.barStart(bar + 1);
  const cands = [bs, be];
  if (m !== "bar") {
    const step = m === "beat" ? 1 / meters.meterAtBar(bar).den : NOTE[m];
    if (step) {
      const k = within / step;
      for (const q of [Math.floor(k + 1e-9), Math.ceil(k - 1e-9)]) {
        const w = bs + q * step;
        if (w <= be + 1e-12) cands.push(w);
      }
    }
  }
  let best = sample;
  let bestD = Infinity;
  for (const w of cands) {
    const s = tempo.samplesAt(w);
    const d = Math.abs(s - sample);
    if (d < bestD) {
      bestD = d;
      best = s;
    }
  }
  return Math.round(best);
}

/** Même signature que `snapSample` de `time.js` : tout le code d'édition passe par cette fonction. */
export function snapSample(vp, widthPx, tempo, meters, sample, thresholdPx = 8) {
  if (mode === "off") return Math.round(sample);
  if (mode === "auto") return snapVisible(vp, widthPx, tempo, meters, sample, thresholdPx);
  return gridSnap(tempo, meters, sample, mode);
}

/** Position de la tête de lecture : aimantée seulement avec une grille fixe (en « auto » le clic sur la règle reste libre). */
export function snapPlayhead(vp, widthPx, tempo, meters, sample) {
  return isFixedGrid() ? gridSnap(tempo, meters, sample, mode) : Math.round(sample);
}
