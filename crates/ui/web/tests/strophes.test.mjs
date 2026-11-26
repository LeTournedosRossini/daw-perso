import test from "node:test";
import assert from "node:assert/strict";
import { parseStrophesText, strophesToText, withMeta, sectionsFor, uncoveredSections, strophesCommand } from "../js/strophes-model.js";

test("texte à marqueurs → strophes, lignes vides ignorées", () => {
  const p = parseStrophesText("[Couplet 1]\nMa maman\n\nEst belle\n[Refrain]\nLa la la\n");
  assert.deepEqual(p, [{ name: "Couplet 1", lines: ["Ma maman", "Est belle"] }, { name: "Refrain", lines: ["La la la"] }]);
  assert.deepEqual(parseStrophesText("juste du texte\nsur deux lignes"), [{ name: "Couplet 1", lines: ["juste du texte", "sur deux lignes"] }]);
  assert.deepEqual(parseStrophesText(""), []);
});

test("aller-retour texte ↔ strophes", () => {
  const t = "[Couplet 1]\nune\ndeux\n\n[Refrain]\ntrois";
  assert.equal(strophesToText(parseStrophesText(t)), t);
});

test("la catégorie et le lien d'une strophe de même nom sont conservés", () => {
  const prev = [{ name: "Refrain", category: "chorus", lines: ["x"], link: "Refrain 2" }];
  const next = withMeta(parseStrophesText("[Couplet 1]\na\n[Refrain]\nb"), prev);
  assert.equal(next[0].category, "verse");
  assert.equal(next[1].category, "chorus");
  assert.equal(next[1].link, "Refrain 2");
  assert.equal(next[0].link, undefined);
});

const structure = [
  { name: "Intro", at: [0, 1], dur: [1, 1] },
  { name: "Couplet 1", at: [1, 1], dur: [2, 1] },
  { name: "Refrain", at: [3, 1], dur: [2, 1] },
  { name: "Couplet 2", at: [5, 1], dur: [2, 1] },
  { name: "Refrain 2", at: [7, 1], dur: [2, 1] },
];

test("une strophe correspond aux sections de même nom ; « Refrain » couvre aussi « Refrain 2 »", () => {
  assert.deepEqual(sectionsFor({ name: "Couplet 2" }, structure).map((s) => s.name), ["Couplet 2"]);
  assert.deepEqual(sectionsFor({ name: "Refrain" }, structure).map((s) => s.name), ["Refrain", "Refrain 2"]);
  assert.deepEqual(sectionsFor({ name: "Couplet 3" }, structure), []);
});

test("un lien explicite l'emporte ; lien vide : aucune section", () => {
  assert.deepEqual(sectionsFor({ name: "Pont", link: "Intro" }, structure).map((s) => s.name), ["Intro"]);
  assert.deepEqual(sectionsFor({ name: "Refrain", link: "" }, structure), []);
});

test("sections sans strophe", () => {
  const list = [{ name: "Couplet 1" }, { name: "Refrain" }];
  assert.deepEqual(uncoveredSections(list, structure).map((s) => s.name), ["Intro", "Couplet 2"]);
});

test("commande set_strophes : champs vides omis", () => {
  const c = strophesCommand([{ name: "A", lines: ["x"] }, { name: "B", category: "chorus", lines: [], link: "" }], "L");
  assert.deepEqual(c, { type: "set_strophes", strophes: [{ name: "A", lines: ["x"] }, { name: "B", category: "chorus", lines: [], link: "" }], label: "L" });
});
