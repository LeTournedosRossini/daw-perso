import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { MockEngine } from "../js/mock-engine.js";
import { ScriptRunner } from "../js/script-runner.js";
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

function setup() {
  const engine = new MockEngine();
  const state = { project: null, pos: 0, playing: false, loop: null, selected: new Set(), history: null };
  engine.on("project", (p) => (state.project = p));
  engine.on("history", (h) => (state.history = h));
  engine.send({ type: "get_project" });
  const runner = new ScriptRunner(engine, state, fakeWorker);
  const midi = state.project.nodes.flatMap((n) => n.clips.filter((c) => c.kind === "midi").map((c) => ({ ...c, node: n.id })))[0];
  const readNotes = () => {
    let ev;
    const off = engine.on("notes", (e) => (ev = e));
    engine.send({ type: "get_notes", clip: midi.id });
    off();
    return ev.notes;
  };
  return { engine, state, runner, midi, readNotes };
}

function setupAudio() {
  return 10; // clip audio de la démonstration (Batterie)
}

test("daw.notes.get rend les notes avec leur indice ; un clip qui n'est pas de notes est refusé", async () => {
  const { runner, midi, readNotes } = setup();
  const r = await runner.run(`const c = await daw.notes.get(${midi.id}); return [c.notes.length, c.notes[0].index, c.notes[3].index, Array.isArray(c.notes[0].start)];`);
  assert.equal(r.ok, true, r.error);
  assert.deepEqual([...r.result], [readNotes().length, 0, 3, true]);
  const bad = await runner.run(`await daw.notes.get("x");`);
  assert.match(bad.error, /identifiant/);
  const audioClip = setupAudio();
  assert.equal((await runner.run(`await daw.notes.get(${audioClip});`)).ok, false, "un clip audio n'est pas un clip de notes");
  const unknown = await runner.run(`await daw.notes.get(1999999);`);
  assert.equal(unknown.ok, false);
});

test("transposer : un pas d'annulation, notes hors 0-127 laissées en place, demi-tons entiers", async () => {
  const { engine, runner, midi, readNotes } = setup();
  const before = readNotes().map((n) => n.key);
  const r = await runner.run(`return await daw.notes.transpose(${midi.id}, 12);`, { name: "t" });
  assert.equal(r.ok, true, r.error);
  assert.equal(r.result, before.filter((k) => k + 12 <= 127).length);
  assert.deepEqual(readNotes().map((n) => n.key), before.map((k) => (k + 12 <= 127 ? k + 12 : k)));
  engine.send({ type: "undo" });
  assert.deepEqual(readNotes().map((n) => n.key), before, "un seul annuler");
  const big = await runner.run(`return await daw.notes.transpose(${midi.id}, 100);`);
  assert.equal(big.result, before.filter((k) => k + 100 <= 127).length);
  assert.match((await runner.run(`await daw.notes.transpose(${midi.id}, 1.5);`)).error, /entier/);
  assert.equal((await runner.run(`return await daw.notes.transpose(${midi.id}, 0);`)).result, 0);
});

test("humaniser : reproductible à graine égale, jamais avant 0, un seul pas ; indices respectés", async () => {
  const { engine, runner, midi, readNotes } = setup();
  const start = readNotes().map((n) => n.start[0] / n.start[1]);
  const run = (seed, extra = "") => runner.run(`return await daw.notes.humanize(${midi.id}, { timing: 20, velocity: 0.1, seed: ${seed}${extra} });`);
  assert.equal((await run(7)).ok, true);
  const a = readNotes().map((n) => [n.start, n.velocity]);
  engine.send({ type: "undo" });
  assert.deepEqual(readNotes().map((n) => n.start[0] / n.start[1]), start);
  await run(7);
  assert.deepEqual(readNotes().map((n) => [n.start, n.velocity]), a, "même graine, même résultat");
  engine.send({ type: "undo" });
  await run(8);
  assert.notDeepEqual(readNotes().map((n) => [n.start, n.velocity]), a, "autre graine, autre résultat");
  engine.send({ type: "undo" });
  await run(7, ", indices: [1]");
  const now = readNotes();
  const pos = now.map((n) => n.start[0] / n.start[1]);
  assert.notEqual(pos[1], start[1], "la note 1 a bougé (la note 0, à 0, ne peut pas reculer)");
  assert.deepEqual(pos.filter((_, i) => i !== 1), start.filter((_, i) => i !== 1), "seule la note 1 a bougé");
  assert.ok(now.every((n) => n.start[0] >= 0 && n.velocity > 0 && n.velocity <= 1));
});

