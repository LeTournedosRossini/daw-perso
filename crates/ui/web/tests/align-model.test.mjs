import test from "node:test";
import assert from "node:assert/strict";
import { addPoint, alignProgressText, audioClips, cleanConsonants, cleanCurve, defaultRoles, guideTimeOf, mergeGeneral, movePoint, parseSettings, removePoint, shiftColumns, strengthAt, stringifySettings } from "../js/align-model.js";
import { validateCommand } from "../js/protocol.js";

test("la fiche est complétée et bornée, jamais d'exception", () => {
  const d = parseSettings(undefined);
  assert.equal(d.general.engine, "signalsmith");
  assert.equal(d.general.strength, 1);
  assert.deepEqual(d.local.strength_curve, []);
  const weird = parseSettings(JSON.stringify({ general: { engine: "inconnu", strength: 7, tightness: "x", analysis_mode: "harmony" }, local: { strength_curve: [[2, 2], [1, -1], [1, 0.5], "?"] } }));
  assert.equal(weird.general.engine, "signalsmith");
  assert.equal(weird.general.strength, 1);
  assert.equal(weird.general.tightness, 0.95);
  assert.equal(weird.general.analysis_mode, "harmony");
  assert.deepEqual(weird.local.strength_curve, [[1, 0], [2, 1]], "triée, bornée, temps distincts (le premier de deux temps égaux gagne)");
  assert.equal(parseSettings("pas du json").general.protect, 1);
  assert.deepEqual(parseSettings(stringifySettings(weird)), weird, "aller-retour");
});

test("copier les réglages : le bloc général vient de la source, la courbe de force reste celle de la destination", () => {
  const dst = parseSettings(JSON.stringify({ general: { strength: 0.4 }, local: { strength_curve: [[1, 0.2]] } }));
  const src = parseSettings(JSON.stringify({ general: { strength: 0.9, engine: "rubberband_r2" }, local: { strength_curve: [[5, 1]] } }));
  const m = mergeGeneral(dst, src);
  assert.equal(m.general.strength, 0.9);
  assert.equal(m.general.engine, "rubberband_r2");
  assert.deepEqual(m.local.strength_curve, [[1, 0.2]]);
});

test("courbe de force : valeur, ajout, déplacement borné, suppression", () => {
  const c = [[1, 0], [3, 1]];
  assert.equal(strengthAt([], 0.7, 2), 0.7);
  assert.equal(strengthAt(c, 1, 0), 0);
  assert.equal(strengthAt(c, 1, 2), 0.5);
  assert.equal(strengthAt(c, 1, 9), 1);
  const a = addPoint(c, 2, 0.25);
  assert.deepEqual(a.curve, [[1, 0], [2, 0.25], [3, 1]]);
  assert.equal(a.index, 1);
  assert.deepEqual(addPoint(c, 1.01, 0.5).curve, [[1, 0.5], [3, 1]], "un point tout proche est remplacé");
  assert.deepEqual(movePoint(a.curve, 1, 9, 5), [[1, 0], [2.99, 1], [3, 1]], "reste avant son voisin, force bornée");
  assert.deepEqual(movePoint(a.curve, 1, -4, -1), [[1, 0], [1.01, 0], [3, 1]]);
  assert.deepEqual(removePoint(a.curve, 1), c);
  assert.deepEqual(cleanCurve("rien"), []);
});

test("consonnes et correspondance de temps", () => {
  assert.deepEqual(cleanConsonants([[2, 3], [1, 1], [1, 1.5], [-1, 2], "?"]), [[1, 1.5], [2, 3]]);
  const path = [0, 0.1, 0.2, 0.3]; // double 20 ms en avance sur le guide : 0,1 s de double tous les 5 ms
  assert.equal(guideTimeOf(path, 0.005, 0.15), 0.0075);
  assert.equal(guideTimeOf(path, 0.005, -1), 0);
  assert.equal(guideTimeOf(path, 0.005, 9), 0.015);
  assert.equal(guideTimeOf([], 0.005, 1), 0);
});

test("colonnes du tracé du décalage", () => {
  const cols = shiftColumns([0, 0.01, 0.02, -0.02], 2);
  assert.deepEqual(cols.map((c) => c.map((x) => Math.round(x))), [[0, 10], [-20, 20]]);
  assert.deepEqual(shiftColumns([], 3), [[0, 0], [0, 0], [0, 0]]);
});

