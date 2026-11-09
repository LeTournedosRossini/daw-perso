import test from "node:test";
import assert from "node:assert/strict";
import { ROOT_CELLS, barLength, chordNotes, draftFromText, newChordLength, prettyPitch, textFromDraft } from "../js/chord-model.js";
import { MeterMap } from "../js/time.js";

test("durée par défaut : la mesure du projet à cet endroit", () => {
  const m = new MeterMap([{ bar: 0, num: 4, den: 4 }, { bar: 2, num: 3, den: 4 }, { bar: 4, num: 6, den: 8 }]);
  assert.deepEqual(barLength(m, 0), [4, 4]);
  assert.deepEqual(barLength(m, 1.9), [4, 4], "dans la 2ᵉ mesure (4/4)");
  assert.deepEqual(barLength(m, 2.0), [3, 4], "3/4 dès la mesure 3");
  assert.deepEqual(barLength(m, 3.5), [6, 8], "6/8 à partir de la mesure 5");
  assert.deepEqual(barLength(null, 5), [1, 1], "sans carte : une ronde");
  // Un accord posé avant le suivant s'arrête à lui ; sinon il prend la mesure.
  assert.deepEqual(newChordLength([], [2, 1], [3, 4]), [3, 4]);
  assert.deepEqual(newChordLength([{ at: [9, 4], dur: [1, 1] }], [2, 1], [3, 4]), [1, 4]);
  assert.equal(newChordLength([{ at: [2, 1], dur: [1, 1] }], [9, 4], [3, 4]), null);
});

test("fenêtre de saisie : brouillon ↔ texte", () => {
  assert.deepEqual(draftFromText("Dm7/G"), { nc: false, root: "D", quality: "m7", bass: "G" });
  assert.deepEqual(draftFromText("Bb"), { nc: false, root: "Bb", quality: "", bass: null });
  assert.deepEqual(draftFromText("N.C."), { nc: true, root: null, quality: "", bass: null });
  assert.equal(draftFromText("xyz"), null);
  assert.equal(textFromDraft({ nc: false, root: "F#", quality: "7", bass: "A#" }), "F#7/A#");
  assert.equal(textFromDraft({ nc: true }), "N.C.");
  assert.equal(textFromDraft({ nc: false, root: null, quality: "m", bass: null }), "", "sans fondamentale : rien à valider");
  assert.equal(textFromDraft(draftFromText("C[0,4,7,13]")), "C[0,4,7,13]", "qualité libre conservée");
});

test("12 notes, deux orthographes pour les touches noires", () => {
  assert.equal(ROOT_CELLS.length, 12);
  assert.deepEqual(ROOT_CELLS.filter((c) => c.names.length === 2).map((c) => c.pc), [1, 3, 6, 8, 10]);
  assert.equal(prettyPitch("Db"), "D♭");
  assert.equal(prettyPitch("F#"), "F♯");
  assert.equal(prettyPitch("B"), "B");
});

test("notes de l'accord avec l'orthographe de la fondamentale", () => {
  assert.deepEqual(chordNotes("D", [0, 3, 7, 10]), ["D", "F", "A", "C"]);
  assert.deepEqual(chordNotes("Bb", [0, 4, 7]), ["Bb", "D", "F"]);
  assert.deepEqual(chordNotes("F#", [0, 4, 7]), ["F#", "A#", "C#"]);
  assert.deepEqual(chordNotes("C", [0, 4, 7], "E"), ["C", "E", "G"], "basse déjà dans l'accord");
  assert.deepEqual(chordNotes("C", [0, 4, 7], "D"), ["D", "C", "E", "G"], "basse étrangère en tête");
  assert.deepEqual(chordNotes("C", []), []);
});

import { KEY_CHOICES, keyEntry, choiceOf, fifthsAt, keyChangesWithin, keyName, fifthsText } from "../js/chord-model.js";
test("tonalités : 30 choix, armure en vigueur, changements dans une fenêtre", () => {
  assert.equal(KEY_CHOICES.length, 30);
  const d = KEY_CHOICES.find((c) => c.id === "D");
  assert.deepEqual([d.fifths, d.minor, d.root], [2, false, "D"]);
  assert.equal(KEY_CHOICES.find((c) => c.id === "Bm").fifths, 2, "si mineur = 2 dièses");
  assert.equal(KEY_CHOICES.find((c) => c.id === "Dm").fifths, -1);
  assert.deepEqual(keyEntry(d, [4, 1]), { at: [4, 1], root: "D", mode: "major", fifths: 2 });
  assert.equal(choiceOf({ fifths: -3, mode: "minor" }).id, "Cm");
  const track = { globalKey: { fifths: 1 }, keys: [{ at: [8, 1], fifths: -2 }, { at: [4, 1], fifths: 3 }] };
  assert.equal(fifthsAt(track, 0), 1, "avant le premier marqueur : tonalité du morceau");
  assert.equal(fifthsAt(track, 4), 3, "à la position du marqueur : déjà en vigueur");
  assert.equal(fifthsAt(track, 7.99), 3);
  assert.equal(fifthsAt(track, 9), -2);
  assert.equal(fifthsAt({ keys: [] }, 3), 0);
  assert.deepEqual(keyChangesWithin(track, 2, 9).map((k) => k.whole), [4, 8]);
  assert.deepEqual(keyChangesWithin(track, 4, 8), [], "ni le début ni la fin de la fenêtre");
  assert.equal(keyName({ fifths: 2, mode: "major", root: "D" }), "D majeur · 2 dièses");
  assert.equal(fifthsText(0), "aucune altération");
});

import { keyAt, markerAt } from "../js/chord-model.js";
test("tonalité en vigueur et marqueur exact", () => {
  const track = { globalKey: { root: "C", mode: "major", fifths: 0 }, keys: [{ at: [8, 1], root: "F", mode: "major", fifths: -1 }, { at: [4, 1], root: "D", mode: "major", fifths: 2 }] };
  assert.equal(keyAt(track, 0).fifths, 0);
  assert.equal(keyAt(track, 4).fifths, 2);
  assert.equal(keyAt(track, 7.5).root, "D");
  assert.equal(keyAt(track, 9).root, "F");
  assert.equal(keyAt({ keys: [] }, 3), null);
  assert.equal(markerAt(track, 4).root, "D");
  assert.equal(markerAt(track, 5), null);
});
