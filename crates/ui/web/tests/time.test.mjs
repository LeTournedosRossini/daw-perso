import test from "node:test";
import assert from "node:assert/strict";
import { TempoMap, MeterMap, zoomAt, clampView, sampleToX, xToSample, gridLines, formatBBT, formatTime, formatDb } from "../js/time.js";

const SR = 48_000;
const tempo120 = () => new TempoMap([{ pos: [0, 1], num: 120, den: 1 }], SR);
const four4 = () => new MeterMap([{ bar: 0, num: 4, den: 4 }]);

test("tempo constant : une ronde = 2 s à 120", () => {
  const t = tempo120();
  assert.equal(t.samplesAt(1), 96_000);
  assert.equal(t.wholeAt(48_000), 0.5);
  assert.equal(t.bpmAt(0), 120);
});

test("changements de tempo : cumul et conversion inverse", () => {
  const t = new TempoMap([{ pos: [0, 1], num: 120, den: 1 }, { pos: [1, 1], num: 60, den: 1 }], SR);
  assert.equal(t.samplesAt(1), 96_000);
  assert.equal(t.samplesAt(2), 96_000 + 192_000);
  assert.equal(t.wholeAt(96_000 + 48_000), 1.25);
  assert.equal(t.bpmAt(100_000), 60);
  for (const s of [0, 1, 95_999, 96_000, 200_000, 500_000]) assert.ok(Math.abs(t.samplesAt(t.wholeAt(s)) - s) < 1e-6);
});

test("chiffrages : début des mesures et recherche", () => {
  const m = new MeterMap([{ bar: 0, num: 4, den: 4 }, { bar: 2, num: 7, den: 8 }]);
  assert.equal(m.barStart(0), 0);
  assert.equal(m.barStart(2), 2);
  assert.equal(m.barStart(3), 2.875);
  assert.deepEqual(m.locate(2.9), { bar: 3, within: 2.9 - 2.875 });
  assert.equal(m.locate(0).bar, 0);
  assert.equal(m.locate(1.99).bar, 1);
});

test("zoom : l'échantillon sous le curseur ne bouge pas", () => {
  const vp = { start: 10_000, spp: 100 };
  const z = zoomAt(vp, 300, 0.5, { minSpp: 1, maxSpp: 1e6 });
  assert.equal(z.spp, 50);
  assert.ok(Math.abs(xToSample(z, 300) - xToSample(vp, 300)) < 1e-9);
  // Bornes.
  assert.equal(zoomAt(vp, 0, 1e-9, { minSpp: 1, maxSpp: 1e6 }).spp, 1);
  assert.equal(zoomAt(vp, 0, 1e9, { minSpp: 1, maxSpp: 1e6 }).spp, 1e6);
  assert.equal(zoomAt({ start: 0, spp: 100 }, 500, 2, { minSpp: 1, maxSpp: 1e6 }).start, 0, "jamais de début négatif");
  assert.equal(sampleToX({ start: 1000, spp: 10 }, 1500), 50);
});

test("clampView : pas de début négatif, pas trop loin après la fin", () => {
  assert.equal(clampView({ start: -50, spp: 10 }, 800, 1e6).start, 0);
  assert.equal(clampView({ start: 5e6, spp: 10 }, 800, 1e6).start, 1e6 - 800 * 10 * 0.25);
});

test("grille : barres, temps et subdivisions selon l'espace", () => {
  const t = tempo120();
  const m = four4();
  // 96 000 échantillons par mesure ; à 100 échantillons par pixel : 960 px par mesure, 240 par temps.
  const lines = gridLines({ start: 0, spp: 100 }, 1000, t, m);
  const bars = lines.filter((l) => l.kind === "bar");
  assert.deepEqual(bars.map((b) => [b.x, b.label]), [[0, "1"], [960, "2"]]);
  assert.equal(lines.filter((l) => l.kind === "beat").length, 3, "3 temps visibles dans la première mesure (la seconde commence à 960 px)");
  assert.ok(lines.some((l) => l.kind === "sub"));
  // Très dézoomé : plus de temps ni de subdivisions, étiquettes espacées.
  const far = gridLines({ start: 0, spp: 4000 }, 1000, t, m);
  assert.ok(far.every((l) => l.kind === "bar"));
  const labels = far.filter((l) => l.label).map((l) => l.x);
  for (let i = 1; i < labels.length; i++) assert.ok(labels[i] - labels[i - 1] >= 44, "étiquettes non chevauchantes");
  // Une grille partielle (défilement) ne contient que des lignes visibles.
  for (const l of gridLines({ start: 123_456, spp: 50 }, 700, t, m)) assert.ok(l.x >= -1 && l.x <= 701);
});

test("grille : mesure en 7/8 après un changement de chiffrage", () => {
  const m = new MeterMap([{ bar: 0, num: 4, den: 4 }, { bar: 1, num: 7, den: 8 }]);
  const lines = gridLines({ start: 0, spp: 150 }, 2000, tempo120(), m);
  const bar2 = lines.filter((l) => l.kind === "bar")[1];
  assert.equal(bar2.x, 96_000 / 150);
  const beatsInBar2 = lines.filter((l) => l.kind === "beat" && l.x > bar2.x && l.x < bar2.x + (0.875 * 96_000) / 150);
  assert.equal(beatsInBar2.length, 6, "7/8 : six temps après le premier");
});

test("affichage : mesure.temps.tick, temps, décibels", () => {
  const t = tempo120();
  const m = four4();
  assert.equal(formatBBT(0, t, m), "1.1.000");
  assert.equal(formatBBT(24_000, t, m), "1.2.000");
  assert.equal(formatBBT(96_000 + 12_000, t, m), "2.1.480");
  assert.equal(formatBBT(96_000 * 3 + 72_000, t, m), "4.4.000");
  assert.equal(formatTime(0, SR), "00:00.000");
  assert.equal(formatTime(SR * 61.5, SR), "01:01.500");
  assert.equal(formatDb(-Infinity), "-∞");
  assert.equal(formatDb(-61), "-∞");
  assert.equal(formatDb(0), "+0.0");
  assert.equal(formatDb(-6.02), "-6.0");
});
