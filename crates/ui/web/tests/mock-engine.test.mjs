import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand, dbToGain, gainToDb } from "../js/protocol.js";
import { columnPeaks, waveformFor } from "../js/mock-waveform.js";

function collect(engine, ...types) {
  const log = [];
  for (const t of types) engine.on(t, (e) => log.push(e));
  return log;
}

test("validation : commandes valides et refusées", () => {
  assert.equal(validateCommand({ type: "transport", action: "play" }), null);
  assert.equal(validateCommand({ type: "seek", pos: 0 }), null);
  assert.equal(validateCommand({ type: "loop", start: null, end: null }), null);
  assert.equal(validateCommand({ type: "set_node", node: 1, gain: 0.5, mute: true }), null);
  for (const bad of [
    null, {}, { type: "zigzag" }, { type: "transport", action: "rewind" }, { type: "seek", pos: -1 }, { type: "seek", pos: 1.5 },
    { type: "loop", start: 10, end: 10 }, { type: "loop", start: 10, end: null }, { type: "set_node", node: 1 }, { type: "set_node", node: 1, gain: -1 },
    { type: "set_node", node: 1, pan: 2 }, { type: "set_node", node: 1, mute: "oui" }, { type: "set_node", node: 1, volume: 3, gain: 1 },
    { type: "get_peaks", clip: 1, from: 5, to: 5, columns: 10 }, { type: "get_peaks", clip: 1, from: 0, to: 9, columns: 0 }, { type: "get_peaks", clip: 1, from: 0, to: 9, columns: 100000 },
  ]) assert.notEqual(validateCommand(bad), null, JSON.stringify(bad));
  assert.equal(dbToGain(-60), 0);
  assert.ok(Math.abs(gainToDb(dbToGain(-12)) - -12) < 1e-9);
  assert.equal(gainToDb(0), -Infinity);
});

test("get_project : projet complet et sans données de simulation", () => {
  const e = new MockEngine();
  const log = collect(e, "project");
  e.send({ type: "get_project" });
  const p = log[0];
  assert.equal(p.sample_rate, 48_000);
  assert.equal(p.nodes[0].kind, "master");
  assert.ok(p.nodes.length >= 5 && p.nodes.every((n) => Array.isArray(n.clips)));
  assert.ok(p.nodes.flatMap((n) => n.clips).every((c) => !("wave" in c)), "la forme d'onde de démonstration ne fuit pas dans le protocole");
  const ids = p.nodes.flatMap((n) => [n.id, ...n.clips.map((c) => c.id)]);
  assert.equal(new Set(ids.filter((_, i) => i > 0 || true)).size >= p.nodes.length, true);
});

test("transport : lecture, position, boucle, arrêt qui conserve la position", () => {
  const e = new MockEngine();
  const pos = collect(e, "position");
  e.send({ type: "transport", action: "play" });
  e.advance(500);
  assert.equal(pos.at(-1).pos, 24_000);
  assert.equal(pos.at(-1).playing, true);
  e.send({ type: "transport", action: "stop" });
  e.advance(500);
  assert.equal(pos.at(-1).pos, 24_000, "la position est conservée à l'arrêt");
  e.send({ type: "seek", pos: 90_000 });
  e.send({ type: "loop", start: 48_000, end: 96_000 });
  e.send({ type: "transport", action: "play" });
  e.advance(250); // 12 000 échantillons : 102 000 → repli dans la boucle
  assert.equal(pos.at(-1).pos, 48_000 + 6_000);
  e.send({ type: "loop", start: null, end: null });
  assert.equal(pos.at(-1).loop, null);
  e.send({ type: "transport", action: "toggle" });
  assert.equal(pos.at(-1).playing, false);
});

test("set_node : réglages confirmés par un nouvel état, inconnus refusés", () => {
  const e = new MockEngine();
  const projects = collect(e, "project");
  const errors = collect(e, "error");
  assert.equal(e.send({ type: "set_node", node: 2, gain: 0.25, pan: 0.5, mute: true }), true);
  const n = projects.at(-1).nodes.find((n) => n.id === 2);
  assert.deepEqual([n.gain, n.pan, n.mute], [0.25, 0.5, true]);
  assert.equal(e.send({ type: "set_node", node: 99, gain: 1 }), false);
  assert.equal(errors.at(-1).command, "set_node");
  assert.equal(e.send({ type: "set_node", node: 2, gain: -3 }), false);
  assert.match(errors.at(-1).message, /gain/);
  assert.equal(e.node(2).gain, 0.25, "une commande refusée ne change rien");
});

