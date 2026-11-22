// Talkback de la régie : validation des commandes, calculs de la page (retours visés, mise d'accord avec l'hôte), faux moteur.
import test from "node:test";
import assert from "node:assert/strict";

import { validateCommand } from "../js/protocol.js";
import { MockEngine } from "../js/mock-engine.js";
import { desiredTargets, inputChoices, reconcile, talkLabel } from "../js/talkback-model.js";

const ok = { type: "set_talkback", channel: 2, level_db: 0, duck_db: 12, targets: [3] };

test("set_talkback and talk are validated", () => {
  assert.equal(validateCommand(ok), null);
  assert.equal(validateCommand({ ...ok, channel: null, targets: [] }), null);
  for (const bad of [{ channel: 0 }, { channel: 33 }, { level_db: 13 }, { duck_db: 61 }, { duck_db: -1 }, { targets: [0] }, { targets: "3" }]) assert.match(validateCommand({ ...ok, ...bad }), /set_talkback/, JSON.stringify(bad));
  assert.equal(validateCommand({ type: "talk", active: true }), null);
  assert.match(validateCommand({ type: "talk", active: "oui" }), /talk/);
});

test("targets are all the returns except those unchecked by name, sorted", () => {
  const project = { nodes: [{ id: 0, kind: "master", name: "Master" }, { id: 5, kind: "bus", is_return: true, name: "Marie" }, { id: 3, kind: "bus", is_return: true, name: "Paul" }, { id: 4, kind: "bus", name: "Bus" }, { id: 1, kind: "track", name: "T" }] };
  assert.deepEqual(desiredTargets(project, []), [3, 5]);
  assert.deepEqual(desiredTargets(project, ["Paul", "Inconnu"]), [5]);
});

test("reconcile only asks for a change when the host differs, and never without a microphone", () => {
  const wanted = { channel: 2, level_db: 0, duck_db: 12, targets: [3] };
  assert.equal(reconcile({ channel: 2, level_db: 0, duck_db: 12, targets: [3] }, wanted), null);
  assert.deepEqual(reconcile({ channel: 2, level_db: 0, duck_db: 12, targets: [] }, wanted), { type: "set_talkback", ...wanted });
  assert.equal(reconcile({ channel: null, targets: [] }, { ...wanted, channel: null }), null);
});

test("labels tell what to do and the inputs are bounded", () => {
  assert.equal(talkLabel({ channel: null, targets: [], active: false }), "Parler (choisir le micro)");
  assert.equal(talkLabel({ channel: 1, targets: [], active: false }), "Parler (choisir un retour)");
  assert.equal(talkLabel({ channel: 1, targets: [3], active: false }), "Parler");
  assert.equal(talkLabel({ channel: 1, targets: [3], active: true }), "● Parle…");
  assert.deepEqual(inputChoices(3), [1, 2, 3]);
  assert.equal(inputChoices(100).length, 32);
});

test("the mock engine talks like the host: refuses without mic or return, keeps the state, ends on a structure change", () => {
  const e = new MockEngine();
  const events = [];
  const errors = [];
  e.on("talkback", (t) => events.push(t));
  e.on("error", (x) => errors.push(x.message));
  let project;
  e.on("project", (p) => (project = p));
  e.send({ type: "get_project" });
  e.send({ type: "talk", active: true });
  assert.equal(errors.length, 1);
  e.send({ type: "add_track", kind: "return", name: "Marie" });
  const ret = project.nodes.find((n) => n.name === "Marie").id;
  e.send({ type: "set_talkback", channel: 2, level_db: -3, duck_db: 6, targets: [ret] });
  const { type: _t, ...last } = events.at(-1);
  assert.deepEqual(last, { channel: 2, level_db: -3, duck_db: 6, targets: [ret], active: false });
  e.send({ type: "set_talkback", channel: 2, level_db: -3, duck_db: 6, targets: [1] }); // une piste
  assert.equal(errors.length, 2);
  e.send({ type: "talk", active: true });
  assert.equal(events.at(-1).active, true);
  const n = events.length;
  e.send({ type: "talk", active: true });
  assert.equal(events.length, n, "redire « je parle » n'émet rien");
  e.send({ type: "add_track", kind: "stereo", name: "Nouvelle" });
  assert.deepEqual(events.at(-1).targets, []);
  assert.equal(events.at(-1).active, false);
});
