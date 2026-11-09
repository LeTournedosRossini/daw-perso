import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { sectionColor } from "../js/ui/chord-lane.js";

function mock() {
  const e = new MockEngine();
  const log = { chords: [], errors: [] };
  e.on("chords", (x) => log.chords.push(x));
  e.on("error", (x) => log.errors.push(x.message));
  e.send({ type: "get_project" });
  return { e, log };
}
const sec = (at, dur, name, color) => ({ at, dur, name, ...(color ? { color } : {}) });

test("structure : posée, triée, annulable en un pas", () => {
  const { e, log } = mock();
  assert.deepEqual(log.chords.at(-1).structure, []);
  e.send({ type: "set_structure", sections: [sec([4, 1], [4, 1], "Refrain", "#ff4d5e"), sec([0, 1], [4, 1], "Intro")] });
  assert.deepEqual(log.chords.at(-1).structure.map((s) => s.name), ["Intro", "Refrain"]);
  e.send({ type: "undo" });
  assert.equal(log.chords.at(-1).structure.length, 0);
  e.send({ type: "redo" });
  assert.equal(log.chords.at(-1).structure.length, 2);
  assert.deepEqual(log.errors, []);
});

test("structure : refus sans trace (chevauchement, nom, couleur, durée)", () => {
  const { e, log } = mock();
  e.send({ type: "set_structure", sections: [sec([0, 1], [4, 1], "A")] });
  const bad = [
    [sec([0, 1], [4, 1], "A"), sec([2, 1], [4, 1], "B")],
    [sec([0, 1], [4, 1], "  ")],
    [sec([0, 1], [4, 1], "A", "rouge")],
    [sec([0, 1], [0, 1], "A")],
  ];
  for (const sections of bad) e.send({ type: "set_structure", sections });
  assert.equal(log.errors.length, bad.length);
  assert.equal(log.chords.at(-1).structure.length, 1);
});

test("structure : validation des messages de la page", () => {
  assert.equal(validateCommand({ type: "set_structure", sections: [sec([0, 1], [1, 1], "A")] }), null);
  assert.match(validateCommand({ type: "set_structure" }), /sections/);
  assert.match(validateCommand({ type: "set_structure", sections: [{ name: "A" }] }), /sections/);
});

test("couleur d'une section : choisie, sinon d'après le nom (même nom, même couleur)", () => {
  assert.equal(sectionColor({ name: "x", color: "#123456" }), "#123456");
  assert.equal(sectionColor({ name: "Refrain" }), sectionColor({ name: " refrain " }));
  assert.notEqual(sectionColor({ name: "Intro" }), sectionColor({ name: "Refrain" }));
  assert.match(sectionColor({ name: "Quelque chose" }), /^#[0-9a-f]{6}$/);
});

import { suggestCategory, categoryOf, sectionColor as catColor, CATEGORIES, CATEGORY_COLORS, LETTER_COLORS } from "../js/section-model.js";

test("catégories de sections : suggérées par le nom, couleurs de Scorsaire", () => {
  assert.equal(suggestCategory("Couplet 1"), "verse");
  assert.equal(suggestCategory("  refrain "), "chorus");
  assert.equal(suggestCategory("Pré-refrain"), "preChorus");
  assert.equal(suggestCategory("C"), "letter");
  assert.equal(suggestCategory("Ma partie"), "custom");
  assert.equal(categoryOf({ name: "x", category: "bridge" }), "bridge", "la catégorie portée l'emporte sur le nom");
  assert.equal(categoryOf({ name: "Refrain", category: "n'importe quoi" }), "chorus", "catégorie inconnue : on retombe sur le nom");
  assert.equal(catColor({ name: "Refrain" }), CATEGORY_COLORS.chorus);
  assert.equal(catColor({ name: "B" }), LETTER_COLORS[1]);
  assert.equal(catColor({ name: "A", color: "#123456" }), "#123456");
  assert.equal(CATEGORIES.length, 11);
  assert.equal(validateCommand({ type: "set_structure", sections: [{ at: [0, 1], dur: [4, 1], name: "X", category: "verse" }] }), null);
  assert.notEqual(validateCommand({ type: "set_structure", sections: [{ at: [0, 1], dur: [4, 1], name: "X", category: "couplet" }] }), null);
});

import { readFileSync } from "node:fs";
test("couleurs des sections : identiques au fichier d'échange de Scorsaire (section-colors.json)", () => {
  const ref = JSON.parse(readFileSync(new URL("../js/vendor/scorsaire-score-view/section-colors.json", import.meta.url), "utf8"));
  assert.deepEqual(CATEGORIES, ref.categories);
  for (const [cat, col] of Object.entries(ref.colors)) assert.equal(CATEGORY_COLORS[cat], col, cat);
  assert.deepEqual(LETTER_COLORS, ref.letters);
});
