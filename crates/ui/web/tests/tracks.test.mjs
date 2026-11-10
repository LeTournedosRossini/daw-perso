import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";

function setup() {
  const e = new MockEngine();
  const projects = [];
  const errors = [];
  const histories = [];
  e.on("project", (p) => projects.push(p));
  e.on("error", (x) => errors.push(x));
  e.on("history", (h) => histories.push(h));
  e.send({ type: "get_project" });
  return { e, project: () => projects.at(-1), errors, histories };
}

const names = (p) => p.nodes.map((n) => n.name);

test("validation des commandes de pistes", () => {
  assert.equal(validateCommand({ type: "add_track", kind: "mono" }), null);
  assert.equal(validateCommand({ type: "add_track", kind: "bus", name: "Réverbe" }), null);
  assert.notEqual(validateCommand({ type: "add_track", kind: "guitare" }), null);
  assert.notEqual(validateCommand({ type: "add_track", kind: "mono", name: "   " }), null);
  assert.notEqual(validateCommand({ type: "add_track", kind: "mono", name: "x".repeat(61) }), null);
  assert.equal(validateCommand({ type: "delete_track", node: 2 }), null);
  assert.notEqual(validateCommand({ type: "delete_track", node: 0 }), null);
  assert.equal(validateCommand({ type: "rename_track", node: 2, name: "Basse" }), null);
  assert.notEqual(validateCommand({ type: "rename_track", node: 0, name: "x" }), null);
  assert.notEqual(validateCommand({ type: "rename_track", node: 2, name: "" }), null);
  assert.equal(validateCommand({ type: "set_track_color", node: 2, color: "#aabbcc" }), null);
  assert.notEqual(validateCommand({ type: "set_track_color", node: 2, color: "rouge" }), null);
  assert.notEqual(validateCommand({ type: "set_track_color", node: 2, color: "#12345" }), null);
  assert.equal(validateCommand({ type: "set_track_output", node: 1, output: 0 }), null);
  assert.notEqual(validateCommand({ type: "set_track_output", node: 0, output: 1 }), null);
  assert.equal(validateCommand({ type: "move_track", node: 5 }), null);
  assert.equal(validateCommand({ type: "move_track", node: 5, before: 1 }), null);
  assert.notEqual(validateCommand({ type: "move_track", node: 5, before: "a" }), null);
});

test("ajouter une piste ou un bus : nom libre, sortie principale, un pas d'annulation", () => {
  const { e, project, histories } = setup();
  const before = names(project());
  e.send({ type: "add_track", kind: "mono" });
  e.send({ type: "add_track", kind: "bus", name: "  Réverbe " });
  const p = project();
  const t = p.nodes.find((n) => n.name === "Piste 1");
  assert.deepEqual([t.kind, t.output, t.clips.length], ["mono", 0, 0]);
  assert.equal(p.nodes.at(-1).name, "Réverbe");
  assert.equal(histories.at(-1).undo_label, "Ajouter un bus");
  e.send({ type: "undo" });
  e.send({ type: "undo" });
  assert.deepEqual(names(project()), before);
});

test("supprimer : les pistes qui y allaient repartent vers la sortie principale ; annuler remet tout, réglages d'aujourd'hui conservés", () => {
  const { e, project } = setup();
  e.send({ type: "add_track", kind: "bus", name: "Bus A" });
  const bus = project().nodes.find((n) => n.name === "Bus A").id;
  e.send({ type: "set_track_output", node: 1, output: bus });
  assert.equal(project().nodes.find((n) => n.id === 1).output, bus);
  e.send({ type: "delete_track", node: bus });
  assert.equal(project().nodes.find((n) => n.id === 1).output, 0);
  assert.equal(project().nodes.some((n) => n.name === "Bus A"), false);
  // Réglage de mixage fait après la suppression : il survit à l'annulation.
  e.send({ type: "set_node", node: 2, gain: 0.31 });
  e.send({ type: "undo" });
  assert.equal(project().nodes.find((n) => n.id === 1).output, bus, "rebranchée");
  assert.equal(project().nodes.find((n) => n.id === 2).gain, 0.31);
  assert.equal(project().nodes.some((n) => n.name === "Bus A"), true);
});

