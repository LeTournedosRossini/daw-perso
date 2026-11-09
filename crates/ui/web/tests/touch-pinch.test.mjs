import test from "node:test";
import assert from "node:assert/strict";
import { horizontalFactor, TouchPinch } from "../js/touch-pinch.js";

test("facteur horizontal : rapport des écarts, 1 si les doigts sont trop proches, borné", () => {
  const p = (x, y = 0) => ({ x, y });
  assert.equal(horizontalFactor(p(100), p(200), p(50), p(250)), 2);
  assert.equal(horizontalFactor(p(100), p(110), p(50), p(250)), 1, "écart de départ trop petit");
  assert.equal(horizontalFactor(p(0), p(30), p(0), p(1000)), 5);
  assert.equal(horizontalFactor(p(100), p(200), p(100), p(110)), 1, "écart d'arrivée trop petit");
});

/** Faux élément : garde les écouteurs et leur ordre ; envoie des événements de doigts (et de souris). */
function fakeEl() {
  const l = { pointerdown: [], pointermove: [], pointerup: [], pointercancel: [] };
  return {
    addEventListener: (t, f) => l[t].push(f),
    fire(t, id, x, y, pointerType = "touch") {
      const e = { pointerType, pointerId: id, clientX: x, clientY: y, stopped: false, prevented: false, stopImmediatePropagation() { this.stopped = true; }, preventDefault() { this.prevented = true; } };
      for (const f of l[t]) f(e);
      return e;
    },
  };
}

test("un doigt ne fait rien ; le deuxième abandonne le geste en cours et mange les événements jusqu'à ce que tous soient levés", () => {
  const el = fakeEl();
  const log = [];
  new TouchPinch(el, { onStart: () => log.push("start"), onMove: (m) => log.push(["move", m.factor, m.dx]), onEnd: () => log.push("end") });
  assert.equal(el.fire("pointerdown", 1, 100, 50).stopped, false, "un seul doigt : l'élément garde la main");
  assert.equal(el.fire("pointermove", 1, 101, 50).stopped, false);
  const d2 = el.fire("pointerdown", 2, 200, 50);
  assert.ok(d2.stopped && d2.prevented);
  assert.deepEqual(log, ["start"]);
  el.fire("pointermove", 1, 50, 50);
  el.fire("pointermove", 2, 250, 50);
  const moves = log.filter((x) => x[0] === "move");
  assert.ok(moves.length >= 1 && moves.at(-1)[1] > 1, "zoom avant quand les doigts s'écartent");
  // Un doigt se lève : celui qui reste ne rend pas la main (il ne doit pas déplacer un clip).
  assert.ok(el.fire("pointerup", 2, 250, 50).stopped);
  assert.ok(el.fire("pointermove", 1, 60, 60).stopped);
  assert.deepEqual(log.filter((x) => x === "end"), []);
  assert.ok(el.fire("pointerup", 1, 60, 60).stopped);
  assert.deepEqual(log.at(-1), "end");
  // Le geste suivant à un doigt est de nouveau normal.
  assert.equal(el.fire("pointerdown", 3, 10, 10).stopped, false);
  el.fire("pointerup", 3, 10, 10);
});

test("la souris ne passe pas par là", () => {
  const el = fakeEl();
  new TouchPinch(el, { onStart: () => assert.fail("pas de pincement à la souris") });
  assert.equal(el.fire("pointerdown", 1, 0, 0, "mouse").stopped, false);
  assert.equal(el.fire("pointerdown", 2, 5, 5, "mouse").stopped, false);
});

import { verticalFactor } from "../js/touch-pinch.js";

test("facteur vertical et sens du geste donnés avec le mouvement", () => {
  const p = (x, y = 0) => ({ x, y });
  assert.equal(verticalFactor(p(0, 100), p(0, 200), p(0, 50), p(0, 250)), 2);
  assert.equal(verticalFactor(p(0, 100), p(0, 110), p(0, 50), p(0, 250)), 1);
  const el = fakeEl();
  const moves = [];
  new TouchPinch(el, { onMove: (m) => moves.push(m) });
  el.fire("pointerdown", 1, 100, 100);
  el.fire("pointerdown", 2, 100, 200);
  el.fire("pointermove", 1, 100, 95); // 10 px d'écart en tout : pas encore décidé
  el.fire("pointermove", 2, 100, 205);
  assert.equal(moves.at(-1).mode, null);
  for (let i = 0; i < 6; i++) {
    el.fire("pointermove", 1, 100 + (i % 2), 95 - i * 5);
    el.fire("pointermove", 2, 100 - (i % 2), 205 + i * 5);
  }
  assert.equal(moves.at(-1).mode, "y", "geste plutôt vertical, décidé une fois pour le geste");
  assert.ok(moves.at(-1).fy > 1);
});
