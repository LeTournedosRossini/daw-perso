// Menus des pistes : le menu ⋮ d'une piste (renommer, couleur, sortie, monter / descendre, supprimer) et le menu « + » (ajouter
// une piste mono, stéréo ou un bus). Chaque choix est une commande du protocole ; l'affichage suit la réponse du moteur.

import { describeFilter, openMidiFilterDialog } from "./midi-filter-dialog.js";
import { describeCurve } from "../velocity-model.js";
import { openVelocityDialog } from "./velocity-dialog.js";
import { describeDynamics, openDynamicsDialog } from "./dynamics-dialog.js";
import { orderedNodes } from "./order.js";
import { getScale } from "../ui-scale.js";

export const PALETTE = ["#4aa3ff", "#e5484d", "#f5a524", "#30a46c", "#a78bfa", "#f76b15", "#12a594", "#d6409f", "#9aa3b2"];

let current = null; // { el, off }

export function closeMenu() {
  if (!current) return;
  current.el.remove();
  current.off();
  current = null;
}

function item(label, onClick, { disabled = false, checked = false, danger = false } = {}) {
  const b = document.createElement("button");
  b.type = "button";
  b.setAttribute("role", "menuitem");
  b.className = `item${danger ? " danger" : ""}`;
  b.disabled = disabled;
  b.innerHTML = `<span class="tick">${checked ? "✓" : ""}</span><span class="label"></span>`;
  b.querySelector(".label").textContent = label;
  b.addEventListener("click", () => {
    closeMenu();
    onClick();
  });
  return b;
}

function heading(text) {
  const h = document.createElement("div");
  h.className = "heading";
  h.textContent = text;
  return h;
}

/** Affiche `menu` sous `anchor` (en coordonnées de la page, qui peut être agrandie ou réduite par la taille de l'interface). */
export function showMenu(menu, anchor) {
  closeMenu();
  document.body.append(menu);
  const zoom = getScale() / 100;
  const r = anchor.getBoundingClientRect();
  const w = menu.offsetWidth;
  const h = menu.offsetHeight;
  const viewW = window.innerWidth / zoom;
  const viewH = window.innerHeight / zoom;
  let left = r.left / zoom;
  let top = r.bottom / zoom + 2;
  if (left + w > viewW - 6) left = Math.max(6, viewW - w - 6);
  if (top + h > viewH - 6) top = Math.max(6, r.top / zoom - h - 2);
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
  const away = (e) => {
    if (!menu.contains(e.target)) closeMenu();
  };
  const key = (e) => {
    if (e.key === "Escape") closeMenu();
  };
  setTimeout(() => document.addEventListener("pointerdown", away, true), 0);
  document.addEventListener("keydown", key, true);
  current = { el: menu, off: () => (document.removeEventListener("pointerdown", away, true), document.removeEventListener("keydown", key, true)) };
  menu.querySelector("button:not(:disabled)")?.focus();
}

export function menuEl() {
  const m = document.createElement("div");
  m.className = "track-menu";
  m.setAttribute("role", "menu");
  return m;
}

/** Pastilles numérotées : une par choix, la courante est marquée. */
function chips(items) {
  const box = document.createElement("div");
  box.className = "chips";
  for (const { label, title, pressed, onClick } of items) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "chip";
    b.textContent = label;
    b.title = title;
    b.setAttribute("aria-pressed", String(pressed));
    b.addEventListener("click", () => {
      closeMenu();
      onClick();
    });
    box.append(b);
  }
  return box;
}

