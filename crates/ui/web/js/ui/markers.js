// Marqueurs de tempo et de mesure : une bande au-dessus de la règle montre chaque endroit où le tempo ou le chiffrage change
// (« ♩ = 90 · 3/4 »). Un clic ouvre le marqueur (modifier, supprimer) ; « ＋ marqueur » en pose un à la mesure de la tête de lecture.

import { buildMarkers, markerLabel, sampleToX } from "../time.js";
import { field, number } from "./time-menu.js";
import { menuEl, showMenu, closeMenu } from "./track-menu.js";

export class Markers {
  constructor(el, state, client, timeline) {
    this.el = el;
    this.state = state;
    this.client = client;
    this.timeline = timeline;
    this.key = "";
    document.getElementById("btn-add-marker")?.addEventListener("click", (e) => this.open(null, e.currentTarget));
  }

  /** Repositionne les marqueurs ; ne reconstruit que si la vue ou le projet ont changé. */
  refresh() {
    const { project, view } = this.state;
    if (!project) return;
    const key = `${view.start}|${view.spp}|${this.timeline.width}|${JSON.stringify(project.tempo_map)}|${JSON.stringify(project.meters)}`;
    if (key === this.key) return;
    this.key = key;
    this.el.replaceChildren();
    for (const m of buildMarkers(project)) {
      const x = sampleToX(view, m.sample);
      if (x < -200 || x > this.timeline.width + 20) continue;
      const b = document.createElement("button");
      b.type = "button";
      b.className = "marker";
      b.style.left = `${Math.max(0, x)}px`;
      b.textContent = markerLabel(m);
      b.title = `Mesure ${m.bar}${m.midBar ? " (en cours de mesure)" : ""} — cliquer pour modifier ou supprimer`;
      b.addEventListener("click", () => this.open(m, b));
      this.el.append(b);
    }
  }

  /** Formulaire : `marker` = un marqueur existant, `null` = en créer un à la mesure de la tête de lecture. */
  open(marker, anchor) {
    const { state, client } = this;
    const { tempo, meters_: meters, project } = state;
    const bar0 = marker ? marker.bar - 1 : Math.max(0, meters.locate(Math.max(0, tempo.wholeAt(state.pos))).bar);
    const sample = tempo.samplesAt(meters.barStart(bar0));
    const curBpm = Math.round((marker?.tempo?.bpm ?? tempo.bpmAt(sample)) * 100) / 100;
    const curMeter = marker?.meter ?? meters.meterAtBar(bar0);

    const m = menuEl();
    m.classList.add("time-form");
    const h = document.createElement("div");
    h.className = "heading";
    h.textContent = marker ? `Marqueur — mesure ${marker.bar}` : "Nouveau marqueur";
    const bar = number(bar0 + 1, { min: 1, max: 9999 });
    bar.disabled = !!marker;
    const bpm = number(curBpm, { min: 20, max: 400, step: 0.1 });
    const num = number(curMeter.num, { min: 1, max: 32 });
    const den = document.createElement("select");
    for (const d of [1, 2, 4, 8, 16, 32]) {
      const o = document.createElement("option");
      o.value = String(d);
      o.textContent = `1/${d}`;
      o.selected = d === curMeter.den;
      den.append(o);
    }
    m.append(h, field("Mesure", bar), field("Noires par minute", bpm), field("Temps par mesure", num), field("Valeur du temps", den));

    const apply = () => {
      const at = Number(bar.value);
      const b0 = at - 1;
      const t = tempo.bpmAt(tempo.samplesAt(meters.barStart(b0)));
      const mt = meters.meterAtBar(b0);
      const hasTempo = !!marker?.tempo;
      const hasMeter = !!marker?.meter;
      closeMenu();
      if (Number(bpm.value) !== (hasTempo ? marker.tempo.bpm : Math.round(t * 100) / 100)) client.send({ type: "set_tempo", bpm: Number(bpm.value), at_bar: at });
      if (Number(num.value) !== (hasMeter ? marker.meter.num : mt.num) || Number(den.value) !== (hasMeter ? marker.meter.den : mt.den)) {
        client.send({ type: "set_meter", num: Number(num.value), den: Number(den.value), at_bar: at });
      }
    };
    const ok = document.createElement("button");
    ok.type = "button";
    ok.className = "item tm-ok";
    ok.textContent = "Appliquer";
    ok.addEventListener("click", apply);
    m.addEventListener("keydown", (e) => e.key === "Enter" && apply());
    m.append(ok);

    if (marker) {
      const first = marker.bar === 1 && !marker.midBar;
      const del = document.createElement("button");
      del.type = "button";
      del.className = "item tm-del";
      del.textContent = "Supprimer le marqueur";
      del.disabled = first;
      del.title = first ? "Le tempo et le chiffrage du début ne se suppriment pas" : "";
      del.addEventListener("click", () => {
        closeMenu();
        if (marker.tempo && marker.tempo.pos[0] !== 0) client.send({ type: "remove_tempo", pos: marker.tempo.pos });
        if (marker.meter && marker.bar >= 2) client.send({ type: "remove_meter", at_bar: marker.bar });
      });
      m.append(del);
    }
    showMenu(m, anchor);
  }
}
