import test from "node:test";
import assert from "node:assert/strict";
import { scoreClipInput, editToCommands, suggestStaff, isVisible, scoreTiming } from "../js/score-adapter.js";
import { validateCommand } from "../js/protocol.js";

const detail = {
  clip: 1000012, start: [4, 1], len: [2, 1], offset: [1, 2],
  notes: [
    { start: [1, 4], len: [1, 8], key: 64, velocity: 0.8, channel: 3 },
    { start: [1, 2], len: [1, 4], key: 60, velocity: 0.5, channel: 0 },
    { start: [3, 1], len: [1, 4], key: 67, velocity: 0.5, channel: 0 },
  ],
  selection: [1],
};

test("l'entrée de setClips garde tous les indices (même les notes cachées par le rognage) et les réglages de vue", () => {
  const c = scoreClipInput(detail, { name: "Arpège", node: 3 });
  assert.equal(c.notes.length, 3);
  assert.deepEqual([c.id, c.start, c.len, c.offset, c.selection], [1000012, [4, 1], [2, 1], [1, 2], [1]]);
  assert.deepEqual(c.presentation, { staff: "treble", grid: [1, 16], keyFifths: 0 });
  assert.equal(c.notes[0].channel, 3);
  assert.deepEqual(scoreClipInput(detail, { presentation: { staff: "bass" } }).presentation.staff, "bass");
  assert.deepEqual(scoreTiming({ tempo_map: [{ pos: [0, 1], num: 120, den: 1 }], meters: [{ bar: 0, num: 4, den: 4 }, { bar: 4, num: 3, den: 4 }] }).meters, [{ bar: 1, num: 4, den: 4 }, { bar: 5, num: 3, den: 4 }], "mesures à partir de 1 pour le module");
});

test("visibilité d'après la fenêtre (décalage 1/2, longueur 2)", () => {
  assert.equal(isVisible(detail, 0), false, "la note finit avant le décalage");
  assert.equal(isVisible(detail, 1), true);
  assert.equal(isVisible(detail, 2), false, "elle commence après la fin visible (5/2)");
});

test("chaque geste du module devient une commande valide du protocole", () => {
  const send = (e) => editToCommands(e, detail, { node: 3 });
  const add = send({ type: "add", start: [3, 4], len: [2, 8], key: 62 });
  assert.deepEqual(add[0].notes[0], { start: [3, 4], len: [1, 4], key: 62, velocity: 0.8 }, "fractions réduites");
  const move = send({ type: "move", indices: [0, 1], dstart: [-1, 8], dkey: 2 });
  assert.deepEqual(move[0].edits, [{ index: 0, start: [1, 8], key: 66 }, { index: 1, start: [3, 8], key: 62 }]);
  assert.deepEqual(send({ type: "move", indices: [0], dstart: [-1, 2], dkey: 0 }), [], "pas avant le début de la matière");
  assert.equal(send({ type: "move", indices: [0], dstart: [0, 1], dkey: 100 })[0].edits[0].key, 127, "borné à 127");
  assert.deepEqual(send({ type: "resize", index: 1, len: [2, 8] })[0].edits, [{ index: 1, len: [1, 4] }]);
  assert.deepEqual(send({ type: "pitch", indices: [2], dkey: -12 })[0].edits, [{ index: 2, key: 55 }]);
  assert.deepEqual(send({ type: "pitch", indices: [2], key: 72 })[0].edits, [{ index: 2, key: 72 }]);
  assert.deepEqual(send({ type: "delete", indices: [2, 0] })[0], { type: "delete_notes", clip: 1000012, indices: [0, 2] });
  assert.deepEqual(send({ type: "preview", key: 64, on: true })[0], { type: "preview_note", node: 3, key: 64, velocity: 0.8, on: true });
  for (const cmd of [add, move, send({ type: "resize", index: 1, len: [2, 8] }), send({ type: "delete", indices: [1] })]) {
    for (const c of cmd) assert.ok(c.type, JSON.stringify(c));
  }
  assert.equal(validateCommand(add[0]), null);
  assert.equal(validateCommand(move[0]), null);
});

