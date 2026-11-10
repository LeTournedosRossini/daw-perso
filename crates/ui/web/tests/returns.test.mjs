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
  e.send({ type: "add_track", kind: "return", name: "Retour Marie" });
  e.send({ type: "add_track", kind: "return", name: "Retour Paul" });
  const [marie, paul] = project().nodes.slice(-2);
  const base = returnChoices(project(), marie);
  assert.equal(base[0].label, "Aucune (muet)", "un retour sans sortie est muet");
  assert.equal(returnChoices(project(), { id: 99, is_return: false })[0].label, "Sortie principale", "un bus ordinaire garde la sortie principale");
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
  e.send({ type: "add_track", kind: "return", name: "Retour Marie" });
  const marie = project().nodes.at(-1);
  e.send({ type: "set_return_output", node: marie.id, output: { first: 5, stereo: false, mono: true } });
  assert.deepEqual(project().nodes.at(-1).phys, { first: 5, stereo: false, mono: true });
  assert.equal(log.history.at(-1).undo_label, "Sortie d'un retour");
  for (const [output, expect] of [[{ first: 2, stereo: true, mono: false }, /sortie principale/], [{ first: 8, stereo: true, mono: false }, /inexistante/], [{ first: 5, stereo: true, mono: false }, /déjà prises/]]) {
    e.send({ type: "set_return_output", node: marie.id === 0 ? 1 : project().nodes.find((n) => n.name === "Batterie").id, output });
  }
  e.send({ type: "add_track", kind: "return", name: "Retour Paul" });
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
  e.send({ type: "add_track", kind: "return", name: "Retour Marie" });
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

test("un retour est un type à part : création avec sa sortie en un pas, bus ordinaire avec sortie, refus", () => {
  const { e, project } = mock();
  const history = [];
  e.on("history", (h) => history.push(h));
  e.send({ type: "add_track", kind: "return", name: "Retour Lou", output: { first: 3, stereo: true, mono: false } });
  const lou = project().nodes.find((n) => n.name === "Retour Lou");
  assert.ok(lou.is_return && lou.kind === "bus" && lou.phys.first === 3);
  assert.equal(history.at(-1).undo_label, "Ajouter un retour");
  e.send({ type: "undo" });
  assert.ok(!project().nodes.some((n) => n.name === "Retour Lou"), "un seul pas retire le retour entier");
  e.send({ type: "redo" });
  // Sans sortie : créé muet.
  e.send({ type: "add_track", kind: "return", name: "Retour Nono" });
  const nono = project().nodes.find((n) => n.name === "Retour Nono");
  assert.ok(nono.is_return && nono.phys === null);
  // Un bus ordinaire sort sur la carte sans devenir un retour ; les canaux pris par un retour lui sont refusés.
  e.send({ type: "add_track", kind: "bus", name: "Sous-groupe", output: { first: 5, stereo: true, mono: false } });
  const sub = project().nodes.find((n) => n.name === "Sous-groupe");
  assert.ok(!sub.is_return && sub.phys.first === 5);
  const before = project().nodes.length;
  const errors = [];
  e.on("error", (x) => errors.push(x));
  e.send({ type: "add_track", kind: "return", name: "Pris", output: { first: 3, stereo: false, mono: true } });
  assert.equal(errors.length, 1);
  assert.equal(project().nodes.length, before, "refus sans trace");
  assert.ok(validateCommand({ type: "add_track", kind: "mono", output: { first: 3, stereo: false, mono: true } }), "une piste n'a pas de sortie physique");
  assert.equal(validateCommand({ type: "add_track", kind: "return", output: { first: 3, stereo: true, mono: false } }), null);
  // Seul un retour a ses propres groupes et une adresse (QR) ; un bus ordinaire n'en a pas.
  e.send({ type: "customize_return_groups", ret: sub.id });
  assert.equal(errors.length, 2);
});

test("copier les groupes d'un autre retour : nouveaux identifiants, remplace les siens, annulable", () => {
  const { e, project } = mock();
  e.send({ type: "add_track", kind: "return", name: "Retour A" });
  e.send({ type: "add_track", kind: "return", name: "Retour B" });
  const [a, b] = project().nodes.slice(-2);
  const guitar = project().nodes.find((n) => n.name === "Guitare");
  e.send({ type: "customize_return_groups", ret: a.id });
  e.send({ type: "create_group", name: "Cordes", color: "#ff8800", nodes: [guitar.id], ret: a.id });
  e.send({ type: "copy_return_groups", ret: b.id, from: a.id });
  const copy = project().nodes.find((n) => n.id === b.id).return_groups;
  const own = project().nodes.find((n) => n.id === a.id).return_groups;
  assert.deepEqual(copy.groups.map((g) => g.name), own.groups.map((g) => g.name));
  assert.ok(copy.groups.every((g) => !own.groups.some((o) => o.id === g.id)), "identifiants neufs");
  assert.ok(copy.members.some(([n]) => n === guitar.id));
  const errors = [];
  e.on("error", (x) => errors.push(x));
  e.send({ type: "copy_return_groups", ret: b.id, from: b.id });
  assert.equal(errors.length, 1, "pas de copie de soi-même");
  assert.ok(validateCommand({ type: "copy_return_groups", ret: 3 }), "from manque");
  e.send({ type: "undo" });
  assert.ok(!project().nodes.find((n) => n.id === b.id).return_groups, "annulé : il suit de nouveau ceux du projet");
});
