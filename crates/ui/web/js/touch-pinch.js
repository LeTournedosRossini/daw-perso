// Pincement à deux doigts sur un élément (écran tactile) : dès qu'un deuxième doigt se pose, le geste d'un doigt en cours (déplacer un clip,
// poser la boucle…) est abandonné et l'élément ne reçoit plus d'événements tant que tous les doigts ne sont pas levés (le doigt qui reste ne
// doit pas déplacer un clip). Le geste donne un **zoom horizontal** (rapport de l'écart horizontal entre les doigts, à chaque mouvement) et
// le déplacement du milieu (défilement) ; le **sens** du geste (horizontal ou vertical) est décidé une fois, sur le mouvement cumulé
// (`pinchMode`). Seule la souris ne passe pas par là.

import { pinchMode } from "./notes-model.js";

/** Facteur de zoom horizontal entre deux positions des deux doigts ; 1 si les doigts sont trop proches en horizontal pour que le rapport ait un sens. */
export function horizontalFactor(a0, b0, a1, b1, min = 24) {
  const d0 = Math.abs(a0.x - b0.x);
  const d1 = Math.abs(a1.x - b1.x);
  return d0 >= min && d1 >= min ? Math.max(0.2, Math.min(5, d1 / d0)) : 1;
}

/** Facteur de zoom vertical (même règle que `horizontalFactor`, sur l'écart vertical entre les doigts). */
export function verticalFactor(a0, b0, a1, b1, min = 24) {
  const d0 = Math.abs(a0.y - b0.y);
  const d1 = Math.abs(a1.y - b1.y);
  return d0 >= min && d1 >= min ? Math.max(0.2, Math.min(5, d1 / d0)) : 1;
}

export class TouchPinch {
  /**
   * `onStart()` : le deuxième doigt vient de se poser (abandonner ce qui était en cours) ;
   * `onMove({ factor, fy, mode, dx, dy, x, y })` : facteurs de zoom horizontal et vertical depuis le mouvement précédent, sens du geste
   * (`"x"`, `"y"`, ou `null` tant qu'il n'est pas décidé), déplacement du milieu et milieu (clientX, clientY) ; `onEnd()` : tous les doigts sont levés.
   */
  constructor(el, { onStart, onMove, onEnd } = {}) {
    this.pts = new Map();
    this.active = false;
    this.prev = null;
    const opts = { capture: true };
    const eat = (e) => {
      e.stopImmediatePropagation();
      e.preventDefault();
    };
    el.addEventListener(
      "pointerdown",
      (e) => {
        if (e.pointerType !== "touch") return;
        this.pts.set(e.pointerId, { x: e.clientX, y: e.clientY });
        if (this.pts.size >= 2) {
          if (!this.active) {
            this.active = true;
            onStart?.();
          }
          this.prev = this.#pair();
          this.start = this.prev;
          this.mode = null;
          eat(e);
        }
      },
      opts,
    );
    el.addEventListener(
      "pointermove",
      (e) => {
        if (e.pointerType !== "touch" || !this.pts.has(e.pointerId)) return;
        this.pts.set(e.pointerId, { x: e.clientX, y: e.clientY });
        if (!this.active) return;
        eat(e);
        if (this.pts.size < 2) return;
        const [a, b] = this.#pair();
        const [pa, pb] = this.prev;
        this.prev = [a, b];
        this.mode ??= pinchMode(this.start[0], this.start[1], a, b);
        onMove?.({
          factor: horizontalFactor(pa, pb, a, b),
          fy: verticalFactor(pa, pb, a, b),
          mode: this.mode,
          dx: (a.x + b.x) / 2 - (pa.x + pb.x) / 2,
          dy: (a.y + b.y) / 2 - (pa.y + pb.y) / 2,
          x: (a.x + b.x) / 2,
          y: (a.y + b.y) / 2,
        });
      },
      opts,
    );
    const up = (e) => {
      if (e.pointerType !== "touch" || !this.pts.has(e.pointerId)) return;
      this.pts.delete(e.pointerId);
      if (!this.active) return;
      eat(e);
      if (this.pts.size === 0) {
        this.active = false;
        onEnd?.();
      } else if (this.pts.size >= 2) this.prev = this.#pair();
    };
    el.addEventListener("pointerup", up, opts);
    el.addEventListener("pointercancel", up, opts);
  }

  #pair() {
    const [a, b] = [...this.pts.values()];
    return [a, b];
  }
}
