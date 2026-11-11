// Menus des pistes : le menu ⋮ d'une piste (couleur, phase, entrée, sortie, supprimer — le reste a un accès direct : fx, nom par double-clic,
// ordre par glisser), la fenêtre des réglages d'entrée (appui long sur ● ou clic droit), le choix de la sortie (clic sur le routage) et le menu « + »
// (ajouter une piste mono, stéréo ou un bus). Chaque choix est une commande du protocole ; l'affichage suit la réponse du moteur.

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

/** Choix de la sortie d'une piste ou d'un bus : la sortie principale ou un bus (un retour sort sur la carte son : mixeur). */
function appendOutput(m, node, project, client) {
  const list = orderedNodes(project).filter((n) => n.kind !== "master");
  m.append(heading("Sortie"));
  m.append(item("Sortie principale", () => client.send({ type: "set_track_output", node: node.id, output: 0 }), { checked: !node.output }));
  for (const bus of list.filter((n) => n.kind === "bus" && n.id !== node.id && !n.is_return)) {
    m.append(item(bus.name, () => client.send({ type: "set_track_output", node: node.id, output: bus.id }), { checked: node.output === bus.id }));
  }
}

/** Réglages d'entrée d'une piste (canal ou paire, MIDI et son filtre, vélocité, dynamique, écoute directe). Un bus n'a pas d'entrée. */
export function openInputMenu(anchor, { node, project, client }) {
  if (!node || node.kind === "bus" || node.kind === "master") return;
  const m = menuEl();
  appendInput(m, node, project, client);
  showMenu(m, anchor);
}

/** Choix de la sortie (clic sur le routage « → Bus 1 » de l'en-tête). Un retour n'envoie rien à la sortie principale : rien à choisir ici. */
export function openOutputMenu(anchor, { node, project, client }) {
  if (!node || node.is_return || node.kind === "master") return;
  const m = menuEl();
  appendOutput(m, node, project, client);
  showMenu(m, anchor);
}

/**
 * Appui long (0,5 s), clic droit ou, au clavier, Maj+F10 sur le bouton d'armement : ouvre les réglages d'entrée de la piste. L'appui long ne
 * bascule pas l'armement. `ctx()` rend `{ node, project, client }` au moment de l'appel.
 */
export function bindInputMenu(btn, ctx) {
  let timer = null;
  let fired = false;
  const cancel = () => {
    clearTimeout(timer);
    timer = null;
  };
  const open = () => {
    const c = ctx();
    if (c) openInputMenu(btn, c);
  };
  btn.addEventListener("pointerdown", (e) => {
    if (e.button > 0) return;
    fired = false;
    timer = setTimeout(() => {
      timer = null;
      fired = true;
      open();
    }, 500);
  });
  for (const t of ["pointerup", "pointerleave", "pointercancel"]) btn.addEventListener(t, cancel);
  // Phase de capture : avant le gestionnaire qui arme la piste.
  btn.addEventListener(
    "click",
    (e) => {
      if (fired) {
        fired = false;
        e.stopImmediatePropagation();
        e.preventDefault();
      }
    },
    true,
  );
  btn.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    open();
  });
}

/** Menu ⋮ d'une piste ou d'un bus : seulement ce qui n'a pas d'accès direct. */
export function openTrackMenu(anchor, { node, project, client }) {
  const m = menuEl();
  if (node.kind !== "bus") m.append(item("Entrée d'enregistrement…", () => setTimeout(() => openInputMenu(anchor, { node, project, client }), 0)));
  if (!node.is_return) m.append(item("Sortie…", () => setTimeout(() => openOutputMenu(anchor, { node, project, client }), 0)));
  m.append(item("Inverser la phase (Ø)", () => client.send({ type: "set_node", node: node.id, invert: !node.invert }), { checked: !!node.invert }));

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

  m.append(item("Supprimer la piste", () => client.send({ type: "delete_track", node: node.id }), { danger: true }));
  showMenu(m, anchor);
}
