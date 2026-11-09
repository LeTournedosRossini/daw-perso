import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { buildImportCommand, describeFile, importProgressText, isAudioPath, needsChoice, referencable } from "../js/import-model.js";

function setup() {
  const e = new MockEngine();
  const log = { info: [], imports: [], errors: [], projects: [], history: [] };
  e.on("audio_info", (x) => log.info.push(x));
  e.on("import", (x) => log.imports.push(x));
  e.on("error", (x) => log.errors.push(x));
  e.on("project", (x) => log.projects.push(x));
  e.on("history", (x) => log.history.push(x));
  e.send({ type: "get_project" });
  return { e, log, project: () => log.projects.at(-1) };
}
const settle = (e, ms = 4000) => { for (let i = 0; i < ms / 100; i++) e.advance(100); };

test("validation : probe_audio, import_audio, cancel_import", () => {
  assert.equal(validateCommand({ type: "probe_audio", files: ["/a.wav"] }), null);
  assert.equal(validateCommand({ type: "import_audio", files: ["/a.wav", "/b.flac"], node: 2, pos: 48000, reference: true, multichannel: "split" }), null);
  assert.equal(validateCommand({ type: "cancel_import" }), null);
  for (const bad of [{ files: [] }, { files: [" "] }, { files: [1] }, { files: ["a"], node: 0 }, { files: ["a"], pos: -1 }, { files: ["a"], pos: 1.5 }, { files: ["a"], multichannel: "mono" }, { files: Array(65).fill("a") }]) {
    assert.notEqual(validateCommand({ type: "import_audio", ...bad }), null, JSON.stringify(bad));
  }
});

test("modèle : extensions, description, choix, commande", () => {
  assert.ok(isAudioPath("C:\\Sons\\Boucle.WAV") && isAudioPath("/a/b.flac") && isAudioPath("x.mp3"));
  assert.ok(!isAudioPath("note.txt") && !isAudioPath("sans-extension") && !isAudioPath(null));
  const st = { path: "a", name: "a", channels: 2, sample_rate: 44100, frames: 44100 * 75, needs_conversion: true, error: null };
  assert.equal(describeFile(st, 48000), "stéréo · 44,1 kHz → 48 kHz (converti) · 1:15");
  assert.equal(describeFile({ ...st, channels: 6, sample_rate: 48000, needs_conversion: false, frames: null }, 48000), "6 canaux · 48 kHz");
  assert.match(describeFile({ ...st, error: "format non reconnu" }, 48000), /illisible : format non reconnu/);
  assert.ok(!referencable(st) && referencable({ ...st, needs_conversion: false }) && !referencable({ ...st, needs_conversion: false, channels: 6 }) && !referencable({ ...st, error: "x" }));
  assert.ok(needsChoice([st, { ...st, channels: 6 }]) && !needsChoice([st]) && !needsChoice([{ ...st, channels: 6, error: "x" }]));
  assert.deepEqual(buildImportCommand({ files: ["/a.wav"] }), { type: "import_audio", files: ["/a.wav"], reference: false });
  const c = buildImportCommand({ files: ["/a.wav"], target: { node: 3, sample: 1234.6 }, reference: true, multichannel: "stereo" });
  assert.deepEqual(c, { type: "import_audio", files: ["/a.wav"], reference: true, node: 3, pos: 1235, multichannel: "stereo" });
  assert.equal(validateCommand(c), null);
  assert.equal(buildImportCommand({ files: ["a"], target: { node: null, sample: 10 } }).node, undefined);
  assert.equal(importProgressText({ active: true, done: 1, total: 3, current: "x", progress: 0.5 }), "Import 2 / 3 — x · 50 %");
  assert.equal(importProgressText({ active: false, clips: [{}], error: null }), "Importé : 1 clip");
  assert.match(importProgressText({ active: false, clips: [{}, {}], error: null }), /2 clips/);
  assert.match(importProgressText({ active: false, cancelled: true, clips: [] }), /annulé/);
  assert.match(importProgressText({ active: false, error: "disque plein", clips: [] }), /impossible — disque plein/);
});

