// Tempo et mesure : un clic sur l'affichage de la barre ouvre un petit formulaire (valeur, et à partir de quelle mesure). Chaque
// validation est une commande annulable ; les notes MIDI et l'automation suivent le nouveau tempo, pas les clips audio.

import { menuEl, showMenu, closeMenu } from "./track-menu.js";

export function field(label, input) {
  const l = document.createElement("label");
  l.className = "tm-field";
  const t = document.createElement("span");
  t.textContent = label;
  l.append(t, input);
  return l;
}

export function number(value, { min, max, step = 1 }) {
  const i = document.createElement("input");
  i.type = "number";
  i.min = String(min);
  i.max = String(max);
  i.step = String(step);
  i.value = String(value);
  return i;
}

/** Formulaire : `fields` (label → champ), un bouton « Appliquer » qui envoie `command()` ; Entrée valide, Échap ferme. */
export function form(anchor, title, fields, command, client) {
  const m = menuEl();
  m.classList.add("time-form");
  const h = document.createElement("div");
  h.className = "heading";
  h.textContent = title;
  m.append(h, ...fields);
  const ok = document.createElement("button");
  ok.type = "button";
  ok.className = "item tm-ok";
  ok.textContent = "Appliquer";
  const apply = () => {
    const cmd = command();
    closeMenu();
    if (cmd) client.send(cmd);
  };
  ok.addEventListener("click", apply);
  m.addEventListener("keydown", (e) => {
    if (e.key === "Enter") apply();
  });
  m.append(ok);
  showMenu(m, anchor);
  m.querySelector("input")?.focus();
  m.querySelector("input")?.select();
}

/** Tempo : valeur en noires par minute et mesure de départ (1 = tout le projet). */
export function openTempoForm(anchor, state, client) {
  const bpm = number(Math.round(state.tempo.bpmAt(state.pos) * 100) / 100, { min: 20, max: 400, step: 0.1 });
  const bar = number(1, { min: 1, max: 9999 });
  form(anchor, "Tempo", [field("Noires par minute", bpm), field("À partir de la mesure", bar)], () => ({ type: "set_tempo", bpm: Number(bpm.value), at_bar: Number(bar.value) }), client);
}

/** Chiffrage : nombre de temps, valeur du temps et mesure de départ. */
export function openMeterForm(anchor, state, client) {
  const cur = state.meters_.segments?.[0] ?? { num: 4, den: 4 };
  const num = number(cur.num, { min: 1, max: 32 });
  const den = document.createElement("select");
  for (const d of [1, 2, 4, 8, 16, 32]) {
    const o = document.createElement("option");
    o.value = String(d);
    o.textContent = `1/${d}`;
    o.selected = d === cur.den;
    den.append(o);
  }
  const bar = number(1, { min: 1, max: 9999 });
  form(anchor, "Mesure", [field("Temps par mesure", num), field("Valeur du temps", den), field("À partir de la mesure", bar)], () => ({ type: "set_meter", num: Number(num.value), den: Number(den.value), at_bar: Number(bar.value) }), client);
}
