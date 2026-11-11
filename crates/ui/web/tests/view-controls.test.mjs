import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const header = html.slice(html.indexOf('<header id="transport"'), html.indexOf("</header>"));
const count = (id) => (html.match(new RegExp(`id="${id}"`, "g")) ?? []).length;
const editBar = header.slice(header.indexOf('id="edit-bar"'));
const scaleBlock = header.slice(header.indexOf('id="ui-scale"'), header.indexOf('id="edit-bar"'));

test("taille de l'interface : un bouton « Aa » dans le bandeau et un panneau avec A−, la valeur, A+ et Auto (identifiants conservés)", () => {
  for (const id of ["ui-scale", "btn-ui-scale", "ui-scale-panel", "btn-ui-smaller", "ui-scale-value", "btn-ui-bigger", "btn-ui-auto", "ui-scale-mirror"]) {
    assert.equal(count(id), 1, id);
  }
  const panel = scaleBlock.slice(scaleBlock.indexOf('id="ui-scale-panel"'));
  for (const id of ["btn-ui-smaller", "ui-scale-value", "btn-ui-bigger", "btn-ui-auto"]) assert.ok(panel.includes(`id="${id}"`), `${id} dans le panneau`);
  assert.match(scaleBlock, /id="btn-ui-scale"[^>]*aria-haspopup="true"[^>]*aria-expanded="false"/);
  assert.match(scaleBlock, /id="ui-scale-panel"[^>]*hidden/);
  assert.ok(!scaleBlock.slice(0, scaleBlock.indexOf('id="ui-scale-panel"')).includes("btn-ui-bigger"), "le bouton A+ n'est plus dans la barre");
});

test("zoom : les trois boutons sont à la fin de la barre d'édition, plus dans le transport", () => {
  for (const id of ["btn-zoom-out", "btn-zoom-fit", "btn-zoom-in"]) {
    assert.equal(count(id), 1, id);
    assert.ok(editBar.includes(`id="${id}"`), `${id} dans la barre d'édition`);
  }
  assert.ok(!header.slice(0, header.indexOf('id="edit-bar"')).includes("btn-zoom"), "rien dans le transport");
  assert.ok(editBar.indexOf('class="zoom"') > editBar.indexOf('id="edit-audio"'), "le zoom est à droite des outils");
});

test("structure : la barre d'édition est bien fermée avant la fin du bandeau (pas d'imbrication)", () => {
  const open = (header.match(/<div\b/g) ?? []).length;
  const close = (header.match(/<\/div>/g) ?? []).length;
  assert.equal(open, close, "balises <div> équilibrées dans le bandeau");
  assert.ok(!scaleBlock.includes("edit-bar"), "la barre d'édition n'est pas dans le groupe de taille");
});
