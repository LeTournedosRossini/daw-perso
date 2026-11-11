import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { MockEngine } from "../js/mock-engine.js";
import { ScriptRunner, EVENT_TYPES, LIMITS } from "../js/script-runner.js";
import { EXAMPLE_SCRIPTS } from "../js/scripts-examples.js";

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

const live = [];
test.afterEach(() => {
  for (const r of live.splice(0)) r.abort(); // sans cela le minuteur de sélection d'un script qui écoute garderait Node en vie
});

function setup() {
  const engine = new MockEngine();
  const state = { project: null, pos: 0, playing: false, loop: null, selected: new Set(), history: null };
  engine.on("project", (p) => (state.project = p));
  engine.on("history", (h) => (state.history = h));
  engine.send({ type: "get_project" });
  const runner = new ScriptRunner(engine, state, fakeWorker);
  live.push(runner);
  const names = () => state.project.nodes.map((n) => n.name);
  return { engine, state, runner, names };
}

const until = async (cond, ms = 1500) => {
  const t0 = Date.now();
  while (!cond()) {
    if (Date.now() - t0 > ms) throw new Error("délai d'attente dépassé");
    await new Promise((r) => setTimeout(r, 5));
  }
};
const play = (engine, playing, pos = 0) => engine.emit("position", { pos, playing, loop: null });

test("daw.on garde le script actif ; la réaction à un événement est un pas d'annulation à part", async () => {
  const { engine, runner, names, state } = setup();
  const before = names();
  const ends = [];
  const r = await runner.run(`daw.on("play", async (e) => { daw.log("lecture " + e.position); await daw.send({ type: "add_track", kind: "bus", name: "Marque" }); });`, { name: "écoute", onEnd: (x) => ends.push(x) });
  assert.equal(r.ok, true, r.error);
  assert.deepEqual(r.listening, ["play"]);
  assert.equal(runner.running, true);
  assert.equal(runner.listening, "écoute");
  assert.equal(state.history.can_undo, false, "le premier passage n'a rien changé");
  const logs = [];
  play(engine, true, 480);
  await until(() => names().includes("Marque"));
  await until(() => state.history.undo_label === "Script : écoute (play)");
  engine.send({ type: "undo" });
  assert.deepEqual(names(), before, "un seul annuler défait la réaction");
  assert.equal(runner.running, true, "le script écoute toujours");
  assert.deepEqual(ends, []);
  void logs;
});

test("daw.on refuse un événement inconnu ou un gestionnaire qui n'est pas une fonction", async () => {
  const { runner } = setup();
  assert.deepEqual(EVENT_TYPES, ["play", "stop", "selection", "project", "recording-end"]);
  assert.match((await runner.run(`daw.on("position", () => {});`)).error, /événement inconnu/);
  assert.match((await runner.run(`daw.on("play", 3);`)).error, /fonction/);
  assert.equal(runner.running, false);
});

test("un script qui ne s'abonne à rien se termine comme avant", async () => {
  const { runner } = setup();
  const r = await runner.run(`daw.log("ok");`);
  assert.equal(r.ok, true);
  assert.equal(r.listening, undefined);
  assert.equal(runner.running, false);
});

test("pas de boucle : les événements que provoque la réaction sont ignorés pendant qu'elle dure", async () => {
  const { engine, runner, names } = setup();
  const before = names().length;
  const logs = [];
  await runner.run(`let n = 0; daw.on("project", async () => { n++; daw.log("reaction " + n); await daw.send({ type: "add_track", kind: "bus", name: "B" + n }); });`, { onLog: (l) => logs.push(l) });
  engine.send({ type: "get_project" }); // un événement « project » venu de l'extérieur : une seule réaction
  await until(() => logs.length >= 1);
  await new Promise((r) => setTimeout(r, 150));
  assert.deepEqual(logs, ["reaction 1"], "l'ajout de piste provoqué par la réaction ne la relance pas");
  assert.equal(names().length, before + 1);
});

