import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";

const html = fs.readFileSync(new URL("../index.html", import.meta.url), "utf8");
const count = (id) => (html.match(new RegExp(`id="${id}"`, "g")) ?? []).length;
const panel = html.slice(html.indexOf('id="file-menu"'), html.indexOf('<div id="scroller">'));
const bar = html.slice(html.indexOf('<header id="transport"'), html.indexOf("</header>"));

test("menu Fichier : chaque bouton garde son identifiant, une seule fois (les écouteurs existants s'y accrochent)", () => {
  for (const id of ["btn-file", "file-menu", "btn-new", "btn-open", "btn-save", "btn-saveas", "btn-import", "btn-export", "btn-align", "btn-settings", "btn-scripts"]) {
    assert.equal(count(id), 1, id);
  }
});

test("menu Fichier : le panneau contient les sept actions, le bandeau garde Fichier, Enregistrer et Scripts", () => {
  for (const id of ["btn-new", "btn-open", "btn-saveas", "btn-import", "btn-export", "btn-align", "btn-settings"]) {
    assert.ok(panel.includes(`id="${id}"`), `${id} dans le menu`);
    assert.ok(!bar.includes(`id="${id}"`), `${id} plus dans le bandeau`);
  }
  for (const id of ["btn-file", "btn-save", "btn-scripts"]) assert.ok(bar.includes(`id="${id}"`), `${id} reste dans le bandeau`);
});

test("menu Fichier : accessible (menu, éléments de menu, état déplié) et caché au départ", () => {
  assert.match(html, /id="btn-file"[^>]*aria-haspopup="menu"[^>]*aria-expanded="false"/);
  assert.match(html, /id="file-menu"[^>]*role="menu"[^>]*hidden/);
  assert.equal((panel.match(/role="menuitem"/g) ?? []).length, 7);
});
