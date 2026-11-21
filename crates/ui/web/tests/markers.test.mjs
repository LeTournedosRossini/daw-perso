// Repères (marqueurs nommés) : validation des commandes et faux moteur (pose à la tête de lecture, modification, suppression, annulation).
import test from "node:test";
import assert from "node:assert/strict";

import { validateCommand } from "../js/protocol.js";
import { MockEngine } from "../js/mock-engine.js";

function engine() {
  const e = new MockEngine();
  let chords, last;
  e.on("chords", (c) => (chords = c));
  e.on("error", (x) => (last = x));
  e.send({ type: "get_project" });
  return { e, markers: () => chords.markers, err: () => last };
}

test("marker commands are validated", () => {
  assert.equal(validateCommand({ type: "add_marker" }), null);
  assert.equal(validateCommand({ type: "add_marker", at: [3, 4], name: "Refrain", color: "#2fb457" }), null);
  assert.match(validateCommand({ type: "add_marker", at: [1, 0] }), /add_marker/);
  assert.match(validateCommand({ type: "add_marker", color: "vert" }), /add_marker/);
  assert.equal(validateCommand({ type: "edit_marker", id: 3, color: "" }), null);
  assert.match(validateCommand({ type: "edit_marker", name: "x" }), /edit_marker/);
  assert.equal(validateCommand({ type: "delete_markers", ids: [1, 2] }), null);
  assert.match(validateCommand({ type: "delete_markers", ids: [] }), /delete_markers/);
});

test("a marker is dropped at the play head, renamed, deleted and undone", () => {
  const { e, markers, err } = engine();
  assert.deepEqual(markers(), []);
  e.send({ type: "seek", pos: 96000 }); // 2 s à 48 kHz = une ronde à 120 bpm
  e.send({ type: "add_marker", name: "Bonne prise", color: "#2fb457" });
  assert.equal(markers().length, 1);
  assert.deepEqual(markers()[0].at, [1, 1]);
  assert.equal(markers()[0].name, "Bonne prise");
  e.send({ type: "add_marker" });
  assert.equal(markers()[1].name, "Marqueur 2", "nom par défaut numéroté");
  const [a, b] = markers().map((m) => m.id);
  assert.notEqual(a, b);
  e.send({ type: "edit_marker", id: a, name: "À refaire", color: "" });
  assert.equal(markers().find((m) => m.id === a).name, "À refaire");
  assert.equal(markers().find((m) => m.id === a).color, undefined);
  e.send({ type: "delete_markers", ids: [a] });
  assert.deepEqual(markers().map((m) => m.id), [b]);
  e.send({ type: "undo" });
  assert.equal(markers().length, 2, "la suppression s'annule");
  e.send({ type: "delete_markers", ids: [999] });
  assert.match(err().message, /aucun de ces marqueurs/);
});
