import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { orderedRows, takeLayers } from "../js/ui/order.js";
import { clipsInRect } from "../js/selection.js";

const clip = (id, start, len, extra = {}) => ({ id, kind: "audio", start, len, ...extra });
const project = () => ({
  nodes: [
    { id: 0, kind: "master", clips: [] },
    { id: 1, kind: "mono", color: "#fff", name: "Voix", clips: [clip(1, 0, 100), clip(2, 100, 50, { muted: true, take_group: 1 }), clip(3, 120, 50, { muted: true, take_group: 1 }), clip(4, 100, 50, { take_group: 1 })] },
  ],
});

test("les clips muets quittent la ligne de la piste et se rangent en couches sans chevauchement", () => {
  const rows = orderedRows(project(), new Set(), null, 4, new Set());
  const voix = rows.find((r) => r.id === 1);
  assert.deepEqual(voix.clips.map((c) => c.id), [1, 4]);
  assert.equal(rows.filter((r) => r.kind === "take").length, 0, "repliées : pas de voie");
  const layers = takeLayers(voix);
  assert.deepEqual(layers.map((l) => l.map((c) => c.id)), [[2], [3]], "2 et 3 se chevauchent : deux couches");
});

test("dépliées, les voies de prises suivent la piste, une ligne par couche", () => {
  const rows = orderedRows(project(), new Set(), null, 4, new Set([1]));
  assert.deepEqual(rows.map((r) => r.kind), ["mono", "take", "take", "master"]);
  assert.deepEqual(rows[1].clips.map((c) => c.id), [2]);
  assert.equal(rows[2].rowKey, "take:1:1");
});

test("le rectangle de sélection ignore les voies de prises", () => {
  const rows = orderedRows(project(), new Set(), null, 4, new Set([1]));
  assert.deepEqual(clipsInRect(rows, 50, { s0: 0, s1: 500, y0: 0, y1: 500 }).sort(), [1, 4]);
});

test("validation des commandes de prises", () => {
  assert.equal(validateCommand({ type: "set_active_take", clip: 3 }), null);
  assert.ok(validateCommand({ type: "set_active_take", clip: -1 }));
  assert.equal(validateCommand({ type: "set_clip_muted", clip: 3, muted: true }), null);
  assert.ok(validateCommand({ type: "set_clip_muted", clip: 3 }));
});

function mock() {
  const e = new MockEngine();
  const log = { errors: [], projects: [], history: [] };
  e.on("error", (x) => log.errors.push(x.message));
  e.on("project", (x) => log.projects.push(x));
  e.on("history", (x) => log.history.push(x));
  e.send({ type: "get_project" });
  return { e, log, clips: () => log.projects.at(-1).nodes.flatMap((n) => n.clips) };
}

test("faux moteur : changer de prise est une action annulable, refusé hors groupe", () => {
  const { e, log, clips } = mock();
  // Une prise inactive posée sous « Riff 1 » (clip 40) : même groupe, recouvrement complet.
  const riff = e.project.nodes.flatMap((n) => n.clips).find((c) => c.id === 40);
  const under = { ...riff, id: 900, muted: true, take_group: 7 };
  riff.take_group = 7;
  e.project.nodes.find((n) => n.clips.includes(riff)).clips.push(under);
  e.send({ type: "set_active_take", clip: 900 });
  assert.deepEqual(log.errors, []);
  const by = (id) => clips().find((c) => c.id === id);
  assert.ok(!by(900).muted);
  assert.equal(by(40).muted, true);
  assert.equal(log.history.at(-1).undo_label, "Changer de prise");
  e.send({ type: "undo" });
  assert.equal(by(900).muted, true);
  assert.ok(!by(40).muted);
  e.send({ type: "set_active_take", clip: 10 });
  assert.match(log.errors.at(-1), /groupe/);
});

test("enregistrement en boucle : exclusif avec Punch, accepte une boucle active, validation", () => {
  assert.equal(validateCommand({ type: "set_loop_record", enabled: true }), null);
  assert.ok(validateCommand({ type: "set_loop_record", enabled: 1 }));
  const e = new MockEngine();
  const seen = { loop: [], punch: [], errors: [] };
  e.on("loop_record", (x) => seen.loop.push(x.enabled));
  e.on("punch", (x) => seen.punch.push(x.enabled));
  e.on("error", (x) => seen.errors.push(x.message));
  e.send({ type: "get_project" });
  e.send({ type: "set_punch", enabled: true });
  e.send({ type: "set_loop_record", enabled: true });
  assert.equal(seen.loop.at(-1), true);
  assert.equal(seen.punch.at(-1), false, "allumer l'un éteint l'autre");
  e.send({ type: "set_punch", enabled: true });
  assert.equal(seen.loop.at(-1), false);
});

test("faux moteur : un conteneur de notes s'active en entier, la plage est réservée à l'audio ; plage audio composée en un pas", () => {
  const { e, log, clips } = mock();
  const riff = e.project.nodes.flatMap((n) => n.clips).find((c) => c.id === 40);
  const node = e.project.nodes.find((n) => n.clips.includes(riff));
  riff.take_group = 7;
  const total = riff.len; // la découpe raccourcit `riff` lui-même
  node.clips.push({ ...riff, id: 900, muted: true, take_group: 7 });
  e.send({ type: "activate_take_range", clip: 900, start: riff.start + 1000, end: riff.start + 3000 });
  assert.deepEqual(log.errors, []);
  const by = clips().filter((c) => c.take_group === 7).map((c) => [c.start - riff.start, c.len, !!c.muted]).sort((a, b) => a[0] - b[0]);
  assert.deepEqual(by.filter((x) => !x[2]).map((x) => [x[0], x[1]]), [[0, 1000], [1000, 2000], [3000, total - 3000]], "actif : Riff 1 / plage composée / Riff 1");
  assert.equal(log.history.at(-1).undo_label, "Composer une prise");
  e.send({ type: "undo" });
  assert.equal(clips().filter((c) => c.take_group === 7).length, 2);
});
