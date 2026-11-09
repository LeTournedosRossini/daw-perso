import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { ProjectFiles, windowTitle } from "../js/project-files.js";

function collect(engine, ...types) {
  const log = [];
  for (const t of types) engine.on(t, (e) => log.push(e));
  return log;
}

function setup(answers = {}) {
  globalThis.document = { title: "", getElementById: () => null };
  const engine = new MockEngine();
  const asked = [];
  const dialogs = {
    confirm: async (m) => (asked.push(m), answers.confirm ?? true),
    pickFolder: async () => answers.folder ?? null,
    pickSave: async (n) => (asked.push(`save:${n}`), answers.save ?? null),
  };
  const state = {};
  const loaded = [];
  const files = new ProjectFiles(engine, state, dialogs, { onLoaded: () => loaded.push(1) }, null);
  engine.send({ type: "get_project" });
  return { engine, state, files, asked, loaded };
}

test("validation des commandes de fichier", () => {
  assert.equal(validateCommand({ type: "new_project" }), null);
  assert.equal(validateCommand({ type: "open_project", path: "/x/Chanson.daw" }), null);
  assert.notEqual(validateCommand({ type: "open_project" }), null);
  assert.notEqual(validateCommand({ type: "open_project", path: " " }), null);
  assert.equal(validateCommand({ type: "save_project" }), null);
  assert.equal(validateCommand({ type: "save_project", path: "/x/Chanson" }), null);
  assert.notEqual(validateCommand({ type: "save_project", path: "" }), null);
  assert.notEqual(validateCommand({ type: "save_project", path: 3 }), null);
});

test("titre : le nom, et une pastille tant qu'il y a des modifications", () => {
  assert.equal(windowTitle({ name: "Chanson", dirty: false }), "Chanson — DAW");
  assert.equal(windowTitle({ name: "Chanson", dirty: true }), "● Chanson — DAW");
});

test("modifications non enregistrées : une édition, un réglage de piste ou une annulation en créent ; enregistrer les efface", () => {
  const { engine, state } = setup();
  assert.deepEqual([state.file.name, state.file.dirty, state.file.path], ["Démonstration", false, null]);
  engine.send({ type: "set_node", node: 1, arm: true });
  assert.equal(state.file.dirty, false, "armer n'est pas une modification");
  engine.send({ type: "set_node", node: 1, gain: 0.3 });
  assert.equal(state.file.dirty, true);
  assert.equal(engine.send({ type: "save_project" }), false, "pas de dossier : « Enregistrer sous »");
  assert.equal(engine.send({ type: "save_project", path: "/tmp/Chanson" }), true);
  assert.deepEqual([state.file.name, state.file.path, state.file.dirty], ["Chanson", "/tmp/Chanson.daw", false]);
  engine.send({ type: "move_clip", clip: 11, start: 672_000 });
  assert.equal(state.file.dirty, true);
  assert.equal(engine.send({ type: "save_project" }), true, "dans le dossier du projet");
  assert.equal(state.file.dirty, false);
  engine.send({ type: "undo" });
  assert.equal(state.file.dirty, true);
});

test("nouveau projet et ouverture : le projet est remplacé, l'historique et la lecture repartent de zéro, un numéro de chargement change", () => {
  const { engine, state, loaded } = setup();
  const projects = collect(engine, "project");
  const histories = collect(engine, "history");
  engine.send({ type: "save_project", path: "/tmp/A" });
  engine.send({ type: "move_clip", clip: 11, start: 672_000 });
  const movedStart = projects.at(-1).nodes.flatMap((n) => n.clips).find((c) => c.id === 11).start;
  engine.send({ type: "save_project" });
  engine.send({ type: "new_project" });
  assert.deepEqual([state.file.name, state.file.path, state.file.dirty, state.file.generation], ["Sans titre", null, false, 1]);
  assert.equal(projects.at(-1).nodes.length, 2);
  assert.equal(projects.at(-1).nodes[1].name, "Piste 1");
  assert.equal(histories.at(-1).can_undo, false);
  assert.equal(engine.send({ type: "open_project", path: "/tmp/A.daw" }), true);
  assert.deepEqual([state.file.name, state.file.generation, state.file.dirty], ["A", 2, false]);
  assert.equal(projects.at(-1).nodes.flatMap((n) => n.clips).find((c) => c.id === 11).start, movedStart, "le projet enregistré est revenu");
  assert.equal(histories.at(-1).can_undo, false, "pas d'historique après une ouverture");
  assert.equal(loaded.length, 2, "la page est prévenue de chaque chargement");
  assert.equal(engine.send({ type: "open_project", path: "/tmp/inconnu.daw" }), false);
});

test("boutons : confirmation avant de perdre des modifications, Enregistrer sous quand il n'y a pas de dossier", async () => {
  const { engine, state, files, asked } = setup({ confirm: false, folder: "/tmp/B.daw", save: "/tmp/B" });
  engine.send({ type: "set_node", node: 1, gain: 0.3 });
  await files.newProject();
  assert.equal(asked.length, 1, "confirmation demandée");
  assert.equal(state.file.generation, 0, "refusée : rien n'a changé");
  await files.open();
  assert.equal(state.file.generation, 0);
  // « Enregistrer » sans dossier passe par « Enregistrer sous » (nom proposé : Mon projet).
  await files.save();
  assert.equal(asked.at(-1), "save:Mon projet");
  assert.deepEqual([state.file.name, state.file.dirty], ["B", false]);
  // Plus de modification : plus de confirmation ; Ouvrir demande le dossier au sélecteur.
  const before = asked.length;
  await files.newProject();
  assert.equal(asked.length, before, "rien à perdre : pas de confirmation");
  assert.equal(state.file.generation, 1);
  await files.open();
  assert.deepEqual([state.file.name, state.file.generation], ["B", 2]);
});

test("sauvegarde automatique : la reprise est proposée, acceptée ou refusée par la commande qui convient", async () => {
  for (const [answer, expected] of [[true, { type: "open_project", path: "/x/P.daw", recover: true }], [false, { type: "discard_autosave", path: "/x/P.daw" }]]) {
    const { engine, asked } = setup({ confirm: answer });
    const sent = [];
    const send = engine.send.bind(engine);
    engine.send = (m) => (sent.push(m), send(m));
    engine.emit("autosave", { path: "/x/P.daw", untitled: false, modified: 1_800_000_000 });
    await new Promise((r) => setTimeout(r, 20));
    assert.match(asked.at(-1), /sauvegarde automatique/);
    assert.deepEqual(sent.find((m) => m.type === expected.type), expected);
  }
  assert.equal(validateCommand({ type: "discard_autosave", path: " " }) === null, false);
});
