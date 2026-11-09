import test from "node:test";
import assert from "node:assert/strict";
import { crossfadeBounds, crossfadeEdges, isCrossfadePair } from "../js/crossfade.js";

// Mêmes clips que `session/tests/xfade.rs` : a = [1000, 1600) décalage 500 dans 2000 ; b = [1400, 1900) décalage 300.
const a = { start: 1000, len: 600, offset: 500, source_len: 2000 };
const b = { start: 1400, len: 500, offset: 300, source_len: 2000 };

test("bornes : valeurs connues, identiques à la version Rust", () => {
  assert.deepEqual(crossfadeBounds(a, b, "start"), { min: 1, max: 500 });
  assert.deepEqual(crossfadeBounds(a, b, "end"), { min: 1, max: 500 });
  assert.deepEqual(crossfadeBounds(a, b, "center"), { min: 1, max: 800 });
});

test("arêtes : chaque ancrage garde son bord (arrondi inférieur au centre)", () => {
  assert.deepEqual(crossfadeEdges(a, b, 300, "start"), { bStart: 1400, aEnd: 1700 });
  assert.deepEqual(crossfadeEdges(a, b, 300, "end"), { bStart: 1300, aEnd: 1600 });
  assert.deepEqual(crossfadeEdges(a, b, 300, "center"), { bStart: 1350, aEnd: 1650 });
  assert.deepEqual(crossfadeEdges(a, b, 201, "center"), { bStart: 1399, aEnd: 1600 });
});

test("à la borne maximale les arêtes restent dans les données et dans l'ordre des clips", () => {
  for (const anchor of ["center", "start", "end"]) {
    const { max } = crossfadeBounds(a, b, anchor);
    const { bStart, aEnd } = crossfadeEdges(a, b, max, anchor);
    assert.ok(bStart > a.start && bStart >= b.start - b.offset, anchor);
    assert.ok(aEnd <= b.start + b.len && aEnd <= a.start + a.len + (a.source_len - a.offset - a.len), anchor);
    const over = crossfadeEdges(a, b, max + 1, anchor);
    assert.ok(over.bStart <= a.start || over.bStart < b.start - b.offset || over.aEnd > b.start + b.len || over.aEnd > 2500, `${anchor} : un de plus sort`);
  }
});

test("clips sans chevauchement : pas de paire", () => {
  assert.equal(isCrossfadePair(b, a), false);
  assert.equal(crossfadeBounds(a, { ...b, start: 5000 }, "center"), null);
});
