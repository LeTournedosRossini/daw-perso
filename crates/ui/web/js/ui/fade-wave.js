// Vue « formes d'onde » de l'éditeur de fondu : le ou les clips autour du fondu, avec la matière que le rognage cacherait
// (estompée), les courbes de fondu et le gain réellement entendu, et des poignées qu'on tire pour régler la longueur et
// voir tout de suite le résultat. Une seule commande part au relâchement (un pas d'annulation).

import { fitWindow, windowFits, crossfadeHandleDrag, crossfadeCenterDrag, singleHandleDrag, fadeGainAt } from "../fade-view.js";
import { pixelRatio, canvasPoint } from "../ui-scale.js";

const css = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
const GRAB = 8; // distance (px CSS) à laquelle une poignée se saisit
const RULER = 14;
const TOP_BAND = 20; // hauteur (sous la règle) où la poignée du haut se saisit
const MAX_CACHE = 64;

export class FadeWave {
  /** `sampleRate` : fonction qui donne la fréquence du projet (elle peut changer d'un projet à l'autre). */
  constructor(canvas, client, sampleRate) {
    this.canvas = canvas;
    this.client = client;
    this.sampleRate = sampleRate;
    this.model = null; // voir `set`
    this.win = null;
    this.key = null;
    this.peaks = new Map(); // `clip|from|to` → colonnes [min, max] ou « pending »
    this.drag = null; // { handle, preview }
    this.hover = null;
    this.playhead = null; // position de lecture (échantillons) pendant l'écoute, sinon `null`
    client.on("peaks", (e) => {
      const k = `${e.clip}|${e.from}|${e.to}`;
      if (this.peaks.get(k) === "pending") {
        this.peaks.set(k, e.columns);
        this.draw();
      }
    });
    canvas.addEventListener("pointerdown", (e) => this.#down(e));
    canvas.addEventListener("pointermove", (e) => this.#move(e));
    canvas.addEventListener("pointerup", (e) => this.#up(e, true));
    canvas.addEventListener("pointercancel", (e) => this.#up(e, false));
    canvas.addEventListener("pointerleave", () => {
      if (!this.drag && this.hover) {
        this.hover = null;
        this.draw();
      }
    });
  }

  /**
   * Nouveau contenu : `{ kind: "crossfade", a, b, colorA, colorB, curves }` ou `{ kind: "single", clip, side, color, curve }`
   * (`a`, `b`, `clip` : clips tels que le projet les décrit ; `curves()` / `curve()` : courbes à dessiner, aperçu compris).
   */
  set(model) {
    this.model = model;
    this.drag = null; // le moteur vient de confirmer (ou refuser) : l'aperçu du geste n'a plus lieu d'être
    const [zs, ze] = this.#zone();
    const key = model.kind === "crossfade" ? `x${model.a.id}-${model.b.id}` : `s${model.clip.id}${model.side}`;
    if (key !== this.key || !this.win || !windowFits(this.win, zs, ze)) this.win = fitWindow(zs, ze, this.sampleRate());
    this.key = key;
    this.draw();
  }

  /** Curseur de lecture (échantillons) ; `null` l'efface. Ne redessine que si quelque chose a changé. */
  setPlayhead(pos) {
    if (pos === this.playhead) return;
    this.playhead = pos;
    this.draw();
  }

  clear() {
    this.playhead = null;
    this.model = null;
    this.drag = null;
    this.key = null;
    this.win = null;
  }

  /** Zone du fondu `[début, fin[` (échantillons), avec l'aperçu du geste en cours. */
  #zone() {
    const m = this.model;
    const p = this.drag?.preview;
    if (m.kind === "crossfade") {
      const bs = p ? p.bStart : m.b.start;
      const ae = p ? p.aEnd : m.a.start + m.a.len;
      return [bs, ae];
    }
    const len = p ? p.length : m.side === "in" ? m.clip.fade_in : m.clip.fade_out;
    return m.side === "in" ? [m.clip.start, m.clip.start + len] : [m.clip.start + m.clip.len - len, m.clip.start + m.clip.len];
  }

  /** Bords visibles des clips pendant le geste : `[début, fin]` de chaque voie. */
  #extents() {
    const m = this.model;
    const [zs, ze] = this.#zone();
    return m.kind === "crossfade" ? [[m.a.start, ze], [zs, m.b.start + m.b.len]] : [[m.clip.start, m.clip.start + m.clip.len]];
  }

  #lanes() {
    const m = this.model;
    const ext = this.#extents();
    if (m.kind === "crossfade") {
      const { out, in: inn } = m.curves();
      return [
        { clip: m.a, color: m.colorA, role: "out", curve: out, from: ext[0][0], to: ext[0][1], name: m.a.name },
        { clip: m.b, color: m.colorB, role: "in", curve: inn, from: ext[1][0], to: ext[1][1], name: m.b.name },
      ];
    }
    return [{ clip: m.clip, color: m.color, role: m.side, curve: m.curve(), from: ext[0][0], to: ext[0][1], name: m.clip.name }];
  }

  #x(t, w) {
    return ((t - this.win.start) / (this.win.end - this.win.start)) * w;
  }

