// Télécommande d'une tablette : validation des commandes de rôle et d'arrêt automatique, faux moteur (rôle d'un retour), réglages.
import test from "node:test";
import assert from "node:assert/strict";

import { validateCommand, validatePrefs } from "../js/protocol.js";
import { prefsFromForm } from "../js/ui/settings-dialog.js";
import { MockEngine } from "../js/mock-engine.js";

test("set_monitor_role : un retour et des droits de 0 à 3 (bits : télécommande, régie)", () => {
  assert.equal(validateCommand({ type: "set_monitor_role", node: 7, role: 1 }), null);
  assert.equal(validateCommand({ type: "set_monitor_role", node: 7, role: 0 }), null);
  for (const role of [2, 3]) assert.equal(validateCommand({ type: "set_monitor_role", node: 7, role }), null, `droits combinés ${role}`);
  for (const bad of [{ node: 7, role: 4 }, { node: 7, role: -1 }, { node: 7, role: "1" }, { node: 0, role: 1 }, { node: 7 }, { role: 1 }]) {
    assert.match(validateCommand({ type: "set_monitor_role", ...bad }) ?? "", /set_monitor_role/, JSON.stringify(bad));
  }
});

const basePrefs = { version: 1, driver: null, buffer: null, threads: 1, ahead_ms: 0, ahead_threads: 2, metronome: { enabled: false, only_recording: false, volume: 0.8, count_in_bars: 0, pre_roll: 0, post_roll: 0 } };

test("l'arrêt automatique se règle de 10 à 3 600 secondes, et reste facultatif", () => {
  assert.equal(validatePrefs(basePrefs), null);
  assert.equal(validatePrefs({ ...basePrefs, remote: { elevated_hours: 0 } }), null);
  for (const bad of [{ elevated_hours: 169 }, { elevated_hours: -1 }, { elevated_hours: 1.5 }, null]) assert.match(validatePrefs({ ...basePrefs, remote: bad }) ?? "", /télécommande/, JSON.stringify(bad));
  assert.equal(validateCommand({ type: "get_monitor_log" }), null);
  assert.equal(validatePrefs({ ...basePrefs, transport: { play_pre_roll: true, return_on_stop: false } }), null);
  assert.match(validatePrefs({ ...basePrefs, transport: { play_pre_roll: 1, return_on_stop: true } }) ?? "", /transport/);
  assert.equal(validatePrefs({ ...basePrefs, auto_stop: { enabled: true, after_end_secs: 120, recording_secs: 300 } }), null);
  for (const bad of [{ enabled: true, after_end_secs: 5, recording_secs: 300 }, { enabled: true, after_end_secs: 120, recording_secs: 4000 }, { enabled: "oui", after_end_secs: 120, recording_secs: 300 }, { enabled: true, after_end_secs: 12.5, recording_secs: 300 }, null]) {
    assert.match(validatePrefs({ ...basePrefs, auto_stop: bad }) ?? "", /arrêt automatique/, JSON.stringify(bad));
  }
});

test("la fenêtre des réglages renvoie l'arrêt automatique avec les autres réglages", () => {
  const f = { driver: "UMC", buffer: 64, threads: 1, ahead_ms: 0, ahead_threads: 2, auto_stop: { enabled: true, after_end_secs: 90, recording_secs: 240 } };
  const p = prefsFromForm(f, {}, {});
  assert.deepEqual(p.auto_stop, { enabled: true, after_end_secs: 90, recording_secs: 240 });
  assert.equal(validatePrefs(p), null);
});

test("le faux moteur donne un rôle à un retour, le refuse à une piste, et l'annonce dans l'événement monitor", () => {
  const e = new MockEngine();
  let monitor;
  let error;
  let project;
  e.on("monitor", (m) => (monitor = m));
  e.on("error", (x) => (error = x));
  e.on("project", (p) => (project = p));
  e.send({ type: "get_project" });
  e.send({ type: "add_track", kind: "bus", name: "Retour Marie" });
  const bus = project.nodes.find((n) => n.name === "Retour Marie").id;
  e.send({ type: "set_return_output", node: bus, output: { first: 3, stereo: true, mono: false } });
  e.send({ type: "start_monitor" });
  assert.equal(monitor.returns[0].role, 0, "simple musicien par défaut");
  e.send({ type: "set_monitor_role", node: bus, role: 1 });
  assert.equal(monitor.returns[0].role, 1);
  e.send({ type: "set_monitor_role", node: project.nodes.find((n) => n.kind === "stereo").id, role: 1 });
  assert.match(error.message, /retour de monitoring/);
  e.send({ type: "set_monitor_role", node: bus, role: 0 });
  assert.equal(monitor.returns[0].role, 0, "révoqué");
});