test("sortie : une piste n'est pas une destination, pas de boucle", () => {
  const { e, project, errors } = setup();
  e.send({ type: "add_track", kind: "bus", name: "A" });
  e.send({ type: "add_track", kind: "bus", name: "B" });
  const id = (name) => project().nodes.find((n) => n.name === name).id;
  assert.equal(e.send({ type: "set_track_output", node: 1, output: 2 }), false, "une piste");
  assert.match(errors.at(-1).message, /sortie refusée/);
  assert.equal(e.send({ type: "set_track_output", node: id("A"), output: id("A") }), false);
  assert.equal(e.send({ type: "set_track_output", node: id("B"), output: id("A") }), true);
  assert.equal(e.send({ type: "set_track_output", node: id("A"), output: id("B") }), false, "boucle A → B → A");
  assert.match(errors.at(-1).message, /boucle/);
});

test("renommer, colorer, réordonner : annulables", () => {
  const { e, project } = setup();
  e.send({ type: "rename_track", node: 2, name: " Basse électrique " });
  e.send({ type: "set_track_color", node: 2, color: "#AABBCC" });
  e.send({ type: "move_track", node: 5, before: 1 });
  assert.deepEqual(names(project()), ["Sortie", "Voix", "Batterie", "Basse électrique", "Claviers", "Guitare"]);
  assert.equal(project().nodes.find((n) => n.id === 2).color, "#aabbcc");
  e.send({ type: "move_track", node: 5 });
  assert.equal(names(project()).at(-1), "Voix");
  for (let i = 0; i < 4; i++) e.send({ type: "undo" });
  assert.deepEqual(names(project()), ["Sortie", "Batterie", "Basse", "Claviers", "Guitare", "Voix"]);
  assert.equal(project().nodes.find((n) => n.id === 2).color, "#f5a524");
  assert.equal(e.send({ type: "move_track", node: 2, before: 99 }), false);
});

test("les modifications de structure comptent comme non enregistrées", () => {
  const e = new MockEngine();
  const files = [];
  e.on("file", (f) => files.push(f));
  e.send({ type: "get_project" });
  assert.equal(files.at(-1).dirty, false);
  e.send({ type: "add_track", kind: "stereo" });
  assert.equal(files.at(-1).dirty, true);
});

test("plugins : inventaire, ajout, ordre, retrait, contournement, réglages (faux moteur)", () => {
  const e = new MockEngine();
  const projects = [];
  const errors = [];
  const lists = [];
  const params = [];
  e.on("project", (p) => projects.push(p));
  e.on("error", (x) => errors.push(x));
  e.on("plugins", (x) => lists.push(x));
  e.on("plugin_params", (x) => params.push(x));
  e.send({ type: "get_project" });
  const chain = (id = 3) => projects.at(-1).nodes.find((n) => n.id === id).inserts;
  e.send({ type: "list_plugins" });
  assert.deepEqual([lists[0].scanning, lists[0].plugins.length], [true, 0]);
  e.send({ type: "add_plugin", node: 3, clap_id: "mock.analogsynth", file: "AnalogSynth.clap" });
  e.send({ type: "add_plugin", node: 3, clap_id: "mock.compresseur" });
  e.send({ type: "add_plugin", node: 3, clap_id: "mock.digue" });
  const [a, b, c] = chain().map((i) => i.id);
  assert.deepEqual(chain().map((i) => i.name), ["AnalogSynth", "Compresseur", "Digue"]);
  assert.equal(chain()[2].latency, 480);
  e.send({ type: "move_plugin", insert: c, before: a });
  assert.deepEqual(chain().map((i) => i.id), [c, a, b]);
  e.send({ type: "set_plugin_bypass", insert: a, bypass: true });
  assert.equal(chain().find((i) => i.id === a).bypass, true);
  // Réglage puis retrait puis annulation : le réglage revient.
  e.send({ type: "get_plugin_params", insert: b });
  assert.equal(params.at(-1).params[0].name, "Seuil");
  e.send({ type: "set_plugin_param", insert: b, param: 0, value: -30 });
  e.send({ type: "remove_plugin", insert: b });
  assert.deepEqual(chain().map((i) => i.id), [c, a]);
  e.send({ type: "undo" });
  assert.deepEqual(chain().map((i) => i.id), [c, a, b]);
  e.send({ type: "get_plugin_params", insert: b });
  assert.equal(params.at(-1).params[0].value, -30);
  // Refus.
  assert.equal(e.send({ type: "add_plugin", node: 3, clap_id: "inconnu" }), false);
  assert.match(errors.at(-1).message, /introuvable/);
  assert.equal(e.send({ type: "remove_plugin", insert: 999 }), false);
  assert.equal(e.send({ type: "set_plugin_bypass", insert: 999, bypass: true }), false);
  assert.equal(e.send({ type: "get_plugin_params", insert: 999 }), false);
});

