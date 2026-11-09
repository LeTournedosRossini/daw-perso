import test from "node:test";
import assert from "node:assert/strict";

// La règle touche au DOM seulement quand on construit une Timeline : l'import et `loopHit` n'en ont pas besoin.
const { loopHit, LOOP_H } = await import("../js/ui/timeline.js");

const view = { start: 0, spp: 100 }; // 1 px = 100 échantillons
const state = (loop) => ({ view, loop });

test("bande de boucle : sans boucle, tout le haut de la règle est « vide »", () => {
  assert.equal(loopHit(state(null), 300, 3), "new");
  assert.equal(loopHit(state(null), 300, LOOP_H), null, "sous la bande : la tête de lecture");
  assert.equal(loopHit(state(null), 300, LOOP_H + 10), null);
});

test("bande de boucle : bords, corps et vide", () => {
  const s = state({ start: 10_000, end: 30_000 }); // de x = 100 à x = 300
  assert.equal(loopHit(s, 100, 4), "start");
  assert.equal(loopHit(s, 104, 4), "start");
  assert.equal(loopHit(s, 296, 4), "end");
  assert.equal(loopHit(s, 305, 4), "end");
  assert.equal(loopHit(s, 200, 4), "body");
  assert.equal(loopHit(s, 20, 4), "new");
  assert.equal(loopHit(s, 400, 4), "new");
  assert.equal(loopHit(s, 200, LOOP_H + 1), null);
});

test("bande de boucle : une boucle très courte reste saisissable par ses deux bords", () => {
  const s = state({ start: 10_000, end: 10_400 }); // 4 px de large
  assert.equal(loopHit(s, 99, 4), "start");
  assert.equal(loopHit(s, 105, 4), "end");
});
