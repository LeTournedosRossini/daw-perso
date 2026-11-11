// Page de la fenêtre d'un plugin (ouverte par l'application, `?insert=<numéro>`) : à gauche la liste des plugins de la piste, à droite la bande
// (contourner, nom, sorties, bouton Interface) et la liste des paramètres. L'interface native du plugin n'est pas dessinée ici :
// l'application la loge dans la zone sous la bande (`docked`), et le bouton Interface la montre ou la cache. Choisir un autre plugin dans
// la liste de gauche change le plugin de la **même** fenêtre (`switch_plugin_ui`).

import { createEngine } from "./engine-select.js";
import { PluginView } from "./ui/plugin-view.js";

const q = new URLSearchParams(location.search);
let insertId = Number(q.get("insert"));
if (q.get("theme")) document.documentElement.dataset.theme = q.get("theme");

const state = { project: null, pos: 0, tempo: null };
const { engine, start } = await createEngine();
const view = new PluginView({ state, client: engine, root: document.querySelector(".pv-host"), docked: true });
view.boundInsert = insertId;
let shown = false;

/**
 * Dit à l'application où est la zone de l'interface native (coin haut-gauche de `.pv-body`, en pixels de l'écran, dans le repère de la vue web) :
 * c'est mesuré ici, dans la page réellement affichée, plutôt que supposé par l'application. La zone va jusqu'aux bords droit et bas.
 */
let lastSlot = "";
function reportSlot() {
  const r = document.querySelector(".pv-body").getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const x = Math.round(r.left * dpr);
  const y = Math.round(r.top * dpr);
  const key = `${insertId}:${x}:${y}`;
  if (key === lastSlot) return;
  lastSlot = key;
  engine.send({ type: "set_plugin_area", insert: insertId, x, y });
}
new ResizeObserver(reportSlot).observe(document.body);
window.addEventListener("resize", reportSlot);

const rowsBox = document.querySelector(".pw-rows");
const titleEl = document.querySelector(".pw-title");

function owner(project) {
  return project?.nodes.find((n) => n.inserts.some((i) => i.id === insertId)) ?? null;
}

/** La liste des plugins de la piste (comme la chaîne d'effets de Reaper) : un clic change le plugin de cette fenêtre. */
function renderList() {
  const node = owner(state.project);
  rowsBox.replaceChildren();
  if (!node) return;
  titleEl.textContent = node.name;
  for (const ins of node.inserts) {
    const row = document.createElement("div");
    row.className = `pw-row${ins.id === insertId ? " selected" : ""}`;
    const byp = document.createElement("button");
    byp.type = "button";
    byp.className = "pw-byp";
    byp.textContent = "⏻";
    byp.setAttribute("aria-pressed", String(!!ins.bypass));
    byp.setAttribute("aria-label", "Contourner");
    byp.title = ins.bypass ? "Contourné : cliquer pour réactiver" : "Actif : cliquer pour contourner";
    byp.addEventListener("click", () => engine.send({ type: "set_plugin_bypass", insert: ins.id, bypass: !ins.bypass }));
    const name = document.createElement("button");
    name.type = "button";
    name.className = "pw-name";
    name.textContent = ins.name;
    name.title = ins.name;
    name.addEventListener("click", () => {
      if (ins.id !== insertId) engine.send({ type: "switch_plugin_ui", from: insertId, to: ins.id });
      else view.show(view.view === "native" ? "native" : "params");
    });
    row.append(byp, name);
    if (ins.id === insertId) {
      // Le plugin affiché : monter, descendre, retirer (la fenêtre passe au voisin avant le retrait, pour ne pas se fermer).
      const i = node.inserts.indexOf(ins);
      const tools = document.createElement("div");
      tools.className = "pw-tools";
      const mk = (label, title, disabled, fn) => {
        const b = document.createElement("button");
        b.type = "button";
        b.textContent = label;
        b.title = title;
        b.setAttribute("aria-label", title);
        b.disabled = disabled;
        b.addEventListener("click", fn);
        return b;
      };
      tools.append(
        mk("↑", "Monter dans la chaîne", i === 0, () => engine.send({ type: "move_plugin", insert: ins.id, before: node.inserts[i - 1].id })),
        mk("↓", "Descendre dans la chaîne", i === node.inserts.length - 1, () => engine.send(node.inserts[i + 2] ? { type: "move_plugin", insert: ins.id, before: node.inserts[i + 2].id } : { type: "move_plugin", insert: ins.id })),
        mk("✕", "Retirer ce plugin de la piste", false, () => {
          const next = node.inserts[i + 1] ?? node.inserts[i - 1];
          if (next) engine.send({ type: "switch_plugin_ui", from: ins.id, to: next.id });
          engine.send({ type: "remove_plugin", insert: ins.id });
        }),
      );
      row.append(tools);
    }
    rowsBox.append(row);
  }
}

document.querySelector(".pw-add").addEventListener("click", () => {
  const node = owner(state.project);
  if (node) view.setTarget(node.id, null); // emplacement vide : le sélecteur s'affiche (la zone native est cachée)
});
view.onAdded = (fresh) => engine.send({ type: "switch_plugin_ui", from: insertId, to: fresh });

engine.on("plugin_ui_switched", (e) => {
  if (e.from !== insertId) return;
  insertId = e.to;
  view.boundInsert = insertId;
  const node = owner(state.project);
  if (node) {
    view.setTarget(node.id, insertId);
    document.title = `${node.inserts.find((i) => i.id === insertId)?.name ?? "Plugin"} — ${node.name}`;
  }
  renderList();
  lastSlot = "";
  reportSlot();
});

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
  renderList();
  reportSlot();
  setTimeout(reportSlot, 300); // après le chargement des polices : la bande ne bouge plus
});
engine.on("position", (e) => {
  state.pos = e.pos;
});
engine.on("error", (e) => console.warn("DAW", e.command, e.message));

await start();
engine.send({ type: "get_project" });
engine.send({ type: "get_control_pages" });
// Navigateur (essai de la page avec le faux moteur) : le plugin demandé n'existe pas, on en ajoute deux à la première piste.
if (!globalThis.__TAURI__) {
  setTimeout(() => {
    if (shown) return;
    engine.send({ type: "list_plugins" });
    engine.on("plugins", (e) => {
      if (e.scanning || shown) return;
      const pick = (re) => e.plugins.find((x) => re.test(x.name)) ?? e.plugins[0];
      for (const p of [pick(/compress/i), pick(/egalis|equal/i)]) engine.send({ type: "add_plugin", node: 1, clap_id: p.clap_id, file: p.file });
      setTimeout(() => {
        const first = state.project?.nodes.find((x) => x.id === 1)?.inserts[0];
        if (first && !shown) {
          shown = true;
          insertId = first.id;
          view.boundInsert = insertId;
          view.setTarget(1, insertId);
          renderList();
        }
      }, 300);
    });
  }, 400);
}
