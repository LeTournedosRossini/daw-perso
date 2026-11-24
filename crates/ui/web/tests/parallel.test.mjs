import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { parallelParent } from "../js/ui/track-menu.js";

function setup() {
  const e = new MockEngine();
  let project = null;
  e.on("project", (p) => (project = p));
  e.send({ type: "get_project" });
  return { e, project: () => project };
}

test("add_parallel crée un bus qui suit le muet, vers la même sortie, et s'annule d'un coup", () => {
  const { e, project } = setup();
  const g = project().nodes.find((n) => n.name === "Guitare");
  const n0 = project().nodes.length;
  e.send({ type: "add_parallel", node: g.id, tap: "input" });
  assert.equal(project().nodes.length, n0 + 1);
  const par = project().nodes.find((n) => n.name === "Guitare ∥");
  assert.equal(par.output, g.output);
  const send = project().nodes.find((n) => n.id === g.id).sends.find((s) => s.to === par.id);
  assert.deepEqual([send.tap, send.follow], ["input", true]);
  assert.equal(parallelParent(project(), par).id, g.id);
  assert.equal(parallelParent(project(), g), null);
  e.send({ type: "undo" });
  assert.equal(project().nodes.length, n0);
});

test("set_send_tap change le point de captation ; un effet inconnu est refusé", () => {
  const { e, project } = setup();
  const g = project().nodes.find((n) => n.name === "Guitare");
  e.send({ type: "add_parallel", node: g.id, tap: "pre_fader" });
  const par = project().nodes.find((n) => n.name === "Guitare ∥");
  e.send({ type: "set_send_tap", node: g.id, to: par.id, tap: "post_fader" });
  assert.equal(project().nodes.find((n) => n.id === g.id).sends[0].tap, "post_fader");
  let err = null;
  e.on("error", (x) => (err = x.message));
  e.send({ type: "set_send_tap", node: g.id, to: par.id, tap: "after_insert", insert: 424242 });
  assert.match(err, /pas d'effet/);
});

test("validation des commandes de piste parallèle", () => {
  assert.equal(validateCommand({ type: "add_parallel", node: 1, tap: "input" }), null);
  assert.equal(validateCommand({ type: "add_parallel", node: 1, tap: "after_insert", insert: 3 }), null);
  assert.match(validateCommand({ type: "add_parallel", node: 1, tap: "after_insert" }), /insert/);
  assert.match(validateCommand({ type: "add_parallel", node: 1, tap: "input", insert: 3 }), /insert/);
  assert.match(validateCommand({ type: "add_parallel", node: 1, tap: "ailleurs" }), /inconnu/);
  assert.match(validateCommand({ type: "add_parallel", node: 0, tap: "input" }), /node/);
  assert.match(validateCommand({ type: "set_send_tap", node: 1, tap: "input" }), /to/);
});