test("textes d'avancement, clips audio et rôles par défaut", () => {
  assert.equal(alignProgressText({ active: true, done: 1, total: 4, current: "Voix 2", progress: 0.372 }), "Calage 2 / 4 — Voix 2 · 37 %");
  assert.equal(alignProgressText({ active: false, cancelled: true }), "Calage annulé");
  assert.equal(alignProgressText({ active: false, error: "oups" }), "Calage impossible — oups");
  assert.equal(alignProgressText({ active: false, clips: [1, 2], warnings: ["x"] }), "2 prises calées (1 avertissement)");
  const project = { nodes: [{ id: 0, kind: "master", clips: [] }, { id: 1, kind: "mono", name: "Voix", clips: [{ id: 10, kind: "audio", name: "Guide", start: 0, len: 100 }, { id: 11, kind: "audio", name: "D1", start: 0, len: 100, aligned: true }, { id: 12, kind: "audio", name: "D2", start: 0, len: 100, muted: true }, { id: 13, kind: "midi", name: "Notes" }] }] };
  assert.deepEqual(audioClips(project).map((c) => c.id), [10, 11, 12]);
  const r = defaultRoles(project, 10, new Set([10, 11, 12]));
  assert.equal(r.guide.id, 10);
  assert.deepEqual(r.doubles, [11], "les prises inactives ne sont pas proposées");
  assert.equal(defaultRoles(project, null, new Set()).guide, null);
});

test("validation des commandes de calage", () => {
  assert.equal(validateCommand({ type: "align_clips", guide: 1, doubles: [2, 3] }), null);
  assert.equal(validateCommand({ type: "align_clips", guide: 1, doubles: [2], settings: "{}" }), null);
  for (const bad of [{ doubles: [] }, { doubles: [1] }, { guide: "x" }, { doubles: [2, "y"] }, { settings: 5 }]) assert.notEqual(validateCommand({ type: "align_clips", guide: 1, doubles: [2], ...bad }), null, JSON.stringify(bad));
  assert.equal(validateCommand({ type: "align_redo", clips: [5], reanalyse: true, consonants: [[0.1, 0.2]], general_only: false }), null);
  assert.notEqual(validateCommand({ type: "align_redo", clips: [] }), null);
  assert.notEqual(validateCommand({ type: "align_redo", clips: [5, 6], consonants: [[0.1, 0.2]] }), null, "consonnes : un seul clip");
  assert.notEqual(validateCommand({ type: "align_redo", clips: [5], consonants: [[0.3, 0.2]] }), null);
  assert.equal(validateCommand({ type: "cancel_align" }), null);
  assert.equal(validateCommand({ type: "get_align", clip: 5, threshold_ms: 30 }), null);
  assert.notEqual(validateCommand({ type: "get_align", clip: 5, threshold_ms: 0 }), null);
  assert.equal(validateCommand({ type: "merge_align_settings", dst: "{}", src: "{}" }), null);
  assert.notEqual(validateCommand({ type: "merge_align_settings", dst: 1, src: "{}" }), null);
});

import { MockEngine } from "../js/mock-engine.js";

function mockWith() {
  const e = new MockEngine();
  const events = { align: [], detail: [], errors: [], merged: [] };
  e.on("align", (x) => events.align.push(x));
  e.on("align_detail", (x) => events.detail.push(x));
  e.on("align_settings", (x) => events.merged.push(x));
  e.on("error", (x) => events.errors.push(x.message));
  e.send({ type: "get_project" });
  return { e, events };
}

test("faux moteur : caler un double sur un guide d'une autre piste ajoute une prise active, puis refaire et copier", () => {
  const { e, events } = mockWith();
  const before = e.project.nodes.find((n) => n.name === "Basse").clips.length;
  e.send({ type: "align_clips", guide: 10, doubles: [20] });
  assert.equal(events.align.at(-1).active, true);
  e.advance(2000);
  const done = events.align.at(-1);
  assert.equal(done.active, false);
  assert.equal(done.clips.length, 1);
  const bass = e.project.nodes.find((n) => n.name === "Basse");
  assert.equal(bass.clips.length, before + 1);
  const taken = bass.clips.find((c) => c.id === done.clips[0].clip);
  assert.ok(taken.aligned && !taken.muted && taken.take_group > 0);
  assert.ok(bass.clips.find((c) => c.id === 20).muted, "l'original est muet");
  assert.equal(e.undoStack.at(-1).label, "Caler des prises");
  // Détails, puis refaire avec une courbe de force.
  e.send({ type: "get_align", clip: taken.id });
  const d = events.detail.at(-1);
  assert.ok(d.path.length > 100 && d.hop === 0.005);
  const s = JSON.parse(d.settings);
  s.local.strength_curve = [[1, 0.5]];
  e.send({ type: "align_redo", clips: [taken.id], settings: JSON.stringify(s) });
  e.advance(2000);
  const second = events.align.at(-1).clips[0].clip;
  assert.notEqual(second, taken.id);
  assert.equal(e.project.nodes.find((n) => n.name === "Basse").clips.filter((c) => c.aligned).length, 2);
  // Copier : le bloc général d'une fiche, la courbe de force de l'autre.
  const g = JSON.parse(d.settings);
  g.general.strength = 0.3;
  e.send({ type: "align_redo", clips: [second], settings: JSON.stringify(g), general_only: true });
  e.advance(2000);
  e.send({ type: "get_align", clip: events.align.at(-1).clips[0].clip });
  const copied = JSON.parse(events.detail.at(-1).settings);
  assert.equal(copied.general.strength, 0.3);
  assert.deepEqual(copied.local.strength_curve, [[1, 0.5]]);
  assert.deepEqual(events.errors, []);
});

