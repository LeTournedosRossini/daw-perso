// Éditeur de fondu : fenêtre qui montre les courbes d'un fondu enchaîné (ou d'un fondu seul) et laisse régler chacune :
// réglages rapides (linéaire, puissance constante, S, lente, rapide) ou réglage continu — **courbure** de concave à convexe,
// **adoucissement en S**, **puissance constante**. Pendant qu'on tire un curseur, la courbe se redessine tout de suite
// (aperçu local) ; la commande n'est envoyée au moteur qu'au relâchement (un seul pas d'annulation). La fenêtre se redessine
// ensuite sur ce que le moteur confirme (événement `project`), donc l'annulation y est visible.

import { PRESETS, DEFAULT_CURVE, fadeGain, crossfadeCurves, curvePoints, presetOf } from "../fades.js";
import { pixelRatio, getScale } from "../ui-scale.js";
import { crossfadeBounds } from "../crossfade.js";
import { FadeWave } from "./fade-wave.js";

const TWO_COL_MIN = 640; // largeur disponible (px CSS) en dessous de laquelle on reste sur une colonne
const TWO_COL_WIDTH = 860;

const css = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
const db = (v) => (v <= 0 ? -Infinity : 20 * Math.log10(v));
const fmtDb = (v) => (!Number.isFinite(v) ? "-∞ dB" : `${v >= 0 ? "+" : ""}${v.toFixed(1)} dB`);
const pct = (v) => `${v > 0 ? "+" : ""}${Math.round(v / 10)} %`;

export class FadeEditor {
  constructor(state, client) {
    this.state = state;
    this.client = client;
    this.dialog = document.getElementById("fade-editor");
    this.target = null;
    this.preview = {}; // courbes en cours de réglage, par côté (`out`, `in`, `single`)
    this.anchor = "center"; // ce qui reste en place quand on change la longueur d'un fondu enchaîné
    this.wave = new FadeWave(document.getElementById("fe-wave"), client, () => this.state.project.sample_rate);
    this.previewing = false;
    this.listen = document.getElementById("fe-listen");
    this.solo = document.getElementById("fe-solo");
    this.margin = document.getElementById("fe-margin");
    this.#loadPreviewOptions();
    client.on("preview", (e) => {
      this.previewing = e.active;
      this.listen.setAttribute("aria-pressed", String(e.active));
      this.listen.textContent = e.active ? "■ Arrêter" : "▶ Écouter";
      if (!e.active) this.wave.setPlayhead(null);
    });
    client.on("position", (e) => {
      if (this.previewing && this.dialog.open) this.wave.setPlayhead(e.pos);
    });
    this.listen.addEventListener("click", () => (this.previewing ? this.client.send({ type: "stop_preview" }) : this.#startPreview()));
    for (const el of [this.solo, this.margin]) {
      el.addEventListener("change", () => {
        this.#savePreviewOptions();
        if (this.previewing) this.#startPreview(); // le moteur garde l'état d'origine de l'utilisateur et change seulement la cible
      });
    }
    this.dialog.addEventListener("close", () => {
      if (this.previewing) this.client.send({ type: "stop_preview" });
      this.wave.clear();
      this.target = null;
      this.preview = {};
    });
  }

  isOpen() {
    return this.dialog.open && this.target !== null;
  }

  /** `target` : `{ kind: "crossfade", outId, inId }` ou `{ kind: "single", clipId, side: "in" | "out" }`. */
  open(target) {
    this.target = target;
    this.preview = {};
    if (!this.dialog.open) this.dialog.showModal();
    this.refresh();
  }

  #loadPreviewOptions() {
    try {
      const o = JSON.parse(localStorage.getItem("daw-fade-preview") ?? "null");
      if (typeof o?.solo === "boolean") this.solo.checked = o.solo;
      if (Number.isFinite(o?.margin)) this.margin.value = String(Math.min(10, Math.max(0, o.margin)));
    } catch {
      // Stockage indisponible : réglages par défaut (piste seule, 1,5 s).
    }
  }

  #savePreviewOptions() {
    try {
      localStorage.setItem("daw-fade-preview", JSON.stringify({ solo: this.solo.checked, margin: Number(this.margin.value) }));
    } catch {
      // Pas de mémorisation possible : les réglages restent valables pour cette ouverture.
    }
  }

