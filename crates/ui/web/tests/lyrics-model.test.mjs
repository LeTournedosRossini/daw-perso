import test from "node:test";
import assert from "node:assert/strict";
import { parseLyricsText, syllableLabel, placeSyllables, mergeSyllables, verseSyllables, verseCount, lyricsForClip } from "../js/lyrics-model.js";

test("un texte saisi se découpe en syllabes liées ou non", () => {
  const p = parseLyricsText("ma-man est la plus bel-le");
  assert.deepEqual(p.map(syllableLabel), ["ma-", "man", "est", "la", "plus", "bel-", "le"]);
  assert.deepEqual(parseLyricsText("  ma-  "), [{ text: "ma", hyphen: true }]); // tiret final : la syllabe reste liée
  assert.deepEqual(parseLyricsText("-- - "), []);
  assert.deepEqual(parseLyricsText(""), []);
  assert.equal(parseLyricsText("x".repeat(80))[0].text.length, 40);
});

test("les syllabes se posent une par pas, en fractions exactes", () => {
  const s = placeSyllables(parseLyricsText("ma-man est"), [3, 4], [1, 8], 2);
  assert.deepEqual(s.map((x) => x.at), [[3, 4], [7, 8], [1, 1]]);
  assert.deepEqual(s.map((x) => x.verse), [2, 2, 2]);
  assert.deepEqual(s.map((x) => x.hyphen), [true, false, false]);
});

test("poser sur des syllabes existantes les remplace dans le même couplet seulement", () => {
  const old = [{ at: [0, 1], text: "a", hyphen: false, verse: 1 }, { at: [0, 1], text: "b", hyphen: false, verse: 2 }, { at: [1, 4], text: "c", hyphen: false, verse: 1 }];
  const merged = mergeSyllables(old, [{ at: [0, 1], text: "X", hyphen: false, verse: 1 }]);
  assert.deepEqual(merged.map((s) => `${s.verse}:${s.text}`), ["1:X", "2:b", "1:c"]);
});

test("couplets : filtre par numéro avec l'indice de la liste complète, et nombre proposé", () => {
  const list = [{ at: [0, 1], text: "a", verse: 1 }, { at: [0, 1], text: "b", verse: 2 }, { at: [1, 4], text: "c" }];
  assert.deepEqual(verseSyllables(list, 1).map((s) => s.index), [0, 2]); // sans `verse` : couplet 1
  assert.equal(verseCount(list), 3);
  assert.equal(verseCount([]), 2);
  assert.equal(verseCount([{ at: [0, 1], text: "z", verse: 8 }]), 8);
});

test("les paroles d'un clip sont en positions depuis le début de la matière, seule la partie visible compte", () => {
  const list = [{ at: [1, 1], text: "a", hyphen: true, verse: 1 }, { at: [2, 1], text: "b", verse: 2 }, { at: [5, 1], text: "hors", verse: 1 }, { at: [0, 1], text: "avant", verse: 1 }];
  // clip posé en 1 (rondes), long de 2, dont la matière commence à 1/2 : la syllabe en 1 est à 1/2 dans la matière
  const out = lyricsForClip(list, { start: [1, 1], len: [2, 1], offset: [1, 2] });
  assert.deepEqual(out, [{ start: [1, 2], text: "a", hyphen: true, verse: 1 }, { start: [3, 2], text: "b", hyphen: false, verse: 2 }]);
});

import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";

function mock() {
  const e = new MockEngine();
  const log = { chords: [], errors: [] };
  e.on("chords", (x) => log.chords.push(x));
  e.on("error", (x) => log.errors.push(x.message));
  e.send({ type: "get_project" });
  return { e, log };
}
const syl = (at, text, extra = {}) => ({ at, text, ...extra });

test("paroles : posées, triées, annulables en un pas (faux moteur)", () => {
  const { e, log } = mock();
  assert.deepEqual(log.chords.at(-1).lyrics, []);
  e.send({ type: "set_lyrics", syllables: [syl([1, 4], "man"), syl([0, 1], "ma", { hyphen: true }), syl([1, 4], "mie", { verse: 2 })] });
  assert.deepEqual(log.chords.at(-1).lyrics.map((s) => `${s.verse}:${s.text}`), ["1:ma", "1:man", "2:mie"]);
  e.send({ type: "undo" });
  assert.equal(log.chords.at(-1).lyrics.length, 0);
  e.send({ type: "redo" });
  assert.equal(log.chords.at(-1).lyrics.length, 3);
  assert.deepEqual(log.errors, []);
});

test("paroles : refus sans trace (texte, couplet, doublon, fraction)", () => {
  const { e, log } = mock();
  e.send({ type: "set_lyrics", syllables: [syl([0, 1], "ma")] });
  const bad = [[syl([0, 1], "")], [syl([0, 1], "deux mots")], [syl([0, 1], "ma", { verse: 9 })], [syl([1, 0], "ma")], [syl([0, 1], "a"), syl([0, 1], "b")]];
  for (const syllables of bad) e.send({ type: "set_lyrics", syllables });
  assert.equal(log.errors.length, bad.length);
  assert.equal(log.chords.at(-1).lyrics.length, 1);
});

test("paroles : validation de la commande côté page", () => {
  assert.equal(validateCommand({ type: "set_lyrics", syllables: [syl([0, 1], "ma")] }), null);
  assert.match(validateCommand({ type: "set_lyrics" }), /syllables/);
  assert.match(validateCommand({ type: "set_lyrics", syllables: [syl([0, 1], "ma", { verse: 12 })] }), /couplet/);
});
