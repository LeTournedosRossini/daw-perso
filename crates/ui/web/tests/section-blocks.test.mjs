import test from "node:test";
import assert from "node:assert/strict";
import { splitBlocks, blockData, sectionOf, orderBlocks, toClipIndices, toBlockIndices } from "../js/section-blocks.js";

const clip = { clip: 7, start: [4, 1], len: [8, 1], offset: [1, 2], notes: [
  { start: [1, 2], len: [1, 4], key: 60 }, // 4+0   (matière 1/2 = début de la fenêtre)
  { start: [3, 2], len: [1, 4], key: 62 }, // 4+1
  { start: [5, 2], len: [1, 4], key: 64 }, // 4+2 — pile au début de la 2e section si elle commence en 6
  { start: [17, 2], len: [1, 4], key: 65 }, // 4+8 = fin exclue : hors fenêtre
] };
const sec = (at, dur, name) => ({ at, dur, name });

test("sans section : un seul morceau, tout le clip", () => {
  assert.deepEqual(splitBlocks(clip, []), [{ from: [4, 1], to: [12, 1], sec: -1 }]);
  assert.deepEqual(splitBlocks(clip, [sec([20, 1], [4, 1], "Pont")]), [{ from: [4, 1], to: [12, 1], sec: -1 }]); // section hors du clip
});

test("le clip se coupe aux bords des sections, avec les trous en morceaux sans section", () => {
  const st = [sec([6, 1], [3, 1], "Couplet"), sec([9, 1], [8, 1], "Refrain")];
  assert.deepEqual(splitBlocks(clip, st).map((b) => [b.from, b.to, b.sec]), [
    [[4, 1], [6, 1], -1], [[6, 1], [9, 1], 0], [[9, 1], [12, 1], 1],
  ]);
  // section qui commence avant le clip et finit dedans, structure non triée
  const st2 = [sec([10, 1], [2, 1], "B"), sec([0, 1], [6, 1], "A")];
  assert.deepEqual(splitBlocks(clip, st2).map((b) => [b.from, b.to, b.sec]), [[[4, 1], [6, 1], 1], [[6, 1], [10, 1], -1], [[10, 1], [12, 1], 0]]);
});

test("un morceau garde les positions de matière d'origine et ses notes seulement", () => {
  const b = { from: [6, 1], to: [9, 1], sec: 0 };
  const { data, indexMap } = blockData(clip, b);
  assert.deepEqual(data.start, [6, 1]);
  assert.deepEqual(data.len, [3, 1]);
  assert.deepEqual(data.offset, [5, 2]); // 1/2 + (6 − 4)
  assert.deepEqual(data.notes.map((n) => n.key), [64]);
  assert.deepEqual(indexMap, [2]);
  assert.deepEqual(data.notes[0].start, [5, 2]); // inchangée : une note posée ici arrive au bon endroit du vrai clip
});

test("les morceaux de tous les clips d'une section se suivent, pistes dans l'ordre", () => {
  const blocks = [
    { id: "b2-sec2", nodeIndex: 1, from: [8, 1], secAt: [8, 1] },
    { id: "a-sec1", nodeIndex: 0, from: [0, 1], secAt: [0, 1] },
    { id: "b-sec1", nodeIndex: 1, from: [2, 1], secAt: [0, 1] },
    { id: "a-sec2", nodeIndex: 0, from: [8, 1], secAt: [8, 1] },
    { id: "free", nodeIndex: 0, from: [4, 1], secAt: null },
  ];
  assert.deepEqual(orderBlocks(blocks).map((b) => b.id), ["a-sec1", "b-sec1", "free", "a-sec2", "b2-sec2"]);
});

test("indices : du morceau au clip et retour", () => {
  const map = [2, 5, 6];
  assert.deepEqual(toClipIndices([0, 2], map), [2, 6]);
  assert.deepEqual(toClipIndices([9], map), []);
  assert.deepEqual(toBlockIndices([5, 3, 6], map), [1, 2]);
});

test("la section envoyée au module déduit la catégorie du nom", () => {
  assert.deepEqual(sectionOf({ name: "Couplet 1" }), { name: "Couplet 1", category: "verse" });
  assert.deepEqual(sectionOf({ name: "Zorg", category: "chorus", color: "#123456" }), { name: "Zorg", category: "chorus", color: "#123456" });
});
