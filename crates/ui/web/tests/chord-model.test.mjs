import test from "node:test";
import assert from "node:assert/strict";
import { parsePitch, pitchName, parseChordText, chordLabel, scaleLayers, snapWhole, moveItem, resizeItem, newChordLength, resolveQuality, normalizePitch } from "../js/chord-model.js";

test("notes : écriture conservée, doubles altérations, refus", () => {
  assert.deepEqual(parsePitch("Bb"), { pc: 10, sharp: false });
  assert.deepEqual(parsePitch("F#"), { pc: 6, sharp: true });
  assert.deepEqual(parsePitch("Ebb"), { pc: 2, sharp: false });
  assert.equal(pitchName({ pc: 6, sharp: false }), "Gb");
  assert.equal(parsePitch("H"), null);
  assert.equal(normalizePitch("bb"), "Bb");
  assert.equal(normalizePitch("c♯"), "C#");
});

test("accords saisis : racine, qualité telle que saisie, basse, pas d'accord, qualité libre", () => {
  assert.deepEqual(parseChordText("Dm7"), { root: "D", quality: "m7", bass: null });
  assert.deepEqual(parseChordText("G7/B"), { root: "G", quality: "7", bass: "B" });
  assert.deepEqual(parseChordText("Bb"), { root: "Bb", quality: "", bass: null });
  assert.deepEqual(parseChordText("C6/9"), { root: "C", quality: "6/9", bass: null }, "6/9 n'est pas une basse");
  assert.deepEqual(parseChordText("cmaj7/e"), { root: "C", quality: "maj7", bass: "E" });
  assert.deepEqual(parseChordText("C[0,4,7,13]"), { root: "C", quality: "[0,4,7,13]", bass: null });
  assert.deepEqual(parseChordText("N.C."), { nc: true });
  assert.equal(parseChordText("Zm"), null);
  assert.equal(chordLabel({ root: "G", quality: "7", bass: "B" }), "G7/B");
  assert.equal(chordLabel({ nc: true }), "N.C.");
});

test("gammes qui se chevauchent : rangées en couches pour l'affichage", () => {
  const s = (a, d) => ({ at: [a, 1], dur: [d, 1] });
  assert.deepEqual(scaleLayers([s(0, 4), s(1, 1), s(2, 4), s(4, 1), s(5, 1)]), [0, 1, 1, 0, 0]);
});

test("aimantation et édition : déplacer, redimensionner sans durée nulle", () => {
  assert.deepEqual(snapWhole(0.3, [1, 4]), [1, 4]);
  assert.deepEqual(snapWhole(0.37, [1, 4]), [1, 4]);
  assert.deepEqual(snapWhole(0.4, [1, 8]), [3, 8]);
  assert.deepEqual(snapWhole(-1, [1, 4]), [0, 1], "jamais avant le début");
  const chords = [{ at: [0, 1], dur: [1, 4] }, { at: [1, 2], dur: [1, 4] }];
  const moved = moveItem(chords, 0, [3, 4]);
  assert.deepEqual(moved.map((c) => c.at), [[1, 2], [3, 4]], "retrié par début");
  const r = resizeItem(chords, 0, "end", [3, 8]);
  assert.deepEqual(r[0].dur, [3, 8]);
  assert.equal(resizeItem(chords, 0, "end", [0, 1]), null, "durée nulle refusée");
  assert.deepEqual(resizeItem(chords, 1, "start", [1, 4])[0].at, [0, 1]);
  assert.deepEqual(resizeItem(chords, 1, "start", [1, 4]).find((c) => c.at[0] === 1 && c.at[1] === 4).dur, [1, 2]);
});

test("nouvel accord : une ronde, ou jusqu'au prochain ; refusé dans un accord", () => {
  const chords = [{ at: [1, 1], dur: [1, 2] }, { at: [5, 2], dur: [1, 1] }];
  assert.deepEqual(newChordLength(chords, [0, 1]), [1, 1], "jusqu'au prochain accord à la ronde 1 : exactement une ronde");
  assert.deepEqual(newChordLength(chords, [3, 2]), [1, 1], "juste après la fin d'un accord (fin exclue)");
  assert.deepEqual(newChordLength(chords, [2, 1]), [1, 2], "il reste une demi-ronde avant l'accord de 5/2");
  assert.equal(newChordLength(chords, [5, 4]), null, "dans un accord");
});

test("qualités : identifiant, alias, qualité libre normalisée, inconnue", () => {
  const catalog = { qualities: [["", [0, 4, 7]], ["M7", [0, 4, 7, 11]], ["m7", [0, 3, 7, 10]]], aliases: [["maj7", "M7"], ["Δ", "M7"]] };
  assert.equal(resolveQuality("m7", catalog), "m7");
  assert.equal(resolveQuality("maj7", catalog), "M7");
  assert.equal(resolveQuality("", catalog), "");
  assert.equal(resolveQuality("[0, 7, 4,4]", catalog), "[0,4,7]");
  assert.equal(resolveQuality("[0,24]", catalog), null);
  assert.equal(resolveQuality("2", catalog), null);
  assert.equal(resolveQuality("x", null), "x", "sans catalogue (pas encore reçu) : l'hôte tranchera");
});
