import test from "node:test";
import assert from "node:assert/strict";
import { outputsLabel, sidechainSources } from "../js/ui/plugin-view.js";
import { validateCommand } from "../js/protocol.js";

test("la bande dit le nombre de sorties du plugin", () => {
  assert.equal(outputsLabel(null), "stéréo");
  assert.equal(outputsLabel({ aux_ports: [] }), "stéréo");
  assert.equal(outputsLabel({}), "stéréo");
  assert.equal(outputsLabel({ aux_ports: [{}, {}] }), "3 sorties");
  assert.equal(outputsLabel({ aux_ports: new Array(31).fill({}) }), "32 sorties");
});

test("set_plugin_view : un plugin et une vue native ou web", () => {
  assert.equal(validateCommand({ type: "set_plugin_view", insert: 3, view: "native" }), null);
  assert.equal(validateCommand({ type: "set_plugin_view", insert: 0, view: "web" }), null);
  for (const bad of [{}, { insert: 3 }, { insert: 3, view: "params" }, { insert: -1, view: "web" }, { insert: 1.5, view: "web" }, { view: "web" }]) {
    assert.match(validateCommand({ type: "set_plugin_view", ...bad }) ?? "", /set_plugin_view/, JSON.stringify(bad));
  }
});

test("la chaîne latérale se choisit parmi les pistes et les bus (la fonction vit maintenant dans la vue du plugin)", () => {
  const p = { nodes: [{ id: 0, kind: "master" }, { id: 1, kind: "stereo" }, { id: 2, kind: "bus" }, { id: 3, kind: "bus", is_return: true }] };
  assert.deepEqual(sidechainSources(p, 1).map((n) => n.id), [2]);
});