test("niveaux : silence à l'arrêt, mute et solo respectés, sortie = mélange", () => {
  const e = new MockEngine();
  const meters = collect(e, "meters");
  e.advance(33);
  assert.ok(Object.values(meters.at(-1).levels).every(([l, r]) => l === 0 && r === 0));
  e.send({ type: "seek", pos: 96_000 * 5 });
  e.send({ type: "transport", action: "play" });
  let peak = 0;
  for (let i = 0; i < 30; i++) {
    e.advance(33);
    peak = Math.max(peak, ...meters.at(-1).levels[1]);
  }
  assert.ok(peak > 0.05, `la batterie sonne (${peak})`);
  e.send({ type: "set_node", node: 1, mute: true });
  e.advance(33);
  assert.deepEqual(meters.at(-1).levels[1], [0, 0]);
  e.send({ type: "set_node", node: 1, mute: false });
  e.send({ type: "set_node", node: 2, solo: true });
  let others = 0;
  for (let i = 0; i < 30; i++) {
    e.advance(33);
    others = Math.max(others, ...meters.at(-1).levels[1], ...meters.at(-1).levels[3]);
  }
  assert.equal(others, 0, "solo : les autres pistes se taisent");
  const m = meters.at(-1).levels;
  assert.ok(m[0][0] <= Math.max(...Object.entries(m).filter(([k]) => k !== "0").map(([, v]) => v[0])) + 1e-9);
});

test("get_peaks : colonnes, ordre min ≤ max, déterminisme et respect de l'écart", () => {
  const e = new MockEngine();
  const peaks = collect(e, "peaks");
  const errors = collect(e, "error");
  e.send({ type: "get_peaks", clip: 10, from: 0, to: 96_000, columns: 200 });
  const p = peaks[0];
  assert.equal(p.columns.length, 200);
  assert.ok(p.columns.every(([lo, hi]) => lo <= hi && lo >= -1 && hi <= 1));
  assert.ok(p.columns.some(([lo, hi]) => hi - lo > 0.3), "il y a du signal");
  e.send({ type: "get_peaks", clip: 10, from: 0, to: 96_000, columns: 200 });
  assert.deepEqual(peaks[1].columns, p.columns);
  // Un clip MIDI n'a pas de crêtes ; un clip inconnu non plus.
  assert.equal(e.send({ type: "get_peaks", clip: 30, from: 0, to: 10, columns: 2 }), false);
  assert.equal(e.send({ type: "get_peaks", clip: 12345, from: 0, to: 10, columns: 2 }), false);
  assert.equal(errors.length, 2);
  // Deux tuiles contiguës donnent les mêmes colonnes qu'une seule tuile (sans trou ni recouvrement).
  const w = waveformFor("bass", 1);
  const whole = columnPeaks(w, 48_000, 0, 4_000, 8);
  const halves = [...columnPeaks(w, 48_000, 0, 2_000, 4), ...columnPeaks(w, 48_000, 2_000, 4_000, 4)];
  assert.deepEqual(halves, whole);
});

test("set_crossfade_length : longueur autour de l'ancrage, contenu en place, un pas d'annulation, refus sans trace", () => {
  const eng = new MockEngine();
  const projects = collect(eng, "project");
  const errors = collect(eng, "error");
  eng.send({ type: "get_project" });
  const clips = () => {
    const all = projects.at(-1).nodes.flatMap((n) => n.clips);
    return [all.find((c) => c.name === "Riff 1"), all.find((c) => c.name === "Riff 2")];
  };
  const [a0, b0] = clips();
  const overlap = a0.start + a0.len - b0.start;
  assert.ok(overlap > 0);
  const content = (c) => c.start - c.offset;
  assert.equal(eng.send({ type: "set_crossfade_length", clip_out: a0.id, clip_in: b0.id, length: overlap / 2, anchor: "center" }), true);
  const [a, b] = clips();
  assert.equal(a.start + a.len - b.start, overlap / 2);
  assert.equal(a.fade_out, overlap / 2);
  assert.equal(b.fade_in, overlap / 2);
  assert.equal(content(b), content(b0));
  assert.equal(b.start + b.len, b0.start + b0.len);
  assert.equal(eng.send({ type: "set_crossfade_length", clip_out: a0.id, clip_in: b0.id, length: 10 ** 12, anchor: "end" }), false);
  assert.equal(errors.at(-1).command, "set_crossfade_length");
  assert.equal(eng.send({ type: "set_crossfade_length", clip_out: a0.id, clip_in: b0.id, length: 100, anchor: "left" }), false);
  eng.send({ type: "undo" });
  const [a2, b2] = clips();
  assert.deepEqual([a2.start, a2.len, a2.fade_out, b2.start, b2.offset, b2.fade_in], [a0.start, a0.len, a0.fade_out, b0.start, b0.offset, b0.fade_in]);
});

function riffs(eng, projects) {
  eng.send({ type: "get_project" });
  const all = () => projects.at(-1).nodes.flatMap((n) => n.clips);
  return { a: all().find((c) => c.name === "Riff 1"), b: all().find((c) => c.name === "Riff 2"), all };
}

