import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { ScriptRunner, WORKER_SOURCE, parseHeader, parseShortcut, shortcutMatches, FORBIDDEN_COMMANDS, snapshotOf } from "../js/script-runner.js";
import { toFileName } from "../js/script-store.js";
import { EXAMPLE_SCRIPTS } from "../js/scripts-examples.js";

/** Un « Worker » qui exécute le vrai code du Worker (WORKER_SOURCE) dans un contexte isolé, avec la même messagerie. */
function fakeWorker(source) {
  let terminated = false;
  const ctx = { setTimeout, navigator: {} }; // les objets de base viennent du contexte lui-même (son propre `Function`)
  ctx.self = ctx;
  const w = {
    onmessage: null,
    onerror: null,
    postMessage: (data) => queueMicrotask(() => !terminated && ctx.onmessage({ data })),
    terminate: () => (terminated = true),
  };
  ctx.postMessage = (data) => queueMicrotask(() => !terminated && w.onmessage?.({ data }));
  vm.createContext(ctx);
  vm.runInContext(source, ctx);
  return w;
}

function setup() {
  const engine = new MockEngine();
  const state = { project: null, pos: 0, playing: false, loop: null, selected: new Set(), history: null };
  engine.on("project", (p) => (state.project = p));
  engine.on("history", (hh) => (state.history = hh));
  engine.send({ type: "get_project" });
  const runner = new ScriptRunner(engine, state, fakeWorker);
  return { engine, state, runner };
}

test("en-tête : nom, description, raccourci, droits ; le code ne fait pas partie de l'en-tête", () => {
  const m = parseHeader("// @name Mon script\n// commentaire libre\n// @description Fait ceci\n\n// @shortcut Ctrl+Alt+1\n// @permissions files, other\nconst x = 1;\n// @name ignoré");
  assert.deepEqual(m, { name: "Mon script", description: "Fait ceci", shortcut: "Ctrl+Alt+1", permissions: ["files", "other"] });
  assert.equal(parseHeader("const a = 1;\n// @name tard").name, "");
});

test("raccourcis : Ctrl+Alt ou Alt+Maj avec une lettre ou 1-9 ; le reste est refusé", () => {
  assert.equal(parseShortcut("ctrl+alt+u"), "Ctrl+Alt+U");
  assert.equal(parseShortcut("Alt+Shift+7"), "Alt+Maj+7");
  for (const bad of ["Ctrl+S", "Alt+U", "Ctrl+Alt+0", "Ctrl+Alt++", "Ctrl+Alt+F5", "Ctrl+Alt+AB", "", "Meta+Alt+U", "Ctrl+Ctrl+Alt+U"]) assert.equal(parseShortcut(bad), null, bad);
  const e = (o) => ({ code: "KeyU", key: "u", ctrlKey: false, altKey: false, shiftKey: false, ...o });
  assert.ok(shortcutMatches("Ctrl+Alt+U", e({ ctrlKey: true, altKey: true })));
  assert.ok(!shortcutMatches("Ctrl+Alt+U", e({ ctrlKey: true })));
  assert.ok(!shortcutMatches("Ctrl+Alt+U", e({ ctrlKey: true, altKey: true, shiftKey: true })));
  assert.ok(shortcutMatches("Alt+Maj+3", { code: "Digit3", key: "#", ctrlKey: false, altKey: true, shiftKey: true }));
});

test("un script modifie le projet par daw.send ; tout forme un seul pas d'annulation", async () => {
  const { engine, state, runner } = setup();
  const before = state.project.nodes.map((n) => n.name);
  const r = await runner.run(`
    const { project } = await daw.state();
    const n = project.nodes.find((x) => x.id !== 0);
    const after = await daw.send({ type: "rename_track", node: n.id, name: "Renommée" });
    await daw.send({ type: "add_track", kind: "bus", name: "Bus du script" });
    daw.log("vu : " + after.project.nodes.find((x) => x.id === n.id).name);
    return project.nodes.length;
  `, { name: "essai" });
  assert.equal(r.ok, true, r.error);
  assert.equal(r.commands, 2);
  assert.deepEqual(r.log, ["vu : Renommée"]);
  assert.equal(r.result, before.length);
  assert.ok(state.project.nodes.some((n) => n.name === "Bus du script"));
  assert.equal(state.history.undo_label, "Script : essai");
  engine.send({ type: "undo" });
  assert.deepEqual(state.project.nodes.map((n) => n.name), before, "un seul annuler défait tout");
});