test("plugins : validation des commandes", () => {
  assert.equal(validateCommand({ type: "add_plugin", node: 2, clap_id: "x" }), null);
  assert.notEqual(validateCommand({ type: "add_plugin", node: 2, clap_id: " " }), null);
  assert.notEqual(validateCommand({ type: "add_plugin", clap_id: "x" }), null);
  assert.equal(validateCommand({ type: "move_plugin", insert: 1 }), null);
  assert.notEqual(validateCommand({ type: "move_plugin", insert: 1, before: "a" }), null);
  assert.equal(validateCommand({ type: "set_plugin_bypass", insert: 1, bypass: false }), null);
  assert.notEqual(validateCommand({ type: "set_plugin_bypass", insert: 1, bypass: "oui" }), null);
  assert.equal(validateCommand({ type: "set_plugin_param", insert: 1, param: 0, value: 0.5 }), null);
  assert.notEqual(validateCommand({ type: "set_plugin_param", insert: 1, param: 0, value: NaN }), null);
  assert.equal(validateCommand({ type: "list_plugins" }), null);
  assert.equal(validateCommand({ type: "get_plugin_params", insert: 2 }), null);
});

test("polarité : validation, réglage hors historique, marque le projet comme modifié", () => {
  assert.equal(validateCommand({ type: "set_node", node: 1, invert: true }), null);
  assert.notEqual(validateCommand({ type: "set_node", node: 1, invert: "oui" }), null);
  const e = new MockEngine();
  const projects = [];
  const files = [];
  const histories = [];
  e.on("project", (p) => projects.push(p));
  e.on("file", (f) => files.push(f));
  e.on("history", (h) => histories.push(h));
  e.send({ type: "get_project" });
  const node = (id) => projects.at(-1).nodes.find((n) => n.id === id);
  assert.equal(node(1).invert, false);
  e.send({ type: "save_project", path: "/tmp/Pol" });
  e.send({ type: "set_node", node: 1, invert: true });
  assert.deepEqual([node(1).invert, node(2).invert, files.at(-1).dirty], [true, false, true]);
  assert.equal(histories.at(-1)?.undo_label ?? null, null, "hors historique");
});

test("tempo et chiffrage : validation et faux moteur (carte des tempos, mesure de départ)", () => {
  assert.equal(validateCommand({ type: "set_tempo", bpm: 90.5, at_bar: 1 }), null);
  assert.notEqual(validateCommand({ type: "set_tempo", bpm: 10, at_bar: 1 }), null);
  assert.notEqual(validateCommand({ type: "set_tempo", bpm: 120, at_bar: 0 }), null);
  assert.notEqual(validateCommand({ type: "set_tempo", bpm: "vite", at_bar: 1 }), null);
  assert.equal(validateCommand({ type: "set_meter", num: 7, den: 8, at_bar: 3 }), null);
  assert.notEqual(validateCommand({ type: "set_meter", num: 4, den: 6, at_bar: 1 }), null);
  assert.notEqual(validateCommand({ type: "set_meter", num: 0, den: 4, at_bar: 1 }), null);
  const e = new MockEngine();
  const projects = [];
  e.on("project", (p) => projects.push(p));
  e.send({ type: "get_project" });
  e.send({ type: "set_tempo", bpm: 90, at_bar: 1 });
  assert.deepEqual([projects.at(-1).tempo_map[0].num, projects.at(-1).tempo_map[0].den], [90, 1]);
  e.send({ type: "set_tempo", bpm: 133.333, at_bar: 5 });
  assert.deepEqual(projects.at(-1).tempo_map.map((t) => [t.pos, t.num, t.den]), [[[0, 1], 90, 1], [[4, 1], 133333, 1000]]);
  e.send({ type: "set_meter", num: 7, den: 8, at_bar: 3 });
  assert.deepEqual(projects.at(-1).meters.map((m) => [m.bar, m.num, m.den]), [[0, 4, 4], [2, 7, 8]]);
});

