import test from "node:test";
import assert from "node:assert/strict";
import { validateCommand, validatePrefs } from "../js/protocol.js";
import { MockEngine } from "../js/mock-engine.js";
import { coreBudget, budgetText, matchDriver, bufferChoices, bufferMs, threadsWarning, prefsFromForm, runningText } from "../js/ui/settings-dialog.js";

const base = () => ({ version: 1, driver: null, buffer: null, threads: 1, ahead_ms: 0, ahead_threads: 2, metronome: { enabled: false, only_recording: false, volume: 0.8, count_in_bars: 0, pre_roll: 0, post_roll: 0, pre_roll_bars: 0, pre_roll_unit: "seconds" } });

test("les préférences valides sont acceptées, les autres refusées avec les bornes du moteur", () => {
  assert.equal(validatePrefs(base()), null);
  assert.equal(validatePrefs({ ...base(), driver: "ASIO4ALL v2", buffer: 32, threads: 3, ahead_ms: 50 }), null);
  for (const bad of [{ buffer: 4 }, { buffer: 4096 }, { buffer: 1.5 }, { threads: 0 }, { threads: 17 }, { ahead_ms: 5 }, { ahead_ms: 900 }, { ahead_threads: 0 }, { driver: " " }, { metronome: { ...base().metronome, volume: 2 } }, { metronome: { ...base().metronome, count_in_bars: 3 } }]) {
    assert.ok(validatePrefs({ ...base(), ...bad }), JSON.stringify(bad));
  }
  assert.equal(validateCommand({ type: "get_prefs" }), null);
  assert.equal(validateCommand({ type: "set_prefs", prefs: base() }), null);
  assert.match(validateCommand({ type: "set_prefs", prefs: { ...base(), threads: 0 } }), /set_prefs/);
});

test("tampons proposés, durée, avertissements et phrase d'état", () => {
  assert.deepEqual(bufferChoices(64), [16, 32, 64, 128, 256, 512]);
  assert.deepEqual(bufferChoices(48), [16, 32, 48, 64, 128, 256, 512]);
  assert.ok(Math.abs(bufferMs(64, 44100) - 1.451) < 0.001);
  assert.equal(threadsWarning(3, 0, 2, 4), null);
  assert.match(threadsWarning(4, 0, 2, 4), /[Aa]ucun cœur n.est libre/);
  const w = threadsWarning(3, 50, 2, 4);
  assert.match(w, /plus de fils \(5\) que de cœurs \(4\)/);
  assert.match(w, /laisse les fils de calcul à 1/);
  assert.match(threadsWarning(2, 50, 2, 4), /[Aa]ucun cœur n.est libre/);
  assert.equal(threadsWarning(1, 50, 2, 4), null);
  assert.match(runningText(null), /pas encore répondu/);
  const r = { effective: { driver: "UMC", buffer: 32, threads: 3, ahead_ms: 50, ahead_threads: 2, forced: [] }, sample_rate: 44100, inputs: 10, drivers: [], cores: 4, audio: true };
  assert.match(runningText(r), /UMC, 44100 Hz, tampon 32 \(0,73 ms\), 10 entrée\(s\), 3 fil\(s\) de calcul, avance 50 ms/);
  assert.match(runningText({ ...r, audio: false }), /sans son/);
});

test("les préférences issues du formulaire gardent le métronome courant sans le champ type", () => {
  const p = prefsFromForm({ driver: "UMC", buffer: 32, threads: 3, ahead_ms: 50, ahead_threads: 2 }, { type: "metronome", enabled: true, only_recording: true, volume: 1, count_in_bars: 1, pre_roll: 2, post_roll: 0 }, base());
  assert.equal(validatePrefs(p), null);
  assert.equal(p.metronome.type, undefined);
  assert.deepEqual(p.metronome, { enabled: true, only_recording: true, volume: 1, count_in_bars: 1, pre_roll: 2, post_roll: 0, pre_roll_bars: 0, pre_roll_unit: "seconds" });
});

test("le faux moteur donne les préférences, signale le redémarrage et reprend le métronome", () => {
  const e = new MockEngine();
  const got = [];
  e.on("prefs", (p) => got.push(p));
  const metro = [];
  e.on("metronome", (m) => metro.push(m));
  e.send({ type: "get_project" });
  assert.equal(got.at(-1).restart_needed, false);
  assert.equal(got.at(-1).running.effective.buffer, 64);
  e.send({ type: "set_prefs", prefs: { ...base(), buffer: 64, metronome: { ...base().metronome, enabled: true } } });
  assert.equal(got.at(-1).restart_needed, false, "mêmes valeurs que celles qui tournent");
  assert.equal(metro.at(-1).enabled, true);
  e.send({ type: "set_prefs", prefs: { ...base(), buffer: 32, ahead_ms: 50 } });
  assert.equal(got.at(-1).restart_needed, true);
  assert.equal(got.at(-1).prefs.buffer, 32);
  e.send({ type: "set_metronome", enabled: false, only_recording: true, volume: 0.5, count_in_bars: 1, pre_roll: 0, post_roll: 0 });
  e.send({ type: "get_prefs" });
  assert.equal(got.at(-1).prefs.metronome.only_recording, true);
  e.destroy?.();
});

test("un pilote se retrouve par un morceau de son nom", () => {
  const list = ["UMC ASIO Driver", "ASIO4ALL v2"];
  assert.equal(matchDriver("UMC", list), "UMC ASIO Driver");
  assert.equal(matchDriver("asio4all", list), "ASIO4ALL v2");
  assert.equal(matchDriver("UMC ASIO Driver", list), "UMC ASIO Driver");
  assert.equal(matchDriver("Focusrite", list), null);
  assert.equal(matchDriver("", list), null);
});

test("le budget de cœurs compte le fil audio, les aides du calcul et les fils de rendu", () => {
  assert.deepEqual(coreBudget(1, 0, 2, 4), { audio: 1, workers: 0, render: 0, total: 1, cores: 4, level: "ok" });
  assert.deepEqual(coreBudget(3, 0, 2, 4), { audio: 1, workers: 2, render: 0, total: 3, cores: 4, level: "ok" });
  assert.equal(coreBudget(1, 50, 2, 4).total, 3);
  assert.equal(coreBudget(1, 50, 2, 4).level, "ok");
  assert.equal(coreBudget(2, 50, 2, 4).level, "full");
  assert.equal(coreBudget(3, 50, 2, 4).level, "over");
  assert.equal(coreBudget(4, 0, 2, 4).level, "full");
  assert.equal(budgetText(coreBudget(1, 0, 2, 4)), "1 fil audio = 1 cœur occupé sur 4");
});
