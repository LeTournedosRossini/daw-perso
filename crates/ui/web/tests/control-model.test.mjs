import test from "node:test";
import assert from "node:assert/strict";
import { LANE_DEFS, laneById, eventsOf, valueOfY, yOfValue, withPoint, withoutPoints, nearestPoint, laneCommand, pointId, valueText, isVelocity, copyPoints, pastePoints, thinPoints } from "../js/control-model.js";
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

const P = (n, d, value, channel = 0) => ({ start: [n, d], channel, value });
const cc1 = laneById("cc1");
const bend = laneById("bend");

test("copier garde les écarts exacts et trie ; sans sélection : toute la voie", () => {
  const list = [P(1, 2, 80), P(1, 4, 20), P(3, 4, 100)];
  const c = copyPoints(list, cc1, new Set([pointId(list[0]), pointId(list[2])]));
  assert.deepEqual(c.points.map((q) => q.dt), [[0, 1], [1, 4]]);
  assert.equal(copyPoints(list, cc1, new Set()).points.length, 3);
  assert.equal(copyPoints([], cc1, null), null);
});

test("coller : premier point à l'instant voulu, remplace au même instant, laisse de côté ce qui sort du clip", () => {
  const clip = copyPoints([P(0, 1, 10), P(1, 4, 50), P(1, 2, 90)], cc1, null);
  const r = pastePoints([P(1, 1, 5), P(5, 4, 77)], cc1, clip, [1, 1], [0, 1], [3, 2]);
  assert.deepEqual(r.added.map((p) => p.start), [[1, 1], [5, 4]]); // le 3e (3/2) est hors du clip
  assert.deepEqual(r.list.map((p) => `${p.start[0]}/${p.start[1]}=${p.value}`), ["1/1=10", "5/4=50"]); // 1/1 et 5/4 remplacés
});

test("coller dans une autre voie garde la hauteur relative", () => {
  const clip = copyPoints([P(0, 1, 0), P(1, 4, 127)], cc1, null);
  const r = pastePoints([], bend, clip, [0, 1], [0, 1], [1, 1]);
  assert.deepEqual(r.added.map((p) => p.value), [-8192, 8191]);
});

test("amincir : les répétitions partent sans perte, une droite devient deux points, un coin reste", () => {
  const ramp = Array.from({ length: 21 }, (_, i) => P(i, 16, i * 5)); // 0 → 100 en ligne droite
  assert.deepEqual(thinPoints(ramp, cc1, null).map((p) => p.value), [0, 100]);
  const flat = [P(0, 1, 7), P(1, 8, 7), P(2, 8, 7), P(3, 8, 9)];
  assert.deepEqual(thinPoints(flat, cc1, null).map((p) => p.value), [7, 9]); // doublons retirés, dernier gardé
  const corner = [P(0, 1, 0), P(1, 8, 50), P(2, 8, 100), P(3, 8, 50), P(4, 8, 0)];
  assert.equal(thinPoints(corner, cc1, null).length, 3); // 0, 100, 0
  assert.equal(thinPoints(corner.slice(0, 2), cc1, null).length, 2); // trop court : intact
});

test("amincir une voie en marches ne retire que les répétitions ; la sélection protège le reste", () => {
  const pedal = [P(0, 1, 0), P(1, 8, 0), P(2, 8, 127), P(3, 8, 127), P(4, 8, 0)];
  const lane = laneById("cc64");
  assert.deepEqual(thinPoints(pedal, lane, null).map((p) => p.value), [0, 127, 0]);
  const ramp = Array.from({ length: 11 }, (_, i) => P(i, 16, i * 10));
  const ids = new Set(ramp.slice(0, 5).map(pointId)); // seulement les 5 premiers
  const out = thinPoints(ramp, cc1, ids);
  assert.equal(out.length, 2 + 6); // 0 et 40 gardés, les 6 autres intacts
});
