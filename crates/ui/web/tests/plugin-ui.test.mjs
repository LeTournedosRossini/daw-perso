import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";

function setup() {
  const e = new MockEngine();
  const log = { ui: [], errors: [], projects: [] };
  e.on("plugin_ui", (x) => log.ui.push(x));
  e.on("error", (x) => log.errors.push(x));
  e.on("project", (x) => log.projects.push(x));
  e.send({ type: "get_project" });
  const insert = (clap_id, node = 2) => {
    e.send({ type: "add_plugin", node, clap_id });
    return log.projects.at(-1).nodes.find((n) => n.id === node).inserts.at(-1);
  };
  const find = (id) => log.projects.at(-1).nodes.flatMap((n) => n.inserts).find((i) => i.id === id);
  return { e, log, insert, find };
}

test("validation : open_plugin_ui et close_plugin_ui", () => {
  assert.equal(validateCommand({ type: "open_plugin_ui", insert: 3 }), null);
  assert.equal(validateCommand({ type: "close_plugin_ui", insert: 0 }), null);
  assert.notEqual(validateCommand({ type: "open_plugin_ui" }), null);
  assert.notEqual(validateCommand({ type: "open_plugin_ui", insert: -1 }), null);
  assert.notEqual(validateCommand({ type: "close_plugin_ui", insert: "a" }), null);
});

test("faux moteur : les plugins annoncent leur interface, ouvrir et fermer se voient dans le projet", () => {
  const { e, log, insert, find } = setup();
  const comp = insert("mock.compresseur");
  assert.deepEqual([comp.gui, comp.ui_open], [true, false]);
  e.send({ type: "open_plugin_ui", insert: comp.id });
  assert.deepEqual(log.ui.at(-1), { type: "plugin_ui", insert: comp.id, open: true });
  assert.equal(find(comp.id).ui_open, true);
  e.send({ type: "open_plugin_ui", insert: comp.id }); // déjà ouverte : rien de plus, toujours ouverte
  assert.equal(find(comp.id).ui_open, true);
  e.send({ type: "close_plugin_ui", insert: comp.id });
  assert.deepEqual(log.ui.at(-1), { type: "plugin_ui", insert: comp.id, open: false });
  assert.equal(find(comp.id).ui_open, false);
  assert.equal(log.errors.length, 0);
});

test("faux moteur : refus, interface qui se ferme avec son plugin ou son projet", () => {
  const { e, log, insert, find } = setup();
  const tal = insert("mock.tal-chorus");
  assert.equal(tal.gui, false, "un plugin sans interface native");
  e.send({ type: "open_plugin_ui", insert: tal.id });
  assert.match(log.errors.pop().message, /pas d'interface/);
  e.send({ type: "open_plugin_ui", insert: 9999 });
  assert.match(log.errors.pop().message, /inconnu/);
  e.send({ type: "close_plugin_ui", insert: 9999 }); // fermer ce qui n'existe pas : sans effet
  assert.equal(log.errors.length, 0);
  const comp = insert("mock.compresseur");
  e.send({ type: "open_plugin_ui", insert: comp.id });
  e.send({ type: "remove_plugin", insert: comp.id });
  e.send({ type: "undo" }); // le plugin revient : son interface ne se rouvre pas toute seule
  assert.equal(find(comp.id).ui_open, false);
  e.send({ type: "open_plugin_ui", insert: comp.id });
  assert.equal(find(comp.id).ui_open, true);
  e.send({ type: "new_project" });
  assert.ok(log.projects.at(-1).nodes.every((n) => n.inserts.every((i) => !i.ui_open)));
});