test("une erreur dans une réaction la défait sans arrêter le script ; trois de suite l'arrêtent", async () => {
  const { engine, runner, names } = setup();
  const before = names();
  const ends = [];
  const logs = [];
  await runner.run(`daw.on("play", async () => { await daw.send({ type: "add_track", kind: "bus", name: "X" }); throw new Error("zut"); });`, { name: "err", onEnd: (x) => ends.push(x), onLog: (l) => logs.push(l) });
  play(engine, true);
  await until(() => logs.some((l) => /zut/.test(l)));
  assert.deepEqual(names(), before, "la réaction fautive est défaite");
  assert.equal(runner.running, true);
  for (let i = 0; i < 2; i++) {
    play(engine, false);
    await until(() => logs.length >= 1 && true);
    play(engine, true);
    await new Promise((r) => setTimeout(r, 30));
  }
  await until(() => ends.length === 1);
  assert.match(ends[0], /3 erreurs de suite/);
  assert.equal(runner.running, false);
  assert.deepEqual(names(), before);
});

test("trop d'événements en peu de temps arrêtent le script", async () => {
  const { engine, runner } = setup();
  const ends = [];
  await runner.run(`daw.on("play", () => {}); daw.on("stop", () => {});`, { name: "vite", onEnd: (x) => ends.push(x) });
  let playing = false;
  for (let i = 0; i < LIMITS.burst + 5 && !ends.length; i++) {
    playing = !playing;
    play(engine, playing);
    await new Promise((r) => setTimeout(r, 8));
  }
  await until(() => ends.length === 1);
  assert.match(ends[0], /trop d'événements/);
  assert.equal(runner.running, false);
});

test("arrêt demandé : plus d'écoute, la suite est possible ; un second script est refusé pendant l'écoute", async () => {
  const { engine, runner } = setup();
  const ends = [];
  const seen = [];
  await runner.run(`daw.on("play", () => daw.log("vu"));`, { name: "A", onEnd: (x) => ends.push(x), onLog: (l) => seen.push(l) });
  const other = await runner.run(`daw.log("autre")`);
  assert.equal(other.ok, false);
  assert.match(other.error, /écoute des événements/);
  runner.abort();
  await until(() => ends.length === 1);
  assert.equal(ends[0], "arrêté");
  assert.equal(runner.running, false);
  play(engine, true);
  await new Promise((r) => setTimeout(r, 40));
  assert.deepEqual(seen, [], "plus aucune réaction après l'arrêt");
  assert.equal((await runner.run(`return 1`)).ok, true);
});

test("une réaction trop longue arrête le script et défait ce qu'elle avait fait", async () => {
  const { engine, runner, names } = setup();
  const before = names();
  const ends = [];
  await runner.run(`daw.on("play", async () => { await daw.send({ type: "add_track", kind: "bus", name: "Long" }); await daw.sleep(2000); });`, { name: "lent", handlerMs: 60, onEnd: (x) => ends.push(x) });
  play(engine, true);
  await until(() => ends.length === 1);
  assert.match(ends[0], /trop longue/);
  assert.deepEqual(names(), before);
  assert.equal(runner.running, false);
});

test("sélection (relevée toutes les 150 ms), fin d'enregistrement et projet", async () => {
  const { engine, state, runner } = setup();
  const logs = [];
  await runner.run(`daw.on("selection", (e) => daw.log("sel " + JSON.stringify(e.clips))); daw.on("recording-end", () => daw.log("fin prise")); daw.on("project", () => daw.log("projet"));`, { name: "multi", onLog: (l) => logs.push(l) });
  state.selected = new Set([10, 11]);
  await until(() => logs.includes("sel [10,11]"), 1000);
  state.selected = new Set();
  await until(() => logs.includes("sel []"), 1000);
  engine.emit("recording", { active: true, stopping: false });
  engine.emit("recording", { active: true, stopping: true });
  assert.ok(!logs.includes("fin prise"), "pas avant la fin");
  engine.emit("recording", { active: false, stopping: false });
  await until(() => logs.includes("fin prise"));
  engine.send({ type: "get_project" });
  await until(() => logs.includes("projet"));
  runner.abort();
});

test("l'exemple « Annonce la lecture » reste à l'écoute", async () => {
  const { engine, runner } = setup();
  const ex = EXAMPLE_SCRIPTS.find((e) => e.file === "Annonce la lecture.js");
  const logs = [];
  const r = await runner.run(ex.source, { name: "ex", onLog: (l) => logs.push(l) });
  assert.deepEqual(r.listening.sort(), ["play", "selection", "stop"]);
  play(engine, true, 960);
  await until(() => logs.includes("lecture depuis 960 échantillons"));
  runner.abort();
});
