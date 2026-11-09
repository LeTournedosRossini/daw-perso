// Potentiomètre rotatif (volume, panoramique des en-têtes de pistes) : compact et utilisable au toucher.
// Geste : glisser verticalement (réglage **relatif**, la valeur ne saute jamais sous le doigt ; Maj ou un second doigt = cinq fois
// plus fin) ; molette ; flèches / PageUp / PageDown / Début / Fin ; double-clic (ou double tape) = valeur de repos.

import { angleOf, arcPath, arcSpan, dragValue, keyValue, polar, wheelValue } from "../knob-model.js";

const SVG = "http://www.w3.org/2000/svg";
const R = 11; // rayon de l'arc dans un cadre 28 × 28

export class Knob {
  /**
   * @param {{min:number,max:number,step?:number,value:number,rest:number,origin?:number,label:string,travel?:number,
   *   onInput:(v:number)=>void, text?:(v:number)=>string}} o
   *  `rest` : valeur du double-clic ; `origin` : d'où part l'arc coloré (par défaut le minimum).
   */
  constructor(o) {
    this.o = { step: 0.1, travel: 160, origin: o.min, ...o };
    this.value = o.value;
    this.dragging = false;
    const el = document.createElement("div");
    el.className = "knob";
    el.tabIndex = 0;
    el.setAttribute("role", "slider");
    el.setAttribute("aria-label", o.label);
    el.setAttribute("aria-valuemin", String(o.min));
    el.setAttribute("aria-valuemax", String(o.max));
    el.innerHTML = `<svg viewBox="0 0 28 28" aria-hidden="true"><path class="rail"/><path class="arc"/><line class="needle"/></svg>`;
    this.el = el;
    this.rail = el.querySelector(".rail");
    this.arc = el.querySelector(".arc");
    this.needle = el.querySelector(".needle");
    this.rail.setAttribute("d", arcPath(14, 14, R, angleOf(o.min, o.min, o.max), angleOf(o.max, o.min, o.max)));
    this.#bind();
    this.#paint();
  }

  /** Affiche `v` sans rien envoyer ; ignoré pendant un geste (le moteur ne reprend pas la main sous le doigt). */
  set(v) {
    if (this.dragging) return;
    this.value = v;
    this.#paint();
  }

  #commit(v, silent = false) {
    if (v === this.value) return;
    this.value = v;
    this.#paint();
    if (!silent) this.o.onInput(v);
  }

  #paint() {
    const { min, max, origin, text } = this.o;
    const v = this.value;
    const { a0, a1 } = arcSpan(v, origin, min, max);
    this.arc.setAttribute("d", arcPath(14, 14, R, a0, a1));
    const a = angleOf(v, min, max);
    const [x0, y0] = polar(14, 14, 3, a);
    const [x1, y1] = polar(14, 14, R - 1, a);
    this.needle.setAttribute("x1", x0.toFixed(2));
    this.needle.setAttribute("y1", y0.toFixed(2));
    this.needle.setAttribute("x2", x1.toFixed(2));
    this.needle.setAttribute("y2", y1.toFixed(2));
    this.el.setAttribute("aria-valuenow", String(v));
    this.el.setAttribute("aria-valuetext", text ? text(v) : String(v));
  }

  #bind() {
    const el = this.el;
    const pts = new Map(); // doigts / pointeurs posés : id → y
    let start = null; // { y, value }
    const lead = () => [...pts.keys()][0];
    const base = (fine) => ({ min: this.o.min, max: this.o.max, step: this.o.step, travel: this.o.travel, fine });
    el.addEventListener("pointerdown", (e) => {
      if (e.button > 0) return;
      e.preventDefault();
      el.focus({ preventScroll: true });
      el.setPointerCapture(e.pointerId);
      pts.set(e.pointerId, e.clientY);
      if (pts.size === 1) {
        this.dragging = true;
        el.classList.add("active");
      }
      // Un second doigt ne déplace pas la valeur : il passe en réglage fin et repart de la valeur courante.
      start = { y: pts.get(lead()), value: this.value };
    });
    el.addEventListener("pointermove", (e) => {
      if (!pts.has(e.pointerId)) return;
      pts.set(e.pointerId, e.clientY);
      if (e.pointerId !== lead() || !start) return;
      const fine = e.shiftKey || pts.size > 1;
      this.#commit(dragValue(start.value, start.y - e.clientY, base(fine)));
    });
    const up = (e) => {
      if (!pts.delete(e.pointerId)) return;
      if (pts.size === 0) {
        this.dragging = false;
        el.classList.remove("active");
        start = null;
      } else start = { y: pts.get(lead()), value: this.value };
    };
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
    el.addEventListener("dblclick", () => this.#commit(this.o.rest));
    el.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        this.#commit(wheelValue(this.value, e.deltaY, { ...base(false), shift: e.shiftKey }));
      },
      { passive: false },
    );
    el.addEventListener("keydown", (e) => {
      const v = keyValue(e.key, this.value, { ...base(false), shift: e.shiftKey });
      if (v === null) return;
      e.preventDefault();
      this.#commit(v);
    });
  }
}

/** Crée un potentiomètre avec son étiquette et son texte de valeur ; rend `{ root, knob, out }`. */
export function knobControl(label, o) {
  const root = document.createElement("div");
  root.className = "ctl-knob";
  const cap = document.createElement("span");
  cap.className = "cap";
  cap.textContent = label;
  const knob = new Knob({ label: label === "Vol" ? "Volume" : label === "Pan" ? "Panoramique" : label, ...o });
  const out = document.createElement("output");
  root.append(knob.el, cap, out);
  return { root, knob, out };
}