test("set_metronome : réglages validés", () => {
  const ok = { type: "set_metronome", enabled: true, only_recording: false, volume: 0.8, count_in_bars: 1, pre_roll: 2, post_roll: 0 };
  assert.equal(validateCommand(ok), null);
  for (const bad of [{ volume: 2 }, { count_in_bars: 3 }, { pre_roll: 11 }, { post_roll: -1 }, { enabled: "oui" }]) {
    assert.notEqual(validateCommand({ ...ok, ...bad }), null, JSON.stringify(bad));
  }
});

test("set_metronome : renvoyer l'événement reçu (qui porte son propre type) reste une commande valide", () => {
  const e = new MockEngine();
  const seen = [];
  e.on("metronome", (x) => seen.push(x));
  e.send({ type: "get_project" });
  const back = { ...seen.at(-1), type: "set_metronome", enabled: true };
  assert.equal(validateCommand(back), null);
  e.send(back);
  assert.equal(seen.at(-1).enabled, true);
});

test("marqueurs : tempo et chiffrage d'une même mesure réunis, tempo en milieu de mesure à part", async () => {
  const { buildMarkers, markerLabel } = await import("../js/time.js");
  const project = {
    sample_rate: 48000,
    tempo_map: [{ pos: [0, 1], num: 120, den: 1 }, { pos: [1, 1], num: 90, den: 1 }, { pos: [9, 8], num: 100, den: 1 }],
    meters: [{ bar: 0, num: 4, den: 4 }, { bar: 1, num: 3, den: 4 }],
  };
  const list = buildMarkers(project);
  assert.deepEqual(list.map(markerLabel), ["♩ = 120 · 4/4", "♩ = 90 · 3/4", "♩ = 100"]);
  assert.deepEqual(list.map((m) => [m.bar, m.midBar]), [[1, false], [2, false], [2, true]]);
  assert.deepEqual(list[1].tempo.pos, [1, 1]);
});

test("marqueurs : suppression d'un tempo et d'un chiffrage (faux moteur) et validation", () => {
  const e = new MockEngine();
  const projects = [];
  const errors = [];
  e.on("project", (p) => projects.push(p));
  e.on("error", (x) => errors.push(x));
  e.send({ type: "get_project" });
  e.send({ type: "set_tempo", bpm: 90, at_bar: 5 });
  e.send({ type: "set_meter", num: 3, den: 4, at_bar: 5 });
  const p = projects.at(-1);
  assert.equal(p.tempo_map.length, 2);
  e.send({ type: "remove_tempo", pos: p.tempo_map[1].pos });
  e.send({ type: "remove_meter", at_bar: 5 });
  assert.deepEqual([projects.at(-1).tempo_map.length, projects.at(-1).meters.length], [1, 1]);
  e.send({ type: "remove_meter", at_bar: 5 });
  assert.equal(errors.length, 1);
  assert.notEqual(validateCommand({ type: "remove_meter", at_bar: 1 }), null);
  assert.notEqual(validateCommand({ type: "remove_tempo", pos: [1, 0] }), null);
});

test("piano roll : notes en temps musical, modifications groupées en un pas d'annulation (faux moteur)", () => {
  const e = new MockEngine();
  const notes = [];
  const errors = [];
  const hist = [];
  const projects = [];
  e.on("notes", (x) => notes.push(x));
  e.on("error", (x) => errors.push(x));
  e.on("history", (x) => hist.push(x));
  e.on("project", (x) => projects.push(x));
  e.send({ type: "get_project" });
  const clip = projects.at(-1).nodes.flatMap((n) => n.clips).find((c) => c.kind === "midi");
  e.send({ type: "get_notes", clip: clip.id });
  const first = notes.at(-1);
  assert.ok(first.notes.length > 8);
  assert.deepEqual(first.notes[0].start.length, 2);
  const n = first.notes.length;
  e.send({ type: "edit_notes", clip: clip.id, label: "Déplacer des notes", edits: [{ index: 0, key: first.notes[0].key + 1 }, { index: 1, key: first.notes[1].key + 1 }] });
  assert.equal(notes.at(-1).selection.length, 2);
  assert.equal(hist.at(-1).undo_label, "Déplacer des notes");
  e.send({ type: "add_notes", clip: clip.id, notes: [{ start: [1, 8], len: [1, 16], key: 70, velocity: 0.6 }] });
  assert.equal(notes.at(-1).notes.length, n + 1);
  e.send({ type: "delete_notes", clip: clip.id, indices: [0, 1] });
  assert.equal(notes.at(-1).notes.length, n - 1);
  e.send({ type: "undo" });
  e.send({ type: "undo" });
  e.send({ type: "get_notes", clip: clip.id });
  assert.equal(notes.at(-1).notes.length, n);
  assert.equal(notes.at(-1).notes[0].key, first.notes[1].key === notes.at(-1).notes[0].key ? first.notes[1].key : first.notes[0].key + 1);
  // Refus : indice inexistant, début hors du clip, clip audio.
  e.send({ type: "edit_notes", clip: clip.id, edits: [{ index: 9999, key: 60 }] });
  e.send({ type: "add_notes", clip: clip.id, notes: [{ start: [900, 1], len: [1, 8], key: 60, velocity: 0.5 }] });
  e.send({ type: "get_notes", clip: 1 });
  assert.equal(errors.length, 3);
  assert.notEqual(validateCommand({ type: "preview_note", node: 3, key: 200, velocity: 0.5, on: true }), null);
});

