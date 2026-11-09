// Options du métronome : un petit formulaire (même présentation que tempo et mesure). Chaque réglage est envoyé tel quel au moteur
// dès qu'il change ; le moteur renvoie l'événement `metronome` qui met la barre à jour.

import { field, number } from "./time-menu.js";
import { menuEl, showMenu } from "./track-menu.js";

export const DEFAULT_METRONOME = { enabled: false, only_recording: false, volume: 0.8, count_in_bars: 0, pre_roll: 0, post_roll: 0, pre_roll_bars: 0, pre_roll_unit: "seconds" };

/** Bornes du champ de pré-roll selon l'unité : 0 à 10 s (pas de 0,5) ou 0 à 8 mesures (entières). */
export function preBounds(unit) {
  return unit === "bars" ? { min: 0, max: 8, step: 1 } : { min: 0, max: 10, step: 0.5 };
}

/** Texte court du pré-roll pour les bulles d'aide : « 2 s » ou « 2 mesures ». */
export function preRollText(m) {
  if (m.pre_roll_unit === "bars") return m.pre_roll_bars ? `${m.pre_roll_bars} mesure${m.pre_roll_bars > 1 ? "s" : ""}` : null;
  return m.pre_roll ? `${m.pre_roll} s` : null;
}

export function openMetronomeForm(anchor, state, client) {
  const cur = { ...DEFAULT_METRONOME, ...state.metronome };
  const m = menuEl();
  m.classList.add("time-form");
  const h = document.createElement("div");
  h.className = "heading";
  h.textContent = "Métronome";
  const send = () => client.send({ ...cur, type: "set_metronome" });

  const only = document.createElement("input");
  only.type = "checkbox";
  only.checked = cur.only_recording;
  only.addEventListener("change", () => ((cur.only_recording = only.checked), send()));
  const onlyLabel = field("Seulement pendant l'enregistrement", only);

  const vol = document.createElement("input");
  vol.type = "range";
  vol.min = "0";
  vol.max = "150";
  vol.value = String(Math.round(cur.volume * 100));
  vol.addEventListener("change", () => ((cur.volume = Number(vol.value) / 100), send()));

  const bars = document.createElement("select");
  for (const n of [0, 1, 2]) {
    const o = document.createElement("option");
    o.value = String(n);
    o.textContent = n === 0 ? "aucun" : n === 1 ? "1 mesure" : "2 mesures";
    o.selected = n === cur.count_in_bars;
    bars.append(o);
  }
  bars.addEventListener("change", () => ((cur.count_in_bars = Number(bars.value)), send()));

  // Pré-roll : un nombre et son unité (secondes, ou mesures du projet).
  const pre = number(cur.pre_roll_unit === "bars" ? cur.pre_roll_bars : cur.pre_roll, preBounds(cur.pre_roll_unit));
  const unit = document.createElement("select");
  for (const [v, label] of [["seconds", "secondes"], ["bars", "mesures"]]) {
    const o = document.createElement("option");
    o.value = v;
    o.textContent = label;
    o.selected = v === cur.pre_roll_unit;
    unit.append(o);
  }
  const preBox = document.createElement("span");
  preBox.className = "tm-pre";
  preBox.append(pre, unit);
  const applyPre = () => {
    const b = preBounds(cur.pre_roll_unit);
    pre.min = String(b.min);
    pre.max = String(b.max);
    pre.step = String(b.step);
    pre.value = String(cur.pre_roll_unit === "bars" ? cur.pre_roll_bars : cur.pre_roll);
  };
  pre.addEventListener("change", () => {
    const b = preBounds(cur.pre_roll_unit);
    const v = Math.max(b.min, Math.min(b.max, Number(pre.value) || 0));
    if (cur.pre_roll_unit === "bars") cur.pre_roll_bars = Math.round(v);
    else cur.pre_roll = v;
    applyPre();
    send();
  });
  unit.addEventListener("change", () => {
    cur.pre_roll_unit = unit.value;
    applyPre();
    send();
  });
  const post = number(cur.post_roll, { min: 0, max: 10, step: 0.5 });
  post.addEventListener("change", () => ((cur.post_roll = Math.max(0, Math.min(10, Number(post.value) || 0))), send()));

  const note = document.createElement("div");
  note.className = "tm-note";
  note.textContent = "Sortie du clic : sortie principale.";
  m.append(h, onlyLabel, field("Volume", vol), field("Pré-compte avant l'enregistrement", bars), field("Pré-roll avant le point d'enregistrement", preBox), field("Post-roll (secondes après l'arrêt)", post), note);
  showMenu(m, anchor);
}
