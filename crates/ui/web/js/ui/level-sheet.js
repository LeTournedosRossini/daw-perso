// Feuille de réglage fin d'un niveau (envoi ou fader) : un grand fader horizontal sur toute la largeur de l'écran, pensé pour le
// toucher. Il se règle en **relatif** (on attrape où l'on veut, la valeur ne saute pas sous le doigt) ; à côté : −1 / −0,1 / +0,1 /
// +1 dB, retour à 0 dB, coupure. Les changements sont envoyés au fil du geste, comme sur le petit curseur.

import { formatDb } from "../time.js";
import { gainToDb, dbToGain } from "../protocol.js";

const DB_MIN = -60;
const DB_MAX = 6;
const clamp = (db) => Math.max(DB_MIN, Math.min(DB_MAX, db));

/** dB → gain linéaire (0 sous la butée basse), et retour. */
export const dbToLinear = (db) => (db <= DB_MIN ? 0 : dbToGain(Math.min(DB_MAX, db)));
export const linearToDb = (g) => (g <= 0 ? DB_MIN : clamp(gainToDb(g)));

/** Valeur après un glissement relatif de `dx` pixels sur un fader de `width` pixels couvrant toute la plage. */
export function dragDb(startDb, dx, width, fine = false) {
  const span = DB_MAX - DB_MIN;
  return clamp(startDb + (dx / Math.max(1, width)) * span * (fine ? 0.2 : 1));
}

export class LevelSheet {
  constructor() {
    this.root = document.createElement("section");
    this.root.id = "level-sheet";
    this.root.className = "level-sheet";
    this.root.hidden = true;
    this.root.setAttribute("aria-label", "Réglage fin du niveau");
    this.root.innerHTML = `
      <header><div class="ls-title"></div><output class="ls-db" aria-live="off"></output><button class="ls-close" aria-label="Fermer">✕</button></header>
      <div class="ls-fader" role="slider" aria-valuemin="${DB_MIN}" aria-valuemax="${DB_MAX}" tabindex="0"><div class="ls-fill"></div><div class="ls-zero"></div><div class="ls-thumb"></div></div>
      <div class="ls-steps">
        <button data-d="-1">−1</button><button data-d="-0.1">−0,1</button><button data-d="0.1">+0,1</button><button data-d="1">+1</button>
        <button data-d="unity">0 dB</button><button data-d="off">−∞</button>
      </div>
      <p class="ls-hint">Glisser sur le fader : réglage relatif (la valeur ne saute pas). Deux doigts ou Maj : plus fin.</p>`;
    document.body.append(this.root);
    this.fader = this.root.querySelector(".ls-fader");
    this.db = 0;
    this.target = null;
    this.drag = null;
    this.root.querySelector(".ls-close").addEventListener("click", () => this.close());
    for (const b of this.root.querySelectorAll(".ls-steps button")) {
      b.addEventListener("click", () => {
        const d = b.dataset.d;
        this.#set(d === "unity" ? 0 : d === "off" ? DB_MIN : this.db + Number(d));
      });
    }
    const f = this.fader;
    f.addEventListener("pointerdown", (e) => {
      f.setPointerCapture?.(e.pointerId);
      this.drag = { x: e.clientX, db: this.db, ids: new Set([e.pointerId]) };
      this.target?.onGesture?.(true);
    });
    f.addEventListener("pointermove", (e) => {
      if (!this.drag) return;
      this.#set(dragDb(this.drag.db, (e.clientX - this.drag.x) / this.#zoom(), f.clientWidth, e.shiftKey));
    });
    const end = () => {
      if (this.drag) this.target?.onGesture?.(false);
      this.drag = null;
    };
    f.addEventListener("pointerup", end);
    f.addEventListener("pointercancel", end);
    f.addEventListener("keydown", (e) => {
      if (e.key === "ArrowLeft" || e.key === "ArrowDown") this.#set(this.db - (e.shiftKey ? 0.1 : 1));
      else if (e.key === "ArrowRight" || e.key === "ArrowUp") this.#set(this.db + (e.shiftKey ? 0.1 : 1));
      else if (e.key === "Home") this.#set(0);
      else if (e.key === "Escape") this.close();
      else return;
      e.preventDefault();
      e.stopPropagation();
    });
    // Un deuxième doigt pendant le geste : réglage fin (le geste repart de la valeur actuelle).
    f.addEventListener("pointerdown", (e) => {
      if (e.pointerType === "touch" && this.drag && this.drag.ids.size > 0 && !this.drag.ids.has(e.pointerId)) this.drag = { ...this.drag, fine: true };
    });
    this.root.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.close();
    });
  }

  #zoom() {
    return parseFloat(getComputedStyle(document.documentElement).zoom) || 1;
  }

  isOpen() {
    return !this.root.hidden;
  }

  /** Ouvre la feuille pour un niveau : `title`, valeur en dB, et `onChange(gain)` ; `onUnity` n'est pas nécessaire (bouton 0 dB). */
  open({ title, gain, onChange, onGesture }) {
    this.target = { onChange, onGesture };
    this.root.querySelector(".ls-title").textContent = title;
    this.db = linearToDb(gain);
    this.root.hidden = false;
    this.#paint();
    this.fader.focus();
  }

  /** Nouvelle valeur venue du moteur (pas pendant un geste). */
  update(gain) {
    if (this.root.hidden || this.drag) return;
    this.db = linearToDb(gain);
    this.#paint();
  }

  close() {
    this.root.hidden = true;
    this.target = null;
    this.drag = null;
  }

  #set(db) {
    this.db = Math.round(clamp(db) * 10) / 10;
    this.#paint();
    this.target?.onChange(dbToLinear(this.db));
  }

  #paint() {
    const frac = (this.db - DB_MIN) / (DB_MAX - DB_MIN);
    this.root.querySelector(".ls-db").textContent = `${formatDb(this.db <= DB_MIN ? -Infinity : this.db)} dB`;
    this.root.querySelector(".ls-thumb").style.left = `${frac * 100}%`;
    this.root.querySelector(".ls-fill").style.width = `${frac * 100}%`;
    this.root.querySelector(".ls-zero").style.left = `${(-DB_MIN / (DB_MAX - DB_MIN)) * 100}%`;
    this.fader.setAttribute("aria-valuenow", String(Math.round(this.db)));
  }
}
