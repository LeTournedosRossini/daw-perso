import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { ProjectFiles } from "../js/project-files.js";

function collect(engine, ...types) {
  const log = [];
  for (const t of types) engine.on(t, (e) => log.push(e));
  return log;
}

function setup() {
  const e = new MockEngine();
  const projects = collect(e, "project");
  const recs = collect(e, "recording");
  const errors = collect(e, "error");
  const histories = collect(e, "history");
  e.send({ type: "get_project" });
  return { e, project: () => projects.at(-1), recs, errors, histories };
}

test("validation : entrée d'une piste, record, stop_record, écoute directe", () => {
  assert.equal(validateCommand({ type: "record" }), null);
  assert.equal(validateCommand({ type: "stop_record" }), null);
  assert.equal(validateCommand({ type: "set_track_input", node: 2, input: { kind: "audio", channels: [3] } }), null);
  assert.equal(validateCommand({ type: "set_track_input", node: 2, input: { kind: "audio", channels: [5, 6] } }), null);
  assert.equal(validateCommand({ type: "set_track_input", node: 2, input: { kind: "midi" } }), null);
  assert.notEqual(validateCommand({ type: "set_track_input", node: 0, input: { kind: "midi" } }), null);
  assert.notEqual(validateCommand({ type: "set_track_input", node: 2, input: { kind: "audio", channels: [] } }), null);
  assert.notEqual(validateCommand({ type: "set_track_input", node: 2, input: { kind: "audio", channels: [1, 2, 3] } }), null);
  assert.notEqual(validateCommand({ type: "set_track_input", node: 2, input: { kind: "audio", channels: [0] } }), null);
  assert.notEqual(validateCommand({ type: "set_track_input", node: 2, input: { kind: "video" } }), null);
  assert.notEqual(validateCommand({ type: "set_track_input", node: 2 }), null);
  assert.equal(validateCommand({ type: "set_node", node: 2, monitor: false }), null);
  assert.notEqual(validateCommand({ type: "set_node", node: 2, monitor: "non" }), null);
});

test("enregistrer : refus clairs, puis prises devenues clips en une seule action annulable", () => {
  const { e, project, recs, errors, histories } = setup();
  const node = (id) => project().nodes.find((n) => n.id === id);
  e.send({ type: "set_node", node: 1, arm: true });
  assert.equal(e.send({ type: "record" }), false);
  assert.match(errors.at(-1).message, /Enregistrer sous/);
  e.send({ type: "save_project", path: "/tmp/Rec" });
  e.send({ type: "set_node", node: 1, arm: false });
  assert.equal(e.send({ type: "record" }), false);
  assert.match(errors.at(-1).message, /aucune piste armée/);
  e.send({ type: "set_node", node: 1, arm: true });
  e.send({ type: "set_node", node: 2, arm: true });
  e.send({ type: "set_track_input", node: 2, input: { kind: "midi" } });
  e.send({ type: "seek", pos: 10_000 });
  const before = project().nodes.map((n) => n.clips.length);
  assert.equal(e.send({ type: "record" }), true);
  assert.deepEqual([recs.at(-1).active, recs.at(-1).stopping], [true, false]);
  // Pendant la prise : on ne déplace pas la tête, on ne change pas la structure.
  for (const bad of [{ type: "seek", pos: 5 }, { type: "add_track", kind: "mono" }, { type: "undo" }, { type: "set_node", node: 1, arm: false }, { type: "set_track_input", node: 1, input: { kind: "midi" } }]) {
    assert.equal(e.send(bad), false, bad.type);
    assert.match(errors.at(-1).message, /arrête d'abord l'enregistrement/);
  }
  e.advance(2000);
  assert.equal(e.send({ type: "transport", action: "stop" }), true, "arrêter la lecture arrête aussi la prise");
  assert.deepEqual(recs.slice(-2).map((r) => [r.active, r.stopping]), [[true, true], [false, false]]);
  const kinds = [1, 2].map((id) => node(id).clips.at(-1));
  assert.equal(project().nodes.map((n) => n.clips.length).join(), before.map((c, i) => c + ([1, 2].includes(project().nodes[i].id) ? 1 : 0)).join());
  assert.deepEqual(kinds.map((c) => [c.kind, c.start, c.name]), [["audio", 10_000, "Batterie prise 1"], ["midi", 10_000, "Basse prise 1"]]);
  assert.ok(kinds[0].len >= 90_000 && kinds[0].len <= 100_000, `2 s à 48 kHz : ${kinds[0].len}`);
  assert.equal(histories.at(-1).undo_label, "Enregistrer");
  e.send({ type: "undo" });
  assert.deepEqual(project().nodes.map((n) => n.clips.length), before);
  // Une boucle empêche d'enregistrer.
  e.send({ type: "loop", start: 0, end: 48_000 });
  assert.equal(e.send({ type: "record" }), false);
  assert.match(errors.at(-1).message, /boucle/);
});

test("entrée d'une piste : numéros du pilote vérifiés, pistes seulement", () => {
  const { e, project, errors } = setup();
  assert.equal(project().audio_inputs, 10);
  assert.equal(e.send({ type: "set_track_input", node: 2, input: { kind: "audio", channels: [7, 8] } }), true);
  assert.deepEqual(project().nodes.find((n) => n.id === 2).input, { kind: "audio", channels: [7, 8] });
  assert.equal(e.send({ type: "set_track_input", node: 2, input: { kind: "audio", channels: [11] } }), false);
  assert.match(errors.at(-1).message, /inexistante/);
  e.send({ type: "add_track", kind: "bus", name: "B" });
  const bus = project().nodes.find((n) => n.name === "B").id;
  assert.equal(e.send({ type: "set_track_input", node: bus, input: { kind: "midi" } }), false);
  assert.match(errors.at(-1).message, /pas une piste/);
  e.send({ type: "set_node", node: 2, monitor: false });
  assert.equal(project().nodes.find((n) => n.id === 2).monitor, false);
});

test("ensureSaved : demande où enregistrer quand le projet n'a pas de dossier, rien sinon", async () => {
  globalThis.document = { title: "", getElementById: () => null };
  const e = new MockEngine();
  const state = {};
  let asked = 0;
  const files = new ProjectFiles(e, state, { confirm: async () => true, pickFolder: async () => null, pickSave: async () => (asked++, "/tmp/Prise") }, {}, null);
  e.send({ type: "get_project" });
  assert.equal(await files.ensureSaved(), true);
  assert.equal(asked, 1);
  assert.equal(state.file.path, "/tmp/Prise.daw");
  assert.equal(await files.ensureSaved(), true, "déjà enregistré : aucune question");
  assert.equal(asked, 1);
  // Annulation du dialogue : faux, on n'enregistre pas.
  const e2 = new MockEngine();
  const state2 = {};
  const files2 = new ProjectFiles(e2, state2, { confirm: async () => true, pickFolder: async () => null, pickSave: async () => null }, {}, null);
  e2.send({ type: "get_project" });
  assert.equal(await files2.ensureSaved(), false);
});