test("envois (faux moteur) : plusieurs destinations dont la sortie principale, niveau, avant fader, boucles refusées", async () => {
  const { sendTargets, outputTargets, structureKey, defaultMode } = await import("../js/ui/mixer.js");
  const e = new MockEngine();
  const projects = [];
  const errors = [];
  const hist = [];
  e.on("project", (p) => projects.push(p));
  e.on("error", (x) => errors.push(x));
  e.on("history", (x) => hist.push(x));
  e.send({ type: "get_project" });
  e.send({ type: "add_track", kind: "bus" });
  e.send({ type: "add_track", kind: "bus" });
  const nodes = () => projects.at(-1).nodes;
  const [b1, b2] = nodes().filter((n) => n.kind === "bus").map((n) => n.id);
  const batt = nodes().find((n) => n.name === "Batterie").id;
  e.send({ type: "add_send", node: batt, to: b1 });
  e.send({ type: "add_send", node: batt, to: 0 });
  e.send({ type: "add_send", node: batt, to: b2 });
  const sends = () => nodes().find((n) => n.id === batt).sends;
  assert.deepEqual(sends().map((s) => [s.to, s.gain, s.pre]), [[b1, 1, false], [0, 1, false], [b2, 1, false]]);
  assert.equal(hist.at(-1).undo_label, "Ajouter un envoi");
  e.send({ type: "set_send", node: batt, to: 0, gain: 0.4 });
  e.send({ type: "set_send", node: batt, to: b1, pre: true });
  assert.deepEqual(sends().map((s) => [s.to, s.gain, s.pre]), [[b1, 1, true], [0, 0.4, false], [b2, 1, false]]);
  // Refus : doublon, boucle (Bus 1 → Bus 2 est permis ; Bus 2 → Bus 1 fermerait la boucle), vers une piste, pas d'envoi à régler.
  e.send({ type: "add_send", node: batt, to: b1 });
  e.send({ type: "add_send", node: b1, to: b2 });
  e.send({ type: "add_send", node: b2, to: b1 });
  e.send({ type: "add_send", node: batt, to: nodes().find((n) => n.name === "Basse").id });
  e.send({ type: "set_send", node: batt, to: 99, gain: 1 });
  assert.equal(errors.length, 4);
  e.send({ type: "remove_send", node: batt, to: 0 });
  assert.equal(sends().length, 2);
  e.send({ type: "undo" });
  assert.equal(sends().length, 3);
  assert.notEqual(validateCommand({ type: "set_send", node: batt, to: 0 }), null);
  assert.notEqual(validateCommand({ type: "add_send", node: 0, to: 1 }), null);
  // Aides du mixeur : destinations possibles et signature de structure.
  const p = projects.at(-1);
  const bat = p.nodes.find((n) => n.id === batt);
  assert.deepEqual(sendTargets(p, bat).map((n) => n.kind), [], "toutes les destinations sont déjà prises");
  assert.deepEqual(outputTargets(p, bat).map((n) => n.id).sort(), [0, b1, b2].sort());
  assert.notEqual(structureKey(p), structureKey({ ...p, nodes: p.nodes.map((n) => (n.id === batt ? { ...n, sends: [] } : n)) }));
  assert.equal(defaultMode(1920, 1080), "dock");
  assert.equal(defaultMode(1366, 640), "full");
});

