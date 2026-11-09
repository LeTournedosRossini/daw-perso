import test from "node:test";
import assert from "node:assert/strict";
import { validateCommand } from "../js/protocol.js";
import { MockEngine } from "../js/mock-engine.js";
import { preBounds, preRollText, DEFAULT_METRONOME } from "../js/ui/metronome-menu.js";

test("set_punch : un booléen obligatoire", () => {
  assert.equal(validateCommand({ type: "set_punch", enabled: true }), null);
  assert.equal(validateCommand({ type: "set_punch", enabled: false }), null);
  for (const bad of [{}, { enabled: 1 }, { enabled: "oui" }, { start: 1, end: 2 }]) {
    assert.match(validateCommand({ type: "set_punch", ...bad }) ?? "", /set_punch/, JSON.stringify(bad));
  }
});

test("pré-roll : secondes ou mesures, bornes et texte", () => {
  assert.deepEqual(preBounds("seconds"), { min: 0, max: 10, step: 0.5 });
  assert.deepEqual(preBounds("bars"), { min: 0, max: 8, step: 1 });
  assert.equal(preRollText({ ...DEFAULT_METRONOME, pre_roll: 2 }), "2 s");
  assert.equal(preRollText({ ...DEFAULT_METRONOME, pre_roll_unit: "bars", pre_roll_bars: 1 }), "1 mesure");
  assert.equal(preRollText({ ...DEFAULT_METRONOME, pre_roll_unit: "bars", pre_roll_bars: 3 }), "3 mesures");
  assert.equal(preRollText({ ...DEFAULT_METRONOME, pre_roll_unit: "bars", pre_roll_bars: 0 }), null);
  assert.equal(preRollText(DEFAULT_METRONOME), null);
  const base = { type: "set_metronome", enabled: false, only_recording: false, volume: 0.8, count_in_bars: 0, pre_roll: 0, post_roll: 0 };
  assert.equal(validateCommand(base), null, "les anciens messages (sans mesures) restent valides");
  assert.equal(validateCommand({ ...base, pre_roll_bars: 8, pre_roll_unit: "bars" }), null);
  for (const bad of [{ pre_roll_bars: 9 }, { pre_roll_bars: -1 }, { pre_roll_bars: 1.5 }, { pre_roll_unit: "beats" }]) {
    assert.match(validateCommand({ ...base, ...bad }) ?? "", /set_metronome/, JSON.stringify(bad));
  }
});

function armed() {
  const e = new MockEngine();
  const events = [];
  for (const t of ["punch", "recording", "error", "file"]) e.on(t, (x) => events.push([t, x]));
  e.send({ type: "save_project", path: "/tmp/Chanson.daw" });
  const track = e.project.nodes.find((n) => n.kind !== "master" && n.kind !== "bus");
  e.send({ type: "set_node", node: track.id, arm: true });
  return { e, events, track };
}
const lastPunch = (events) => events.filter((x) => x[0] === "punch").at(-1)[1].enabled;
const lastError = (events) => events.filter((x) => x[0] === "error").at(-1)[1].message;

test("le punch s'allume, s'enregistre avec le projet, est éteint par un nouveau projet", () => {
  const { e, events } = armed();
  e.send({ type: "set_punch", enabled: true });
  assert.equal(lastPunch(events), true);
  e.send({ type: "save_project", path: "/tmp/Chanson.daw" });
  e.send({ type: "new_project" });
  assert.equal(lastPunch(events), false);
  e.send({ type: "open_project", path: "/tmp/Chanson.daw" });
  assert.equal(lastPunch(events), true);
});

test("sans punch une boucle empêche d'enregistrer ; avec punch il faut une zone", () => {
  const { e, events } = armed();
  e.send({ type: "loop", start: 48000, end: 96000 });
  e.send({ type: "record" });
  assert.match(lastError(events), /allume Punch/);
  e.send({ type: "loop", start: null, end: null });
  e.send({ type: "set_punch", enabled: true });
  e.send({ type: "record" });
  assert.match(lastError(events), /pose d'abord une zone/);
});

test("avec punch, record part du début de la zone bleue, finit seul à sa fin, la lecture continue et la boucle revient", () => {
  const { e, track } = armed();
  e.send({ type: "loop", start: 48000, end: 96000 });
  e.send({ type: "set_punch", enabled: true });
  e.send({ type: "record" });
  assert.equal(e.recording.start, 48000);
  assert.equal(e.pos, 48000);
  assert.equal(e.loop, null, "la boucle est suspendue pendant la prise");
  for (let i = 0; i < 400 && e.recording; i++) e.advance(100);
  assert.equal(e.recording, null, "terminée toute seule");
  assert.ok(e.playing, "la lecture continue");
  assert.deepEqual(e.loop, { start: 48000, end: 96000 }, "la zone est de nouveau bouclée");
  const clip = e.project.nodes.find((n) => n.id === track.id).clips.find((c) => c.name.includes("prise"));
  assert.deepEqual([clip.start, clip.len], [48000, 48000]);
});

test("en direct : après la zone refusé ; dans la zone commence tout de suite ; pendant une prise le punch ne change pas", () => {
  const { e, events } = armed();
  e.send({ type: "loop", start: 1000, end: 2000 });
  e.send({ type: "set_punch", enabled: true });
  e.send({ type: "transport", action: "play" });
  e.pos = 3000;
  e.send({ type: "record" });
  assert.match(lastError(events), /après la fin de la zone de punch/);
  e.pos = 1500;
  e.send({ type: "record" });
  assert.equal(e.recording.start, 1500);
  assert.equal(e.recording.end, 2000);
  e.send({ type: "set_punch", enabled: false });
  assert.match(lastError(events), /arrête d'abord l'enregistrement/);
});
