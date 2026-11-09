import test from "node:test";
import assert from "node:assert/strict";
import { fitWindow, windowFits, crossfadeHandleDrag, crossfadeCenterDrag, singleHandleDrag, fadeGainAt } from "../js/fade-view.js";

// Mêmes clips que `crossfade.test.mjs`.
const a = { start: 1000, len: 600, offset: 500, source_len: 2000 };
const b = { start: 1400, len: 500, offset: 300, source_len: 2000 };

test("fenêtre : une fois et demie la zone de chaque côté, au moins 0,3 s, jamais avant 0", () => {
  const w = fitWindow(100_000, 148_000, 48_000);
  assert.deepEqual(w, { start: 28_000, end: 220_000 });
  assert.deepEqual(fitWindow(1_000, 1_100, 48_000), { start: 0, end: 1_100 + 14_400 });
  assert.equal(windowFits(w, 100_000, 148_000), true);
  assert.equal(windowFits(w, 60_000, 148_000), true);
  assert.equal(windowFits(w, 28_000, 148_000), false, "la zone touche le bord gauche");
  assert.equal(windowFits(w, 100_000, 101_000), false, "zone devenue minuscule");
});

test("poignée de début : la fin du premier clip reste, le début du second bouge", () => {
  const d = crossfadeHandleDrag(a, b, "start", 1300);
  assert.deepEqual(d, { length: 300, anchor: "end", bStart: 1300, aEnd: 1600 });
  // Au-delà de la matière disponible avant le second clip (300 échantillons) et du début du premier : bornes.
  assert.equal(crossfadeHandleDrag(a, b, "start", 0).length, 500);
  assert.equal(crossfadeHandleDrag(a, b, "start", 1_599).length, 1);
});

test("poignée de fin : le début du second clip reste, la fin du premier bouge", () => {
  const d = crossfadeHandleDrag(a, b, "end", 1700);
  assert.deepEqual(d, { length: 300, anchor: "start", bStart: 1400, aEnd: 1700 });
  assert.equal(crossfadeHandleDrag(a, b, "end", 99_999).length, 500, "borné par la matière après le premier clip et par la fin du second");
  assert.equal(crossfadeHandleDrag(a, b, "end", 1_100).length, 1);
  assert.equal(crossfadeHandleDrag(b, a, "end", 1700), null, "clips qui ne forment pas une paire");
});

test("fondu seul : longueur entre 0 et celle du clip", () => {
  const c = { start: 1000, len: 600 };
  assert.equal(singleHandleDrag(c, "in", 1250), 250);
  assert.equal(singleHandleDrag(c, "in", 900), 0);
  assert.equal(singleHandleDrag(c, "in", 9_999), 600);
  assert.equal(singleHandleDrag(c, "out", 1_400), 200);
  assert.equal(singleHandleDrag(c, "out", 1_700), 0);
});

test("gain entendu : sortie de 1 à 0, entrée de 0 à 1, hors zone 1", () => {
  const lin = { shape: "linear", bend: 0, smooth: 0 };
  assert.equal(fadeGainAt("out", lin, 100, 200, 100), 1);
  assert.equal(fadeGainAt("out", lin, 100, 200, 150), 0.5);
  assert.equal(fadeGainAt("out", lin, 100, 200, 200), 0);
  assert.equal(fadeGainAt("in", lin, 100, 200, 100), 0);
  assert.equal(fadeGainAt("in", lin, 100, 200, 175), 0.75);
  assert.equal(fadeGainAt("out", lin, 100, 100, 50), 1, "zone vide");
});

test("poignée du haut : le milieu reste en place, chaque pixel monté élargit d'un pixel de chaque côté", () => {
  // Zone actuelle [1400, 1600[ : milieu 1500, longueur 200 ; 10 échantillons par pixel.
  const up = crossfadeCenterDrag(a, b, 200, 5, 10);
  assert.deepEqual(up, { length: 300, anchor: "center", bStart: 1350, aEnd: 1650 });
  assert.equal((up.bStart + up.aEnd) / 2, 1500, "le milieu ne bouge pas");
  const down = crossfadeCenterDrag(a, b, 200, -5, 10);
  assert.deepEqual(down, { length: 100, anchor: "center", bStart: 1450, aEnd: 1550 });
  assert.equal(crossfadeCenterDrag(a, b, 200, 0, 10).length, 200, "sans mouvement : inchangé");
});

test("poignée du haut : bornée par la matière disponible et par 1 échantillon", () => {
  assert.equal(crossfadeCenterDrag(a, b, 200, 10_000, 10).length, 800, "maximum centré de ces clips");
  assert.equal(crossfadeCenterDrag(a, b, 200, -10_000, 10).length, 1);
  assert.equal(crossfadeCenterDrag(b, a, 200, 5, 10), null);
});
