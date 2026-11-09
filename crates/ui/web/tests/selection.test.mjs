import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { clickSelect, clipsInRect, allClipIds, clampGroupDelta, prune, groupDragCommand } from "../js/selection.js";

const nodes = [
  { id: 1, kind: "stereo", clips: [{ id: 10, start: 0, len: 100 }, { id: 11, start: 200, len: 100 }] },
  { id: 2, kind: "mono", clips: [{ id: 20, start: 50, len: 100 }] },
  { id: 3, kind: "lane", clips: [] },
  { id: 4, kind: "stereo", clips: [{ id: 40, start: 0, len: 1000 }] },
];

test("clic : Ctrl/Maj ajoute ou retire, un clic simple garde le groupe s'il y est déjà", () => {
  assert.deepEqual([...clickSelect(new Set([1]), 2, { toggle: true })], [1, 2]);
  assert.deepEqual([...clickSelect(new Set([1, 2]), 2, { toggle: true })], [1]);
  assert.deepEqual([...clickSelect(new Set([1, 2]), 2)], [1, 2], "glisser le groupe depuis un de ses membres");
  assert.deepEqual([...clickSelect(new Set([1, 2]), 3)], [3]);
});

test("rectangle : clips qui touchent la zone, lignes d'automation ignorées", () => {
  const rowH = 50;
  // Le rectangle ne descend pas jusqu'à la 4e ligne (y 150-200) : le clip 40 est exclu.
  assert.deepEqual(clipsInRect(nodes, rowH, { s0: 90, s1: 210, y0: 0, y1: 120 }).sort(), [10, 11, 20]);
  assert.deepEqual(clipsInRect(nodes, rowH, { s0: 160, s1: 190, y0: 0, y1: 300 }), [40], "entre les clips : seul le long clip touche");
  assert.deepEqual(clipsInRect(nodes, rowH, { s1: 0, s0: 0, y0: 0, y1: 10 }), [], "largeur nulle");
  // Les bornes sont exclusives : un clip qui finit exactement au bord n'est pas touché.
  assert.deepEqual(clipsInRect(nodes, rowH, { s0: 100, s1: 150, y0: 0, y1: 40 }), []);
  assert.deepEqual(allClipIds(nodes).sort(), [10, 11, 20, 40]);
});

test("groupe : décalage borné par le clip le plus à gauche, arrondi entier", () => {
  assert.equal(clampGroupDelta([100, 300], -500), -100);
  assert.equal(clampGroupDelta([100, 300], 25.6), 26);
  assert.equal(clampGroupDelta([0, 5], -3), 0);
  assert.deepEqual(groupDragCommand([1, 2], 40, false), { type: "move_clips", clips: [1, 2], delta: 40 });
  assert.equal(groupDragCommand([1, 2], 0, false), null);
  assert.deepEqual(groupDragCommand([1], 0, true), { type: "duplicate_clips", clips: [1], delta: 0 });
});

test("prune : les clips disparus sortent de la sélection", () => {
  assert.deepEqual([...prune(new Set([10, 99, 40]), nodes)].sort(), [10, 40]);
});

test("validation des commandes de groupe", () => {
  assert.equal(validateCommand({ type: "move_clips", clips: [1, 2], delta: -5 }), null);
  assert.notEqual(validateCommand({ type: "move_clips", clips: [], delta: 1 }), null);
  assert.notEqual(validateCommand({ type: "move_clips", clips: [1], delta: 1.5 }), null);
  assert.equal(validateCommand({ type: "delete_clips", clips: [3] }), null);
  assert.notEqual(validateCommand({ type: "copy_clips", clips: ["a"] }), null);
  assert.equal(validateCommand({ type: "paste_clips", at: 100 }), null);
  assert.equal(validateCommand({ type: "paste_clips", at: 100, node: 2 }), null);
  assert.notEqual(validateCommand({ type: "paste_clips", at: -1 }), null);
  assert.equal(validateCommand({ type: "duplicate_clips", clips: [3], delta: -10 }), null);
  assert.notEqual(validateCommand({ type: "duplicate_clips", clips: [3], delta: "x" }), null);
});

