import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { returnChoices, parseReturnChoice, viewNodes, mixSources, isReturn, structureKey } from "../js/ui/mixer.js";

function mock() {
  const e = new MockEngine();
  const log = { projects: [], errors: [], history: [] };
  e.on("project", (x) => log.projects.push(x));
  e.on("error", (x) => log.errors.push(x.message));
  e.on("history", (x) => log.history.push(x));
  e.send({ type: "get_project" });
  return { e, log, project: () => log.projects.at(-1) };
}

test("choix de sortie d'un retour : paires stéréo, somme mono, canaux seuls, sorties prises grisées", () => {
  const { e, project } = mock();
  e.send({ type: "add_track", kind: "bus", name: "Retour Marie" });
  e.send({ type: "add_track", kind: "bus", name: "Retour Paul" });
  const [marie, paul] = project().nodes.slice(-2);
  const base = returnChoices(project(), marie);
  assert.equal(base[0].label, "Sortie principale");
  assert.deepEqual(base.filter((c) => c.value.endsWith(":1:0")).map((c) => c.label), ["Sorties 3-4 stéréo", "Sorties 5-6 stéréo", "Sorties 7-8 stéréo"], "8 sorties : trois paires après la principale");
  assert.ok(base.some((c) => c.label === "Sortie 5 mono"));
  assert.deepEqual(parseReturnChoice("3:1:1"), { first: 3, stereo: true, mono: true });
  assert.equal(parseReturnChoice(""), null);
  e.send({ type: "set_return_output", node: marie.id, output: parseReturnChoice("3:1:0") });
  const forPaul = returnChoices(project(), paul);
  assert.ok(forPaul.find((c) => c.value === "3:1:0").disabled && /Retour Marie/.test(forPaul.find((c) => c.value === "3:1:0").label), "paire prise par Marie");
  assert.ok(forPaul.find((c) => c.label.startsWith("Sortie 4 mono")).disabled, "le canal 4 fait partie de sa paire");
  assert.ok(!forPaul.find((c) => c.label.startsWith("Sortie 5 mono")).disabled);
  assert.ok(!returnChoices(project(), marie).find((c) => c.value === "3:1:0").disabled, "pour Marie elle-même, sa paire reste choisissable");
});

test("faux moteur : sortie d'un retour, refus sans trace, annulation", () => {
  const { e, log, project } = mock();
  e.send({ type: "add_track", kind: "bus", name: "Retour Marie" });
  const marie = project().nodes.at(-1);
  e.send({ type: "set_return_output", node: marie.id, output: { first: 5, stereo: false, mono: true } });
  assert.deepEqual(project().nodes.at(-1).phys, { first: 5, stereo: false, mono: true });
  assert.equal(log.history.at(-1).undo_label, "Sortie d'un retour");
  for (const [output, expect] of [[{ first: 2, stereo: true, mono: false }, /sortie principale/], [{ first: 8, stereo: true, mono: false }, /inexistante/], [{ first: 5, stereo: true, mono: false }, /déjà prises/]]) {
    e.send({ type: "set_return_output", node: marie.id === 0 ? 1 : project().nodes.find((n) => n.name === "Batterie").id, output });
  }
  e.send({ type: "add_track", kind: "bus", name: "Retour Paul" });
  const paul = project().nodes.at(-1);
  e.send({ type: "set_return_output", node: paul.id, output: { first: 5, stereo: true, mono: false } });
  assert.match(log.errors.at(-1), /déjà prises/);
  e.send({ type: "set_return_output", node: project().nodes.find((n) => n.kind === "stereo").id, output: { first: 3, stereo: true, mono: false } });
  assert.match(log.errors.at(-1), /seul un bus/);
  assert.equal(project().nodes.find((n) => n.name === "Retour Paul").phys, null);
  e.send({ type: "undo" }); // le dernier changement réussi : la création du bus Paul
  e.send({ type: "undo" });
  assert.equal(project().nodes.find((n) => n.name === "Retour Marie").phys, null, "l'annulation rend le bus à la sortie principale");
  assert.equal(validateCommand({ type: "set_return_output", node: 3, output: null }), null);
  assert.ok(validateCommand({ type: "set_return_output", node: 3, output: { first: 2, stereo: true, mono: false } }));
  assert.ok(validateCommand({ type: "set_return_output", node: 3 }));
});

test("vues du mixeur et vue « Mix de X »", () => {
  const { e, project } = mock();
  e.send({ type: "add_track", kind: "bus", name: "Retour Marie" });
  e.send({ type: "add_track", kind: "bus", name: "Bus batterie" });
  const [marie] = project().nodes.slice(-2);
  e.send({ type: "set_return_output", node: marie.id, output: { first: 3, stereo: true, mono: false } });
  const names = (view) => viewNodes(project(), view).map((n) => n.name);
  assert.ok(names("tracks").every((n) => !/Retour|Bus batterie|Sortie/.test(n) || n === "Sortie"), "pistes seulement (et la sortie principale)");
  assert.deepEqual(names("returns"), ["Retour Marie", "Sortie"]);
  assert.ok(names("buses").includes("Bus batterie") && !names("buses").includes("Retour Marie"));
  assert.ok(names("all").includes("Retour Marie"));
  // « Mix de Marie » : toutes les pistes et les bus qui ne sont pas des retours, avec leur envoi s'il existe.
  const guitar = project().nodes.find((n) => n.name === "Guitare");
  e.send({ type: "add_send", node: guitar.id, to: marie.id });
  e.send({ type: "set_send", node: guitar.id, to: marie.id, gain: 0.5 });
  e.send({ type: "set_send", node: guitar.id, to: marie.id, pre: true });
  const src = mixSources(project(), project().nodes.find((n) => n.id === marie.id));
  assert.ok(!src.some((s) => s.node.id === marie.id), "pas le retour lui-même");
  assert.deepEqual(src.find((s) => s.node.name === "Guitare").send, { to: marie.id, gain: 0.5, pre: true, mute: false, solo: false });
  assert.equal(src.find((s) => s.node.name === "Basse").send, null);
  assert.ok(isReturn(project().nodes.find((n) => n.id === marie.id)));
  assert.notEqual(structureKey(project()), structureKey({ ...project(), nodes: project().nodes.map((n) => ({ ...n, phys: null })) }), "la structure change avec la sortie d'un retour");
});