  /** Aperçu sonore du fondu affiché : boucle du moteur sur le fondu et sa marge (voir `preview_fade` dans le protocole). */
  #startPreview() {
    const t = this.target;
    if (!t) return;
    const margin = Math.round(Math.min(10, Math.max(0, Number(this.margin.value) || 0)) * 1000);
    const common = { margin_ms: margin, solo: this.solo.checked };
    this.client.send(t.kind === "crossfade" ? { type: "preview_fade", clip_out: t.outId, clip_in: t.inId, ...common } : { type: "preview_fade", clip: t.clipId, side: t.side, ...common });
  }

  #clip(id) {
    for (const n of this.state.project.nodes) {
      const c = n.clips.find((x) => x.id === id);
      if (c) return { clip: c, node: n };
    }
    return null;
  }

  /** Redessine depuis l'état courant ; ferme si les clips ont disparu (supprimés, annulés). */
  refresh() {
    if (!this.isOpen()) return;
    this.preview = {}; // le moteur vient de confirmer : l'état réel remplace les aperçus
    // La fenêtre est reconstruite à chaque confirmation : on retient quel réglage avait le focus pour le lui rendre
    // (sans cela, régler un curseur au clavier perdrait le focus à chaque pas).
    const active = document.activeElement;
    const focus = active && this.dialog.contains(active) ? { group: [...document.querySelectorAll("#fe-groups .group")].findIndex((g) => g.contains(active)), cls: active.className || active.dataset?.preset, preset: active.dataset?.preset } : null;
    const t = this.target;
    const q = (id) => document.getElementById(id);
    const groups = q("fe-groups");
    groups.replaceChildren();
    this.plot = null;
    if (t.kind === "crossfade") {
      const a = this.#clip(t.outId);
      const b = this.#clip(t.inId);
      if (!a || !b) return this.dialog.close();
      q("fe-title").textContent = "Fondu enchaîné";
      q("fe-sub").textContent = `« ${a.clip.name} » sort, « ${b.clip.name} » entre — zone commune de ${(a.clip.fade_out / this.state.project.sample_rate).toFixed(2)} s`;
      const cur = { out: a.clip.fade_out_curve, in: b.clip.fade_in_curve };
      this.#lengthGroup(groups, a.clip, b.clip);
      this.#group(groups, "Clip qui sort", a.clip.name, cur.out, "out", (curve) => this.client.send({ type: "set_crossfade", clip_out: a.clip.id, clip_in: b.clip.id, curve_out: curve }));
      this.#group(groups, "Clip qui entre", b.clip.name, cur.in, "in", (curve) => this.client.send({ type: "set_crossfade", clip_out: a.clip.id, clip_in: b.clip.id, curve_in: curve }));
      this.wave.set({ kind: "crossfade", a: a.clip, b: b.clip, colorA: a.node.color, colorB: b.node.color, curves: () => ({ out: this.preview.out ?? cur.out, in: this.preview.in ?? cur.in }) });
      this.plot = () => {
        this.#plotCross(this.preview.out ?? cur.out, this.preview.in ?? cur.in, a.node.color, css("--solo") || "#ffd23f", a.clip.name, b.clip.name);
        this.wave.draw();
      };
    } else {
      const c = this.#clip(t.clipId);
      if (!c) return this.dialog.close();
      const side = t.side === "in" ? "d'entrée" : "de sortie";
      const cur = t.side === "in" ? c.clip.fade_in_curve : c.clip.fade_out_curve;
      const len = t.side === "in" ? c.clip.fade_in : c.clip.fade_out;
      q("fe-title").textContent = `Fondu ${side}`;
      q("fe-sub").textContent = `« ${c.clip.name} » — ${(len / this.state.project.sample_rate).toFixed(2)} s`;
      this.#group(groups, `Courbe du fondu ${side}`, c.clip.name, cur, "single", (curve) => this.client.send({ type: "set_clip_fades", clip: c.clip.id, [t.side === "in" ? "curve_in" : "curve_out"]: curve }));
      this.wave.set({ kind: "single", clip: c.clip, side: t.side, color: c.node.color, curve: () => this.preview.single ?? cur });
      this.plot = () => {
        this.#plotSingle(this.preview.single ?? cur, t.side, c.node.color);
        this.wave.draw();
      };
    }
    this.#layout();
    this.plot();
    if (focus && focus.group >= 0) {
      const grp = document.querySelectorAll("#fe-groups .group")[focus.group];
      const el = focus.preset ? grp?.querySelector(`[data-preset="${focus.preset}"]`) : grp?.querySelector(`.${String(focus.cls).split(" ")[0]}`);
      el?.focus();
    }
  }

  /** Longueur du fondu enchaîné : curseur (aperçu du texte pendant le geste, une commande au relâchement) et ancrage. */
  #lengthGroup(parent, a, b) {
    const sr = this.state.project.sample_rate;
    const ms = (n) => `${Math.round((n / sr) * 1000)} ms`;
    const len = a.start + a.len - b.start;
    const bounds = crossfadeBounds(a, b, this.anchor);
    if (!bounds) return; // clips qui ne forment plus une paire : la fenêtre se fermera au prochain rafraîchissement
    const g = document.createElement("div");
    g.className = "group";
    g.innerHTML = `
      <h3>Longueur du fondu <span>(rogne les clips, leur contenu ne bouge pas)</span></h3>
      <div class="sl">
        <span class="name">Longueur</span>
        <span class="ends"><i>${ms(bounds.min)}</i><i>max ${ms(bounds.max)}</i></span>
        <input class="len" type="range" min="${bounds.min}" max="${bounds.max}" step="1" value="${len}" aria-label="Longueur du fondu enchaîné">
        <output>${ms(len)}</output>
      </div>
      <div class="anchors" role="radiogroup" aria-label="Ce qui reste en place">
        ${[["center", "Centré", "Le milieu du fondu reste en place : les deux clips s'allongent ou se raccourcissent de chaque côté"], ["start", "Début fixe", "Le début du fondu (début du second clip) reste en place : seule la fin du premier bouge"], ["end", "Fin fixe", "La fin du fondu (fin du premier clip) reste en place : seul le début du second bouge"]]
          .map(([v, label, tip]) => `<label title="${tip}"><input type="radio" name="fe-anchor" value="${v}" ${v === this.anchor ? "checked" : ""}><span>${label}</span></label>`)
          .join("")}
      </div>`;
    const range = g.querySelector("input.len");
    const out = g.querySelector("output");
    range.addEventListener("input", () => (out.textContent = ms(Number(range.value))));
    range.addEventListener("change", () => this.client.send({ type: "set_crossfade_length", clip_out: a.id, clip_in: b.id, length: Number(range.value), anchor: this.anchor }));
    g.querySelectorAll('input[name="fe-anchor"]').forEach((r) =>
      r.addEventListener("change", () => {
        this.anchor = r.value;
        this.refresh();
      }),
    );
    parent.append(g);
  }

  /** Une colonne si tout tient dans la hauteur de la fenêtre, sinon deux (courbes à gauche, réglages à droite) si la largeur le permet. */
  #layout() {
    const form = this.dialog.querySelector("form");
    const zoom = getScale() / 100;
    const availH = window.innerHeight / zoom - 16;
    const availW = window.innerWidth / zoom - 16;
    form.classList.remove("two-col");
    this.dialog.style.removeProperty("--fe-width");
    if (form.offsetHeight <= availH || availW < TWO_COL_MIN) return;
    form.classList.add("two-col");
    this.dialog.style.setProperty("--fe-width", `${Math.min(TWO_COL_WIDTH, availW)}px`);
  }

  /** Redisposition quand la fenêtre ou la taille de l'interface change. */
  relayout() {
    if (this.isOpen()) this.refresh();
  }

  /** Un groupe de réglages : réglages rapides, puis curseurs de courbure et d'adoucissement et case « puissance constante ». */
  #group(parent, title, clipName, current, key, commit) {
    const cur = { ...DEFAULT_CURVE, ...current };
    const g = document.createElement("div");
    g.className = "group";
    g.innerHTML = `
      <h3>${title} <span>« ${clipName.replace(/[<&]/g, "")} »</span></h3>
      <div class="shapes" role="radiogroup" aria-label="${title} : réglages rapides"></div>
      <div class="sliders">
        <label class="sl"><span class="name">Courbure</span><span class="ends"><i>concave</i><i>convexe</i></span>
          <input type="range" class="bend" min="-100" max="100" step="1" aria-label="Courbure de ${title.toLowerCase()} : de concave à convexe"><output></output></label>
        <label class="sl"><span class="name">Adoucissement en S</span><span class="ends"><i>droit</i><i>en S</i></span>
          <input type="range" class="smooth" min="0" max="100" step="1" aria-label="Adoucissement en S de ${title.toLowerCase()}"><output></output></label>
        <label class="chk"><input type="checkbox" class="power"> Puissance constante <span>(racine : volume perçu constant)</span></label>
      </div>`;
    const row = g.querySelector(".shapes");
    const bend = g.querySelector(".bend");
    const smooth = g.querySelector(".smooth");
    const power = g.querySelector(".power");
    const outs = g.querySelectorAll("output");
    const setControls = (c) => {
      bend.value = String(c.bend / 10);
      smooth.value = String(c.smooth / 10);
      power.checked = c.shape === "equal_power";
      outs[0].textContent = pct(c.bend);
      outs[1].textContent = `${Math.round(c.smooth / 10)} %`;
      for (const b of row.children) b.setAttribute("aria-checked", String(b.dataset.preset === presetOf(c)));
    };
    const read = () => ({ shape: power.checked ? "equal_power" : "linear", bend: Math.round(Number(bend.value) * 10), smooth: Math.round(Number(smooth.value) * 10) });
    for (const p of PRESETS) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "shape";
      b.dataset.preset = p.id;
      b.setAttribute("role", "radio");
      b.title = p.hint;
      const c = document.createElement("canvas");
      b.append(c, document.createTextNode(p.label));
      b.addEventListener("click", () => {
        setControls(p.curve);
        commit({ ...p.curve });
      });
      row.append(b);
      this.#miniCurve(c, p.curve);
    }
    for (const el of [bend, smooth, power]) {
      el.addEventListener("input", () => {
        const c = read();
        setControls(c);
        this.preview[key] = c;
        this.plot?.();
      });
      el.addEventListener("change", () => commit(read()));
    }
    setControls(cur);
    parent.append(g);
  }

  #miniCurve(canvas, curve) {
    const dpr = pixelRatio();
    const w = 44;
    const h = 24;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    const g = canvas.getContext("2d");
    g.scale(dpr, dpr);
    g.strokeStyle = css("--accent") || "#4aa3ff";
    g.lineWidth = 1.7;
    g.beginPath();
    curvePoints(curve, 24).forEach(([x, y], i) => (i ? g.lineTo(3 + x * (w - 6), h - 3 - y * (h - 6)) : g.moveTo(3 + x * (w - 6), h - 3 - y * (h - 6))));
    g.stroke();
  }

  #canvas() {
    const c = document.getElementById("fe-canvas");
    const dpr = pixelRatio();
    const w = Math.max(200, Math.round(c.clientWidth));
    const h = Math.round(c.clientHeight);
    c.width = w * dpr;
    c.height = h * dpr;
    const g = c.getContext("2d");
    g.scale(dpr, dpr);
    return { g, w, h };
  }

  #frame(g, w, h, yMax) {
    const pad = { l: 34, r: 10, t: 8, b: 20 };
    const X = (x) => pad.l + x * (w - pad.l - pad.r);
    const Y = (v) => h - pad.b - (v / yMax) * (h - pad.t - pad.b);
    g.font = "10px system-ui, sans-serif";
    g.textBaseline = "middle";
    g.fillStyle = css("--muted");
    g.strokeStyle = css("--line");
    g.lineWidth = 1;
    for (const v of [0, 0.5, 1]) {
      g.beginPath();
      g.moveTo(pad.l, Y(v));
      g.lineTo(w - pad.r, Y(v));
      g.stroke();
      g.textAlign = "right";
      g.fillText(v === 1 ? "100 %" : v === 0 ? "0" : "50 %", pad.l - 5, Y(v));
    }
    g.textAlign = "center";
    for (const x of [0, 0.5, 1]) g.fillText(x === 0 ? "début" : x === 1 ? "fin" : "milieu", X(x), h - 8);
    return { X, Y };
  }

  #line(g, pts, X, Y, color, width = 2, dash = []) {
    g.strokeStyle = color;
    g.lineWidth = width;
    g.setLineDash(dash);
    g.beginPath();
    pts.forEach(([x, v], i) => (i ? g.lineTo(X(x), Y(v)) : g.moveTo(X(x), Y(v))));
    g.stroke();
    g.setLineDash([]);
  }

  #legend(items) {
    document.getElementById("fe-legend").innerHTML = items.map(([c, label, dash]) => `<span><i class="${dash ? "dash" : ""}" style="--c:${c}"></i>${label}</span>`).join("");
  }

  #plotCross(curveOut, curveIn, colorOut, colorIn, nameOut, nameIn) {
    const { g, w, h } = this.#canvas();
    const { X, Y } = this.#frame(g, w, h, 1.5);
    const curves = crossfadeCurves(curveOut, curveIn);
    this.#line(g, curves.map((c) => [c.x, c.out]), X, Y, colorOut, 2.4);
    this.#line(g, curves.map((c) => [c.x, c.in]), X, Y, colorIn, 2.4);
    this.#line(g, curves.map((c) => [c.x, c.sum]), X, Y, css("--text"), 1.3, [5, 4]);
    this.#line(g, curves.map((c) => [c.x, c.power]), X, Y, css("--muted"), 1.3, [2, 3]);
    this.#legend([[colorOut, `${nameOut} (sortie)`], [colorIn, `${nameIn} (entrée)`], [css("--text"), "somme des gains", true], [css("--muted"), "volume perçu, signaux sans rapport", true]]);
    const dev = (key) => {
      const v = curves.map((c) => db(c[key]));
      return [Math.min(...v), Math.max(...v)];
    };
    const [sMin, sMax] = dev("sum");
    const [pMin, pMax] = dev("power");
    const flat = (lo, hi) => (Math.abs(lo) < 0.15 && Math.abs(hi) < 0.15 ? "constant" : `de ${fmtDb(lo)} à ${fmtDb(hi)}`);
    document.getElementById("fe-note").textContent = `Signaux semblables (même prise) : niveau ${flat(sMin, sMax)}. Signaux sans rapport : volume perçu ${flat(pMin, pMax)}.`;
  }

  #plotSingle(curve, side, color) {
    const { g, w, h } = this.#canvas();
    const { X, Y } = this.#frame(g, w, h, 1.1);
    // Le long du temps : un fondu d'entrée monte de 0 à 1, un fondu de sortie descend de 1 à 0.
    const pts = Array.from({ length: 49 }, (_, i) => {
      const x = i / 48;
      return [x, fadeGain(curve, side === "in" ? x : 1 - x)];
    });
    this.#line(g, pts, X, Y, color, 2.6);
    this.#line(g, [[0, side === "in" ? 0 : 1], [1, side === "in" ? 1 : 0]], X, Y, css("--muted"), 1, [3, 4]);
    this.#legend([[color, "gain du fondu"], [css("--muted"), "rampe linéaire (repère)", true]]);
    document.getElementById("fe-note").textContent = "En chevauchant deux clips, un fondu enchaîné se crée tout seul : double-cliquez sur sa zone commune pour régler les deux courbes ensemble.";
  }
}