test("faux moteur : en-têtes simulés", () => {
  const { e, log } = setup();
  e.send({ type: "probe_audio", files: ["/s/Boucle 44k.wav", "/s/Piste mono.flac", "/s/Film 5.1.wav", "/s/notes.txt"] });
  const f = log.info.at(-1).files;
  assert.deepEqual([f[0].sample_rate, f[0].needs_conversion, f[0].channels], [44100, true, 2]);
  assert.deepEqual([f[1].channels, f[1].needs_conversion], [1, false]);
  assert.equal(f[2].channels, 6);
  assert.ok(f[3].error);
});

test("faux moteur : refus (dossier, nœud, multicanal sans choix)", () => {
  const { e, log } = setup();
  e.send({ type: "import_audio", files: ["/s/a.wav"] });
  assert.match(log.errors.pop().message, /dossier/);
  e.send({ type: "save_project", path: "/p/C" });
  e.send({ type: "import_audio", files: ["/s/a.wav"], node: 0 });
  assert.match(log.errors.pop().message, /sortie principale/);
  e.send({ type: "import_audio", files: ["/s/a.wav"], node: 99 });
  assert.match(log.errors.pop().message, /piste/);
  e.send({ type: "import_audio", files: ["/s/Film 5.1.wav"] });
  settle(e);
  assert.match(log.imports.at(-1).error, /canaux/);
});

test("faux moteur : import sur la piste visée puis nouvelles pistes, en un seul pas d'annulation", () => {
  const { e, log, project } = setup();
  e.send({ type: "save_project", path: "/p/C" });
  const before = project().nodes.length;
  const clipsOnBass = project().nodes.find((n) => n.id === 2).clips.length;
  e.send({ type: "import_audio", files: ["/s/Un.wav", "/s/Deux 44k.flac"], node: 2, pos: 12345, reference: true });
  assert.equal(log.imports.at(-1).active, true);
  settle(e);
  const end = log.imports.at(-1);
  assert.equal(end.active, false);
  assert.deepEqual(end.clips.map((c) => [c.name, c.track, c.converted, c.referenced]), [["Un", "Basse", false, true], ["Deux 44k", "Deux 44k", true, false]]);
  assert.ok(end.warnings.some((w) => /copié/.test(w)));
  const p = project();
  assert.equal(p.nodes.length, before + 1);
  assert.equal(p.nodes.find((n) => n.id === 2).clips.length, clipsOnBass + 1);
  const added = p.nodes.at(-1).clips[0];
  assert.equal(added.start, 12345);
  assert.equal(added.len, 4 * 48000);
  assert.equal(log.history.at(-1).undo_label, "Importer de l'audio");
  e.send({ type: "undo" });
  assert.equal(project().nodes.length, before);
  assert.equal(project().nodes.find((n) => n.id === 2).clips.length, clipsOnBass);
});

test("faux moteur : multicanal en pistes séparées, annulation, et pas de nouveau projet pendant l'import", () => {
  const { e, log, project } = setup();
  e.send({ type: "save_project", path: "/p/C" });
  const before = project().nodes.length;
  e.send({ type: "import_audio", files: ["/s/Film 5.1.wav"], multichannel: "split" });
  settle(e);
  assert.equal(project().nodes.length, before + 6);
  e.send({ type: "undo" });
  assert.equal(project().nodes.length, before);
  e.send({ type: "import_audio", files: ["/s/a.wav", "/s/b.wav"] });
  e.send({ type: "import_audio", files: ["/s/c.wav"] });
  assert.match(log.errors.pop().message, /déjà/);
  e.send({ type: "new_project" });
  assert.match(log.errors.pop().message, /import/);
  e.send({ type: "cancel_import" });
  e.advance(30);
  assert.equal(log.imports.at(-1).cancelled, true);
  assert.equal(project().nodes.length, before, "rien n'est ajouté");
  e.send({ type: "new_project" });
  assert.equal(log.errors.length, 0);
});
