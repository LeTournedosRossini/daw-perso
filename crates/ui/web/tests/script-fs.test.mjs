import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import fs from "node:fs";
import { MockEngine } from "../js/mock-engine.js";
import { ScriptRunner } from "../js/script-runner.js";
import { ScriptFs, MemoryFsBackend, normalizePath, inRoots, joinPath, hashText, unknownPermissions, consentText, MAX_TEXT } from "../js/script-fs.js";
import { parseHeader } from "../js/script-runner.js";

function fakeWorker(source) {
  let terminated = false;
  const ctx = { setTimeout, navigator: {} };
  ctx.self = ctx;
  const w = { onmessage: null, onerror: null, postMessage: (data) => queueMicrotask(() => !terminated && ctx.onmessage({ data })), terminate: () => (terminated = true) };
  ctx.postMessage = (data) => queueMicrotask(() => !terminated && w.onmessage?.({ data }));
  vm.createContext(ctx);
  vm.runInContext(source, ctx);
  return w;
}

const PROJET = "C:/Users/lcj/Musique/Chanson.daw";
const SCRIPTS = "C:/Users/lcj/AppData/Local/DAW/scripts";

function setup({ confirm = async () => true, projectDir = PROJET } = {}) {
  const engine = new MockEngine();
  const state = { project: null, pos: 0, playing: false, loop: null, selected: new Set(), history: null, lyrics: [], structure: [], chordTrack: null };
  engine.on("project", (p) => (state.project = p));
  engine.on("history", (h) => (state.history = h));
  engine.on("chords", (e) => (state.lyrics = e.lyrics ?? []));
  engine.send({ type: "get_project" });
  engine.send({ type: "get_chords" });
  const backend = new MemoryFsBackend();
  const confirms = [];
  const fsHandler = new ScriptFs({ name: "essai", backend, projectDir, scriptsDir: SCRIPTS, confirm: async (m) => (confirms.push(m), confirm(m)) });
  const runner = new ScriptRunner(engine, state, fakeWorker);
  const run = (src, extra = {}) => runner.run(src, { name: "essai", fs: fsHandler, ...extra });
  return { engine, state, backend, runner, run, confirms, fsHandler };
}

test("chemins : normalisation, appartenance à une racine, jonction, empreinte", () => {
  assert.equal(normalizePath("C:\\Users\\LCJ\\a\\..\\b\\.\\c"), "c:/users/lcj/b/c");
  assert.equal(normalizePath("/home/jean//x/"), "/home/jean/x");
  assert.ok(inRoots("C:/Users/lcj/Musique/Chanson.daw/export/x.txt", [PROJET]));
  assert.ok(inRoots("c:\\users\\lcj\\musique\\chanson.daw", [PROJET]), "la racine elle-même, sans tenir compte de la casse sous Windows");
  assert.ok(!inRoots("C:/Users/lcj/Musique/Chanson.daw-bis/x.txt", [PROJET]), "un préfixe commun n'est pas « dedans »");
  assert.ok(!inRoots("C:/Users/lcj/Musique/Chanson.daw/../Autre/x.txt", [PROJET]), "« .. » ne sort pas de la racine");
  assert.ok(!inRoots("/home/jean/Secret", ["/home/jean/secret"]), "sous Linux la casse compte");
  assert.ok(!inRoots("/x", [null, ""]));
  assert.equal(joinPath("C:\\a\\", "/b/", "c.txt"), "C:\\a/b/c.txt");
  assert.equal(hashText("abc"), hashText("abc"));
  assert.notEqual(hashText("abc"), hashText("abd"));
  assert.deepEqual(unknownPermissions(["files", "net", "x"]), ["net", "x"]);
  assert.match(consentText("Mon script"), /Mon script/);
});

test("sans « @permissions files » (pas de ScriptFs) : refus net ; join reste disponible", async () => {
  const { runner } = setup();
  const r = await runner.run(`await daw.fs.readText("x");`);
  assert.equal(r.ok, false);
  assert.match(r.error, /@permissions files/);
  const j = await runner.run(`return daw.fs.join("a/", "/b", "c.txt");`);
  assert.equal(j.result, "a/b/c.txt");
});

test("lire et écrire dans le dossier du projet ; refus hors des racines ; extensions interdites", async () => {
  const { run, backend } = setup();
  backend.files.set(normalizePath(`${PROJET}/notes.txt`), "bonjour");
  let r = await run(`return await daw.fs.readText(daw.fs.join(await daw.fs.projectDir(), "notes.txt"));`);
  assert.equal(r.result, "bonjour", r.error);
  r = await run(`await daw.fs.writeText(daw.fs.join(await daw.fs.projectDir(), "sortie.txt"), "écrit");`);
  assert.equal(r.ok, true, r.error);
  assert.equal(backend.files.get(normalizePath(`${PROJET}/sortie.txt`)), "écrit");
  r = await run(`await daw.fs.readText("C:/Windows/System32/drivers/etc/hosts");`);
  assert.match(r.error, /pas le droit/);
  r = await run(`await daw.fs.readText(daw.fs.join(await daw.fs.projectDir(), "..", "Autre", "x.txt"));`);
  assert.match(r.error, /pas le droit/);
  r = await run(`await daw.fs.writeText(daw.fs.join(await daw.fs.projectDir(), "virus.exe"), "x");`);
  assert.match(r.error, /ne peut pas écrire/);
  r = await run(`await daw.fs.writeText(daw.fs.join(await daw.fs.projectDir(), "x.js"), "x");`);
  assert.match(r.error, /ne peut pas écrire/);
  r = await run(`await daw.fs.writeText(daw.fs.join(await daw.fs.projectDir(), "gros.txt"), "x".repeat(${MAX_TEXT + 1}));`);
  assert.match(r.error, /trop long/);
  r = await run(`await daw.fs.writeText("pas/de/racine", 12);`);
  assert.equal(r.ok, false);
});

