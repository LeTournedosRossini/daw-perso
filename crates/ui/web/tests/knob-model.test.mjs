import test from "node:test";
import assert from "node:assert/strict";
import { angleOf, arcPath, arcSpan, dragValue, fraction, keyValue, snap, wheelValue } from "../js/knob-model.js";

const gain = { min: -60, max: 6, step: 0.1, travel: 180 };

test("angle : minimum à gauche, milieu en haut, maximum à droite", () => {
  assert.equal(angleOf(-1, -1, 1), -135);
  assert.equal(angleOf(0, -1, 1), 0);
  assert.equal(angleOf(1, -1, 1), 135);
  assert.equal(angleOf(5, -1, 1), 135, "hors plage : borné");
  assert.equal(fraction(0, 0, 0), 0, "plage nulle");
});

test("glisser est relatif et borné", () => {
  assert.equal(dragValue(-10, 0, gain), -10, "pas de saut au premier pixel");
  const v = dragValue(-10, 20, gain);
  assert.ok(v > -10 && v < 6);
  assert.equal(dragValue(0, 10_000, gain), 6);
  assert.equal(dragValue(0, -10_000, gain), -60);
});

test("réglage fin : cinq fois plus lent", () => {
  const coarse = dragValue(-20, 20, gain) - -20;
  const fine = dragValue(-20, 20, { ...gain, step: 0, fine: true }) - -20;
  assert.ok(Math.abs(coarse / fine - 5) < 0.2, `rapport ${coarse / fine}`);
});

test("le pas arrondit sans dépasser les bornes", () => {
  assert.equal(snap(0.04, -1, 1, 0.1), 0);
  assert.equal(snap(0.06, -1, 1, 0.1), 0.1);
  assert.equal(snap(-2, -1, 1, 0.1), -1);
  assert.equal(snap(0.123, -1, 1, 0), 0.123);
});

test("clavier et molette", () => {
  const o = { min: -1, max: 1, step: 0.01 };
  assert.equal(keyValue("ArrowUp", 0, o), 0.01);
  assert.equal(keyValue("ArrowDown", 0, { ...o, shift: true }), -0.1);
  assert.equal(keyValue("Home", 0.3, o), -1);
  assert.equal(keyValue("End", 0.3, o), 1);
  assert.equal(keyValue("x", 0, o), null);
  assert.equal(keyValue("ArrowUp", 1, o), 1, "borné");
  assert.equal(wheelValue(0, -120, o), 0.01, "vers le haut : augmente");
  assert.equal(wheelValue(0, 120, o), -0.01);
  assert.equal(wheelValue(0.5, 0, o), 0.5);
});

test("arc de valeur : depuis le centre pour un panoramique, depuis le minimum pour un volume", () => {
  const pan = arcSpan(-0.5, 0, -1, 1);
  assert.deepEqual([pan.a0, pan.a1], [-67.5, 0]);
  const vol = arcSpan(6, -60, -60, 6);
  assert.deepEqual([vol.a0, vol.a1], [-135, 135]);
  assert.equal(arcPath(14, 14, 11, 10, 10.2), "", "arc quasi nul : rien à tracer");
  assert.match(arcPath(14, 14, 11, -135, 135), /^M[\d. -]+A11 11 0 1 1 /);
});