test("faux moteur : refus (même piste, clip inconnu, réglages invalides), annulation, fusion", () => {
  const { e, events } = mockWith();
  e.send({ type: "align_clips", guide: 40, doubles: [41] });
  assert.match(events.errors.at(-1), /même piste/);
  e.send({ type: "align_clips", guide: 999, doubles: [20] });
  assert.match(events.errors.at(-1), /inconnu/);
  e.send({ type: "align_clips", guide: 10, doubles: [20], settings: "pas du json" });
  assert.match(events.errors.at(-1), /JSON/);
  e.send({ type: "align_redo", clips: [20] });
  assert.match(events.errors.at(-1), /pas une prise calée/);
  const n = e.project.nodes.reduce((s, x) => s + x.clips.length, 0);
  e.send({ type: "align_clips", guide: 10, doubles: [20] });
  e.send({ type: "align_clips", guide: 10, doubles: [20] });
  assert.match(events.errors.at(-1), /déjà en cours/);
  e.send({ type: "cancel_align" });
  e.advance(100);
  assert.equal(events.align.at(-1).cancelled, true);
  assert.equal(e.project.nodes.reduce((s, x) => s + x.clips.length, 0), n, "rien d'ajouté");
  e.send({ type: "merge_align_settings", dst: JSON.stringify({ general: { strength: 0.2 }, local: { strength_curve: [[1, 1]] } }), src: JSON.stringify({ general: { strength: 0.9 }, local: { strength_curve: [[5, 0]] } }) });
  const m = JSON.parse(events.merged.at(-1).settings);
  assert.equal(m.general.strength, 0.9);
  assert.deepEqual(m.local.strength_curve, [[1, 1]]);
  e.send({ type: "merge_align_settings", dst: "{", src: "{}" });
  assert.match(events.errors.at(-1), /JSON/);
});

import { overlaps } from "../js/align-model.js";
test("recouvrement de deux clips", () => {
  assert.ok(overlaps({ start: 0, len: 10 }, { start: 5, len: 10 }));
  assert.ok(!overlaps({ start: 0, len: 10 }, { start: 10, len: 5 }), "qui se touchent seulement : non");
});

import { alignedOn, bestAligned, doublesFor, guideInfo, originalsOn, sampleX, xSample } from "../js/align-model.js";

const proj = () => ({
  nodes: [
    { id: 0, kind: "master", clips: [] },
    { id: 1, kind: "mono", name: "Guide", clips: [{ id: 1, kind: "audio", name: "G", start: 1000, len: 1000 }] },
    { id: 2, kind: "mono", name: "Voix 2", clips: [{ id: 2, kind: "audio", name: "D", start: 900, len: 1200, muted: true, take_group: 1 }, { id: 5, kind: "audio", name: "D calé", start: 1000, len: 1000, aligned: true, take_group: 1 }, { id: 7, kind: "audio", name: "D calé 2", start: 1000, len: 1000, aligned: true, muted: true, take_group: 1 }] },
    { id: 3, kind: "stereo", name: "Voix 3", clips: [{ id: 3, kind: "audio", name: "E", start: 1500, len: 1000 }, { id: 4, kind: "audio", name: "loin", start: 5000, len: 100 }] },
    { id: 4, kind: "mono", name: "Vide", clips: [{ id: 9, kind: "audio", name: "ailleurs", start: 9000, len: 10 }] },
    { id: 5, kind: "bus", name: "Bus", clips: [] },
  ],
});

test("doubles d'un guide : une entrée par piste, clip d'origine et prise calée à montrer", () => {
  const p = proj();
  const g = guideInfo(p, 1);
  assert.deepEqual([g.node, g.start, g.len], [1, 1000, 1000]);
  assert.equal(guideInfo(p, 999), null);
  const d = doublesFor(p, g);
  assert.deepEqual(d.map((x) => [x.node, x.clipId, x.aligned]), [[2, 2, 5], [3, 3, null]], "pas la piste du guide, ni les bus, ni les pistes qui ne recouvrent pas la fenêtre");
  assert.deepEqual(originalsOn(p, 2, g).map((c) => c.id), [2]);
  assert.deepEqual(alignedOn(p, 2, g).map((c) => c.id), [7, 5], "la plus récente d'abord");
  assert.equal(bestAligned(p, 2, g).id, 5, "l'active d'abord");
  p.nodes[2].clips[1].muted = true;
  assert.equal(bestAligned(p, 2, g).id, 7, "sinon la plus récente");
  assert.equal(bestAligned(p, 3, g), null);
  assert.deepEqual(doublesFor(p, null), []);
});

