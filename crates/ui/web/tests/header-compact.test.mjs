import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { audioInfo, khz } from "../js/audio-status.js";

test("fréquence : 44100 → « 44,1 kHz », 48000 → « 48 kHz »", () => {
  assert.equal(khz(44100), "44,1 kHz");
  assert.equal(khz(48000), "48 kHz");
  assert.equal(khz(96000), "96 kHz");
  assert.equal(khz(88200), "88,2 kHz");
});

test("audio du bandeau : fréquence et tampon, latences annoncées par le pilote (pas la compensation des plugins)", () => {
  const project = { sample_rate: 44100, latency_samples: 441 };
  const a = audioInfo({ driver: "UMC", sample_rate: 44100, buffer: 64, in_latency: 130, out_latency: 66 }, project);
  assert.equal(a.main, "44,1 kHz · 64 éch.");
  assert.equal(a.sub, "E 2,9 · S 1,5 ms");
  assert.match(a.title, /UMC — 44,1 kHz, tampon de 64 échantillons \(1,5 ms\)/);
  assert.match(a.title, /entrée 130 éch\. \(2,9 ms\), sortie 66 éch\. \(1,5 ms\) — aller-retour ≈ 4,4 ms/);
  assert.match(a.title, /Compensation de latence des plugins : 10,0 ms/, "la compensation passe dans l'infobulle, plus dans l'affichage");
  assert.ok(!a.main.includes("10,0") && !a.sub.includes("10,0"));
});

test("audio du bandeau : latences pas encore connues, tampon inconnu (JACK), pas de pilote, pas de compensation", () => {
  const project = { sample_rate: 48000, latency_samples: 0 };
  const early = audioInfo({ driver: "UMC", sample_rate: 48000, buffer: 128, in_latency: null, out_latency: null }, project);
  assert.equal(early.sub, "latence : —");
  assert.match(early.title, /pas encore connues/);
  const jack = audioInfo({ driver: "JACK", sample_rate: 48000, buffer: 0, in_latency: null, out_latency: null }, project);
  assert.equal(jack.main, "48 kHz");
  const none = audioInfo(undefined, project);
  assert.deepEqual({ main: none.main, sub: none.sub }, { main: "48 kHz", sub: "sans pilote" });
  assert.ok(!audioInfo({ driver: "X", sample_rate: 48000, buffer: 64, in_latency: 1, out_latency: 1 }, project).title.includes("Compensation"), "sans compensation, pas de ligne");
});

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const header = html.slice(html.indexOf('<header id="transport"'), html.indexOf("</header>"));
const recPanel = html.slice(html.indexOf('id="rec-options"'), html.indexOf('<div id="scroller">'));
const count = (id) => (html.match(new RegExp(`id="${id}"`, "g")) ?? []).length;

test("bandeau compact : plus de nom de projet, Enregistrer en icône avec point des modifications, Accords « CM7 », Partition « ♪ », Panique en icône", () => {
  assert.equal(count("project-name"), 0);
  assert.match(header, /id="btn-save"[^>]*aria-label="Enregistrer"[^>]*data-dirty="false"><svg/);
  assert.match(header, /id="btn-chords"[^>]*aria-label="Accords">CM7<\/button>/);
  assert.match(header, /id="btn-score"[^>]*aria-label="Partition">♪<\/button>/);
  assert.match(header, /id="btn-panic"[^>]*aria-label="Panique[^"]*"><svg/);
  for (const id of ["btn-save", "btn-chords", "btn-score", "btn-panic"]) assert.equal(count(id), 1, id);
});

test("mode d'enregistrement : un chevron à côté de ●, Punch et Boucle dans un panneau (identifiants conservés)", () => {
  assert.match(header, /id="btn-record"[\s\S]*id="btn-rec-opts"[^>]*aria-haspopup="true"[^>]*aria-expanded="false"/);
  for (const id of ["btn-punch", "btn-looprec"]) {
    assert.equal(count(id), 1, id);
    assert.ok(recPanel.includes(`id="${id}"`), `${id} dans le panneau`);
    assert.ok(!header.includes(`id="${id}"`), `${id} plus dans le bandeau`);
    assert.match(recPanel, new RegExp(`id="${id}"[^>]*aria-pressed="false"`));
  }
  assert.match(recPanel, /id="rec-options"[^>]*role="group"[^>]*hidden/);
});

test("audio : la zone « Latence » est devenue « Audio » avec deux petites lignes", () => {
  assert.match(header, /<dt>Audio<\/dt><dd id="disp-latency"[^>]*><span id="disp-audio-main">[^<]*<\/span><span id="disp-audio-sub">/);
  assert.ok(!header.includes("<dt>Latence</dt>"));
});

test("bandeau : balises <div> équilibrées", () => {
  assert.equal((header.match(/<div\b/g) ?? []).length, (header.match(/<\/div>/g) ?? []).length);
});
