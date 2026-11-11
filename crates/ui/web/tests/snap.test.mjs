import test from "node:test";
import assert from "node:assert/strict";
import { TempoMap, MeterMap } from "../js/time.js";
import { gridSnap } from "../js/snap.js";

const SR = 48_000;
const tempo = new TempoMap([{ pos: [0, 1], num: 120, den: 1 }], SR); // une ronde = 96 000 échantillons
const meters = new MeterMap([{ bar: 0, num: 4, den: 4 }]);

test("grille de mesures : la mesure la plus proche, avant ou après", () => {
  assert.equal(gridSnap(tempo, meters, 10_000, "bar"), 0);
  assert.equal(gridSnap(tempo, meters, 90_000, "bar"), 96_000);
  assert.equal(gridSnap(tempo, meters, 150_000, "bar"), 192_000, "150 000 est plus près de la mesure 3 (192 000) que de la 2 (96 000)");
});

test("grille de temps : un temps = une noire en 4/4", () => {
  assert.equal(gridSnap(tempo, meters, 25_000, "beat"), 24_000);
  assert.equal(gridSnap(tempo, meters, 37_000, "beat"), 48_000);
});

test("grilles de croches et de doubles croches", () => {
  assert.equal(gridSnap(tempo, meters, 13_000, "1/8"), 12_000);
  assert.equal(gridSnap(tempo, meters, 7_000, "1/16"), 6_000);
  assert.equal(gridSnap(tempo, meters, 2_000, "1/32"), 3_000);
});

test("le temps suit le chiffrage : 6/8 → croche", () => {
  const m68 = new MeterMap([{ bar: 0, num: 6, den: 8 }]);
  assert.equal(gridSnap(tempo, m68, 13_000, "beat"), 12_000);
});

test("une position après le dernier temps d'une mesure s'aimante sur la mesure suivante", () => {
  assert.equal(gridSnap(tempo, meters, 95_000, "beat"), 96_000);
});

test("changement de tempo : l'aimantation suit la carte des tempos", () => {
  const t = new TempoMap([{ pos: [0, 1], num: 120, den: 1 }, { pos: [1, 1], num: 60, den: 1 }], SR); // 2e mesure deux fois plus lente
  assert.equal(gridSnap(t, meters, 96_000 + 45_000, "beat"), 96_000 + 48_000);
});
