// Calage de prises : éléments communs de l'interface (champs de réglages, mémoire des derniers réglages, petits utilitaires de DOM).

import { ANALYSIS_KEYS, ENGINES, MODES, parseSettings } from "../align-model.js";

export const KEY = "daw.align";
export const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};
export const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
export const fmtTime = (s) => `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, "0")}`;
export const clipLabel = (c, sr) => `${c.track} · ${c.name} (${fmtTime(c.start / sr)})`;

/** Les champs de réglages généraux : une ligne par réglage, liée à `settings.general`. `onChange(clé)` à chaque modification. */
export function settingsFields(settings, onChange, { analysisMark = false } = {}) {
  const box = h("div", "al-fields");
  const rows = new Map();
  const mark = (key) => (analysisMark && ANALYSIS_KEYS.includes(key) ? " ↻" : "");
  const row = (key, label, help, control, outText) => {
    const r = h("label", "dyn-row");
    r.title = help + (analysisMark && ANALYSIS_KEYS.includes(key) ? " Nécessite de refaire l'analyse." : "");
    const out = h("output", "dyn-out");
    r.append(h("span", "dyn-name", label + mark(key)), control, out);
    box.append(r);
    rows.set(key, () => {
      control.value = String(settings.general[key]);
      out.textContent = outText(settings.general[key]);
    });
    control.addEventListener("input", () => {
      settings.general[key] = control.tagName === "SELECT" ? control.value : Number(control.value);
      out.textContent = outText(settings.general[key]);
      onChange(key);
    });
  };
  const select = (opts) => {
    const s = h("select");
    for (const [v, t] of opts) s.append(new Option(t, v));
    return s;
  };
  const range = (min, max, step) => {
    const r = h("input");
    r.type = "range";
    Object.assign(r, { min, max, step });
    return r;
  };
  row("engine", "Moteur", "Moteur d'étirement du double. Signalsmith est rapide et convient presque toujours ; Rubber Band R3 est plus lent.", select(ENGINES), () => "");
  row("strength", "Force", "0 : le double reste tel quel ; 1 : calé entièrement sur le guide. La courbe de force (ci-dessous) la remplace là où elle existe.", range(0, 1, 0.01), (v) => `${Math.round(v * 100)} %`);
  row("protect", "Consonnes", "Protège les consonnes : elles gardent leur vitesse d'origine, seule l'attaque de la voyelle suivante est calée.", range(0, 1, 0.01), (v) => `${Math.round(v * 100)} %`);
  row("tightness", "Serrage", "Plus c'est serré, plus le double suit finement le guide ; moins, plus il reste naturel.", range(0, 1, 0.01), (v) => v.toFixed(2).replace(".", ","));
  row("max_shift_seconds", "Écart max.", "Écart maximal accepté entre le double et le guide, en secondes.", range(0.1, 5, 0.1), (v) => `${v.toFixed(1).replace(".", ",")} s`);
  row("analysis_mode", "Analyse", "« Même hauteur » pour une doublure ; « Harmonie » pour une voix qui chante les mêmes paroles à une autre hauteur (tierce, quinte, octave).", select(MODES), () => "");
  const refresh = () => rows.forEach((f) => f());
  refresh();
  return { el: box, refresh };
}

export function loadPref() {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}");
  } catch {
    return {};
  }
}
export function savePref(general) {
  try {
    localStorage.setItem(KEY, JSON.stringify(general));
  } catch {
    /* stockage indisponible */
  }
}

