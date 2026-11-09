import test from "node:test";
import assert from "node:assert/strict";

test("applyLyricExtend : tirer la poignée pose, déplace ou retire le trait d'une syllabe", async () => {
  const { applyLyricExtend } = await import("../js/lyrics-model.js");
  const d = { start: [2, 1], offset: [0, 1] };
  const list = [{ at: [2, 1], text: "ma", hyphen: false, verse: 1 }, { at: [5, 2], text: "man", hyphen: false, verse: 1 }, { at: [2, 1], text: "ma", hyphen: false, verse: 2 }];
  const r = applyLyricExtend(list, d, { start: [0, 1], verse: 1, end: [1, 2] });
  assert.deepEqual(r.syllables[0].end, [5, 2]);
  assert.equal(r.syllables[2].end, undefined, "l'autre couplet ne bouge pas");
  const back = applyLyricExtend(r.syllables, d, { start: [0, 1], verse: 1 });
  assert.equal(back.syllables[0].end, undefined);
  assert.equal(applyLyricExtend(list, d, { start: [0, 1], verse: 1 }), null, "rien à retirer");
  assert.equal(applyLyricExtend(list, d, { start: [0, 1], verse: 1, end: [0, 1] }), null, "fin avant le début");
  assert.equal(applyLyricExtend(list, d, { start: [9, 1], verse: 1, end: [10, 1] }), null, "syllabe inconnue");
});
