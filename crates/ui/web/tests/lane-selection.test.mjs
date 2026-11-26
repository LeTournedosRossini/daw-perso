import test from "node:test";
import assert from "node:assert/strict";
import { indicesInRect, toggled, copySelection, pasteSelection, duplicateAt, clampDelta, moveGroup, removeGroup } from "../js/lane-selection.js";

const chords = [
  { at: [0, 1], dur: [1, 2], name: "C" },
  { at: [1, 2], dur: [1, 2], name: "F" },
  { at: [1, 1], dur: [1, 1], name: "G" },
];

test("rectangle : les blocs touchés par l'étendue", () => {
  const blocks = [{ index: 0, x0: 0, x1: 50 }, { index: 1, x0: 60, x1: 100 }, { index: 2, x0: 120, x1: 200 }];
  assert.deepEqual(indicesInRect(blocks, 70, 130), [1, 2]);
  assert.deepEqual(indicesInRect(blocks, 130, 70), [1, 2]);
  assert.deepEqual(indicesInRect(blocks, 51, 59), []);
});

test("Maj + clic : ajoute puis retire", () => {
  const a = toggled(new Set([1]), 2);
  assert.deepEqual([...a].sort(), [1, 2]);
  assert.deepEqual([...toggled(a, 1)], [2]);
});

test("copie : début du premier, étendue jusqu'à la fin du dernier", () => {
  const clip = copySelection(chords, [2, 0]);
  assert.deepEqual(clip.origin, [0, 1]);
  assert.deepEqual(clip.span, [2, 1]);
  assert.deepEqual(clip.items.map((i) => i.name), ["C", "G"]);
  assert.equal(copySelection(chords, []), null);
});

test("coller des accords : remplace ce qu'ils recouvrent, garde les écarts", () => {
  const clip = copySelection(chords, [0, 1]); // C F sur une ronde
  const r = pasteSelection(chords, clip, [1, 1], "chord");
  assert.equal(r.added, 2);
  assert.deepEqual(r.list.map((c) => c.name), ["C", "F", "C", "F"]); // G recouvert : retiré
  assert.deepEqual(r.list[3].at, [3, 2]);
});

test("dupliquer : juste après le groupe", () => {
  const clip = copySelection(chords, [0, 1]);
  assert.deepEqual(duplicateAt(clip, "chord"), [1, 1]);
  const r = pasteSelection(chords.slice(0, 2), clip, duplicateAt(clip, "chord"), "chord");
  assert.deepEqual(r.list.map((c) => c.name), ["C", "F", "C", "F"]);
});

test("tonalités : une tonalité au même instant est remplacée", () => {
  const keys = [{ at: [0, 1], fifths: 0 }, { at: [2, 1], fifths: 2 }];
  const clip = copySelection(keys, [1]);
  const r = pasteSelection(keys, clip, [0, 1], "key");
  assert.equal(r.list.length, 2);
  assert.equal(r.list[0].fifths, 2);
});

test("syllabes : prennent le couplet choisi et remplacent celles du même instant", () => {
  const syl = [{ at: [0, 1], text: "ma", verse: 1 }, { at: [1, 4], text: "man", verse: 1 }, { at: [0, 1], text: "x", verse: 2 }];
  const clip = copySelection(syl, [0, 1]);
  const r = pasteSelection(syl, clip, [1, 1], "lyric", 2);
  assert.deepEqual(r.list.filter((s) => s.verse === 2).map((s) => s.text), ["x", "ma", "man"]);
  assert.equal(r.list.filter((s) => s.verse === 1).length, 2);
});

test("déplacer un groupe : décalage commun, jamais avant 0", () => {
  assert.equal(clampDelta(chords, [0, 1], -1), 0);
  assert.equal(clampDelta(chords, [1, 2], -0.25), -0.25);
  const moved = moveGroup(chords, [1, 2], [1, 1]);
  assert.deepEqual(moved.map((c) => c.name), ["C", "F", "G"]);
  assert.deepEqual(moved[1].at, [3, 2]);
  assert.deepEqual(moved[2].at, [2, 1]);
});

test("supprimer un groupe", () => {
  assert.deepEqual(removeGroup(chords, [0, 2]).map((c) => c.name), ["F"]);
});