/** Entrée d'enregistrement d'une piste : un canal, une paire, ou le MIDI ; et l'écoute directe de l'entrée. */
function appendInput(m, node, project, client) {
  const count = project.audio_inputs ?? 0;
  const cur = node.input ?? { kind: "audio", channels: [1] };
  const set = (input) => client.send({ type: "set_track_input", node: node.id, input });
  m.append(heading("Entrée d'enregistrement"));
  if (count === 0) {
    m.append(item("Aucune entrée audio ouverte par le pilote", () => {}, { disabled: true }));
  } else {
    const mono = Array.from({ length: count }, (_, i) => i + 1).map((c) => ({ label: String(c), title: `Entrée ${c} (mono)`, pressed: cur.kind === "audio" && cur.channels.length === 1 && cur.channels[0] === c, onClick: () => set({ kind: "audio", channels: [c] }) }));
    const pairs = Array.from({ length: Math.floor(count / 2) }, (_, i) => 2 * i + 1).map((c) => ({ label: `${c}-${c + 1}`, title: `Entrées ${c} et ${c + 1} (stéréo)`, pressed: cur.kind === "audio" && cur.channels.length === 2 && cur.channels[0] === c && cur.channels[1] === c + 1, onClick: () => set({ kind: "audio", channels: [c, c + 1] }) }));
    m.append(chips(mono), chips(pairs));
  }
  m.append(item("MIDI (clavier, pads…)", () => set({ kind: "midi" }), { checked: cur.kind === "midi" }));
  if (cur.kind === "midi") m.append(item(`Filtre d'entrée MIDI… (${describeFilter(node.midi_filter)})`, () => openMidiFilterDialog({ client, project, node })));
  m.append(item(`Courbe de vélocité… (${describeCurve(node.velocity_curve)})`, () => openVelocityDialog({ client, node })));
  if (cur.kind !== "midi") m.append(item(`Dynamique d'entrée… (${describeDynamics(node.dynamics)})`, () => openDynamicsDialog({ client, node })));
  m.append(item("Écoute directe de l'entrée", () => client.send({ type: "set_node", node: node.id, monitor: !node.monitor }), { checked: !!node.monitor }));
}

/** Menu « + » : ajouter une piste, un bus ou un retour de monitoring (celui-ci demande d'abord sa sortie : `onReturn`). */
export function openAddMenu(anchor, client, onReturn) {
  const m = menuEl();
  m.append(item("Piste stéréo", () => client.send({ type: "add_track", kind: "stereo" })), item("Piste mono", () => client.send({ type: "add_track", kind: "mono" })), item("Bus", () => client.send({ type: "add_track", kind: "bus" })));
  m.append(item("Retour…", () => (onReturn ? onReturn() : client.send({ type: "add_track", kind: "return" }))));
  showMenu(m, anchor);
}

/** Menu d'une piste ou d'un bus. `rename` : lance le renommage sur place. */
export function openTrackMenu(anchor, { node, project, client, rename, plugins }) {
  const list = orderedNodes(project).filter((n) => n.kind !== "master");
  const at = list.findIndex((n) => n.id === node.id);
  const m = menuEl();
  m.append(item("Renommer", rename));
  m.append(item("Inverser la phase (Ø)", () => client.send({ type: "set_node", node: node.id, invert: !node.invert }), { checked: !!node.invert }));
  if (plugins) m.append(item(`Plugins…${node.inserts?.length ? ` (${node.inserts.length})` : ""}`, plugins));

  m.append(heading("Couleur"));
  const sw = document.createElement("div");
  sw.className = "swatches";
  for (const c of PALETTE) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "swatch";
    b.style.background = c;
    b.title = c;
    b.setAttribute("aria-label", `Couleur ${c}`);
    b.setAttribute("aria-pressed", String(node.color.toLowerCase() === c));
    b.addEventListener("click", () => {
      closeMenu();
      client.send({ type: "set_track_color", node: node.id, color: c });
    });
    sw.append(b);
  }
  const custom = document.createElement("input");
  custom.type = "color";
  custom.className = "swatch custom";
  custom.value = /^#[0-9a-f]{6}$/i.test(node.color) ? node.color : "#9aa3b2";
  custom.title = "Autre couleur…";
  custom.addEventListener("change", () => {
    closeMenu();
    client.send({ type: "set_track_color", node: node.id, color: custom.value });
  });
  sw.append(custom);
  m.append(sw);

  if (node.kind !== "bus") appendInput(m, node, project, client);

  // Un retour n'envoie jamais rien à la sortie principale : sa sortie se règle sur la carte son (mixeur), pas ici.
  if (!node.is_return) {
    m.append(heading("Sortie"));
    m.append(item("Sortie principale", () => client.send({ type: "set_track_output", node: node.id, output: 0 }), { checked: !node.output }));
    for (const bus of list.filter((n) => n.kind === "bus" && n.id !== node.id && !n.is_return)) {
      m.append(item(bus.name, () => client.send({ type: "set_track_output", node: node.id, output: bus.id }), { checked: node.output === bus.id }));
    }
  }

  m.append(heading("Ordre"));
  m.append(
    item("Monter", () => client.send({ type: "move_track", node: node.id, before: list[at - 1].id }), { disabled: at <= 0 }),
    item("Descendre", () => client.send(list[at + 2] ? { type: "move_track", node: node.id, before: list[at + 2].id } : { type: "move_track", node: node.id }), { disabled: at < 0 || at >= list.length - 1 }),
  );
  m.append(item("Supprimer", () => client.send({ type: "delete_track", node: node.id }), { danger: true }));
  showMenu(m, anchor);
}
