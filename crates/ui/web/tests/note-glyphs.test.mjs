import test from "node:test";
import assert from "node:assert/strict";
import { glyphSvg, nextCycle, CYCLE_PAIR, DIRECT_DURATIONS, durationLabel } from "../js/note-glyphs.js";
import { GRIDS, gridById } from "../js/notes-model.js";

test("chaque durée proposée a un glyphe et une grille", () => {
  for (const id of [...DIRECT_DURATIONS, ...CYCLE_PAIR]) {
    assert.ok(glyphSvg(id)?.startsWith("<svg"), id);
    assert.ok(GRIDS.some((g) => g.id === id), `grille ${id}`);
  }
  assert.equal(glyphSvg("free"), null);
  assert.equal(durationLabel("1/32"), "Triple croche");
});

test("le nombre de crochets suit la durée", () => {
  const flags = (id) => (glyphSvg(id).match(/<path/g) ?? []).length;
  assert.deepEqual(["1/4", "1/8", "1/16", "1/32"].map(flags), [0, 1, 2, 3]);
  assert.ok(!glyphSvg("1/1").includes("<line")); // la ronde n'a pas de hampe
  assert.ok(glyphSvg("1/2").includes("<line"));
});

test("ronde et blanche existent comme grilles, la grille par défaut reste la double croche", () => {
  assert.deepEqual(gridById("1/1").f, [1, 1]);
  assert.deepEqual(gridById("1/2").f, [1, 2]);
  assert.equal(gridById("inconnue").id, "1/16");
});

test("le bouton cyclique alterne ronde et triple croche, depuis l'état en attente s'il y en a un", () => {
  assert.equal(nextCycle(null, "1/4"), "1/1"); // ni l'un ni l'autre : d'abord la ronde
  assert.equal(nextCycle("1/1", "1/4"), "1/32"); // second clic rapide : la triple croche
  assert.equal(nextCycle("1/32", "1/4"), "1/1");
  assert.equal(nextCycle(null, "1/1"), "1/32"); // déjà sur la ronde : un clic suffit
  assert.equal(nextCycle(null, "1/32"), "1/1");
});

import { displayGrid } from "../js/notes-model.js";
test("la grille d'affichage ne déforme pas les notes", () => {
  const n = (s, l) => ({ start: s, len: l });
  const sixteenths = [n([1, 16], [1, 16]), n([1, 4], [1, 8])];
  assert.deepEqual(displayGrid(sixteenths, [1, 2]), [1, 16]); // « blanche » choisie : on dessine quand même les doubles croches
  assert.deepEqual(displayGrid(sixteenths, [1, 32]), [1, 32]); // jamais plus gros que la grille choisie... ni plus fin que demandé
  assert.deepEqual(displayGrid([n([0, 1], [1, 4]), n([1, 4], [1, 4])], [1, 2]), [1, 4]);
  assert.deepEqual(displayGrid([n([0, 1], [1, 1])], [1, 1]), [1, 1]);
  assert.deepEqual(displayGrid([n([1, 12], [1, 12])], [1, 4]), [1, 16]); // triolets : non binaire, on retombe sur la double croche
  assert.deepEqual(displayGrid(sixteenths, [1, 12]), [1, 12]); // grille choisie non binaire : inchangée
});
