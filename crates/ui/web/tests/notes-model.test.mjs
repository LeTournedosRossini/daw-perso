import test from "node:test";
import assert from "node:assert/strict";
import { pinchFactors, pinchMode, fadd, fsub, fcmp, snapAbs, gridById, matToAbs, absToMat, moveGroup, resizeGroup, diffEdits, keyName, noteId } from "../js/notes-model.js";

const clip = { start: [1, 1], len: [1, 1], offset: [0, 1] };
const note = (s, l, key) => ({ start: s, len: l, key, velocity: 0.8, channel: 0 });

test("fractions exactes et aimantation", () => {
  assert.deepEqual(fadd([1, 8], [1, 8]), [1, 4]);
  assert.deepEqual(fsub([1, 4], [1, 3]), [-1, 12]);
  assert.ok(fcmp([1, 3], [1, 4]) > 0);
  const g16 = gridById("1/16").f;
  assert.deepEqual(snapAbs(0.30, g16), [5, 16]); // 0,30 → 5/16 = 0,3125
  assert.deepEqual(snapAbs(0.30, g16, "floor"), [1, 4]);
  assert.deepEqual(snapAbs(0.34, gridById("1/8T").f), [1, 3]); // triolet de croches : 1/12 par pas
  assert.deepEqual(snapAbs(-1, g16), [0, 1]);
});

test("matière ↔ position absolue tiennent compte du début et du décalage du clip", () => {
  const c = { start: [2, 1], len: [1, 1], offset: [1, 4] };
  assert.deepEqual(matToAbs(c, [1, 4]), [2, 1]);
  assert.deepEqual(absToMat(c, [9, 4]), [1, 2]);
});

test("déplacement de groupe : décalage exact commun, touches bornées, débuts dans le clip", () => {
  const g = gridById("1/8").f;
  const a = note([0, 1], [1, 8], 60);
  const b = note([1, 4], [1, 8], 64);
  // On saisit `a` (abs 1) et on le traîne à abs 1,26 (→ 1 + 2/8) et une touche plus haut.
  const moved = moveGroup(clip, [a, b], a, 1.26, 61, g);
  assert.deepEqual(moved.map((n) => n.start), [[1, 4], [1, 2]]);
  assert.deepEqual(moved.map((n) => n.key), [61, 65]);
  // Trop à gauche : borné au début du clip ; trop haut : borné à 127.
  const left = moveGroup(clip, [a, b], a, 0.2, 60, g);
  assert.deepEqual(left[0].start, [0, 1]);
  const up = moveGroup(clip, [note([0, 1], [1, 8], 126)], note([0, 1], [1, 8], 126), 1, 140, g);
  assert.equal(up[0].key, 127);
  // Trop à droite : le début reste dans le clip (< fin).
  const right = moveGroup(clip, [a], a, 5, 60, g);
  assert.ok(fcmp(right[0].start, [1, 1]) < 0);
});

test("redimensionnement de groupe : même variation de durée, minimum un pas de grille", () => {
  const g = gridById("1/8").f;
  const a = note([0, 1], [1, 4], 60);
  const b = note([1, 2], [1, 8], 62);
  // La fin de `a` (abs 1 + 1/4) est tirée jusqu'à abs 1 + 1/2 : +1/4 pour chacune.
  const r = resizeGroup(clip, [a, b], a, 1.5, g);
  assert.deepEqual(r.map((n) => n.len), [[1, 2], [3, 8]]);
  // Réduction : la plus courte du groupe limite (jamais sous 1/8) ; ici `b` est déjà au minimum, donc rien ne change.
  const s = resizeGroup(clip, [a, b], a, 1.0, g);
  assert.deepEqual(s.map((n) => n.len), [[1, 4], [1, 8]]);
  // Une seule note : réduite jusqu'au minimum.
  assert.deepEqual(resizeGroup(clip, [a], a, 1.0, g)[0].len, [1, 8]);
});

test("modifications à envoyer : seulement ce qui a changé", () => {
  const a = note([0, 1], [1, 8], 60);
  const b = { ...a, key: 62 };
  const c = { ...a, velocity: 0.5 };
  assert.deepEqual(diffEdits([3, 7, 9], [a, a, a], [a, b, c]), [{ index: 7, key: 62 }, { index: 9, velocity: 0.5 }]);
});

test("noms de touches et identité d'une note", () => {
  assert.equal(keyName(60), "Do4");
  assert.equal(keyName(69), "La4");
  assert.equal(noteId({ start: [1, 8], key: 60, channel: 0 }), "1/8:60:0");
});

test("pincement : zoom par axe, axe ignoré quand les doigts sont alignés, déplacement du milieu", () => {
  const p = (x, y) => ({ x, y });
  // Écartement horizontal de 100 à 200 px, doigts à la même hauteur : zoom horizontal ×2, vertical inchangé.
  const h = pinchFactors(p(100, 50), p(200, 50), p(50, 50), p(250, 50));
  assert.equal(h.fx, 2);
  assert.equal(h.fy, 1);
  // Écartement vertical seul.
  const v = pinchFactors(p(100, 50), p(100, 150), p(100, 20), p(100, 230));
  assert.equal(v.fx, 1);
  assert.equal(v.fy, 2.1);
  // Diagonale : les deux axes.
  const d = pinchFactors(p(100, 100), p(200, 200), p(75, 75), p(225, 225));
  assert.deepEqual([d.fx, d.fy], [1.5, 1.5]);
  // Déplacement du milieu (deux doigts qui glissent ensemble) sans zoom.
  const m = pinchFactors(p(100, 100), p(200, 200), p(110, 130), p(210, 230));
  assert.deepEqual([m.fx, m.fy, m.dx, m.dy], [1, 1, 10, 30]);
  // Bornes : jamais plus de ×5 ni moins de ×0,2.
  assert.equal(pinchFactors(p(0, 0), p(30, 0), p(0, 0), p(1000, 0)).fx, 5);
});

test("sens d'un pincement : décidé sur le mouvement cumulé, pas sur le bruit", () => {
  const p = (x, y) => ({ x, y });
  // Pas encore décidé tant que l'écart a peu changé.
  assert.equal(pinchMode(p(100, 100), p(200, 100), p(98, 101), p(204, 99)), null);
  // Surtout horizontal (les doigts dérivent un peu en hauteur) / nettement vertical / diagonale.
  assert.equal(pinchMode(p(100, 100), p(200, 100), p(60, 95), p(240, 112)), "x");
  assert.equal(pinchMode(p(100, 100), p(100, 200), p(95, 60), p(105, 240)), "y");
  assert.equal(pinchMode(p(100, 100), p(200, 200), p(70, 70), p(230, 230)), "x", "diagonale : horizontal par défaut");
  assert.equal(pinchMode(p(100, 100), p(130, 200), p(100, 60), p(130, 240)), "y", "nettement vertical");
  assert.equal(pinchMode(p(100, 100), p(160, 160), p(85, 70), p(175, 190)), "y", "plutôt vertical : un doigt qui dérive un peu ne suffit pas à en faire un geste horizontal");
  assert.equal(pinchMode(p(100, 100), p(160, 160), p(60, 90), p(200, 190)), "x", "plutôt horizontal");
  // Un geste lent vertical avec un tremblement horizontal d'un pixel par événement : une fois cumulé il reste « vertical ».
  let a = p(100, 100);
  let b = p(100, 200);
  const start = [a, b];
  for (let i = 0; i < 30; i++) {
    a = p(100 + (i % 2), a.y - 0.5);
    b = p(100 - (i % 2), b.y + 0.5);
  }
  assert.equal(pinchMode(...start, a, b), "y");
});
