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

import { layStrophe, verseOfStrophe } from "../js/strophes-model.js";

const onsetsAt = (...n) => n.map((k) => [k, 4]);

test("couplet d'une strophe : le numéro du nom, sinon 1", () => {
  assert.equal(verseOfStrophe({ name: "Couplet 2" }), 2);
  assert.equal(verseOfStrophe({ name: "Refrain" }), 1);
  assert.equal(verseOfStrophe({ name: "Couplet 12" }), 1);
});

test("poser une strophe sur toutes ses cartes : le texte recommence à chaque carte", () => {
  const sections = [{ at: [0, 1], dur: [1, 1], name: "Refrain" }, { at: [2, 1], dur: [1, 1], name: "Refrain 2" }];
  const onsets = [...onsetsAt(0, 1, 2, 3), ...onsetsAt(8, 9, 10, 11)];
  const r = layStrophe({ name: "Refrain", lines: ["la la-la"] }, sections, onsets, []);
  assert.deepEqual(r.list.map((s) => s.text), ["la", "la", "la", "la", "la", "la"]);
  assert.deepEqual(r.list.map((s) => s.at[0] / s.at[1]), [0, 0.25, 0.5, 2, 2.25, 2.5]);
  assert.deepEqual(r.report.map((x) => [x.placed, x.extra, x.free]), [[3, 0, 1], [3, 0, 1]]);
  assert.equal(r.list[1].hyphen, true);
});

test("trop de syllabes ou de notes : signalé, rien ne déborde sur la carte voisine", () => {
  const sections = [{ at: [0, 1], dur: [1, 1], name: "Couplet 1" }];
  const r = layStrophe({ name: "Couplet 1", lines: ["a b c d e"] }, sections, [...onsetsAt(0, 1, 2), [5, 4]], []);
  assert.equal(r.list.length, 3);
  assert.deepEqual([r.report[0].placed, r.report[0].extra, r.report[0].free], [3, 2, 0]);
});

test("les syllabes déjà posées du même couplet sur la carte sont remplacées, les autres couplets gardés", () => {
  const sections = [{ at: [0, 1], dur: [1, 1], name: "Couplet 1" }];
  const existing = [{ at: [0, 1], text: "vieux", hyphen: false, verse: 1 }, { at: [1, 4], text: "garde", hyphen: false, verse: 2 }, { at: [3, 1], text: "loin", hyphen: false, verse: 1 }];
  const r = layStrophe({ name: "Couplet 1", lines: ["neuf"] }, sections, onsetsAt(0), existing);
  assert.deepEqual(r.list.map((s) => s.text), ["neuf", "garde", "loin"]);
});
