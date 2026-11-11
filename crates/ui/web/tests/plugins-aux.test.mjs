import test from "node:test";
import assert from "node:assert/strict";
import { validateCommand } from "../js/protocol.js";
import { MockEngine } from "../js/mock-engine.js";

test("set_plugin_aux : validation", () => {
  assert.equal(validateCommand({ type: "set_plugin_aux", insert: 1, port: 0, to: 3 }), null);
  assert.equal(validateCommand({ type: "set_plugin_aux", insert: 1, port: 30, to: null }), null);
  assert.match(validateCommand({ type: "set_plugin_aux", insert: 1, port: 31, to: null }), /set_plugin_aux/);
  assert.match(validateCommand({ type: "set_plugin_aux", insert: 1, port: 0 }), /set_plugin_aux/);
});

test("faux moteur : les ports auxiliaires d'un plugin se branchent sur un bus, pas sur une piste, et suivent la suppression du bus", () => {
  const e = new MockEngine();
  let project = null;
  const errors = [];
  e.on("project", (p) => (project = p));
  e.on("error", (x) => errors.push(x.message));
  e.send({ type: "get_project" });
  const track = project.nodes.find((n) => n.kind === "stereo" || n.kind === "mono");
  e.send({ type: "add_track", kind: "bus", name: "Sortie 2" });
  const bus = project.nodes.find((n) => n.name === "Sortie 2");
  e.send({ type: "add_plugin", node: track.id, clap_id: "mock.neptune", file: "Neptune.clap" });
  const ins = project.nodes.find((n) => n.id === track.id).inserts.at(-1);
  assert.deepEqual(ins.aux_ports.map((p) => p.name), ["Sortie 2", "Sortie 3", "Sortie 4"]);
  e.send({ type: "set_plugin_aux", insert: ins.id, port: 0, to: bus.id });
  assert.equal(project.nodes.find((n) => n.id === track.id).inserts.at(-1).aux_ports[0].to, bus.id);
  const before = errors.length;
  e.send({ type: "set_plugin_aux", insert: ins.id, port: 1, to: track.id }); // une piste : refusé
  e.send({ type: "set_plugin_aux", insert: ins.id, port: 9, to: bus.id }); // port inexistant : refusé
  assert.equal(errors.length, before + 2);
  e.send({ type: "delete_track", node: bus.id });
  assert.equal(project.nodes.find((n) => n.id === track.id).inserts.at(-1).aux_ports[0].to, null);
});
