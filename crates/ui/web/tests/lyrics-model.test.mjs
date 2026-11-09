import test from "node:test";
import assert from "node:assert/strict";
import { parseLyricsText, syllableLabel, placeSyllables, mergeSyllables, verseSyllables, verseCount, lyricsForClip, lyricsToText, noteOnsets, placeOnNotes, clearVerseRange, notesFromFirstSelected, noteSpans, syllableCount } from "../js/lyrics-model.js";

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

const clipD = { start: [2, 1], len: [1, 1], offset: [1, 4], notes: [
  { start: [0, 1], len: [1, 8], key: 60 }, // avant la partie visible (rognée)
  { start: [1, 4], len: [1, 8], key: 60 },
  { start: [1, 4], len: [1, 8], key: 64 }, // accord : même instant
  { start: [3, 8], len: [1, 8], key: 62 },
  { start: [3, 4], len: [1, 8], key: 65 },
  { start: [5, 4], len: [1, 8], key: 67 }, // à la fin de la partie visible (exclue)
] };

test("le texte d'un couplet se relit comme on l'a saisi", () => {
  const list = placeSyllables(parseLyricsText("ma-man est la plus bel-le"), [0, 1], [1, 4], 1);
  assert.equal(lyricsToText(list, 1), "ma-man est la plus bel-le");
  assert.equal(lyricsToText(list, 2), "");
  assert.deepEqual(parseLyricsText(lyricsToText(list, 1)).map(syllableLabel), list.map(syllableLabel));
});

test("les instants de notes : accords réunis, partie visible seulement, positions absolues", () => {
  assert.deepEqual(noteOnsets(clipD), [[2, 1], [17, 8], [5, 2]]); // 1/4→2, 3/8→2+1/8, 3/4→2+1/2
  assert.deepEqual(noteOnsets(clipD, [3, 4]), [[17, 8], [5, 2]]); // la sélection
  assert.deepEqual(noteOnsets(clipD, []), noteOnsets(clipD)); // sélection vide = tout
});

test("une syllabe par instant de note, les surplus sont comptés", () => {
  const onsets = noteOnsets(clipD);
  const a = placeOnNotes(parseLyricsText("ma-man est la"), onsets, 2);
  assert.deepEqual(a.placed.map((x) => x.at), onsets);
  assert.equal(a.extra, 1);
  assert.deepEqual(a.placed.map((x) => x.verse), [2, 2, 2]);
  const b = placeOnNotes(parseLyricsText("ma"), onsets, 1);
  assert.equal(b.placed.length, 1);
  assert.equal(b.extra, 0);
});

test("remplacer les paroles d'un clip n'efface que le couplet et la plage voulus", () => {
  const list = [
    { at: [1, 1], text: "a", verse: 1 }, { at: [2, 1], text: "b", verse: 1 }, { at: [2, 1], text: "c", verse: 2 }, { at: [3, 1], text: "d", verse: 1 },
  ];
  const kept = clearVerseRange(list, 1, [2, 1], [3, 1]);
  assert.deepEqual(kept.map((s) => s.text), ["a", "c", "d"]);
});

test("les syllabes démarrent à la première note sélectionnée, accord compris", () => {
  // sélectionner la note 3 (3/8) : on garde toutes les notes à partir de cet instant (3/8, 3/4, 5/4) ; l'accord de 1/4 est avant
  assert.deepEqual(notesFromFirstSelected(clipD, [3]), [3, 4, 5]);
  assert.deepEqual(notesFromFirstSelected(clipD, [2, 4]), [1, 2, 3, 4, 5]); // la plus tôt des deux : 1/4 (note 2 de l'accord)
  assert.equal(notesFromFirstSelected(clipD, [99]), null);
  assert.deepEqual(noteOnsets(clipD, notesFromFirstSelected(clipD, [3])), [[17, 8], [5, 2]]);
});