function mock() {
  const e = new MockEngine();
  const log = { errors: [], projects: [], selection: [], clipboard: [], history: [] };
  e.on("error", (x) => log.errors.push(x.message));
  e.on("project", (x) => log.projects.push(x));
  e.on("selection", (x) => log.selection.push(x.clips));
  e.on("clipboard", (x) => log.clipboard.push(x.count));
  e.on("history", (x) => log.history.push(x));
  e.send({ type: "get_project" });
  const clips = () => log.projects.at(-1).nodes.flatMap((n) => n.clips.map((c) => ({ ...c, node: n.id })));
  return { e, log, clips };
}

test("faux moteur : déplacer, supprimer un groupe = une action annulable", () => {
  const { e, log, clips } = mock();
  const [a, b] = clips().filter((c) => c.kind === "audio");
  e.send({ type: "move_clips", clips: [a.id, b.id], delta: 1000 });
  assert.equal(clips().find((c) => c.id === a.id).start, a.start + 1000);
  assert.equal(clips().find((c) => c.id === b.id).start, b.start + 1000);
  assert.equal(log.history.at(-1).undo_label, "Déplacer des clips");
  e.send({ type: "undo" });
  assert.equal(clips().find((c) => c.id === a.id).start, a.start);
  const n = clips().length;
  e.send({ type: "delete_clips", clips: [a.id, b.id] });
  assert.equal(clips().length, n - 2);
  e.send({ type: "undo" });
  assert.equal(clips().length, n);
  e.send({ type: "delete_clips", clips: [a.id, 999999] });
  assert.match(log.errors.at(-1), /clip inconnu/);
  assert.equal(clips().length, n, "refusé sans trace");
});

test("faux moteur : copier, coller et dupliquer", () => {
  const { e, log, clips } = mock();
  const [a, b] = clips().filter((c) => c.kind === "audio");
  e.send({ type: "paste_clips", at: 0 });
  assert.match(log.errors.at(-1), /vide/);
  e.send({ type: "copy_clips", clips: [a.id, b.id] });
  assert.equal(log.clipboard.at(-1), 2);
  const n = clips().length;
  e.send({ type: "paste_clips", at: 500000 });
  assert.equal(clips().length, n + 2);
  const made = log.selection.at(-1);
  assert.equal(made.length, 2);
  const [na, nb] = made.map((id) => clips().find((c) => c.id === id));
  const base = Math.min(a.start, b.start);
  assert.equal(na.start, 500000 + (a.start - base));
  assert.equal(nb.start, 500000 + (b.start - base));
  assert.equal(na.node, a.node);
  assert.equal(log.history.at(-1).undo_label, "Coller des clips");
  e.send({ type: "undo" });
  assert.equal(clips().length, n);
  e.send({ type: "duplicate_clips", clips: [a.id] });
  const d = clips().find((c) => c.id === log.selection.at(-1)[0]);
  assert.equal(d.start, a.start + a.len, "juste après le groupe");
  e.send({ type: "duplicate_clips", clips: [a.id], delta: 777 });
  assert.equal(clips().find((c) => c.id === log.selection.at(-1)[0]).start, a.start + 777);
});

import { trackIds, clampNodeDelta, shiftedTrack } from "../js/selection.js";

