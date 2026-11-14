import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand as validate } from "../js/protocol.js";

function engine() {
  const e = new MockEngine();
  const events = [];
  for (const t of ["recent", "gathered", "error", "file"]) e.on(t, (m) => events.push({ type: t, ...m }));
  return { e, events, last: (t) => [...events].reverse().find((x) => x.type === t) };
}

test("les commandes récentes / rassembler sont valides", () => {
  for (const type of ["get_recent", "clear_recent", "gather_files"]) assert.equal(validate({ type }), null);
});

test("les préférences de démarrage sont validées", () => {
  const prefs = (startup) => ({ type: "set_prefs", prefs: { version: 1, driver: "UMC", buffer: 64, threads: 1, ahead_ms: 0, ahead_threads: 2, metronome: { volume: 0.8, count_in_bars: 0, pre_roll: 0, post_roll: 0 }, startup } });
  assert.equal(validate(prefs({ reopen_last: true })), null);
  assert.equal(validate(prefs(undefined)), null);
  assert.match(validate(prefs({ reopen_last: "oui" })) ?? "", /démarrage/);
});

test("faux moteur : enregistrer ou ouvrir met le projet en tête des récents, sans doublon", () => {
  const { e, last } = engine();
  e.send({ type: "save_project", path: "/p/Un" });
  e.send({ type: "save_project", path: "/p/Deux" });
  assert.deepEqual(last("recent").projects.map((p) => p.name), ["Deux", "Un"]);
  e.send({ type: "open_project", path: "/p/Un.daw" });
  assert.deepEqual(last("recent").projects.map((p) => p.name), ["Un", "Deux"]);
  assert.ok(last("recent").projects.every((p) => p.exists));
  e.send({ type: "clear_recent" });
  assert.deepEqual(last("recent").projects, []);
});

test("faux moteur : rassembler demande un projet enregistré", () => {
  const { e, events, last } = engine();
  e.send({ type: "gather_files" });
  assert.match(last("error").message, /Enregistrer sous/);
  e.send({ type: "save_project", path: "/p/Un" });
  e.send({ type: "gather_files" });
  assert.deepEqual(last("gathered"), { type: "gathered", copied: [], warnings: [] });
  assert.ok(events.length > 0);
});

test("le menu Fichier a une zone des projets récents", async () => {
  const { readFileSync } = await import("node:fs");
  const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");
  assert.match(html, /id="fm-recent"[^>]*role="group"/);
  assert.match(html, /id="st-su-reopen"/);
});