test("aperçu d'un fondu : boucle sur la zone plus la marge, piste seule, tout est rendu à l'arrêt", () => {
  const e = new MockEngine();
  const projects = collect(e, "project");
  const positions = collect(e, "position");
  const previews = collect(e, "preview");
  const { a, b } = riffs(e, projects);
  const zone = [b.start, a.start + a.len];
  e.send({ type: "set_node", node: 2, solo: true });
  e.send({ type: "loop", start: 0, end: 96_000 });
  e.send({ type: "seek", pos: 1000 });
  assert.equal(e.send({ type: "preview_fade", clip_out: a.id, clip_in: b.id, margin_ms: 1500, solo: true }), true);
  assert.deepEqual(previews.map((p) => p.active), [true]);
  assert.deepEqual(positions.at(-1).loop, { start: zone[0] - 72_000, end: zone[1] + 72_000 });
  assert.equal(positions.at(-1).playing, true);
  const soloed = () => projects.at(-1).nodes.filter((n) => n.solo).map((n) => n.id);
  assert.deepEqual(soloed(), [4]);
  // Une édition du fondu : la boucle suit.
  e.send({ type: "set_crossfade_length", clip_out: a.id, clip_in: b.id, length: 12_000, anchor: "center" });
  const mid = (zone[0] + zone[1]) / 2;
  assert.deepEqual(positions.at(-1).loop, { start: mid - 6_000 - 72_000, end: mid + 6_000 + 72_000 });
  assert.deepEqual(soloed(), [4]);
  e.send({ type: "stop_preview" });
  assert.deepEqual(previews.map((p) => p.active), [true, false]);
  assert.deepEqual(soloed(), [2]);
  assert.deepEqual([positions.at(-1).pos, positions.at(-1).playing, positions.at(-1).loop], [1000, false, { start: 0, end: 96_000 }]);
});

test("aperçu : le transport de l'utilisateur y met fin sans lui reprendre la position ; fondu disparu → fin", () => {
  const e = new MockEngine();
  const projects = collect(e, "project");
  const positions = collect(e, "position");
  const previews = collect(e, "preview");
  const errors = collect(e, "error");
  const { a, b } = riffs(e, projects);
  e.send({ type: "preview_fade", clip_out: a.id, clip_in: b.id, margin_ms: 0, solo: true });
  e.send({ type: "transport", action: "toggle" });
  assert.deepEqual(previews.map((p) => p.active), [true, false]);
  assert.deepEqual([positions.at(-1).playing, positions.at(-1).loop], [false, null]);
  assert.ok(positions.at(-1).pos >= b.start, "la position n'a pas été rendue");
  // Boucle de l'utilisateur pendant l'aperçu : conservée.
  e.send({ type: "preview_fade", clip_out: a.id, clip_in: b.id, margin_ms: 0, solo: false });
  e.send({ type: "loop", start: 10, end: 20 });
  assert.deepEqual(positions.at(-1).loop, { start: 10, end: 20 });
  // Pas de fondu d'entrée sur Beat A : refus, rien ne reste réglé.
  const beatA = projects.at(-1).nodes.flatMap((n) => n.clips).find((c) => c.name === "Beat A");
  assert.equal(e.send({ type: "preview_fade", clip: beatA.id, side: "in", margin_ms: 0, solo: true }), false);
  assert.match(errors.at(-1).message, /pas de fondu/);
  assert.equal(projects.at(-1).nodes.some((n) => n.solo), false);
  // Annuler le fondu écouté met fin à l'aperçu.
  e.send({ type: "move_clip", clip: beatA.id + 1, start: 672_000 });
  e.send({ type: "preview_fade", clip_out: beatA.id, clip_in: beatA.id + 1, margin_ms: 0, solo: true });
  assert.equal(previews.at(-1).active, true);
  e.send({ type: "undo" });
  assert.equal(previews.at(-1).active, false);
});

test("preview_fade : validation des champs", () => {
  const ok = { type: "preview_fade", clip_out: 3, clip_in: 4, margin_ms: 1500, solo: true };
  assert.equal(validateCommand(ok), null);
  assert.equal(validateCommand({ type: "preview_fade", clip: 3, side: "in", margin_ms: 0, solo: false }), null);
  assert.equal(validateCommand({ type: "stop_preview" }), null);
  for (const bad of [{ ...ok, clip_in: 3 }, { ...ok, margin_ms: -1 }, { ...ok, margin_ms: 10_001 }, { ...ok, solo: "oui" }, { type: "preview_fade", clip: 3, side: "mid", margin_ms: 0, solo: true }, { ...ok, clip: 3, side: "in" }, { type: "preview_fade", margin_ms: 0, solo: true }]) {
    assert.notEqual(validateCommand(bad), null, JSON.stringify(bad));
  }
});

test("panic : commande sans paramètre acceptée, sans effet sur la lecture", () => {
  const e = new MockEngine();
  const positions = collect(e, "position");
  assert.equal(validateCommand({ type: "panic" }), null);
  e.send({ type: "seek", pos: 4_000 });
  e.send({ type: "transport", action: "play" });
  assert.equal(e.send({ type: "panic" }), true);
  assert.deepEqual([positions.at(-1).pos, positions.at(-1).playing], [4_000, true]);
});
