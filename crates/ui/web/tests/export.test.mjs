import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { buildExportCommand, progressText } from "../js/ui/export-dialog.js";

const base = { type: "export", scope: "project", mode: "mix", format: "wav24" };

function setup() {
  const e = new MockEngine();
  const exports = [];
  const errors = [];
  e.on("export", (x) => exports.push(x));
  e.on("error", (x) => errors.push(x));
  e.send({ type: "get_project" });
  return { e, exports, errors };
}

test("validation : export et cancel_export", () => {
  assert.equal(validateCommand(base), null);
  assert.equal(validateCommand({ type: "cancel_export" }), null);
  assert.equal(validateCommand({ ...base, scope: "range", start: 10, end: 99, mode: "stems", nodes: [1, 2], format: "flac16", tail_secs: 0 }), null);
  assert.equal(validateCommand({ ...base, normalize_db: -1, dir: "/tmp/x", name: "Chanson", dither: true }), null);
  for (const bad of [
    { scope: "tout" }, { scope: "range" }, { scope: "range", start: 5, end: 5 }, { mode: "autre" }, { format: "mp3" }, { tail_secs: 99 }, { tail_secs: -1 },
    { normalize_db: 3 }, { normalize_db: -31 }, { mode: "stems", normalize_db: -1 }, { mode: "stems", nodes: [] }, { mode: "stems", nodes: [0] }, { dir: "  " }, { name: "" },
  ]) assert.notEqual(validateCommand({ ...base, ...bad }), null, JSON.stringify(bad));
});

test("commande d'après les choix de la fenêtre", () => {
  const o = { mode: "mix", scope: "project", format: "wav16", dither: true, tail: 5, normalize: true, normalizeDb: -1 };
  assert.deepEqual(buildExportCommand(o), { type: "export", scope: "project", mode: "mix", format: "wav16", tail_secs: 5, normalize_db: -1, dither: true });
  // Clip sélectionné → zone « range » ; stems → nœuds choisis, jamais de normalisation.
  const c = buildExportCommand({ ...o, mode: "stems", scope: "clip", format: "flac24", dither: false }, { clip: { start: 100, len: 50 }, nodes: [1, 3], dir: "/d", name: " Mix " });
  assert.deepEqual(c, { type: "export", scope: "range", start: 100, end: 150, mode: "stems", format: "flac24", tail_secs: 5, nodes: [1, 3], dither: false, dir: "/d", name: "Mix" });
  // Flottant : pas de dither ; queue bornée.
  const f = buildExportCommand({ ...o, format: "wav32f", normalize: false, tail: 500 });
  assert.equal(f.dither, undefined);
  assert.equal(f.tail_secs, 60);
  for (const cmd of [buildExportCommand(o), c, f]) assert.equal(validateCommand(cmd), null, JSON.stringify(cmd));
});

test("texte d'avancement", () => {
  const e = { active: true, progress: 0.42, done: 2, total: 8, current: "P - Basse", files: [], warnings: [], cancelled: false, error: null };
  assert.equal(progressText(e), "3 / 8 — P - Basse · 42 %");
  assert.match(progressText({ ...e, active: false, cancelled: true }), /Annulé — 2/);
  assert.match(progressText({ ...e, active: false, error: "disque plein" }), /Échec — disque plein/);
  assert.match(progressText({ ...e, active: false, done: 8 }), /Terminé — 8/);
});

test("faux moteur : refus comme le vrai (dossier, boucle, zone, nœud)", () => {
  const { e, errors } = setup();
  e.send({ ...base });
  assert.match(errors.pop().message, /dossier/);
  e.send({ type: "save_project", path: "/p/Chanson" });
  e.send({ ...base, scope: "loop" });
  assert.match(errors.pop().message, /boucle/);
  e.send({ ...base, mode: "stems", nodes: [99] });
  assert.match(errors.pop().message, /inconnu/);
  e.send({ ...base, scope: "range", start: 5, end: 5 }); // refusé dès la validation
  assert.match(errors.pop().message, /start < end/);
});

test("faux moteur : mixage puis stems, avancement croissant, sans écraser", () => {
  const { e, exports } = setup();
  e.send({ type: "save_project", path: "/p/Chanson" });
  e.send({ ...base });
  assert.equal(exports.at(-1).active, true);
  assert.equal(exports.at(-1).total, 1);
  for (let i = 0; i < 4; i++) e.advance(200);
  const done = exports.at(-1);
  assert.equal(done.active, false);
  assert.equal(done.files.length, 1);
  assert.equal(done.files[0].path, "/p/Chanson.daw/export/Chanson.wav");
  assert.ok(exports.map((x) => x.progress).every((p, i, a) => i === 0 || p >= a[i - 1]));
  // Même export de nouveau : numéroté, le premier n'est pas écrasé.
  e.send({ ...base });
  for (let i = 0; i < 4; i++) e.advance(200);
  assert.equal(exports.at(-1).files[0].path, "/p/Chanson.daw/export/Chanson (2).wav");
  // Stems : un fichier par piste et bus.
  e.send({ ...base, mode: "stems", format: "flac16", name: "S" });
  assert.equal(exports.at(-1).total, 5);
  for (let i = 0; i < 20; i++) e.advance(200);
  const stems = exports.at(-1);
  assert.equal(stems.active, false);
  assert.equal(stems.files.length, 5);
  assert.ok(stems.files.every((f) => f.path.endsWith(".flac")));
  assert.ok(stems.files.some((f) => f.path.endsWith("S - Basse.flac")));
});

test("faux moteur : annulation, et pas de nouveau projet ni de second export en cours d'export", () => {
  const { e, exports, errors } = setup();
  e.send({ type: "save_project", path: "/p/C" });
  e.send({ ...base, mode: "stems" });
  e.send({ ...base });
  assert.match(errors.pop().message, /déjà/);
  e.send({ type: "new_project" });
  assert.match(errors.pop().message, /export/);
  e.advance(700); // un stem fini
  e.send({ type: "cancel_export" });
  e.advance(30);
  const end = exports.at(-1);
  assert.equal(end.active, false);
  assert.equal(end.cancelled, true);
  assert.equal(end.files.length, 1, "les fichiers déjà terminés sont conservés");
  e.send({ type: "new_project" }); // libre de nouveau
  assert.equal(errors.length, 0);
});
