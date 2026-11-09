import test from "node:test";
import assert from "node:assert/strict";
import { autoScale, SCALE_STEPS } from "../js/ui-scale.js";

test("autoScale : 70 % sur l'écran de référence 1920×1080", () => {
  assert.equal(autoScale(1920, 1080), 70);
});

test("autoScale : suit le côté le plus limitant", () => {
  assert.equal(autoScale(2560, 1440), 90); // 93 % → cran 90
  assert.equal(autoScale(3840, 1080), 70); // hauteur limitante
  assert.equal(autoScale(1280, 1440), 60); // 46,7 % → plancher
});

test("autoScale : bornes et valeurs invalides", () => {
  assert.equal(autoScale(1366, 768), 60);
  assert.equal(autoScale(7680, 4320), 125);
  assert.equal(autoScale(0, 0), 70);
  assert.equal(autoScale(NaN, 1080), 70);
  for (const [w, h] of [[800, 600], [1920, 1080], [3000, 2000], [10000, 10000]]) assert.ok(SCALE_STEPS.includes(autoScale(w, h)));
});