test("un fichier choisi dans un dialogue devient accessible ; ses voisins non ; annuler rend null", async () => {
  const { run, backend } = setup();
  backend.files.set(normalizePath("D:/Paroles/chanson.txt"), "ma-man est");
  backend.files.set(normalizePath("D:/Paroles/prive.txt"), "non");
  backend.queue.push("D:/Paroles/chanson.txt");
  let r = await run(`const p = await daw.fs.pickFile({ extensions: ["txt"] }); const t = await daw.fs.readText(p); let voisin; try { await daw.fs.readText("D:/Paroles/prive.txt"); voisin = "lu"; } catch (e) { voisin = "refusé"; } return [t, voisin];`);
  assert.deepEqual([...r.result], ["ma-man est", "refusé"], r.error);
  r = await run(`return await daw.fs.pickFile();`); // file vide : l'utilisateur a annulé
  assert.equal(r.result, null);
  backend.queue.push("D:/Dossier");
  backend.files.set(normalizePath("D:/Dossier/a.txt"), "1");
  backend.files.set(normalizePath("D:/Dossier/sous/b.txt"), "22");
  r = await run(`const d = await daw.fs.pickFolder(); return await daw.fs.list(d);`);
  assert.deepEqual(JSON.parse(JSON.stringify(r.result)), [{ name: "a.txt", is_dir: false, size: 1 }, { name: "sous", is_dir: true, size: 0 }]);
});

test("remplacer un fichier demande confirmation ; refusée, rien ne change ; « Enregistrer sous » l'a déjà demandée", async () => {
  const refuse = setup({ confirm: async () => false });
  refuse.backend.files.set(normalizePath(`${PROJET}/a.txt`), "ancien");
  let r = await refuse.run(`await daw.fs.writeText(daw.fs.join(await daw.fs.projectDir(), "a.txt"), "nouveau", { overwrite: true });`);
  assert.match(r.error, /refusé/);
  assert.equal(refuse.backend.files.get(normalizePath(`${PROJET}/a.txt`)), "ancien");
  assert.equal(refuse.confirms.length, 1);
  assert.match(refuse.confirms[0], /a\.txt/);
  // Sans overwrite : l'existant est refusé sans même demander.
  r = await refuse.run(`await daw.fs.writeText(daw.fs.join(await daw.fs.projectDir(), "a.txt"), "x");`);
  assert.match(r.error, /existe déjà/);
  assert.equal(refuse.confirms.length, 1);
  const accepte = setup();
  accepte.backend.files.set(normalizePath(`${PROJET}/a.txt`), "ancien");
  r = await accepte.run(`await daw.fs.writeText(daw.fs.join(await daw.fs.projectDir(), "a.txt"), "nouveau", { overwrite: true });`);
  assert.equal(r.ok, true, r.error);
  assert.equal(accepte.backend.files.get(normalizePath(`${PROJET}/a.txt`)), "nouveau");
  // Un fichier choisi par « Enregistrer sous » : le dialogue a demandé, pas de seconde confirmation.
  accepte.backend.files.set(normalizePath("D:/out/sortie.csv"), "ancien");
  accepte.backend.queue.push("D:/out/sortie.csv");
  const avant = accepte.confirms.length;
  r = await accepte.run(`const p = await daw.fs.pickSave({ defaultName: "sortie.csv" }); await daw.fs.writeText(p, "neuf", { overwrite: true });`);
  assert.equal(r.ok, true, r.error);
  assert.equal(accepte.confirms.length, avant, "pas de confirmation en plus");
  assert.equal(accepte.backend.files.get(normalizePath("D:/out/sortie.csv")), "neuf");
});

test("projet jamais enregistré : projectDir vaut null et le dossier du projet n'est pas accessible", async () => {
  const { run } = setup({ projectDir: null });
  let r = await run(`return await daw.fs.projectDir();`);
  assert.equal(r.result, null);
  r = await run(`await daw.fs.readText("null/x.txt");`);
  assert.equal(r.ok, false);
});

