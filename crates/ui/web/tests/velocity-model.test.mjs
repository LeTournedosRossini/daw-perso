import test from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_CURVE, describeCurve, hitHandle, mapVelocity, normalizeCurve, yToVelocity } from "../js/velocity-model.js";
import { validateCommand } from "../js/protocol.js";

const on = (extra = {}) => ({ ...DEFAULT_CURVE, enabled: true, ...extra });

test("la droite par défaut laisse passer toutes les vélocités ; les bornes fixent la plage", () => {
  for (let v = 1; v <= 127; v++) assert.equal(mapVelocity(on(), v), v);
  const c = on({ min: 40, max: 100 });
  assert.deepEqual([mapVelocity(c, 1), mapVelocity(c, 64), mapVelocity(c, 127)], [40, 70, 100]);
});

test("convexe = plus bas, concave = plus haut, jamais 0 ni de recul", () => {
  assert.ok(mapVelocity(on({ bend: 1000 }), 64) < 64);
  assert.ok(mapVelocity(on({ bend: -1000 }), 64) > 64);
  for (const bend of [-1000, 0, 1000]) {
    let last = 0;
    for (let v = 1; v <= 127; v++) {
      const o = mapVelocity(on({ bend, smooth: 500, min: 20, max: 120 }), v);
      assert.ok(o >= 1 && o >= last);
      last = o;
    }
  }
});

test("vélocité fixe, poignées au même niveau, résumé et normalisation", () => {
  assert.equal(mapVelocity(on({ fixed: 90 }), 3), 90);
  assert.equal(mapVelocity(on({ min: 64, max: 64 }), 120), 64);
  assert.equal(describeCurve(DEFAULT_CURVE), "désactivée");
  assert.equal(describeCurve(on({ fixed: 90 })), "vélocité fixe 90");
  assert.equal(describeCurve(on({ bend: 200, min: 30, max: 110 })), "convexe · 30 à 110");
  assert.deepEqual(normalizeCurve({ enabled: 1, bend: 5000, smooth: -3, min: 0, max: 400, fixed: 999 }), { enabled: true, bend: 1000, smooth: 0, min: 1, max: 127, fixed: 127 });
});

test("les poignées se saisissent sur les bords du tracé, et la hauteur redevient une vélocité", () => {
  const c = on({ min: 1, max: 127 });
  assert.equal(hitHandle(c, 200, 3, 198), "min");
  assert.equal(hitHandle(c, 200, 198, 2), "max");
  assert.equal(hitHandle(c, 200, 100, 100), null);
  assert.equal(yToVelocity(200, 200), 1);
  assert.equal(yToVelocity(200, 0), 127);
  assert.equal(yToVelocity(200, 100), 64);
});

test("validation de set_track_velocity_curve", () => {
  const ok = { type: "set_track_velocity_curve", node: 3, curve: on({ bend: 300, min: 30, max: 110 }) };
  assert.equal(validateCommand(ok), null);
  for (const bad of [{ node: 0 }, { curve: on({ min: 0 }) }, { curve: on({ bend: 1001 }) }, { curve: on({ smooth: 1001 }) }, { curve: on({ fixed: 128 }) }, { curve: { min: 1, max: 127 } }]) {
    assert.notEqual(validateCommand({ ...ok, ...bad }), null, JSON.stringify(bad));
  }
});
