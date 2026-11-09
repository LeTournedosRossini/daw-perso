import test from "node:test";
import assert from "node:assert/strict";
import { describeSelection } from "../js/notes-model.js";
import { validateCommand } from "../js/protocol.js";
import { MockEngine } from "../js/mock-engine.js";

const note = (extra = {}) => ({ start: [1, 4], len: [1, 8], key: 60, velocity: 0.5, channel: 0, ...extra });

test("propriétés de la sélection : rien, une note, plusieurs notes qui diffèrent", () => {
  assert.equal(describeSelection([]), null);
  const one = describeSelection([note({ channel: 4, velocity: 100 / 127 })]);
  assert.deepEqual([one.count, one.key, one.keyName, one.channel, one.velocity, one.startBeats, one.lenBeats], [1, 60, "Do4", 5, 100, 1, 0.5]);
  const many = describeSelection([note(), note({ key: 64, channel: 2, velocity: 1 })]);
  assert.equal(many.count, 2);
  assert.equal(many.key, null);
  assert.equal(many.channel, null, "canaux différents : pas de valeur commune");
  assert.equal(many.velocity, null);
  assert.equal(many.startBeats, null);
  assert.equal(describeSelection([note(), note({ start: [1, 2] })]).channel, 1, "même canal : valeur commune");
});

test("edit_notes accepte un canal de 0 à 15 et le faux moteur le garde", () => {
  assert.equal(validateCommand({ type: "edit_notes", clip: 1, edits: [{ index: 0, channel: 15 }] }), null);
  for (const bad of [16, -1, 1.5]) assert.notEqual(validateCommand({ type: "edit_notes", clip: 1, edits: [{ index: 0, channel: bad }] }), null);
  const e = new MockEngine();
  let notes;
  e.on("notes", (n) => (notes = n));
  e.send({ type: "get_project" });
  e.send({ type: "edit_notes", clip: 30, edits: [{ index: 0, channel: 9 }] });
  assert.equal(notes.notes[notes.selection[0]].channel, 9);
  e.send({ type: "undo" });
  e.send({ type: "get_notes", clip: 30 });
  assert.equal(notes.notes[0].channel, 0);
});