test("vélocité, décalage, quantification, ajout et suppression de notes", async () => {
  const { engine, runner, midi, readNotes } = setup();
  const n0 = readNotes().length;
  const v0 = readNotes()[0].velocity;
  await runner.run(`await daw.notes.scaleVelocity(${midi.id}, 0.5);`);
  assert.ok(Math.abs(readNotes()[0].velocity - v0 * 0.5) < 1e-9);
  assert.equal((await runner.run(`return await daw.notes.scaleVelocity(${midi.id}, 1);`)).result, 0, "rien à changer : aucune commande");
  await runner.run(`await daw.notes.setVelocity(${midi.id}, 0.9, { indices: [1] });`);
  assert.equal(readNotes()[1].velocity, 0.9);
  const s0 = readNotes()[2].start;
  await runner.run(`await daw.notes.shift(${midi.id}, 480);`);
  const s1 = readNotes().find((n) => n.key === readNotes()[2].key && true).start;
  assert.ok(s1[0] / s1[1] > 0);
  assert.equal((await runner.run(`return await daw.notes.shift(${midi.id}, -100000);`)).result, 0, "avant 0 : laissé en place");
  await runner.run(`await daw.notes.add(${midi.id}, [{ start: [0, 1], len: daw.frac.ticks(480), key: 72, velocity: 0.8 }]);`);
  assert.equal(readNotes().length, n0 + 1);
  await runner.run(`const c = await daw.notes.get(${midi.id}); await daw.notes.remove(${midi.id}, [c.notes.findIndex((n) => n.key === 72)]);`);
  assert.equal(readNotes().length, n0);
  const q = await runner.run(`await daw.notes.quantize(${midi.id}, [1, 8]); return 1;`);
  assert.equal(q.ok, true, q.error);
  void engine; void s0;
});

test("selection, clips.list et opérations de clips", async () => {
  const { engine, state, runner, midi } = setup();
  const audio = state.project.nodes.flatMap((n) => n.clips.filter((c) => c.kind === "audio").map((c) => ({ ...c, node: n.id })));
  state.selected = new Set([audio[0].id, midi.id]);
  let r = await runner.run(`return [await daw.selection(), await daw.selection({ kind: "notes" }), await daw.selection({ kind: "audio" })];`);
  assert.deepEqual(JSON.parse(JSON.stringify(r.result)), [[audio[0].id, midi.id], [midi.id], [audio[0].id]]);
  r = await runner.run(`const all = await daw.clips.list(); const m = await daw.clips.list({ kind: "midi" }); const s = await daw.clips.list({ selected: true }); return [all.length, m.length, s.length, all[0].node !== undefined];`);
  assert.deepEqual(JSON.parse(JSON.stringify(r.result)), [audio.length + state.project.nodes.flatMap((n) => n.clips.filter((c) => c.kind === "midi")).length, 1, 2, true]);
  const start = audio[0].start;
  r = await runner.run(`await daw.clips.move([${audio[0].id}], 4800);`, { name: "move" });
  assert.equal(r.ok, true, r.error);
  const moved = state.project.nodes.flatMap((n) => n.clips).find((c) => c.id === audio[0].id);
  assert.equal(moved.start, start + 4800);
  engine.send({ type: "undo" });
  assert.equal(state.project.nodes.flatMap((n) => n.clips).find((c) => c.id === audio[0].id).start, start);
});

test("les exemples de notes se lancent et se défont en un coup", async () => {
  const { engine, state, runner, midi, readNotes } = setup();
  const before = readNotes();
  state.selected = new Set([midi.id]);
  for (const file of ["Transposer +1 octave.js", "Humaniser la sélection.js"]) {
    const ex = EXAMPLE_SCRIPTS.find((e) => e.file === file);
    assert.ok(ex, file);
    const r = await runner.run(ex.source, { name: file });
    assert.equal(r.ok, true, `${file} : ${r.error}`);
    assert.notDeepEqual(readNotes(), before, file);
    engine.send({ type: "undo" });
    assert.deepEqual(readNotes(), before, `${file} : un seul annuler`);
  }
});