  #sample(x, w) {
    return this.win.start + (x / w) * (this.win.end - this.win.start);
  }

  /** Colonnes de crêtes de `clip` sur `[from, to[` (demandées au moteur si besoin), ou `null` tant qu'elles n'ont pas répondu. */
  #columns(clip, from, to, cols) {
    const k = `${clip.id}|${from}|${to}`;
    const hit = this.peaks.get(k);
    if (hit && hit !== "pending") return hit;
    if (!hit) {
      this.peaks.set(k, "pending");
      if (this.peaks.size > MAX_CACHE) for (const [kk, v] of this.peaks) if (v !== "pending" && this.peaks.size > MAX_CACHE * 0.75) this.peaks.delete(kk);
      this.client.send({ type: "get_peaks", clip: clip.id, from, to, columns: Math.max(1, cols) });
    }
    return null;
  }

  draw() {
    const c = this.canvas;
    if (!this.model || !this.win) return;
    const dpr = pixelRatio();
    const w = Math.max(200, Math.round(c.clientWidth));
    const h = Math.round(c.clientHeight);
    if (h <= 0) return;
    c.width = w * dpr;
    c.height = h * dpr;
    const g = c.getContext("2d");
    g.scale(dpr, dpr);
    const bg = css("--bg") || "#101319";
    const text = css("--text") || "#e6e9ef";
    const muted = css("--muted") || "#8a93a3";
    const accent = css("--accent") || "#4aa3ff";
    g.fillStyle = bg;
    g.fillRect(0, 0, w, h);

    const sr = this.sampleRate();
    const [zs, ze] = this.#zone();
    const lanes = this.#lanes();
    const gap = 4;
    const laneH = (h - RULER - gap * (lanes.length - 1) - 3) / lanes.length;

    // Règle en millisecondes, relative au début du fondu.
    g.fillStyle = muted;
    g.font = "10px system-ui, sans-serif";
    g.textBaseline = "top";
    const spanMs = ((this.win.end - this.win.start) / sr) * 1000;
    const stepMs = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000].find((s) => (s / spanMs) * w >= 56) ?? 2000;
    for (let ms = Math.ceil((((this.win.start - zs) / sr) * 1000) / stepMs) * stepMs; zs + (ms / 1000) * sr <= this.win.end; ms += stepMs) {
      const x = this.#x(zs + (ms / 1000) * sr, w);
      g.fillRect(Math.round(x), RULER - 3, 1, 3);
      g.fillText(ms === 0 ? "0" : `${ms > 0 ? "+" : "−"}${Math.abs(ms)} ms`, x + 3, 1);
    }

    // Zone du fondu.
    const xs = this.#x(zs, w);
    const xe = this.#x(ze, w);
    g.fillStyle = accent;
    g.globalAlpha = 0.13;
    g.fillRect(xs, RULER, Math.max(1, xe - xs), h - RULER);
    g.globalAlpha = 1;

    lanes.forEach((lane, i) => {
      const top = RULER + i * (laneH + gap);
      this.#drawLane(g, lane, top, laneH, w, zs, ze, { text, muted });
    });

    // Poignées.
    const m = this.model;
    const handles = m.kind === "crossfade" ? [["start", xs], ["end", xe]] : [[m.side === "in" ? "end" : "start", m.side === "in" ? xe : xs]];
    for (const [name, x] of handles) {
      const active = this.drag?.handle === name || this.hover === name;
      g.strokeStyle = accent;
      g.lineWidth = active ? 2.5 : 1.5;
      g.beginPath();
      g.moveTo(Math.round(x) + 0.5, RULER);
      g.lineTo(Math.round(x) + 0.5, h);
      g.stroke();
      const gy = RULER + (h - RULER) / 2;
      g.fillStyle = active ? accent : bg;
      g.strokeStyle = accent;
      g.lineWidth = 1.5;
      g.beginPath();
      g.roundRect(x - 5, gy - 13, 10, 26, 4);
      g.fill();
      g.stroke();
      g.strokeStyle = active ? bg : accent;
      g.lineWidth = 1;
      g.beginPath();
      g.moveTo(x - 1.5, gy - 6);
      g.lineTo(x - 1.5, gy + 6);
      g.moveTo(x + 1.5, gy - 6);
      g.lineTo(x + 1.5, gy + 6);
      g.stroke();
    }

    // Poignée du haut (fondu enchaîné) : tirée de haut en bas, elle élargit ou rétrécit le fondu autour de son milieu.
    if (m.kind === "crossfade") {
      const cx = (xs + xe) / 2;
      const active = this.drag?.handle === "center" || this.hover === "center";
      g.fillStyle = active ? accent : bg;
      g.strokeStyle = accent;
      g.lineWidth = 1.5;
      g.beginPath();
      g.roundRect(cx - 13, RULER + 3, 26, 11, 4);
      g.fill();
      g.stroke();
      g.strokeStyle = active ? bg : accent;
      g.lineWidth = 1;
      g.beginPath();
      g.moveTo(cx - 6, RULER + 7);
      g.lineTo(cx + 6, RULER + 7);
      g.moveTo(cx - 6, RULER + 10);
      g.lineTo(cx + 6, RULER + 10);
      g.stroke();
    }

    // Curseur de lecture.
    if (this.playhead !== null && this.playhead >= this.win.start && this.playhead <= this.win.end) {
      const px = Math.round(this.#x(this.playhead, w)) + 0.5;
      // Trait clair avec un liseré sombre : visible sur toutes les couleurs de clips.
      g.strokeStyle = bg;
      g.globalAlpha = 0.7;
      g.lineWidth = 5;
      g.beginPath();
      g.moveTo(px, 0);
      g.lineTo(px, h);
      g.stroke();
      g.globalAlpha = 1;
      g.strokeStyle = text;
      g.lineWidth = 2;
      g.beginPath();
      g.moveTo(px, 0);
      g.lineTo(px, h);
      g.stroke();
      g.fillStyle = text;
      g.beginPath();
      g.moveTo(px - 5, 0);
      g.lineTo(px + 5, 0);
      g.lineTo(px, 7);
      g.closePath();
      g.fill();
    }

    // Longueur du fondu.
    const lenMs = Math.round(((ze - zs) / sr) * 1000);
    g.fillStyle = text;
    g.font = "600 11px system-ui, sans-serif";
    g.textAlign = "center";
    g.textBaseline = "bottom";
    g.fillText(`${lenMs} ms`, Math.min(w - 30, Math.max(30, (xs + xe) / 2)), h - 3);
    g.textAlign = "start";
  }

  #drawLane(g, lane, top, laneH, w, zs, ze, { text, muted }) {
    const { clip } = lane;
    const mid = top + laneH / 2;
    const half = laneH / 2 - 2;
    const dataLo = clip.start - clip.offset;
    const dataHi = dataLo + clip.source_len;
    // Fond de la voie et étendue visible du clip.
    g.fillStyle = muted;
    g.globalAlpha = 0.08;
    g.fillRect(0, top, w, laneH);
    g.globalAlpha = 1;
    const x0 = this.#x(lane.from, w);
    const x1 = this.#x(lane.to, w);
    g.fillStyle = lane.color;
    g.globalAlpha = 0.16;
    g.fillRect(Math.max(0, x0), top, Math.max(0, Math.min(w, x1) - Math.max(0, x0)), laneH);
    g.globalAlpha = 1;

    // Formes d'onde : matière du fichier visible dans la fenêtre ; pleine dans le clip (et réduite par le fondu), estompée hors du clip.
    const from = Math.max(this.win.start, dataLo);
    const to = Math.min(this.win.end, dataHi);
    if (to > from) {
      const px = Math.max(1, Math.round(((to - from) / (this.win.end - this.win.start)) * w));
      const cols = this.#columns(clip, Math.round(from), Math.round(to), px);
      if (cols) {
        const per = (to - from) / cols.length;
        for (let i = 0; i < cols.length; i++) {
          const t = from + (i + 0.5) * per;
          const [lo, hi] = cols[i];
          const x = Math.floor(this.#x(from + i * per, w));
          const xw = Math.max(1, Math.ceil(this.#x(from + (i + 1) * per, w)) - x);
          const inside = t >= lane.from && t < lane.to;
          const yTop = mid - hi * half;
          const bar = Math.max(1, (hi - lo) * half);
          g.fillStyle = lane.color;
          if (!inside) {
            g.globalAlpha = 0.28;
            g.fillRect(x, yTop, xw, bar);
            continue;
          }
          // Matière entière en transparence, partie entendue (après le gain du fondu) en couleur pleine.
          const gain = this.#gainAt(lane, t, zs, ze);
          g.globalAlpha = 0.3;
          g.fillRect(x, yTop, xw, bar);
          g.globalAlpha = 1;
          g.fillRect(x, mid - hi * half * gain, xw, Math.max(1, (hi - lo) * half * gain));
        }
        g.globalAlpha = 1;
      }
    }

    // Courbe du fondu sur la voie (1 en haut, 0 en bas), et bords du clip.
    g.strokeStyle = text;
    g.lineWidth = 1.5;
    g.beginPath();
    const steps = 48;
    let started = false;
    for (let k = 0; k <= steps; k++) {
      const t = zs + ((ze - zs) * k) / steps;
      const x = this.#x(t, w);
      const y = top + 2 + (1 - this.#gainAt(lane, t, zs, ze)) * (laneH - 4);
      if (!started) {
        g.moveTo(x, y);
        started = true;
      } else {
        g.lineTo(x, y);
      }
    }
    g.stroke();
    g.strokeStyle = lane.color;
    g.lineWidth = 1.5;
    for (const x of [x0, x1]) {
      if (x >= 0 && x <= w) {
        g.beginPath();
        g.moveTo(Math.round(x) + 0.5, top);
        g.lineTo(Math.round(x) + 0.5, top + laneH);
        g.stroke();
      }
    }
    g.fillStyle = text;
    g.font = "11px system-ui, sans-serif";
    g.textBaseline = "top";
    g.fillText(lane.name, Math.max(4, Math.min(w - 40, x0 + 4)), top + 2);
  }

  /** Gain entendu à l'échantillon `t` sur cette voie (1 hors de la zone du fondu côté « plein »). */
  #gainAt(lane, t, zs, ze) {
    if (lane.role === "out") return t >= ze ? 0 : t < zs ? 1 : fadeGainAt("out", lane.curve, zs, ze, t);
    if (lane.role === "in") return t < zs ? 0 : t >= ze ? 1 : fadeGainAt("in", lane.curve, zs, ze, t);
    return 1;
  }

  // ---- poignées ----

  #handleAt(x, w, y = Infinity) {
    if (!this.model) return null;
    const [zs, ze] = this.#zone();
    const m = this.model;
    if (m.kind === "crossfade" && y <= RULER + TOP_BAND && Math.abs((this.#x(zs, w) + this.#x(ze, w)) / 2 - x) <= 16) return "center";
    const cands = m.kind === "crossfade" ? [["start", this.#x(zs, w)], ["end", this.#x(ze, w)]] : [[m.side === "in" ? "end" : "start", this.#x(m.side === "in" ? ze : zs, w)]];
    const near = cands.filter(([, hx]) => Math.abs(hx - x) <= GRAB).sort((p, q) => Math.abs(p[1] - x) - Math.abs(q[1] - x));
    return near[0]?.[0] ?? null;
  }

  #width() {
    return Math.max(200, Math.round(this.canvas.clientWidth));
  }

  #down(e) {
    const { x, y } = canvasPoint(this.canvas, e);
    const handle = this.#handleAt(x, this.#width(), y);
    if (!handle) return;
    this.canvas.setPointerCapture(e.pointerId);
    const [zs, ze] = this.#zone();
    this.drag = { handle, preview: null, y0: y, length0: ze - zs };
    e.preventDefault();
  }

  #move(e) {
    const w = this.#width();
    const { x, y } = canvasPoint(this.canvas, e);
    if (!this.drag) {
      const hover = this.#handleAt(x, w, y);
      this.canvas.style.cursor = hover === "center" ? "ns-resize" : hover ? "ew-resize" : "default";
      if (hover !== this.hover) {
        this.hover = hover;
        this.draw();
      }
      return;
    }
    const sample = this.#sample(x, w);
    const m = this.model;
    if (this.drag.handle === "center") {
      const spp = (this.win.end - this.win.start) / w;
      const d = crossfadeCenterDrag(m.a, m.b, this.drag.length0, this.drag.y0 - y, spp);
      if (d) this.drag.preview = d;
    } else if (m.kind === "crossfade") {
      const d = crossfadeHandleDrag(m.a, m.b, this.drag.handle, sample);
      if (d) this.drag.preview = d;
    } else {
      this.drag.preview = { length: singleHandleDrag(m.clip, m.side, sample) };
    }
    this.draw();
  }

  #up(e, commit) {
    const d = this.drag;
    if (!d) return;
    if (this.canvas.hasPointerCapture?.(e.pointerId)) this.canvas.releasePointerCapture(e.pointerId);
    const m = this.model;
    const p = d.preview;
    if (!commit || !p) {
      this.drag = null;
      this.draw();
      return;
    }
    // L'aperçu reste affiché jusqu'à la réponse du moteur (`set` le remplace) ; un refus le laisse : on le retire peu après.
    const same = m.kind === "crossfade" ? p.bStart === m.b.start && p.aEnd === m.a.start + m.a.len : p.length === (m.side === "in" ? m.clip.fade_in : m.clip.fade_out);
    if (same) {
      this.drag = null;
      this.draw();
      return;
    }
    const cmd =
      m.kind === "crossfade"
        ? { type: "set_crossfade_length", clip_out: m.a.id, clip_in: m.b.id, length: p.length, anchor: p.anchor }
        : { type: "set_clip_fades", clip: m.clip.id, [m.side === "in" ? "fade_in" : "fade_out"]: p.length };
    this.client.send(cmd);
    d.committed = true;
    setTimeout(() => {
      if (this.drag === d) {
        this.drag = null;
        this.draw();
      }
    }, 1500);
  }
}