test("feuille de réglage fin : glissement relatif (pas de saut), mode fin, conversions dB ↔ gain", async () => {
  const { dragDb, dbToLinear, linearToDb } = await import("../js/ui/level-sheet.js");
  // Largeur 660 px = 66 dB : 10 px = 1 dB ; le point de départ ne compte pas (relatif).
  assert.equal(dragDb(-12, 0, 660), -12);
  assert.equal(dragDb(-12, 60, 660), -6);
  assert.equal(dragDb(-12, 10, 660, true), -11.8, "mode fin : cinq fois moins");
  assert.equal(dragDb(0, 10000, 660), 6, "borné à +6 dB");
  assert.equal(dragDb(0, -10000, 660), -60, "borné à −60 dB (= coupé)");
  assert.equal(dbToLinear(-60), 0);
  assert.ok(Math.abs(dbToLinear(0) - 1) < 1e-9);
  assert.ok(Math.abs(linearToDb(dbToLinear(-6.5)) + 6.5) < 1e-9);
  assert.equal(linearToDb(0), -60);
});

test("automation (faux moteur) : courbes, mode, cible disparue, refus et annulation", () => {
  const e = new MockEngine();
  const projects = [];
  const errors = [];
  const hist = [];
  e.on("project", (p) => projects.push(p));
  e.on("error", (x) => errors.push(x));
  e.on("history", (x) => hist.push(x));
  e.send({ type: "get_project" });
  const lanes = (id = 1) => projects.at(-1).nodes.find((n) => n.id === id).automation;
  const pts = [{ pos: [0, 1], value: 0.5, curve: "linear" }, { pos: [1, 1], value: 1, curve: "step" }];
  e.send({ type: "set_lane", node: 1, target: { kind: "gain" }, points: pts, label: "Ajouter un point" });
  assert.equal(lanes().length, 1);
  assert.equal(hist.at(-1).undo_label, "Ajouter un point");
  e.send({ type: "set_lane_mode", node: 1, target: { kind: "gain" }, mode: "trim" });
  assert.equal(lanes()[0].mode, "trim");
  e.send({ type: "set_lane", node: 1, target: { kind: "gain" }, points: [{ pos: [0, 1], value: 0.8, curve: "linear" }] });
  assert.equal(lanes()[0].mode, "trim", "le mode de la courbe existante est gardé");
  // Un envoi et sa courbe : retirer l'envoi masque la courbe, annuler la rend.
  e.send({ type: "add_track", kind: "bus" });
  const bus = projects.at(-1).nodes.find((n) => n.kind === "bus").id;
  e.send({ type: "add_send", node: 1, to: bus });
  e.send({ type: "set_lane", node: 1, target: { kind: "send", to: bus }, points: pts });
  assert.equal(lanes().length, 2);
  e.send({ type: "remove_send", node: 1, to: bus });
  assert.equal(lanes().length, 1);
  e.send({ type: "undo" });
  assert.equal(lanes().length, 2);
  // Refus : cible inexistante, mode inconnu, positions non croissantes, gain négatif.
  e.send({ type: "set_lane", node: 1, target: { kind: "send", to: 77 }, points: pts });
  e.send({ type: "set_lane_mode", node: 1, target: { kind: "pan" }, mode: "trim" });
  assert.equal(errors.length, 2);
  assert.notEqual(validateCommand({ type: "set_lane", node: 1, target: { kind: "gain" }, mode: "write", points: pts }), null);
  assert.notEqual(validateCommand({ type: "set_lane", node: 1, target: { kind: "gain" }, points: [pts[1], pts[0]] }), null);
  assert.notEqual(validateCommand({ type: "set_lane", node: 1, target: { kind: "gain" }, points: [{ pos: [0, 1], value: -1, curve: "linear" }] }), null);
  assert.equal(validateCommand({ type: "set_lane", node: 1, target: { kind: "gain" }, points: [] }), null);
  // Effacer.
  e.send({ type: "set_lane", node: 1, target: { kind: "gain" }, points: [] });
  assert.equal(lanes().length, 1);
});

