import test from "node:test";
import assert from "node:assert/strict";
import { LANE_DEFS, laneById, eventsOf, valueOfY, yOfValue, withPoint, withoutPoints, nearestPoint, laneCommand, pointId, valueText, isVelocity } from "../js/control-model.js";
import { validateCommand } from "../js/protocol.js";

test("voies : définitions, contrôleur libre, inconnue = vélocité", () => {
  assert.equal(LANE_DEFS.length, 7);
  assert.equal(laneById("cc64").steps, true);
  assert.deepEqual([laneById("cc23").controller, laneById("cc23").kind], [23, "cc"]);
  assert.ok(isVelocity(laneById("cc999")) && isVelocity(laneById("n'importe quoi")));
  assert.equal(laneById("bend").min, -8192);
});

test("événements d'une voie : par type et par contrôleur", () => {
  const ev = [{ kind: "cc", controller: 1, value: 5 }, { kind: "cc", controller: 64, value: 127 }, { kind: "bend", value: 0 }, { kind: "pressure", value: 9 }, { kind: "poly", key: 60, value: 3 }];
  assert.deepEqual(eventsOf(ev, laneById("cc1")).map((e) => e.value), [5]);
  assert.equal(eventsOf(ev, laneById("bend")).length, 1);
  assert.equal(eventsOf(ev, laneById("pressure")).length, 1);
  assert.equal(eventsOf(undefined, laneById("cc1")).length, 0);
});

test("valeur ↔ position verticale : le haut est le maximum, bornes respectées, aller-retour", () => {
  const cc = laneById("cc1");
  assert.equal(valueOfY(cc, 0, 100), 127);
  assert.equal(valueOfY(cc, 100, 100), 0);
  assert.equal(valueOfY(cc, 500, 100), 0, "borné");
  for (const v of [0, 1, 64, 100, 127]) assert.equal(valueOfY(cc, yOfValue(cc, v, 120), 120), v);
  const b = laneById("bend");
  assert.equal(valueOfY(b, 0, 100), 8191);
  assert.ok(Math.abs(valueOfY(b, 50, 100)) <= 100, "le milieu est près du repos");
  for (const v of [-8192, -1000, 0, 4000, 8191]) assert.ok(Math.abs(valueOfY(b, yOfValue(b, v, 200), 200) - v) <= 90, String(v)); // 1 px = ~170 pas
});

test("points : même instant = remplacé, tri par temps, suppression par identifiant, plus proche", () => {
  let l = [];
  l = withPoint(l, { start: [1, 4], channel: 0, value: 10 });
  l = withPoint(l, { start: [1, 8], channel: 0, value: 20 });
  l = withPoint(l, { start: [1, 4], channel: 0, value: 99 });
  assert.deepEqual(l.map((p) => p.value), [20, 99]);
  assert.deepEqual(withoutPoints(l, new Set([pointId(l[0])])).map((p) => p.value), [99]);
  const xs = { "1/8": 100, "1/4": 140 };
  assert.equal(nearestPoint(l, 104, (p) => xs[`${p.start[0]}/${p.start[1]}`]), 0);
  assert.equal(nearestPoint(l, 120, (p) => xs[`${p.start[0]}/${p.start[1]}`]), -1, "trop loin");
});

test("commande set_events : valide pour le protocole, contrôleur seulement pour cc", () => {
  const cmd = laneCommand(1000012, laneById("cc1"), [{ start: [1, 8], channel: 0, value: 64 }], "Dessiner la modulation");
  assert.deepEqual(cmd, { type: "set_events", clip: 1000012, kind: "cc", events: [{ start: [1, 8], channel: 0, value: 64 }], label: "Dessiner la modulation", controller: 1 });
  assert.equal(validateCommand(cmd), null);
  const bend = laneCommand(1000012, laneById("bend"), [{ start: [1, 2], channel: 3, value: -8192 }], "x");
  assert.equal("controller" in bend, false);
  assert.equal(validateCommand(bend), null);
  assert.notEqual(validateCommand({ ...cmd, events: [{ start: [1, 8], channel: 0, value: 128 }] }), null);
  assert.notEqual(validateCommand({ ...bend, controller: 1 }), null);
  assert.notEqual(validateCommand({ ...cmd, controller: undefined }), null);
  assert.notEqual(validateCommand({ ...cmd, kind: "poly" }), null);
});

test("texte d'une valeur", () => {
  assert.equal(valueText(laneById("bend"), 2048), "+2048");
  assert.equal(valueText(laneById("cc64"), 127), "enfoncée (127)");
  assert.equal(valueText(laneById("cc1"), 64), "64");
});
