import test from "node:test";
import assert from "node:assert/strict";
import { scaleAt, scalePcs, chordAt, toneAt, snapKey, stepKey, breakpoints, qualityIntervals } from "../js/scale-model.js";

const MAJOR = [0, 2, 4, 5, 7, 9, 11];
const catalog = { qualities: [["", [0, 4, 7]], ["m7", [0, 3, 7, 10]]] };
const track = {
  scales: [
    { at: [0, 1], dur: [2, 1], root: "C", intervals: MAJOR },
    { at: [1, 1], dur: [2, 1], root: "D", intervals: MAJOR }, // chevauche la première à partir de 1
  ],
  chords: [
    { at: [0, 1], dur: [1, 1], root: "C", quality: "", bass: null },
    { at: [1, 1], dur: [1, 1], nc: true },
    { at: [2, 1], dur: [1, 1], root: "A", quality: "m7", bass: "E" },
  ],
  qualities: {},
};

test("la gamme la plus récemment commencée gagne, aucune gamme hors de leur durée", () => {
  assert.equal(scaleAt(track, 0.5).root, "C");
  assert.equal(scaleAt(track, 1.5).root, "D");
  assert.equal(scaleAt(track, 2.5).root, "D");
  assert.equal(scaleAt(track, 3), null);
  assert.deepEqual([...scalePcs(scaleAt(track, 1.5))].sort((a, b) => a - b), [1, 2, 4, 6, 7, 9, 11]);
});

test("accord en vigueur : notes, basse, pas d'accord, qualité libre", () => {
  assert.deepEqual([...chordAt(track, 0.2, catalog).pcs].sort((a, b) => a - b), [0, 4, 7]);
  assert.equal(chordAt(track, 1.2, catalog), null);
  assert.deepEqual([...chordAt(track, 2.2, catalog).pcs].sort((a, b) => a - b), [0, 4, 7, 9]);
  assert.deepEqual(qualityIntervals("[0,4,7,13]", catalog, track), [0, 4, 7, 13]);
  assert.equal(qualityIntervals("inconnue", catalog, track), null);
});

test("modes : libre ne contraint rien, gamme et accord donnent leurs classes", () => {
  assert.equal(toneAt(track, 0.2, "off", catalog).allowed, null);
  assert.equal(toneAt(track, 0.2, "scale", catalog).allowed.size, 7);
  assert.equal(toneAt(track, 0.2, "chord", catalog).allowed.size, 3);
  assert.equal(toneAt(track, 1.2, "chord", catalog).allowed, null); // pas d'accord : rien n'est contraint
  assert.equal(toneAt(track, 3.5, "scale", catalog).allowed, null); // pas de gamme
});

test("aimantation de la touche : plus proche, à égalité la plus grave, bornes", () => {
  const cMajor = new Set(MAJOR);
  assert.equal(snapKey(60, cMajor), 60);
  assert.equal(snapKey(61, cMajor), 60); // do# : do (60) et ré (62) à égale distance → le grave
  assert.equal(snapKey(66, cMajor), 65); // fa# → fa
  assert.equal(snapKey(61, null), 61);
  assert.equal(snapKey(0, new Set([2])), 2);
  assert.equal(snapKey(127, new Set([0])), 120);
});

test("flèches : prochaine note permise dans le sens demandé", () => {
  const cMajor = new Set(MAJOR);
  assert.equal(stepKey(64, 1, cMajor), 65);
  assert.equal(stepKey(64, -1, cMajor), 62);
  assert.equal(stepKey(71, 1, cMajor), 72);
  assert.equal(stepKey(127, 1, cMajor), 127);
  assert.equal(stepKey(60, 1, null), 61);
});

test("points de changement dans une fenêtre", () => {
  assert.deepEqual(breakpoints(track, 0.5, 2.5), [0.5, 1, 2]);
  assert.deepEqual(breakpoints({}, 0, 4), [0]);
});
