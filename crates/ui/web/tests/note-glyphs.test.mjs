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
  assert.deepEqual(displayGrid([n([1, 12], [1, 12])], [1, 4]), [1, 12]); // triolet de croches : grille 1/12, le module écrit un vrai triolet
  assert.deepEqual(displayGrid(sixteenths, [1, 12]), [1, 12]); // grille choisie non binaire : inchangée
});

test("n-olets : la grille d'écriture suit les notes sans grossir celles qui sont binaires", () => {
  const n = (start, len) => ({ start, len, key: 60 });
  const trip8 = [n([0, 1], [1, 12]), n([1, 12], [1, 12]), n([1, 6], [1, 12])];
  assert.deepEqual(displayGrid(trip8, [1, 1]), [1, 12]);
  assert.deepEqual(displayGrid([n([0, 1], [1, 6]), n([1, 6], [1, 6]), n([1, 3], [1, 6])], [1, 1]), [1, 6]); // triolet de noires
  assert.deepEqual(displayGrid([n([1, 4], [1, 8]), ...trip8], [1, 1]), [1, 12]); // croche + triolet de croches : 1/12 (voisine 1/16)
  assert.deepEqual(displayGrid([n([1, 4], [1, 16]), ...trip8], [1, 1]), [1, 12]); // double croche + triolet de croches
  assert.deepEqual(displayGrid([n([1, 4], [1, 32]), ...trip8], [1, 1]), [1, 24]); // triple croche + triolet : il faut la voisine 1/32, donc 1/24
  assert.deepEqual(displayGrid([n([0, 1], [1, 20]), n([1, 20], [1, 20])], [1, 1]), [1, 20]); // quintolet
  assert.deepEqual(displayGrid([n([1, 7], [1, 7])], [1, 1]), [1, 16]); // ni binaire ni ternaire : comme avant
});