test("déplacement vertical d'un groupe : décalage en pistes borné, disposition conservée", () => {
  const rows = [{ id: 1, kind: "stereo" }, { id: 2, kind: "mono" }, { id: 3, kind: "lane" }, { id: 4, kind: "bus" }, { id: 5, kind: "mono" }, { id: 9, kind: "master" }];
  const tracks = trackIds(rows);
  assert.deepEqual(tracks, [1, 2, 5], "ni voie d'automation, ni bus, ni sortie principale");
  assert.equal(clampNodeDelta(tracks, [1, 2], 1, 2), 1);
  assert.equal(clampNodeDelta(tracks, [1, 2], 1, 5), 1, "le groupe (pistes 1 et 2) ne peut descendre que d'une piste : il resterait sinon sans piste");
  assert.equal(clampNodeDelta(tracks, [2, 5], 5, 1), -1, "vers le haut aussi");
  assert.equal(clampNodeDelta(tracks, [1], 1, 9), 0, "destination qui n'est pas une piste");
  assert.equal(shiftedTrack(tracks, 1, 2), 5);
  assert.equal(shiftedTrack(tracks, 5, 3), 5, "borné");
  assert.deepEqual(groupDragCommand([7], 0, false, 1), { type: "move_clips", clips: [7], delta: 0, node_delta: 1 });
  assert.equal(groupDragCommand([7], 0, false, 0), null);
  assert.deepEqual(groupDragCommand([7], 5, true, -1), { type: "duplicate_clips", clips: [7], delta: 5, node_delta: -1 });
});

test("validation : split_clips et node_delta", () => {
  assert.equal(validateCommand({ type: "split_clips", clips: [1, 2], at: 100 }), null);
  assert.notEqual(validateCommand({ type: "split_clips", clips: [], at: 100 }), null);
  assert.notEqual(validateCommand({ type: "split_clips", clips: [1], at: -1 }), null);
  assert.equal(validateCommand({ type: "move_clips", clips: [1], delta: 0, node_delta: -2 }), null);
  assert.notEqual(validateCommand({ type: "move_clips", clips: [1], delta: 0, node_delta: 0.5 }), null);
});

test("faux moteur : le groupe change de piste, découpe en bloc, collage sur une piste choisie", () => {
  const e = new MockEngine();
  const log = { errors: [], projects: [], selection: [] };
  e.on("error", (x) => log.errors.push(x.message));
  e.on("project", (x) => log.projects.push(x));
  e.on("selection", (x) => log.selection.push(x.clips));
  e.send({ type: "get_project" });
  const p = () => log.projects.at(-1);
  const where = (id) => p().nodes.find((n) => n.clips.some((c) => c.id === id)).id;
  const clip = (id) => p().nodes.flatMap((n) => n.clips).find((c) => c.id === id);
  const tracks = trackIds(p().nodes);
  const first = p().nodes.find((n) => n.id === tracks[0]).clips[0].id;
  e.send({ type: "move_clips", clips: [first], delta: 0, node_delta: 1 });
  assert.equal(where(first), tracks[1]);
  e.send({ type: "undo" });
  assert.equal(where(first), tracks[0]);
  // Découper tout ce qui est sous une position.
  const all = p().nodes.flatMap((n) => n.clips);
  const at = Math.max(...all.map((c) => c.start)) + 100;
  const under = all.filter((c) => c.start < at && at < c.start + c.len).map((c) => c.id);
  assert.ok(under.length >= 2);
  e.send({ type: "split_clips", clips: all.map((c) => c.id), at });
  assert.equal(log.selection.at(-1).length, under.length * 2);
  assert.equal(p().nodes.flatMap((n) => n.clips).length, all.length + under.length);
  e.send({ type: "undo" });
  e.send({ type: "split_clips", clips: [first], at: at + 9e9 });
  assert.match(log.errors.at(-1), /aucun clip/);
  // Collage sur la 2e piste : la plus haute des pistes copiées vient dessus.
  e.send({ type: "copy_clips", clips: [first] });
  e.send({ type: "paste_clips", at: 400000, node: tracks[1] });
  assert.equal(where(log.selection.at(-1)[0]), tracks[1]);
  e.send({ type: "paste_clips", at: 400000, node: 0 });
  assert.match(log.errors.at(-1), /n'est pas une piste/);
});
