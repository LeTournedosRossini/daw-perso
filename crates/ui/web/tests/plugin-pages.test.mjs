import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";

function collect(e, ...types) {
  const log = [];
  for (const t of types) e.on(t, (x) => log.push({ t, ...x }));
  return log;
}

test("banque de pages de contrôle : validation des commandes", () => {
  assert.equal(validateCommand({ type: "get_control_pages" }), null);
  assert.equal(validateCommand({ type: "get_insert_page", insert: 3 }), null);
  assert.equal(validateCommand({ type: "set_control_page", plugin_id: "p", name: "P", params: [{ id: 1, name: "a" }] }), null);
  assert.equal(validateCommand({ type: "set_control_page", plugin_id: "p", name: "", params: [] }), null);
  const many = Array.from({ length: 17 }, (_, i) => ({ id: i, name: `p${i}` }));
  for (const bad of [
    { type: "set_control_page", plugin_id: "", name: "", params: [] },
    { type: "set_control_page", plugin_id: "p", name: "", params: many },
    { type: "set_control_page", plugin_id: "p", name: "", params: [{ id: 1, name: "a" }, { id: 1, name: "b" }] },
    { type: "set_control_page", plugin_id: "p", name: "", params: [{ id: -1, name: "a" }] },
    { type: "set_control_page", plugin_id: "p", name: "", params: "oui" },
    { type: "get_insert_page", insert: "x" },
  ]) assert.notEqual(validateCommand(bad), null, JSON.stringify(bad));
});

test("faux moteur : définir une page, la retrouver par identifiant ou par nom, la supprimer", () => {
  const e = new MockEngine();
  const pages = collect(e, "control_pages");
  const ip = collect(e, "insert_page");
  e.send({ type: "get_project" });
  const node = e.project.nodes.find((n) => n.name === "Basse");
  e.send({ type: "add_plugin", node: node.id, clap_id: "mock.compresseur", file: "Compresseur.clap" });
  const ins = e.project.nodes.find((n) => n.id === node.id).inserts.at(-1).id;
  e.send({ type: "get_insert_page", insert: ins });
  assert.equal(ip.at(-1).plugin_id, "mock.compresseur");
  assert.equal(ip.at(-1).configured, false);
  e.send({ type: "set_control_page", plugin_id: "mock.compresseur", name: "Compresseur", params: [{ id: 0, name: "Seuil" }, { id: 9, name: "Ratio" }, { id: 8, name: "Inexistant" }] });
  assert.equal(pages.at(-1).entries.length, 1);
  e.send({ type: "get_insert_page", insert: ins });
  const p = ip.at(-1);
  assert.equal(p.configured, true);
  assert.deepEqual(p.params.map((x) => x.name), ["Seuil", "Ratio"], "Ratio retrouvé par son nom (identifiant changé)");
  assert.deepEqual(p.missing, ["Inexistant"]);
  e.send({ type: "set_control_page", plugin_id: "mock.compresseur", name: "", params: [] });
  assert.equal(pages.at(-1).entries.length, 0);
});
