import test from "node:test";
import assert from "node:assert/strict";
import { rangeFor, valueToFrac, fracToValue, toPos, insertPoint, movePoint, removePoint, toggleCurve, valueAtWhole, targetKey, sameTarget, targetLabel, valueText } from "../js/automation-model.js";

const P = (n, d, value, curve = "linear") => ({ pos: [n, d], value, curve });

test("plages : dB pour le gain et les envois, linéaire pour le pan et les plugins", () => {
  const db = rangeFor({ kind: "gain" });
  assert.equal(valueToFrac(db, 1), 60 / 66); // 0 dB
  assert.equal(valueToFrac(db, 0), 0);
  assert.ok(Math.abs(fracToValue(db, 60 / 66) - 1) < 1e-9);
  assert.equal(fracToValue(db, 0), 0, "le bas de la plage coupe");
  assert.equal(valueToFrac(rangeFor({ kind: "pan" }), 0), 0.5);
  assert.equal(fracToValue(rangeFor({ kind: "pan" }), 1), 1);
  const plug = rangeFor({ kind: "insert", insert: 3, param: 4 }, { min: 20, max: 20000, default: 1000 });
  assert.equal(fracToValue(plug, 0.5), 10010);
  assert.equal(valueToFrac(plug, 20000), 1);
});

test("positions : grille fine de 1/3 840 de ronde", () => {
  assert.deepEqual(toPos(0.25), [1, 4]);
  assert.deepEqual(toPos(1 / 3), [1280, 3840].map((x, i) => (i ? 3 : 1)).slice(0, 2).length === 2 ? [1, 3] : [1, 3]);
  assert.deepEqual(toPos(-5), [0, 1]);
});

test("édition de points : insérer, déplacer entre les voisins, supprimer, palier", () => {
  let pts = [P(0, 1, 0.5), P(1, 1, 1.0)];
  const ins = insertPoint(pts, [1, 2], 0.2);
  assert.deepEqual(ins.points.map((p) => p.value), [0.5, 0.2, 1.0]);
  assert.equal(ins.index, 1);
  // Même position : remplace.
  assert.equal(insertPoint(ins.points, [1, 2], 0.9).points.length, 3);
  // Déplacer au-delà du voisin : bloqué à un tick près ; en dessous de zéro : 0.
  const moved = movePoint(ins.points, 1, [5, 1], 0.3);
  assert.deepEqual(moved.points[1].pos, [3839, 3840]);
  assert.equal(moved.points[1].value, 0.3);
  assert.deepEqual(movePoint(ins.points, 0, [0, 1], 0.7).points[0].pos, [0, 1]);
  assert.deepEqual(movePoint(ins.points, 2, [1, 2], 0.7).points[2].pos, [1921, 3840], "ne peut pas atteindre le voisin");
  assert.equal(removePoint(ins.points, 1).length, 2);
  assert.equal(toggleCurve(pts, 0)[0].curve, "step");
  assert.equal(toggleCurve(toggleCurve(pts, 0), 0)[0].curve, "linear");
});

test("valeur de la courbe : avant, entre (linéaire et palier), après", () => {
  const pts = [P(0, 1, 0), P(1, 1, 1, "step"), P(2, 1, 0.5)];
  assert.equal(valueAtWhole(pts, -1), 0);
  assert.equal(valueAtWhole(pts, 0.5), 0.5);
  assert.equal(valueAtWhole(pts, 1.5), 1, "palier : garde la valeur du point");
  assert.equal(valueAtWhole(pts, 5), 0.5);
  assert.equal(valueAtWhole([], 1), null);
});

test("cibles : clés, libellés, textes de valeur", () => {
  assert.equal(targetKey({ kind: "send", to: 2 }), "send:2");
  assert.ok(sameTarget({ kind: "insert", insert: 1, param: 3 }, { kind: "insert", insert: 1, param: 3 }));
  assert.ok(!sameTarget({ kind: "gain" }, { kind: "pan" }));
  const project = { nodes: [{ id: 0, kind: "master", name: "Sortie" }, { id: 2, kind: "bus", name: "Réverbe" }] };
  const node = { inserts: [{ id: 7, name: "Compresseur" }] };
  assert.equal(targetLabel(node, project, { kind: "send", to: 2 }), "Envoi → Réverbe");
  assert.equal(targetLabel(node, project, { kind: "send", to: 0 }), "Envoi → Sortie");
  assert.equal(targetLabel(node, project, { kind: "insert", insert: 7, param: 0 }, "Seuil"), "Compresseur : Seuil");
  assert.equal(valueText(rangeFor({ kind: "gain" }), 1), "+0.0 dB");
  assert.equal(valueText(rangeFor({ kind: "gain" }), 0), "-∞ dB");
  assert.equal(valueText(rangeFor({ kind: "pan" }), -0.5), "50 G");
});

test("valeurs affichées : en lecture la courbe commande (le fader suit), en trim, désactivée ou sans courbe c'est le réglage manuel", async () => {
  const { shownValues } = await import("../js/automation-model.js");
  const lane = (kind, mode, v0, v1, extra = {}) => ({ target: { kind, ...extra }, mode, points: [{ pos: [0, 1], value: v0, curve: "linear" }, { pos: [1, 1], value: v1, curve: "linear" }] });
  const node = { gain: 0.9, pan: 0.2, automation: [lane("gain", "read", 0, 1), lane("pan", "trim", -1, 1), lane("send", "read", 0.5, 1.5, { to: 3 })] };
  const s = shownValues(node, 0.5);
  assert.equal(s.gain, 0.5);
  assert.ok(s.gainAuto);
  assert.equal(s.pan, 0.2, "trim : le fader garde sa valeur");
  assert.ok(!s.panAuto);
  assert.equal(s.send(3, 0.7), 1);
  assert.equal(s.send(9, 0.7), 0.7, "pas de courbe sur cet envoi");
  assert.ok(s.sendAuto(3) && !s.sendAuto(9));
  const off = shownValues({ gain: 0.9, pan: 0, automation: [lane("gain", "off", 0, 1)] }, 0.5);
  assert.equal(off.gain, 0.9);
  assert.equal(shownValues({ gain: 0.4, pan: 0 }, 1).gain, 0.4, "sans automation");
});
