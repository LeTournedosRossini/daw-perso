import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { orderedRows } from "../js/ui/order.js";

function mock() {
  const e = new MockEngine();
  const log = { chords: [], errors: [], catalog: [], history: [] };
  e.on("chords", (x) => log.chords.push(x));
  e.on("chord_catalog", (x) => log.catalog.push(x));
  e.on("error", (x) => log.errors.push(x.message));
  e.on("history", (x) => log.history.push(x));
  e.send({ type: "get_project" });
  return { e, log };
}

const chord = (at, dur, root, quality) => ({ at, dur, root, quality, bass: null });

test("faux moteur : la piste d'accords et son catalogue arrivent avec le projet", () => {
  const { log } = mock();
  assert.equal(log.chords.at(-1).track.chords.length, 0);
  const c = log.catalog.at(-1);
  assert.equal(c.qualities.length, 31);
  assert.equal(c.scales.length, 72);
  assert.ok(c.aliases.some(([a, id]) => a === "maj7" && id === "M7"));
});

test("faux moteur : modifier, alias ramené à l'identifiant, annuler, refus sans trace", () => {
  const { e, log } = mock();
  const base = log.chords.at(-1).track;
  e.send({ type: "set_chords", label: "Ajouter un accord", track: { ...base, chords: [chord([0, 1], [1, 1], "D", "m7"), chord([1, 1], [1, 2], "C", "maj7")] } });
  assert.deepEqual(log.errors, []);
  const t = log.chords.at(-1).track;
  assert.equal(t.chords[1].quality, "M7", "maj7 devient M7");
  assert.equal(log.history.at(-1).undo_label, "Ajouter un accord");
  // Refus : qualité inconnue, durée nulle, chevauchement ; rien ne change.
  const revision = log.chords.at(-1).revision;
  for (const [track, expect] of [
    [{ ...t, chords: [chord([0, 1], [1, 1], "D", "nimportequoi")] }, /qualité/],
    [{ ...t, chords: [chord([0, 1], [0, 1], "D", "m7")] }, /durée/],
    [{ ...t, chords: [chord([0, 1], [1, 1], "D", "m7"), chord([1, 2], [1, 1], "G", "7")] }, /chevauchent/],
  ]) {
    e.send({ type: "set_chords", track });
    assert.match(log.errors.at(-1), expect);
  }
  assert.equal(log.chords.at(-1).track.chords.length, 2, "la piste est restée celle d'avant");
  assert.ok(log.chords.at(-1).revision >= revision);
  e.send({ type: "undo" });
  assert.equal(log.chords.at(-1).track.chords.length, 0);
  e.send({ type: "redo" });
  assert.equal(log.chords.at(-1).track.chords.length, 2);
});

test("validation des commandes d'accords et lignes de la voie", () => {
  assert.equal(validateCommand({ type: "get_chords" }), null);
  assert.equal(validateCommand({ type: "set_chords", track: { chords: [] } }), null);
  assert.ok(validateCommand({ type: "set_chords", track: [] }));
  assert.ok(validateCommand({ type: "set_chords" }));
  assert.equal(validateCommand({ type: "import_chords", path: "a.json" }), null);
  assert.ok(validateCommand({ type: "export_chords", path: "" }));
  const project = { nodes: [{ id: 0, kind: "master", clips: [] }, { id: 1, kind: "mono", clips: [] }] };
  assert.deepEqual(orderedRows(project, new Set(), null, 4, new Set()).map((r) => r.kind), ["mono", "master"], "les voies d'accords sont épinglées sous la règle : plus des lignes de la liste");
});
