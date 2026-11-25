import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { viewNodes, childrenOf, rootOf, familyDepth, tapText } from "../js/ui/mixer.js";
import { orderedRows } from "../js/ui/order.js";

function setup() {
  const e = new MockEngine();
  let project = null;
  e.on("project", (p) => (project = p));
  e.send({ type: "get_project" });
  const g = project.nodes.find((n) => n.name === "Guitare").id;
  e.send({ type: "add_parallel", node: g, tap: "input", name: "P1" });
  e.send({ type: "add_parallel", node: g, tap: "post_fader", name: "P2" });
  const p1 = project.nodes.find((n) => n.name === "P1").id;
  e.send({ type: "add_parallel", node: p1, tap: "pre_fader", name: "P1a" });
  return { e, project: () => project, g, p1 };
}
const names = (list) => list.map((n) => n.name);

test("la parallèle connaît sa parente et se range juste après sa famille", () => {
  const { project, g } = setup();
  const order = names(project().nodes);
  const at = order.indexOf("Guitare");
  assert.deepEqual(order.slice(at, at + 4), ["Guitare", "P1", "P1a", "P2"]);
  assert.deepEqual(names(childrenOf(project(), g)), ["P1", "P2"]);
  const a = project().nodes.find((n) => n.name === "P1a");
  assert.equal(rootOf(project(), a).name, "Guitare");
  assert.equal(familyDepth(project(), a), 2);
  assert.equal(familyDepth(project(), project().nodes.find((n) => n.name === "Guitare")), 0);
});

test("mixeur : les parallèles se montrent sous leur parente, dépliées par défaut, et un repli cache aussi les petites-filles", () => {
  const { project, g, p1 } = setup();
  assert.deepEqual(names(viewNodes(project(), "tracks")).filter((x) => x.startsWith("P") || x === "Guitare"), ["Guitare", "P1", "P1a", "P2"]);
  assert.deepEqual(names(viewNodes(project(), "tracks", new Set([g]))).filter((x) => x.startsWith("P")), []);
  assert.deepEqual(names(viewNodes(project(), "tracks", new Set([p1]))).filter((x) => x.startsWith("P")), ["P1", "P2"]);
  // La vue « Bus » ne montre pas les parallèles d'une piste (elles suivent leur racine).
  assert.deepEqual(names(viewNodes(project(), "buses")).filter((x) => x.startsWith("P")), []);
});

test("texte du point de captation", () => {
  const { project } = setup();
  const t = (name) => tapText(project(), project().nodes.find((n) => n.name === name));
  assert.equal(t("P1"), "avant les effets");
  assert.equal(t("P2"), "après le fader");
  assert.equal(t("P1a"), "après les effets");
  assert.equal(tapText(project(), project().nodes.find((n) => n.name === "Guitare")), "");
});

test("timeline : les parallèles sont repliées par défaut et s'ouvrent parente par parente", () => {
  const { project, g, p1 } = setup();
  const rows = (open) => names(orderedRows(project(), new Set(), null, 4, new Set(), open)).filter((x) => x.startsWith("P") || x === "Guitare");
  assert.deepEqual(rows(new Set()), ["Guitare"]);
  assert.deepEqual(rows(new Set([g])), ["Guitare", "P1", "P2"]);
  assert.deepEqual(rows(new Set([g, p1])), ["Guitare", "P1", "P1a", "P2"]);
});

test("faux moteur : déplacer la parente emmène sa famille, une parallèle ne bouge pas seule", () => {
  const { e, project, g } = setup();
  const basse = project().nodes.find((n) => n.name === "Basse").id;
  const p2 = project().nodes.find((n) => n.name === "P2").id;
  e.send({ type: "move_track", node: g, before: basse });
  assert.deepEqual(names(project().nodes).slice(1, 8), ["Batterie", "Guitare", "P1", "P1a", "P2", "Basse", "Claviers"]);
  const before = names(project().nodes);
  e.send({ type: "move_track", node: p2, before: basse });
  assert.deepEqual(names(project().nodes), before, "refusée sans trace");
});

test("faux moteur : supprimer la parente supprime ses parallèles, une seule annulation remet tout", () => {
  const { e, project, g } = setup();
  const full = names(project().nodes);
  e.send({ type: "delete_track", node: g });
  assert.equal(project().nodes.length, full.length - 4);
  e.send({ type: "undo" });
  assert.deepEqual(names(project().nodes), full);
  assert.equal(project().nodes.find((n) => n.name === "P1a").parent, project().nodes.find((n) => n.name === "P1").id);
});
