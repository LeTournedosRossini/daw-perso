import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { tasksSummary } from "../js/tasks-model.js";

test("résumé des tâches : aucune, une seule (son texte), plusieurs (leur nombre et la liste en infobulle)", () => {
  assert.equal(tasksSummary([]), null);
  assert.equal(tasksSummary(undefined), null);
  assert.equal(tasksSummary(["", "  ", null]), null, "les textes vides ne comptent pas");
  const one = tasksSummary(["Import 2/5 · 40 %"]);
  assert.equal(one.count, 1);
  assert.equal(one.label, "Import 2/5 · 40 %");
  const many = tasksSummary(["Import 40 %", "Export 12 %", "Calage 3/4"]);
  assert.equal(many.count, 3);
  assert.equal(many.label, "3 tâches");
  assert.equal(many.title, "Import 40 %\nExport 12 %\nCalage 3/4");
});

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const header = html.slice(html.indexOf('<header id="transport"'), html.indexOf("</header>"));
const panel = html.slice(html.indexOf('id="tasks-panel"'), html.indexOf('<div id="scroller">'));
const count = (id) => (html.match(new RegExp(`id="${id}"`, "g")) ?? []).length;

test("tâches : les cinq boutons d'origine (identifiants uniques) sont dans le panneau, plus dans le bandeau ; la pastille est cachée au départ", () => {
  for (const id of ["import-status", "export-status", "align-status", "stretch-status", "convert-status"]) {
    assert.equal(count(id), 1, id);
    assert.ok(panel.includes(`id="${id}"`), `${id} dans le panneau`);
    assert.ok(!header.includes(`id="${id}"`), `${id} plus dans le bandeau`);
    assert.match(panel, new RegExp(`id="${id}"[^>]*hidden`), `${id} caché au départ`);
  }
  assert.equal(count("tasks-pill"), 1);
  assert.match(header, /id="tasks-pill"[^>]*aria-haspopup="true"[^>]*aria-expanded="false"[^>]*hidden/);
  assert.match(panel, /id="tasks-panel"[^>]*role="group"[^>]*hidden/);
});

test("tâches : l'écoute de contrôle et le script qui écoute restent visibles dans le bandeau ; les voyants d'enregistrement aussi", () => {
  for (const id of ["listen-status", "script-status", "rec-status", "write-status"]) assert.ok(header.includes(`id="${id}"`), `${id} reste dans le bandeau`);
});

test("tâches : chaque bouton dit ce qu'un appui fait (annuler ou afficher)", () => {
  for (const id of ["import-status", "align-status", "stretch-status", "convert-status"]) assert.match(panel, new RegExp(`id="${id}" class="task" data-action="cancel"`));
  assert.match(panel, /id="export-status" class="task" data-action="open"/);
});

test("bandeau : balises <div> équilibrées", () => {
  assert.equal((header.match(/<div\b/g) ?? []).length, (header.match(/<\/div>/g) ?? []).length);
});