test("une erreur du script, une commande refusée non rattrapée ou un arrêt défont tout", async () => {
  const { engine, state, runner } = setup();
  const before = state.project.nodes.map((n) => n.name);
  const names = () => state.project.nodes.map((n) => n.name);
  let r = await runner.run(`await daw.send({ type: "add_track", kind: "bus", name: "X" }); throw new Error("boum");`);
  assert.equal(r.ok, false);
  assert.match(r.error, /boum/);
  assert.deepEqual(names(), before);
  // Une commande refusée par le moteur lève une erreur dans le script.
  r = await runner.run(`await daw.send({ type: "add_track", kind: "bus", name: "Y" }); await daw.send({ type: "rename_track", node: 9999, name: "Z" });`);
  assert.equal(r.ok, false);
  assert.deepEqual(names(), before);
  // Le script peut rattraper l'erreur : le reste est conservé.
  r = await runner.run(`try { await daw.send({ type: "rename_track", node: 9999, name: "Z" }); } catch (e) { daw.log("refusé"); } await daw.send({ type: "add_track", kind: "bus", name: "Garde" });`);
  assert.equal(r.ok, true, r.error);
  assert.ok(names().includes("Garde"));
  engine.send({ type: "undo" });
  assert.deepEqual(names(), before);
  // Rien n'est resté en suspens : un nouveau script peut démarrer, l'historique est intact.
  r = await runner.run(`daw.log("ok")`);
  assert.equal(r.ok, true);
  assert.equal(state.history.can_undo, false);
});

test("commandes interdites, commande mal formée, nombre de commandes et délai bornés", async () => {
  const { state, runner } = setup();
  for (const type of ["undo", "new_project", "save_project", "start_monitor", "set_monitor_role", "begin_group", "set_prefs"]) assert.ok(FORBIDDEN_COMMANDS.has(type), type);
  let r = await runner.run(`await daw.send({ type: "new_project" });`);
  assert.equal(r.ok, false);
  assert.match(r.error, /interdite/);
  r = await runner.run(`await daw.send("rename_track");`);
  assert.match(r.error, /objet avec un champ type/);
  r = await runner.run(`for (;;) { await daw.send({ type: "get_project" }); }`, { maxCommands: 20 });
  assert.equal(r.ok, false);
  assert.match(r.error, /trop de commandes/);
  r = await runner.run(`await daw.sleep(5000);`, { timeoutMs: 30 });
  assert.equal(r.ok, false);
  assert.match(r.error, /délai dépassé/);
  assert.ok(state.project, "l'état reste valide");
});

test("le réseau et les Workers imbriqués sont retirés du Worker ; un seul script à la fois ; arrêt demandé", async () => {
  const { runner } = setup();
  const r = await runner.run(`return [typeof fetch, typeof XMLHttpRequest, typeof WebSocket, typeof Worker, typeof importScripts];`);
  assert.deepEqual([...r.result], ["undefined", "undefined", "undefined", "undefined", "undefined"]);
  const slow = runner.run(`await daw.sleep(2000);`, { timeoutMs: 5000 });
  await new Promise((res) => setTimeout(res, 20));
  assert.equal(runner.running, true);
  assert.match((await runner.run(`1`)).error, /déjà en cours/);
  runner.abort();
  const s = await slow;
  assert.equal(s.ok, false);
  assert.match(s.error, /arrêté/);
  assert.equal(runner.running, false);
});

test("l'instantané est une copie : modifier ce que le script lit ne change pas la page", async () => {
  const { state, runner } = setup();
  const snap = snapshotOf(state);
  snap.project.nodes[0].name = "changé";
  assert.notEqual(state.project.nodes[0].name, "changé");
  const r = await runner.run(`const s = await daw.state(); s.project.nodes[0].name = "x"; return s.position;`);
  assert.equal(r.ok, true);
  assert.notEqual(state.project.nodes[0].name, "x");
});

test("les scripts d'exemple ont un en-tête valide et le nom de fichier est nettoyé", async () => {
  for (const ex of EXAMPLE_SCRIPTS) {
    const m = parseHeader(ex.source);
    assert.ok(m.name && m.description, ex.file);
    if (m.shortcut) assert.ok(parseShortcut(m.shortcut), `${ex.file} : raccourci permis`);
  }
  const { engine, state, runner } = setup();
  const up = EXAMPLE_SCRIPTS.find((e) => e.file.startsWith("Pistes"));
  const r = await runner.run(up.source, { name: "ex" });
  assert.equal(r.ok, true, r.error);
  assert.ok(state.project.nodes.filter((n) => n.id !== 0).every((n) => n.name === n.name.toUpperCase()));
  engine.send({ type: "undo" });
  const rep = EXAMPLE_SCRIPTS.find((e) => e.file.startsWith("Rapport"));
  const r2 = await runner.run(rep.source);
  assert.equal(r2.ok, true, r2.error);
  assert.match(r2.log[0], /nœuds, \d+ clips/);
  assert.equal(toFileName("Mon script/√ 1"), "Mon script-- 1.js");
  assert.equal(toFileName("  ../x.js"), "x.js");
  assert.equal(validateCommand({ type: "begin_group", label: "" }) !== null, true);
  assert.equal(validateCommand({ type: "begin_group", label: "ok" }), null);
});
