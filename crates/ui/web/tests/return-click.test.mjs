// Clic du métronome dans un retour : validation de la commande et faux moteur.
import test from "node:test";
import assert from "node:assert/strict";

import { validateCommand } from "../js/protocol.js";
import { MockEngine } from "../js/mock-engine.js";

test("set_return_click wants a return number and a level from -60 to +12 dB, or null for no click", () => {
  assert.equal(validateCommand({ type: "set_return_click", node: 3, db: -12 }), null);
  assert.equal(validateCommand({ type: "set_return_click", node: 3, db: null }), null);
  assert.equal(validateCommand({ type: "set_return_click", node: 3, db: 12 }), null);
  assert.match(validateCommand({ type: "set_return_click", node: 0, db: -6 }), /set_return_click/);
  assert.match(validateCommand({ type: "set_return_click", node: 3, db: 13 }), /set_return_click/);
  assert.match(validateCommand({ type: "set_return_click", node: 3, db: "fort" }), /set_return_click/);
  assert.match(validateCommand({ type: "set_return_click", node: 3 }), /set_return_click/);
});

test("the mock engine keeps the click level of a return and refuses a plain track", () => {
  const e = new MockEngine();
  let project, last;
  e.on("project", (p) => (project = p));
  e.on("error", (x) => (last = x));
  e.send({ type: "get_project" });
  e.send({ type: "add_track", kind: "return", name: "Retour Marie" });
  const ret = project.nodes.find((n) => n.name === "Retour Marie");
  assert.ok(ret.is_return);
  e.send({ type: "set_return_click", node: ret.id, db: -9 });
  assert.equal(project.nodes.find((n) => n.id === ret.id).click_db, -9);
  e.send({ type: "set_return_click", node: ret.id, db: -60 });
  assert.equal(project.nodes.find((n) => n.id === ret.id).click_db, null);
  const track = project.nodes.find((n) => n.kind === "stereo" || n.kind === "mono");
  last = null;
  e.send({ type: "set_return_click", node: track.id, db: -3 });
  assert.match(last.message, /retour de monitoring/);
});