test("écriture d'automation : armement, mode affiché, validation, indicateur de geste", async () => {
  const { LANE_MODES, isWriteMode, trackGesture } = await import("../js/ui/gesture.js");
  assert.deepEqual(LANE_MODES.map((m) => m[0]), ["read", "trim", "off", "touch", "latch", "write"]);
  assert.ok(isWriteMode("touch") && isWriteMode("latch") && isWriteMode("write") && !isWriteMode("trim"));
  const e = new MockEngine();
  const projects = [];
  e.on("project", (p) => projects.push(p));
  e.send({ type: "get_project" });
  e.send({ type: "set_lane", node: 1, target: { kind: "gain" }, mode: "trim", points: [{ pos: [0, 1], value: 1, curve: "linear" }] });
  e.send({ type: "set_write_mode", node: 1, target: { kind: "gain" }, mode: "latch" });
  const p = projects.at(-1);
  assert.deepEqual(p.write_arms, [{ node: 1, target: { kind: "gain" }, mode: "latch" }]);
  assert.equal(p.nodes.find((n) => n.id === 1).automation[0].mode, "read", "armer met la courbe en lecture");
  e.send({ type: "set_write_mode", node: 1, target: { kind: "gain" }, mode: "none" });
  assert.deepEqual(projects.at(-1).write_arms, []);
  assert.notEqual(validateCommand({ type: "set_write_mode", node: 1, target: { kind: "gain" }, mode: "record" }), null);
  assert.equal(validateCommand({ type: "gesture", node: 1, target: { kind: "send", to: 0 }, active: true }), null);
  assert.notEqual(validateCommand({ type: "gesture", node: 1, target: { kind: "gain" } }), null);
  // trackGesture : prévient quand le réglage est pris, puis lâché (même hors de l'élément).
  const sent = [];
  const listeners = {};
  const el = { addEventListener: (t, f) => (listeners[t] = f) };
  const win = {};
  globalThis.window = { addEventListener: (t, f) => (win[t] = f), removeEventListener: () => {} };
  trackGesture(el, { send: (m) => sent.push(m) }, () => ({ node: 3, target: { kind: "pan" } }));
  listeners.pointerdown();
  listeners.pointerdown(); // un deuxième appui ne double pas le message
  win.pointerup();
  win.pointerup();
  assert.deepEqual(sent, [{ type: "gesture", node: 3, target: { kind: "pan" }, active: true }, { type: "gesture", node: 3, target: { kind: "pan" }, active: false }]);
  delete globalThis.window;
});

test("lignes de la timeline : voie d'automation et piano roll ancré (quatre lignes d'un tenant) sous le bon nœud", async () => {
  const { orderedRows } = await import("../js/ui/order.js");
  const project = { nodes: [{ id: 0, kind: "master", name: "Sortie" }, { id: 1, kind: "stereo", name: "A" }, { id: 2, kind: "mono", name: "B" }] };
  assert.deepEqual(orderedRows(project).map((r) => r.kind), ["stereo", "mono", "master"]);
  const rows = orderedRows(project, new Set([1]), 1, 4);
  assert.deepEqual(rows.map((r) => r.kind), ["stereo", "lane", "roll", "roll-cont", "roll-cont", "roll-cont", "mono", "master"]);
  assert.deepEqual(new Set(rows.map((r) => r.rowKey).filter(Boolean)).size, 5, "les clés de ligne des lignes ajoutées sont distinctes");
  assert.ok(rows.filter((r) => r.kind === "roll" || r.kind === "roll-cont").every((r) => r.clips.length === 0 && r.id === 1));
  assert.deepEqual(orderedRows(project, new Set(), 2, 2).map((r) => r.kind), ["stereo", "mono", "roll", "roll-cont", "master"]);
});

test("hauteur du piano roll ancré : bornée, arrondie aux lignes entières ; les lignes suivent", async () => {
  const { clampRows, ROLL_ROWS_MIN, ROLL_ROWS_MAX } = await import("../js/ui/piano-roll.js").catch(() => ({}));
  if (!clampRows) return; // le module touche au DOM : on ne le teste ici que s'il se charge (sinon couvert dans le navigateur)
  assert.equal(clampRows(0), ROLL_ROWS_MIN);
  assert.equal(clampRows(99), ROLL_ROWS_MAX);
  assert.equal(clampRows(5.4), 5);
  assert.equal(clampRows(NaN), 4);
  const { orderedRows } = await import("../js/ui/order.js");
  const project = { nodes: [{ id: 0, kind: "master", name: "S" }, { id: 1, kind: "stereo", name: "A" }] };
  assert.equal(orderedRows(project, new Set(), 1, 6).filter((r) => String(r.kind).startsWith("roll")).length, 6);
});
