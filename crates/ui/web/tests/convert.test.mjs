import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { ProjectFiles } from "../js/project-files.js";

function setup() {
  const e = new MockEngine();
  const log = { mismatch: [], convert: [], errors: [], files: [], projects: [] };
  e.on("rate_mismatch", (x) => log.mismatch.push(x));
  e.on("convert", (x) => log.convert.push(x));
  e.on("error", (x) => log.errors.push(x));
  e.on("file", (x) => log.files.push(x));
  e.on("project", (x) => log.projects.push(x));
  e.send({ type: "get_project" });
  // Un projet enregistré « à 44,1 kHz » (le faux moteur ne lit que ce qu'il a lui-même enregistré).
  e.send({ type: "save_project", path: "/p/Vieux" });
  e.disk.get("/p/Vieux.daw").rate = 44_100;
  e.send({ type: "new_project" });
  return { e, log };
}
const settle = (e, ms = 2000) => { for (let i = 0; i < ms / 100; i++) e.advance(100); };

test("validation : convert_project, cancel_convert", () => {
  assert.equal(validateCommand({ type: "convert_project", path: "/p/Vieux.daw" }), null);
  assert.equal(validateCommand({ type: "cancel_convert" }), null);
  assert.notEqual(validateCommand({ type: "convert_project", path: "  " }), null);
  assert.notEqual(validateCommand({ type: "convert_project" }), null);
});

test("faux moteur : un projet à une autre fréquence propose la conversion et ne change rien", () => {
  const { e, log } = setup();
  const before = log.projects.length;
  e.send({ type: "open_project", path: "/p/Vieux.daw" });
  assert.deepEqual(log.mismatch.at(-1), { type: "rate_mismatch", path: "/p/Vieux.daw", project_rate: 44100, engine_rate: 48000 });
  assert.equal(log.projects.length, before, "aucun projet chargé");
  assert.equal(log.errors.length, 0);
});

test("faux moteur : la copie convertie est créée, ouverte, l'original reste intact", () => {
  const { e, log } = setup();
  e.send({ type: "convert_project", path: "/p/Vieux.daw" });
  assert.equal(log.convert.at(-1).active, true);
  settle(e);
  const end = log.convert.at(-1);
  assert.equal(end.active, false);
  assert.equal(end.path, "/p/Vieux (48000 Hz).daw");
  assert.ok(log.convert.map((c) => c.progress).every((p, i, a) => i === 0 || p >= a[i - 1]));
  assert.equal(log.files.at(-1).name, "Vieux (48000 Hz)");
  assert.equal(e.disk.get("/p/Vieux.daw").rate, 44100, "l'original n'a pas bougé");
  // Ouvrir la copie ne propose plus rien ; une seconde conversion ne recouvre pas la première.
  const n = log.mismatch.length;
  e.send({ type: "open_project", path: "/p/Vieux (48000 Hz).daw" });
  assert.equal(log.mismatch.length, n);
  e.send({ type: "convert_project", path: "/p/Vieux.daw" });
  settle(e);
  assert.equal(log.convert.at(-1).path, "/p/Vieux (48000 Hz) (2).daw");
});

test("faux moteur : refus et annulation", () => {
  const { e, log } = setup();
  e.send({ type: "convert_project", path: "/p/Inconnu.daw" });
  assert.match(log.errors.pop().message, /introuvable/);
  e.send({ type: "save_project", path: "/p/Actuel" });
  e.send({ type: "convert_project", path: "/p/Actuel.daw" });
  assert.match(log.errors.pop().message, /déjà/);
  e.send({ type: "convert_project", path: "/p/Vieux.daw" });
  e.send({ type: "convert_project", path: "/p/Vieux.daw" });
  assert.match(log.errors.pop().message, /déjà/);
  e.send({ type: "new_project" });
  assert.match(log.errors.pop().message, /conversion/);
  e.send({ type: "cancel_convert" });
  e.advance(30);
  assert.equal(log.convert.at(-1).cancelled, true);
  assert.equal(e.disk.has("/p/Vieux (48000 Hz).daw"), false, "rien n'est créé");
  e.send({ type: "new_project" });
  assert.equal(log.errors.length, 0);
});

test("page : la proposition passe par la confirmation, puis envoie la conversion", async () => {
  const { e } = setup();
  const sent = [];
  const realSend = e.send.bind(e);
  e.send = (m) => (sent.push(m.type), realSend(m));
  const asked = [];
  globalThis.document = { getElementById: () => null, title: "" };
  const dialogs = { confirm: async (message, labels) => (asked.push([message, labels]), true), pickFolder: async () => null, pickSave: async () => null };
  new ProjectFiles(e, { project: { sample_rate: 48000 } }, dialogs, {}, null);
  e.send({ type: "open_project", path: "/p/Vieux.daw" });
  await new Promise((r) => setTimeout(r, 10));
  assert.equal(asked.length, 1);
  assert.match(asked[0][0], /44100 Hz.*48000 Hz.*l'original n'est pas modifié/);
  assert.equal(asked[0][1].ok, "Convertir");
  assert.ok(sent.includes("convert_project"));
  // Refusé : rien n'est envoyé.
  dialogs.confirm = async () => false;
  sent.length = 0;
  e.send({ type: "open_project", path: "/p/Vieux.daw" });
  await new Promise((r) => setTimeout(r, 10));
  assert.ok(!sent.includes("convert_project"));
  delete globalThis.document;
});