test("un dialogue ne fait pas courir le délai du script ; hors d'une réaction, plus d'action", async () => {
  const slow = setup();
  const lent = slow.backend.pickFile.bind(slow.backend);
  slow.backend.pickFile = async () => (await new Promise((r) => setTimeout(r, 120)), lent());
  slow.backend.queue.push("D:/x.txt");
  const r = await slow.run(`return await daw.fs.pickFile();`, { timeoutMs: 40 });
  assert.equal(r.ok, true, r.error);
  assert.equal(r.result, "D:/x.txt");
  // Un minuteur du script ne peut pas agir après le premier passage (hors d'une réaction).
  const t = setup();
  const logs = [];
  const ok = await t.run(`daw.on("play", () => {}); setTimeout(async () => { try { await daw.send({ type: "add_track", kind: "bus", name: "Fuite" }); daw.log("passé"); } catch (e) { daw.log("refusé : " + e.message); } }, 30);`, { onLog: (l) => logs.push(l) });
  assert.equal(ok.ok, true, ok.error);
  await new Promise((res) => setTimeout(res, 120));
  assert.ok(logs.some((l) => /^refusé : hors d'une réaction/.test(l)), logs.join("|"));
  assert.ok(!t.state.project.nodes.some((n) => n.name === "Fuite"));
  t.runner.abort();
});

test("les permissions se lisent dans l'en-tête", () => {
  assert.deepEqual(parseHeader("// @permissions files\nx").permissions, ["files"]);
  assert.deepEqual(parseHeader("// @name a\nx").permissions, []);
});

// ---- les scripts d'exemple, tels que livrés ----------------------------------------------------------------------------------
const exemple = (nom) => fs.readFileSync(new URL(`../../../../docs/scripts-exemples/${nom}`, import.meta.url), "utf8");

test("exemple « Paroles depuis un fichier » : syllabes posées, autres couplets conservés, un pas d'annulation", async () => {
  const { engine, state, run, backend } = setup();
  engine.send({ type: "set_lyrics", syllables: [{ at: [0, 1], text: "ancien", verse: 1 }, { at: [1, 4], text: "deux", verse: 2 }] });
  assert.deepEqual(state.lyrics.map((l) => l.text), ["ancien", "deux"]);
  backend.files.set(normalizePath("D:/p.txt"), "ma-man est\nla plus bel-le");
  backend.queue.push("D:/p.txt");
  engine.emit("position", { pos: 0, playing: false, loop: null });
  state.pos = 0;
  const src = exemple("Paroles depuis un fichier.js");
  assert.ok(parseHeader(src).permissions.includes("files"));
  const r = await run(src, { name: "paroles" });
  assert.equal(r.ok, true, r.error);
  assert.match(r.log[0], /7 syllabes/);
  const v1 = state.lyrics.filter((l) => (l.verse ?? 1) === 1).map((l) => l.text);
  assert.deepEqual(v1, ["ma", "man", "est", "la", "plus", "bel", "le"], "« ancien » à 0 était dans la zone : remplacé");
  assert.deepEqual(state.lyrics.filter((l) => l.verse === 2).map((l) => l.text), ["deux"], "le couplet 2 est conservé");
  const ma = state.lyrics.find((l) => l.text === "ma");
  assert.equal(ma.hyphen, true);
  engine.send({ type: "undo" });
  assert.deepEqual(state.lyrics.map((l) => l.text), ["ancien", "deux"], "un seul annuler");
});

test("exemple « Exporter les notes en CSV »", async () => {
  const { state, run, backend } = setup();
  const midi = state.project.nodes.flatMap((n) => n.clips).find((c) => c.kind === "midi");
  state.selected = new Set([midi.id]);
  backend.queue.push("D:/notes.csv");
  const r = await run(exemple("Exporter les notes en CSV.js"), { name: "csv" });
  assert.equal(r.ok, true, r.error);
  const csv = backend.files.get(normalizePath("D:/notes.csv"));
  const lignes = csv.trim().split("\n");
  assert.equal(lignes[0], "debut_noires;duree_noires;touche;velocite;canal");
  assert.ok(lignes.length > 4);
  assert.match(lignes[1], /^\d+(\.\d+)?;\d+(\.\d+)?;\d+;0\.\d{3};\d+$/);
  const sans = await run(exemple("Exporter les notes en CSV.js"), { name: "csv" }); // plus de sélection
  void sans;
  state.selected = new Set();
  assert.match((await run(exemple("Exporter les notes en CSV.js"))).error, /Sélectionne d'abord/);
});

test("exemple « Rapport du projet en fichier » : remplace seulement après confirmation", async () => {
  const { run, backend, confirms } = setup();
  let r = await run(exemple("Rapport du projet en fichier.js"), { name: "rapport" });
  assert.equal(r.ok, true, r.error);
  const clef = normalizePath(`${PROJET}/rapport.txt`);
  assert.match(backend.files.get(clef), /^Rapport du projet/);
  assert.equal(confirms.length, 0, "premier rapport : rien à remplacer");
  backend.files.set(clef, "vieux");
  r = await run(exemple("Rapport du projet en fichier.js"), { name: "rapport" });
  assert.equal(r.ok, true, r.error);
  assert.equal(confirms.length, 1, "le second remplace : confirmation");
  assert.notEqual(backend.files.get(clef), "vieux");
  const sans = setup({ projectDir: null });
  assert.match((await sans.run(exemple("Rapport du projet en fichier.js"))).error, /Enregistre d'abord/);
});