test("« _ » prolonge la syllabe sur la note suivante : un trait de prolongation jusqu'à la fin de cette note", () => {
  const items = parseLyricsText("ma _ _ man est");
  assert.equal(syllableCount(items), 3);
  const spans = noteSpans(clipD); // instants 2, 17/8, 5/2 (la note la plus longue de chaque instant)
  assert.deepEqual(spans.map((s) => s.at), [[2, 1], [17, 8], [5, 2]]);
  const { placed, extra } = placeOnNotes(items, spans, 1);
  assert.equal(extra, 2); // 5 éléments pour 3 notes : 2 éléments en trop (man, est)
  assert.deepEqual(placed.map((s) => s.text), ["ma"]);
  assert.deepEqual(placed[0].end, spans[2].end); // le trait court jusqu'à la fin de la 3e note
  // un « _ » sans syllabe devant est compté en trop, pas posé
  const lead = placeOnNotes(parseLyricsText("_ ma"), spans, 1);
  assert.deepEqual(lead.placed.map((s) => s.text), ["ma"]);
  assert.equal(lead.extra, 1);
});

test("un mélisme se relit en « _ » et le trait part vers le module dans le repère des notes", () => {
  const spans = noteSpans(clipD);
  const { placed } = placeOnNotes(parseLyricsText("ma _ man"), spans, 1);
  const list = [...placed];
  assert.equal(lyricsToText(list, 1, spans.map((s) => s.at)), "ma _ man");
  const out = lyricsForClip(list, clipD);
  assert.equal(out.length, 2);
  assert.ok(out[0].end, "la fin du trait est transmise");
  assert.deepEqual(out[0].end, [1, 2]); // fin absolue 9/4 (fin de la 2e note) − début du clip 2 + décalage de matière 1/4
  assert.equal(out[1].end, undefined);
});

test("lyricTap : saisie d'une syllabe au clic dans la partition", async () => {
  const { applyLyricTap } = await import("../js/lyrics-model.js");
  // Clip de 2 rondes commençant à la mesure 2 (début 1/1 de ronde), décalage 0 ; notes aux noires 0, 1/4, 2/4, 3/4 du clip.
  const note = (n, d) => ({ start: [n, d], len: [1, 4], key: 60, velocity: 0.8, channel: 0, voice: 1 });
  const data = { start: [1, 1], len: [2, 1], offset: [0, 1], notes: [note(0, 1), note(1, 4), note(1, 2), note(3, 4)] };
  const at = (n, d) => [n, d];
  // Syllabe posée sur une place vide : à l'instant touché (début du clip + start).
  let r = applyLyricTap([], data, { start: [1, 4], verse: 1 }, "man");
  assert.equal(r.label, "Écrire une syllabe");
  assert.deepEqual(r.syllables.map((s) => [s.at, s.text, s.verse]), [[at(5, 4), "man", 1]]);
  // « ma-man » : deux syllabes, sur la note touchée puis la suivante ; la première est liée.
  r = applyLyricTap([], data, { start: [0, 1], verse: 1 }, "ma-man");
  assert.deepEqual(r.syllables.map((s) => [s.at, s.text, s.hyphen]), [[at(1, 1), "ma", true], [at(5, 4), "man", false]]);
  // Remplace la syllabe du même couplet au même instant, sans toucher aux autres couplets.
  const list = [{ at: at(1, 1), text: "la", hyphen: false, verse: 1 }, { at: at(1, 1), text: "lo", hyphen: false, verse: 2 }];
  r = applyLyricTap(list, data, { start: [0, 1], verse: 1 }, "si");
  assert.deepEqual(r.syllables.map((s) => [s.text, s.verse]), [["si", 1], ["lo", 2]]);
  // Une syllabe retouchée garde son trait de prolongation ; un « _ » le recalcule.
  const mel = [{ at: at(1, 1), text: "ma", hyphen: false, verse: 1, end: at(2, 1) }];
  r = applyLyricTap(mel, data, { start: [0, 1], verse: 1, end: [1, 2] }, "mi");
  assert.deepEqual(r.syllables[0].end, at(3, 2)); // le trait du module (fin à 1/2 depuis le début du clip) est repris
  r = applyLyricTap(mel, data, { start: [0, 1], verse: 1 }, "mi _");
  assert.deepEqual(r.syllables[0].end, at(3, 2), "« _ » : fin de la note suivante");
  // Texte vide : supprime la syllabe de cet instant, rien à faire s'il n'y en a pas.
  r = applyLyricTap(list, data, { start: [0, 1], verse: 2 }, "");
  assert.deepEqual(r.syllables.map((s) => s.text), ["la"]);
  assert.equal(r.label, "Effacer une syllabe");
  assert.equal(applyLyricTap([], data, { start: [0, 1], verse: 1 }, "  "), null);
  // Clip rogné (décalage) : le repère de la matière est converti en position du projet.
  const trimmed = { ...data, offset: [1, 2], len: [3, 2] };
  r = applyLyricTap([], trimmed, { start: [1, 2], verse: 1 }, "x");
  assert.deepEqual(r.syllables[0].at, at(1, 1));
});

