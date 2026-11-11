// Page de la fenêtre d'un plugin (ouverte par l'application, `?insert=<numéro>`) : la bande (contourner, nom, sorties, bascule
// Interface / Paramètres) et la liste des paramètres. L'interface native du plugin n'est pas dessinée ici : l'application la loge dans
// la zone sous la bande (`docked`), et la bascule la montre ou la cache.

import { createEngine } from "./engine-select.js";
import { PluginView } from "./ui/plugin-view.js";

const q = new URLSearchParams(location.search);
const insertId = Number(q.get("insert"));
if (q.get("theme")) document.documentElement.dataset.theme = q.get("theme");

const state = { project: null, pos: 0, tempo: null };
const { engine, start } = await createEngine();
const view = new PluginView({ state, client: engine, root: document.querySelector(".pv-host"), docked: true });
let shown = false;

function owner(project) {
  return project?.nodes.find((n) => n.inserts.some((i) => i.id === insertId)) ?? null;
}

engine.on("project", (p) => {
  state.project = p;
  const node = owner(p);
  if (!node) {
    if (shown) view.refresh(); // le plugin a disparu : la fenêtre va être fermée par l'application
    return;
  }
  const ins = node.inserts.find((i) => i.id === insertId);
  document.title = `${ins.name} — ${node.name}`;
  if (!shown) {
    shown = true;
    view.setTarget(node.id, insertId);
  } else {
    view.refresh();
  }
});
engine.on("position", (e) => {
  state.pos = e.pos;
});
engine.on("error", (e) => console.warn("DAW", e.command, e.message));

await start();
engine.send({ type: "get_project" });
engine.send({ type: "get_control_pages" });
// Navigateur (essai de la page avec le faux moteur) : le plugin demandé n'existe pas, on en ajoute un à la première piste.
if (!globalThis.__TAURI__) {
  setTimeout(() => {
    if (!shown) {
      engine.send({ type: "list_plugins" });
      engine.on("plugins", (e) => {
        if (e.scanning || shown) return;
        const p = e.plugins.find((x) => /compress/i.test(x.name)) ?? e.plugins[0];
        engine.send({ type: "add_plugin", node: 1, clap_id: p.clap_id, file: p.file });
        setTimeout(() => {
          const n = state.project?.nodes.find((x) => x.id === 1);
          const last = n?.inserts.at(-1);
          if (last && !shown) {
            shown = true;
            view.setTarget(1, last.id);
          }
        }, 300);
      });
    }
  }, 400);
}
