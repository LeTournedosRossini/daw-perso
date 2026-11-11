import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { selectionContext, contextLabel } from "../js/edit-context.js";

const project = {
  nodes: [
    { id: 1, clips: [{ id: 10, kind: "audio" }, { id: 11, kind: "audio" }] },
    { id: 2, clips: [{ id: 20, kind: "midi" }, { id: 21, kind: "midi" }] },
  ],
};

test("contexte de sélection : rien, audio, notes, mélange, identifiants inconnus ignorés", () => {
  assert.deepEqual(selectionContext(project, new Set()), { audio: 0, notes: 0, total: 0, label: "Aucun clip sélectionné", showAudioTools: false });
  assert.deepEqual(selectionContext(project, new Set([10])), { audio: 1, notes: 0, total: 1, label: "1 clip audio", showAudioTools: true });
  const two = selectionContext(project, new Set([10, 11]));
  assert.equal(two.label, "2 clips audio");
  const notes = selectionContext(project, new Set([20, 21]));
  assert.equal(notes.label, "2 clips de notes");
  assert.equal(notes.showAudioTools, false, "pas d'outils audio pour des notes seules");
  const mixed = selectionContext(project, new Set([10, 20, 21]));
  assert.equal(mixed.label, "1 clip audio + 2 clips de notes");
  assert.equal(mixed.showAudioTools, true);
  assert.equal(selectionContext(project, new Set([999])).total, 0, "un identifiant inconnu ne compte pas");
  assert.equal(selectionContext(null, undefined).label, "Aucun clip sélectionné");
  assert.equal(contextLabel(0, 1), "1 clip de notes");
});

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const header = html.slice(html.indexOf('<header id="transport"'), html.indexOf("</header>"));
const bar = header.slice(header.indexOf('id="edit-bar"'));
const count = (id) => (html.match(new RegExp(`id="${id}"`, "g")) ?? []).length;

test("barre d'édition : les outils gardent leur identifiant (une fois) et sont dans la barre ; annuler / rétablir restent dans le transport", () => {
  for (const id of ["btn-split", "btn-delete", "snap-grid", "btn-xfade", "btn-onsets", "onset-sens", "btn-quant-markers", "stretch-engine"]) {
    assert.equal(count(id), 1, id);
    assert.ok(bar.includes(`id="${id}"`), `${id} dans la barre d'édition`);
  }
  for (const id of ["btn-undo", "btn-redo"]) {
    assert.equal(count(id), 1, id);
    assert.ok(header.includes(`id="${id}"`) && !bar.includes(`id="${id}"`), `${id} dans le transport`);
  }
});

test("barre d'édition : les quatre outils audio sont dans un groupe caché au départ, le reste toujours visible", () => {
  const audio = bar.slice(bar.indexOf('id="edit-audio"'));
  assert.match(audio, /^id="edit-audio"[^>]*hidden/);
  for (const id of ["btn-onsets", "onset-sens", "btn-quant-markers", "stretch-engine"]) assert.ok(audio.includes(`id="${id}"`), id);
  const always = bar.slice(0, bar.indexOf('id="edit-audio"'));
  for (const id of ["btn-split", "btn-delete", "snap-grid", "btn-xfade"]) assert.ok(always.includes(`id="${id}"`), id);
  assert.match(bar, /role="toolbar"/);
});
