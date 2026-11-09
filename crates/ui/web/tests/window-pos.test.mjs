import test from "node:test";
import assert from "node:assert/strict";
import { clampDockRows, clampWindowPos, dragPos } from "../js/window-pos.js";

test("la hauteur ancrée reste dans ses bornes", () => {
  assert.equal(clampDockRows(1), 3);
  assert.equal(clampDockRows(99), 12);
  assert.equal(clampDockRows(6.4), 6);
  assert.equal(clampDockRows(NaN), 5);
});

test("une fenêtre garde toujours son titre atteignable", () => {
  const view = { vw: 1000, vh: 700, w: 400 };
  assert.deepEqual(clampWindowPos({ left: 100, top: 100 }, view), { left: 100, top: 100 });
  assert.deepEqual(clampWindowPos({ left: -5000, top: -50 }, view), { left: -320, top: 0 }); // 80 px restent visibles à gauche
  assert.deepEqual(clampWindowPos({ left: 5000, top: 5000 }, view), { left: 920, top: 660 });
  assert.deepEqual(clampWindowPos({ left: NaN, top: undefined }, view), { left: 40, top: 100 });
});

test("le glissement suit la souris divisée par la taille de l'interface", () => {
  const view = { vw: 1000, vh: 700, w: 400 };
  assert.deepEqual(dragPos({ left: 40, top: 100 }, 100, 50, 1, view), { left: 140, top: 150 });
  assert.deepEqual(dragPos({ left: 40, top: 100 }, 100, 50, 0.5, view), { left: 240, top: 200 }); // interface à 50 % : la souris va deux fois plus vite
  assert.deepEqual(dragPos({ left: 40, top: 100 }, 100, 50, 0, view), { left: 140, top: 150 }); // facteur invalide : 1
});