test("conversions horizontales", () => {
  const view = { start: 1000, spp: 4 };
  assert.equal(sampleX(view, 1400), 100);
  assert.equal(xSample(view, 100), 1400);
});

import { addConsonant, doubleTimeOf, editConsonant } from "../js/align-model.js";

test("temps du double ↔ temps du guide : deux conversions inverses l'une de l'autre", () => {
  const path = [0, 0.012, 0.03, 0.05, 0.07, 0.09]; // le double ralentit puis suit
  const hop = 0.005;
  for (const td of [0.005, 0.02, 0.04, 0.085]) assert.ok(Math.abs(doubleTimeOf(path, hop, guideTimeOf(path, hop, td)) - td) < 1e-9, `td ${td}`);
  assert.equal(doubleTimeOf([], hop, 1), 0);
  assert.equal(doubleTimeOf(path, hop, -3), 0);
  assert.equal(doubleTimeOf(path, hop, 9), 0.09);
});

test("modifier les consonnes sur le tracé : bords et corps bornés par les voisines, ajout refusé s'il recouvre", () => {
  const l = [[1, 1.2], [2, 2.3]];
  assert.deepEqual(editConsonant(l, 0, "l", 0.8), [[0.8, 1.2], [2, 2.3]]);
  assert.deepEqual(editConsonant(l, 0, "l", 5), [[1.19, 1.2], [2, 2.3]], "garde au moins 10 ms");
  assert.deepEqual(editConsonant(l, 0, "r", 9), [[1, 2], [2, 2.3]], "ne dépasse pas la voisine");
  assert.deepEqual(editConsonant(l, 1, "move", 2.6, 2.1).map((p) => p.map((x) => Math.round(x * 100) / 100)), [[1, 1.2], [2.5, 2.8]], "le corps garde sa longueur");
  assert.deepEqual(editConsonant(l, 1, "move", 0, 2.1)[1].map((x) => Math.round(x * 100) / 100), [1.2, 1.5], "ne passe pas sous la voisine");
  assert.deepEqual(editConsonant(l, 7, "l", 1), l, "indice inconnu : inchangé");
  const a = addConsonant(l, 1.5, 1.8);
  assert.deepEqual(a.list, [[1, 1.2], [1.5, 1.8], [2, 2.3]]);
  assert.equal(a.index, 1);
  assert.equal(addConsonant(l, 1.1, 1.5).index, -1, "recouvre la première");
  assert.equal(addConsonant(l, 1.5, 1.505).index, -1, "trop courte");
  assert.deepEqual(addConsonant(l, 3.2, 3.0).list.at(-1), [3, 3.2], "dans n'importe quel ordre");
});

test("guideInfo : des clips de la piste du guide forment une seule fenêtre", async () => {
  const { guideInfo, doublesFor } = await import("../js/align-model.js");
  const mk = (id, start, len, extra = {}) => ({ id, kind: "audio", name: `c${id}`, start, len, ...extra });
  const project = { nodes: [
    { id: 1, name: "Guide", kind: "track", clips: [mk(10, 1000, 500), mk(11, 2000, 500)] },
    { id: 2, name: "Double", kind: "track", clips: [mk(20, 900, 700), mk(21, 1900, 700), mk(22, 4000, 100)] },
  ] };
  const g = guideInfo(project, 10, [11]);
  assert.deepEqual([g.start, g.len, g.ids, g.name], [1000, 1500, [10, 11], "c10 + 1"]);
  assert.equal(guideInfo(project, 10).len, 500);
  const [d] = doublesFor(project, g, new Set([20, 21]));
  assert.deepEqual(d.clipIds, [20, 21]);
  assert.deepEqual(doublesFor(project, g, new Set())[0].clipIds.length, 1, "sans choix : le clip le plus recouvrant");
});

test("align_clips / listen_align : guide_more validé", () => {
  assert.equal(validateCommand({ type: "align_clips", guide: 1, guide_more: [4], doubles: [2] }), null);
  assert.notEqual(validateCommand({ type: "align_clips", guide: 1, guide_more: [2], doubles: [2] }), null, "un clip ne peut être guide et double");
  assert.notEqual(validateCommand({ type: "align_clips", guide: 1, guide_more: "x", doubles: [2] }), null);
  assert.equal(validateCommand({ type: "listen_align", guide: 1, guide_more: [4], node: 2, what: "guide" }), null);
  assert.notEqual(validateCommand({ type: "listen_align", guide: 1, guide_more: [1.5], node: 2, what: "guide" }), null);
});