test("réserve de syllabes : pastilles, pose, ordre des mots, poussée", async () => {
  const m = await import("../js/lyrics-model.js");
  const onsets = [[1, 1], [5, 4], [3, 2], [7, 4], [2, 1]]; // cinq notes
  let chips = m.chipsFromText("ma-man est la _ plus");
  assert.deepEqual(chips.map((c) => [c.text, c.hyphen, c.at]), [["ma", true, null], ["man", false, null], ["est", false, null], ["la", false, null], ["plus", false, null]]);
  // Pose de la 1re sur la note 1 puis de la 3e sur la note 4 : ordre respecté.
  chips = m.placeChip(chips, 0, onsets[0], onsets);
  chips = m.placeChip(chips, 2, onsets[3], onsets);
  assert.deepEqual(chips.map((c) => c.at), [[1, 1], null, [7, 4], null, null]);
  // Une pastille ne passe pas devant la précédente placée.
  assert.equal(m.placeChip(chips, 1, [1, 1], onsets), null, "même instant que la 1re");
  assert.equal(m.placeChip(chips, 3, [5, 4], onsets), null, "avant la 3e, qui est placée avant elle");
  // Poser la 1re sur la note de la 3e pousse celle-ci d'une note.
  let pushed = m.placeChip(chips, 0, onsets[3], onsets);
  assert.deepEqual(pushed.map((c) => c.at), [[7, 4], null, [2, 1], null, null]);
  // Plus de note derrière : refus.
  assert.equal(m.placeChip(pushed, 0, onsets[4], onsets), null);
  // Le trait de prolongation se perd quand la pastille bouge.
  const mel = [{ text: "a", hyphen: false, at: [1, 1], end: [3, 2] }, { text: "b", hyphen: false, at: [3, 2] }];
  assert.deepEqual(m.placeChip(mel, 0, [1, 1], onsets)[0].end, [3, 2], "même place : le trait reste");
  assert.equal(m.placeChip(mel, 0, [5, 4], onsets)[0].end, undefined);
  // Retirer, convertir, relire.
  const back = m.unplaceChip(chips, 2);
  assert.equal(back[2].at, null);
  const syl = m.chipsToSyllables(chips, 2);
  assert.deepEqual(syl.map((s) => [s.text, s.hyphen, s.verse]), [["ma", true, 2], ["est", false, 2]]);
  const list = [{ at: [1, 1], text: "x", hyphen: false, verse: 1 }, { at: [5, 4], text: "y", hyphen: true, verse: 2 }, { at: [3, 1], text: "z", hyphen: false, verse: 2 }];
  assert.deepEqual(m.chipsFromLyrics(list, 2, [1, 1], [3, 1]).map((c) => c.text), ["y"]);
  assert.deepEqual(m.nearestOnset(onsets, [4, 3]), [5, 4]);
  assert.equal(m.nearestOnset([], [1, 1]), null);
});