test("gestes invalides ou inconnus : rien n'est envoyé", () => {
  const send = (e) => editToCommands(e, detail, {});
  assert.deepEqual(send({ type: "add", start: [1, 0], len: [1, 8], key: 60 }), []);
  assert.deepEqual(send({ type: "add", start: [1, 4], len: [0, 8], key: 60 }), []);
  assert.deepEqual(send({ type: "add", start: [1, 4], len: [1, 8], key: 128 }), []);
  assert.deepEqual(send({ type: "delete", indices: [9] }), []);
  assert.deepEqual(send({ type: "resize", index: 7, len: [1, 8] }), []);
  assert.deepEqual(send({ type: "preview", key: 60, on: true }), [], "sans piste, pas d'écoute");
  assert.deepEqual(send({ type: "select", indices: [1] }), []);
  assert.deepEqual(editToCommands(null, detail), []);
});

test("portée suggérée d'après la tessiture", () => {
  const k = (...keys) => keys.map((key) => ({ key }));
  assert.equal(suggestStaff([]), "treble");
  assert.equal(suggestStaff(k(40, 48, 55)), "bass");
  assert.equal(suggestStaff(k(64, 72, 76)), "treble");
  assert.equal(suggestStaff(k(36, 48, 72, 79)), "grand");
});

import { keyText, playheadFraction } from "../js/score-adapter.js";
test("armure en texte et position de lecture en fraction", () => {
  assert.equal(keyText(0), "Aucune altération");
  assert.equal(keyText(1), "1 dièse");
  assert.equal(keyText(3), "3 dièses");
  assert.equal(keyText(-1), "1 bémol");
  assert.equal(keyText(-4), "4 bémols");
  const tempo = { wholeAt: (s) => s / 96000 }; // 120 bpm à 48 kHz : une ronde = 96 000 échantillons
  assert.deepEqual(playheadFraction(tempo, 48000), [1920, 3840]);
  assert.deepEqual(playheadFraction(tempo, -5), [0, 3840], "jamais négative");
});

test("voix d'affichage : lues dans l'événement, écrites par `add` et par la commande `voice`", () => {
  const d = { ...detail, notes: detail.notes.map((n, i) => (i === 1 ? { ...n, voice: 2 } : n)) };
  assert.deepEqual(scoreClipInput(d).notes.map((n) => n.voice), [1, 2, 1], "absente = 1");
  assert.equal(editToCommands({ type: "add", start: [3, 4], len: [1, 8], key: 62, voice: 3 }, detail)[0].notes[0].voice, 3);
  assert.equal("voice" in editToCommands({ type: "add", start: [3, 4], len: [1, 8], key: 62, voice: 1 }, detail)[0].notes[0], false, "voix 1 : pas de champ");
  const v = editToCommands({ type: "voice", indices: [0, 2], voice: 2 }, detail);
  assert.deepEqual(v[0].edits, [{ index: 0, voice: 2 }, { index: 2, voice: 2 }]);
  assert.equal(v[0].label, "Changer de voix");
  assert.equal(validateCommand(v[0]), null);
  assert.deepEqual(editToCommands({ type: "voice", indices: [0], voice: 5 }, detail), []);
  assert.deepEqual(editToCommands({ type: "voice", indices: [9], voice: 2 }, detail), []);
  assert.notEqual(validateCommand({ type: "edit_notes", clip: 1000012, edits: [{ index: 0, voice: 0 }] }), null);
  assert.notEqual(validateCommand({ type: "add_notes", clip: 1000012, notes: [{ start: [0, 1], len: [1, 8], key: 60, velocity: 0.5, voice: 7 }] }), null);
});
