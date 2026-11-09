// Fenêtre de calage (grande, dans l'idée du plugin) : le guide, le double d'origine et le double recalé en formes d'onde **l'une sur l'autre**
// sur un même axe de temps, puis les consonnes, le décalage appliqué et la courbe de force (points à la souris ou au doigt). On écoute
// le guide, l'original, le recalé ou guide + recalé (boucle sur la fenêtre du guide, sans toucher à la prise active ni à l'historique).
// À gauche : le guide et la liste des doubles (une entrée par piste) ; un clic sur un double montre ses formes d'onde et ses réglages
// (chaque double a les siens). Les calculs sont dans `align-model.js`.

import {
  ANALYSIS_KEYS, addConsonant, addPoint, alignProgressText, audioClips, cleanConsonants, clipById, doublesFor, doubleTimeOf, editConsonant, guideInfo, guideTimeOf, maxShiftMs, movePoint, originalsOn, parseSettings,
  removePoint, sampleX, strengthAt, stringifySettings, xSample,
} from "../align-model.js";
import { TouchPinch } from "../touch-pinch.js";
import { pixelRatio } from "../ui-scale.js";
import { snapSpp } from "./timeline.js";
import { clipLabel, css, h, loadPref, savePref, settingsFields } from "./align-fields.js";

const STEP = 2 ** 0.25; // zoom par crans de quart d'octave : les tuiles de crêtes restent réutilisables
const LABEL_W = 78;
const ROWS = [
  { id: "ruler", h: 18 },
  { id: "guide", h: 62, label: "Guide" },
  { id: "orig", h: 62, label: "Double" },
  { id: "aligned", h: 62, label: "Recalé" },
  { id: "cons", h: 22, label: "Consonnes" },
  { id: "shift", h: 56, label: "Décalage" },
  { id: "force", h: 66, label: "Force" },
];
const WHATS = [
  ["guide", "Guide"],
  ["original", "Original"],
  ["aligned", "Recalé"],
  ["guide_original", "Guide + original"],
  ["guide_aligned", "Guide + recalé"],
];

/** Positions verticales des rangées : `{ id → { top, h } }` et hauteur totale. */
function layout() {
  let y = 0;
  const rows = {};
  for (const r of ROWS) {
    rows[r.id] = { top: y, h: r.h };
    y += r.h + 2;
  }
  return { rows, total: y };
}
const LAYOUT = layout();

export class AlignWindow {
  /** `peaks` : le cache de crêtes de l'application (les formes d'onde y sont demandées par tuiles). */
  constructor({ state, client, peaks, controller }) {
    this.state = state;
    this.client = client;
    this.peaks = peaks;
    this.controller = controller;
    this.guideId = null;
    this.checked = new Set(); // pistes (nœuds) cochées pour « Caler »
    this.sel = null; // piste du double affiché
    this.cfg = new Map(); // nœud → { settings, original, consonants, edited, alignedId }
    this.details = new Map(); // clip calé → dernier `align_detail`
    this.view = { start: 0, spp: 64 };
    this.what = "guide_original";
    this.listening = false;
    this.drag = null;
    this.selectedPoint = -1;
    this.selCons = -1; // consonne sélectionnée sur le tracé
    this.consDrag = null;
    this.consNew = null; // consonne en cours de création (paire d'instants du double)
    this.awaiting = false;
    this.pinchAcc = 1;
    this.#build();
  }

  // ---- ouverture et fermeture -----------------------------------------------------------------------------------------------

  /** Ouvre la fenêtre : le clip principal de la sélection est le guide, les pistes des autres clips audio sélectionnés les doubles. */
  open() {
    const { project, selection, selected } = this.state;
    const clips = audioClips(project).filter((c) => !c.aligned);
    const chosen = clips.filter((c) => selected?.has?.(c.id) || c.id === selection);
    const guide = chosen.find((c) => c.id === selection) ?? chosen[0] ?? clips[0] ?? null;
    this.guideId = guide?.id ?? null;
    // Les autres clips choisis sur la piste du guide s'ajoutent au guide ; ceux des autres pistes sont les doubles (plusieurs clips d'une piste = un double).
    this.guideExtra = chosen.filter((c) => c.id !== this.guideId && c.track === guide?.track).map((c) => c.id);
    this.pick = new Set(chosen.filter((c) => c.id !== this.guideId && !this.guideExtra.includes(c.id)).map((c) => c.id));
    this.checked = new Set();
    const g = guideInfo(project, this.guideId, this.guideExtra);
    const picked = new Set(chosen.filter((c) => !this.guideExtra.includes(c.id) && c.id !== this.guideId).map((c) => c.track));
    for (const d of doublesFor(project, g, this.pick)) if (picked.has(d.track)) this.checked.add(d.node);
    this.sel = null;
    this.dlg.showModal();
    this.#refresh(true);
    this.offs = [
      this.client.on("project", () => this.#refresh(false)),
      this.client.on("align_detail", (d) => this.#onDetail(d)),
      this.client.on("listen", (e) => this.#onListen(e)),
      this.client.on("position", () => this.listening && this.#draw()),
    ];
    this.unPeaks = this.peaks.addListener?.(() => this.#draw());
    new ResizeObserver(() => this.#draw()).observe(this.canvas);
  }

  close() {
    if (this.listening) this.client.send({ type: "stop_listen" });
    this.offs?.forEach((f) => f());
    this.offs = null;
    this.unPeaks?.();
    if (this.dlg.open) this.dlg.close();
  }

  // ---- construction du DOM --------------------------------------------------------------------------------------------------

  #build() {
    const dlg = (this.dlg = h("dialog", "al-win"));
    const head = h("header");
    head.append(h("h2", "", "Caler des prises"));
    const close = h("button", "close", "✕");
    close.type = "button";
    close.setAttribute("aria-label", "Fermer");
    close.addEventListener("click", () => this.close());
    head.append(close);
    const body = h("div", "al-body");
    // Colonne de gauche : le guide et les doubles.
    const side = h("aside", "al-side");
    const group = (title, ...children) => {
      const g = h("section", "al-group");
      g.append(h("h3", "", title), ...children);
      return g;
    };
    this.guideSel = h("select");
    this.guideSel.setAttribute("aria-label", "Guide");
    this.guideSel.addEventListener("change", () => {
      this.guideId = Number(this.guideSel.value);
      this.guideExtra = [];
      this.pick = new Set();
      this.checked = new Set();
      this.sel = null;
      this.#refresh(true);
    });
    this.list = h("div", "al-list");
    this.goBtn = h("button", "primary", "Caler les doubles cochés");
    this.goBtn.type = "button";
    this.goBtn.addEventListener("click", () => this.#align());
    this.cancelBtn = h("button", "", "Annuler le calage");
    this.cancelBtn.type = "button";
    this.cancelBtn.hidden = true;
    this.cancelBtn.addEventListener("click", () => this.client.send({ type: "cancel_align" }));
    this.progress = h("p", "al-progress");
    this.progress.setAttribute("aria-live", "polite");
    // Quelle prise la timeline joue : l'original ou le recalé (la prise active du groupe de prises du double).
    this.useBox = h("div", "al-use");
    this.useBox.setAttribute("role", "radiogroup");
    this.useBox.setAttribute("aria-label", "Prise utilisée dans la timeline");
    this.useRadios = {};
    for (const [key, label, title] of [["aligned", "Le double recalé", "La timeline joue la prise calée (la prise active du groupe de prises)."], ["original", "L'original", "La timeline rejoue le double d'origine."]]) {
      const l = h("label");
      l.title = title;
      const r = h("input");
      r.type = "radio";
      r.name = "al-use";
      r.addEventListener("change", () => this.#use(key));
      l.append(r, h("span", "", label));
      this.useBox.append(l);
      this.useRadios[key] = r;
    }
    side.append(
      group("Guide", this.guideSel),
      group("Doubles", h("p", "al-sub", "Une piste chacun : coche ceux à caler."), this.list, this.goBtn, this.cancelBtn, this.progress),
      group("Dans la timeline", this.useBox),
    );
    // Colonne principale.
    const main = h("main", "al-main");
    const bar = h("div", "al-listen");
    this.playBtn = h("button", "primary", "▶ Écouter");
    this.playBtn.type = "button";
    this.playBtn.addEventListener("click", () => this.#togglePlay());
    bar.append(this.playBtn);
    this.radios = new Map();
    for (const [what, label] of WHATS) {
      const l = h("label", "al-radio");
      const r = h("input");
      r.type = "radio";
      r.name = "al-what";
      r.checked = what === this.what;
      r.addEventListener("change", () => {
        this.what = what;
        if (this.listening) this.#sendListen();
      });
      l.append(r, h("span", "", ` ${label}`));
      bar.append(l);
      this.radios.set(what, { r, l });
    }
    bar.append(h("span", "grow"));
    for (const [txt, title, dir] of [["−", "Zoom arrière", -1], ["＋", "Zoom avant", 1]]) {
      const b = h("button", "", txt);
      b.type = "button";
      b.title = title;
      b.addEventListener("click", () => this.#zoom(dir));
      bar.append(b);
    }
    const fit = h("button", "", "Tout voir");
    fit.type = "button";
    fit.addEventListener("click", () => this.#fit());
    bar.append(fit);
    this.canvas = h("canvas", "al-stack");
    this.canvas.tabIndex = 0;
    this.canvas.setAttribute("aria-label", "Guide, double, double recalé, consonnes, décalage appliqué et force : Ctrl + molette ou pincer pour zoomer ; sur la force, cliquer pour poser un point, glisser pour le déplacer, double-clic pour le supprimer ; sur les consonnes, glisser un bord ou le corps, tirer dans le vide pour en créer une, double-clic pour la supprimer");
    this.canvas.style.height = `${LAYOUT.total}px`;
    this.scroll = h("input", "al-scroll");
    this.scroll.type = "range";
    this.scroll.setAttribute("aria-label", "Défilement dans le temps");
    this.scroll.addEventListener("input", () => {
      this.view.start = Number(this.scroll.value);
      this.#draw();
    });
    this.settingsBox = h("div", "al-settings");
    this.fields = settingsFields(parseSettings(undefined), () => {}, { analysisMark: true });
    const side2 = h("div", "al-settings-side");
    this.curveInfo = h("div", "al-info");
    const clearCurve = h("button", "", "Effacer la courbe de force");
    clearCurve.type = "button";
    clearCurve.addEventListener("click", () => {
      const c = this.#cfg();
      if (!c) return;
      c.settings.local.strength_curve = [];
      this.#draw();
    });
    const thr = h("div", "al-row");
    this.threshold = h("input");
    Object.assign(this.threshold, { type: "number", min: "5", max: "500", step: "5", value: "30" });
    this.threshold.style.width = "4.5em";
    this.threshold.setAttribute("aria-label", "Seuil de décalage (ms)");
    this.threshold.addEventListener("keydown", (k) => k.stopPropagation());
    const suggest = h("button", "", "Suggérer la force");
    suggest.type = "button";
    suggest.title = "Propose une courbe de force : calage complet là où le double est très décalé du guide (au-delà du seuil), rien ailleurs.";
    suggest.addEventListener("click", () => {
      const id = this.#alignedId();
      if (id == null) return;
      this.wantSuggest = true;
      this.client.send({ type: "get_align", clip: id, settings: stringifySettings(this.#cfg().settings), threshold_ms: Number(this.threshold.value) || 30 });
    });
    thr.append(h("span", "al-info", "Seuil"), this.threshold, h("span", "al-info", "ms"), suggest);
    this.consList = h("div", "al-cons");
    const addRow = h("div", "al-row");
    this.consFrom = h("input");
    this.consTo = h("input");
    for (const i of [this.consFrom, this.consTo]) {
      Object.assign(i, { type: "number", min: "0", step: "0.01" });
      i.style.width = "5.5em";
      i.addEventListener("keydown", (k) => k.stopPropagation());
    }
    const addCons = h("button", "", "＋");
    addCons.type = "button";
    addCons.title = "Ajouter une consonne protégée (secondes, dans le double)";
    addCons.addEventListener("click", () => {
      const c = this.#cfg();
      if (!c) return;
      c.consonants = cleanConsonants([...c.consonants, [Number(this.consFrom.value), Number(this.consTo.value)]]);
      c.edited = true;
      this.consFrom.value = this.consTo.value = "";
      this.#renderConsonants();
      this.#draw();
    });
    addRow.append(h("span", "al-info", "Consonne de"), this.consFrom, h("span", "al-info", "à"), this.consTo, h("span", "al-info", "s"), addCons);
    side2.append(this.curveInfo, clearCurve, thr, h("div", "al-sub", "Consonnes protégées"), this.consList, addRow);
    const act = h("div", "al-actions");
    this.againBox = h("input");
    this.againBox.type = "checkbox";
    const again = h("label", "dyn-check");
    again.append(this.againBox, h("span", "", " Refaire aussi l'analyse"));
    again.title = "Nécessaire quand le mode d'analyse, le serrage ou l'écart maximal changent (coché tout seul dans ce cas).";
    this.redoBtn = h("button", "primary", "Refaire le calage");
    this.redoBtn.type = "button";
    this.redoBtn.addEventListener("click", () => this.#redo());
    this.copyBtn = h("button", "", "Copier les réglages vers…");
    this.copyBtn.type = "button";
    this.copyBtn.addEventListener("click", () => this.#toggleCopy());
    act.append(again, this.redoBtn, this.copyBtn);
    this.copyBox = h("div", "al-copy");
    this.copyBox.hidden = true;
    this.msg = h("p", "al-progress");
    // Réglages à gauche, courbe de force et consonnes à droite.
    const left = group("Réglages du calage", this.fields.el, act, this.copyBox);
    const right = group("Force et consonnes", ...side2.children);
    this.settingsBox.replaceChildren(left, right);
    main.append(bar, this.canvas, this.scroll, this.settingsBox, this.msg);
    body.append(side, main);
    dlg.append(head, body);
    dlg.addEventListener("keydown", (k) => k.stopPropagation());
    dlg.addEventListener("close", () => {
      if (this.listening) this.client.send({ type: "stop_listen" });
      this.offs?.forEach((f) => f());
      this.offs = null;
      this.controller.windowClosed(this);
    });
    document.body.append(dlg);
    this.#bindCanvas();
  }

  // ---- état -----------------------------------------------------------------------------------------------------------------

  #sr() {
    return this.state.project?.sample_rate ?? 48000;
  }

  #guide() {
    return guideInfo(this.state.project, this.guideId, this.guideExtra ?? []);
  }

  #doubles() {
    return doublesFor(this.state.project, this.#guide(), this.pick);
  }

  #current() {
    return this.#doubles().find((d) => d.node === this.sel) ?? null;
  }

  #alignedId() {
    return this.#current()?.aligned ?? null;
  }

  #cfg() {
    if (this.sel == null) return null;
    if (!this.cfg.has(this.sel)) {
      const s = parseSettings(JSON.stringify({ general: loadPref() }));
      this.cfg.set(this.sel, { settings: s, original: parseSettings(stringifySettings(s)), consonants: [], edited: false, alignedId: null });
    }
    return this.cfg.get(this.sel);
  }

  /** Remet la liste, le guide et le double affiché d'après le projet ; `fit` : recadre aussi la vue sur la fenêtre du guide. */
  #refresh(fit) {
    const project = this.state.project;
    const sr = this.#sr();
    // Guide : les clips audio non calés.
    const clips = audioClips(project).filter((c) => !c.aligned);
    if (this.guideId != null && !clips.some((c) => c.id === this.guideId)) this.guideId = clips[0]?.id ?? null;
    this.guideSel.replaceChildren(...clips.map((c) => new Option(clipLabel(c, sr), String(c.id))));
    if (this.guideId != null) this.guideSel.value = String(this.guideId);
    const doubles = this.#doubles();
    this.list.replaceChildren();
    if (!doubles.length) this.list.append(h("p", "gd-empty", this.guideId == null ? "Aucun clip audio." : "Aucune autre piste n'a d'audio sous le guide."));
    if (this.sel == null || !doubles.some((d) => d.node === this.sel)) this.sel = doubles.find((d) => this.checked.has(d.node))?.node ?? doubles[0]?.node ?? null;
    for (const d of doubles) {
      const row = h("div", `al-item${d.node === this.sel ? " on" : ""}`);
      const box = h("input");
      box.type = "checkbox";
      box.checked = this.checked.has(d.node);
      box.disabled = d.clipId == null;
      box.setAttribute("aria-label", `Caler ${d.track}`);
      box.addEventListener("change", () => {
        box.checked ? this.checked.add(d.node) : this.checked.delete(d.node);
        this.#buttons();
      });
      const name = h("button", "al-name", d.track);
      name.type = "button";
      name.addEventListener("click", () => {
        this.sel = d.node;
        this.#refresh(false);
      });
      const badge = h("span", `al-badge${d.aligned != null ? " ok" : ""}`, d.aligned != null ? "calé" : "à caler");
      row.append(box, name, badge);
      this.list.append(row);
    }
    // Le double affiché : réglages et détails de sa prise calée.
    const cur = this.#current();
    const cfg = this.#cfg();
    if (cfg && cur?.aligned != null && cfg.alignedId !== cur.aligned) {
      cfg.alignedId = cur.aligned;
      this.client.send({ type: "get_align", clip: cur.aligned });
    }
    if (cfg) {
      this.fields = this.#swapFields(cfg);
      this.#renderConsonants();
    }
    for (const [what, { r, l }] of this.radios) {
      const needs = what.endsWith("aligned");
      r.disabled = (needs && cur?.aligned == null) || (what === "original" && !cur) || (what !== "guide" && !cur);
      l.title = r.disabled ? "Il faut d'abord caler ce double." : "";
      if (r.disabled && this.what === what) {
        this.what = "guide";
        this.radios.get("guide").r.checked = true;
      }
    }
    this.#buttons();
    if (fit) this.#fit();
    this.#draw();
  }

  /** Choisit la prise que la timeline joue : le recalé (`aligned`) ou l'original ; c'est une action annulable (« Changer de prise »). */
  #use(which) {
    const cur = this.#current();
    if (!cur) return;
    const id = which === "aligned" ? cur.aligned : cur.clipId;
    if (id == null) return;
    if (this.listening) this.client.send({ type: "stop_listen" });
    this.client.send({ type: "set_active_take", clip: id });
  }

  /** État des boutons d'action : « Caler » demande au moins un double coché, « Refaire » une prise calée ; rien pendant un calage. */
  #buttons() {
    const running = this.controller.running;
    const cur = this.#current();
    this.goBtn.disabled = running || !this.#doubles().some((d) => this.checked.has(d.node) && d.clipId != null);
    this.redoBtn.disabled = running || cur?.aligned == null;
    this.copyBtn.disabled = cur?.aligned == null;
    this.cancelBtn.hidden = !running;
    // Prise jouée par la timeline : le recalé s'il est actif, sinon l'original.
    const al = cur?.aligned != null ? clipById(this.state.project, cur.aligned) : null;
    const inUse = al && !al.muted ? "aligned" : "original";
    this.useRadios.aligned.disabled = cur?.aligned == null;
    this.useRadios.original.disabled = !cur || cur.clipId == null;
    this.useRadios.aligned.checked = !!cur && inUse === "aligned";
    this.useRadios.original.checked = !!cur && inUse === "original";
  }

  #swapFields(cfg) {
    const old = this.fields.el;
    const f = settingsFields(cfg.settings, () => this.#changed(), { analysisMark: true });
    old.replaceWith(f.el);
    return f;
  }

  #analysisChanged() {
    const c = this.#cfg();
    return !!c && ANALYSIS_KEYS.some((k) => c.settings.general[k] !== c.original.general[k]);
  }

  #changed() {
    if (this.#analysisChanged()) this.againBox.checked = true;
    savePref(this.#cfg().settings.general);
    this.#draw();
  }

  /** Un calage est fini ou avance (envoyé par le contrôleur). */
  onAlign(e) {
    this.progress.textContent = alignProgressText(e);
    this.#buttons();
    if (!e.active) {
      this.msg.textContent = e.error ?? "";
      this.#refresh(false);
    }
  }

  fail(message) {
    this.msg.textContent = message;
  }

  #onDetail(d) {
    if (this.wantSuggest && d.clip === this.#alignedId()) {
      this.wantSuggest = false;
      const pairs = (d.suggest ?? []).map((p) => [p[0], p[1]]);
      const c = this.#cfg();
      c.settings.local.strength_curve = pairs;
      this.curveInfo.textContent = pairs.length ? `${pairs.length} point(s) suggéré(s)` : "Rien à suggérer : aucun passage très décalé.";
      this.#draw();
      return;
    }
    this.details.set(d.clip, d);
    const cur = this.#current();
    if (cur?.aligned === d.clip) {
      const c = this.#cfg();
      c.settings = parseSettings(d.settings);
      c.original = parseSettings(d.settings);
      c.consonants = cleanConsonants(d.consonants);
      c.edited = false;
      c.alignedId = d.clip;
      this.againBox.checked = false;
      this.fields = this.#swapFields(c);
      this.#renderConsonants();
      this.#draw();
    }
  }

  #detail() {
    const id = this.#alignedId();
    return id == null ? null : (this.details.get(id) ?? null);
  }

  // ---- actions --------------------------------------------------------------------------------------------------------------

  #align() {
    const doubles = this.#doubles().filter((d) => this.checked.has(d.node) && d.clipId != null);
    if (!doubles.length || this.guideId == null) return;
    const cur = this.#cfg();
    const settings = stringifySettings(cur ? cur.settings : parseSettings(undefined));
    this.msg.textContent = "";
    const msg = { type: "align_clips", guide: this.guideId, doubles: doubles.flatMap((d) => d.clipIds), settings };
    if (this.guideExtra?.length) msg.guide_more = this.guideExtra;
    this.client.send(msg);
  }

  #redo() {
    const id = this.#alignedId();
    const c = this.#cfg();
    if (id == null || !c) return;
    const reanalyse = this.againBox.checked;
    const cmd = { type: "align_redo", clips: [id], settings: stringifySettings(c.settings), reanalyse };
    if (c.edited && !reanalyse) cmd.consonants = c.consonants;
    this.msg.textContent = "";
    this.client.send(cmd);
  }

  #toggleCopy() {
    this.copyBox.hidden = !this.copyBox.hidden;
    if (this.copyBox.hidden) return;
    const others = this.#doubles().filter((d) => d.aligned != null && d.node !== this.sel);
    this.copyBox.replaceChildren(h("div", "al-sub", "Copier le moteur, la force, la protection, le serrage, l'écart maximal et le mode d'analyse (jamais la courbe de force ni les consonnes) vers :"));
    if (!others.length) this.copyBox.append(h("p", "gd-empty", "Aucun autre double calé."));
    const boxes = [];
    for (const d of others) {
      const l = h("label", "dyn-check");
      const b = h("input");
      b.type = "checkbox";
      b.checked = true;
      boxes.push([d, b]);
      l.append(b, h("span", "", ` ${d.track}`));
      this.copyBox.append(l);
    }
    if (!others.length) return;
    const go = h("button", "primary", "Copier et refaire le calage de ces doubles");
    go.type = "button";
    go.addEventListener("click", () => {
      const clips = boxes.filter(([, b]) => b.checked).map(([d]) => d.aligned);
      if (!clips.length) return;
      this.client.send({ type: "align_redo", clips, settings: stringifySettings(this.#cfg().settings), reanalyse: this.#analysisChanged(), general_only: true });
      this.copyBox.hidden = true;
    });
    this.copyBox.append(go);
  }

  #renderConsonants() {
    const c = this.#cfg();
    this.consList.replaceChildren();
    if (!c?.consonants.length) return this.consList.append(h("span", "al-info", "Aucune consonne repérée."));
    c.consonants.forEach((p, i) => {
      const r = h("div", "al-cons-row");
      r.append(h("span", "", `${p[0].toFixed(2).replace(".", ",")} – ${p[1].toFixed(2).replace(".", ",")} s`));
      const del = h("button", "", "✕");
      del.type = "button";
      del.setAttribute("aria-label", "Retirer cette consonne");
      del.addEventListener("click", () => {
        c.consonants.splice(i, 1);
        c.edited = true;
        this.#renderConsonants();
        this.#draw();
      });
      r.append(del);
      this.consList.append(r);
    });
  }

  // ---- écoute ---------------------------------------------------------------------------------------------------------------

  #togglePlay() {
    if (this.listening) return this.client.send({ type: "stop_listen" });
    this.#sendListen();
  }

  #sendListen() {
    const cur = this.#current();
    if (!cur || this.guideId == null) return;
    const cmd = { type: "listen_align", guide: this.guideId, node: cur.node, what: this.what };
    if (this.guideExtra?.length) cmd.guide_more = this.guideExtra;
    if (this.what.endsWith("aligned")) cmd.aligned = cur.aligned;
    this.client.send(cmd);
  }

  #onListen(e) {
    this.listening = e.active;
    this.playBtn.textContent = e.active ? "■ Arrêter" : "▶ Écouter";
    this.#draw();
  }

  // ---- vue : zoom, défilement -----------------------------------------------------------------------------------------------

  #plotW() {
    return Math.max(50, (this.canvas.clientWidth || 700) - LABEL_W - 8);
  }

  /** Cadre toute la fenêtre du guide (plus une petite marge) : le plus petit cran de zoom qui la contient. */
  #fit() {
    const g = this.#guide();
    if (!g) return;
    const pw = this.#plotW();
    this.view = { start: g.start - g.len * 0.02, spp: 2 ** (Math.ceil(Math.log2((g.len * 1.04) / pw) * 4) / 4) };
    this.#clamp();
    this.#draw();
  }

  #zoom(dir, anchorX = this.#plotW() / 2) {
    const spp = Math.max(1, snapSpp(this.view.spp * (dir > 0 ? 1 / STEP : STEP)));
    const anchor = xSample(this.view, anchorX);
    this.view = { start: anchor - anchorX * spp, spp };
    this.#clamp();
    this.#draw();
  }

  #clamp() {
    const g = this.#guide();
    if (!g) return;
    const span = this.#plotW() * this.view.spp;
    const lo = g.start - g.len * 0.1;
    const hi = g.start + g.len * 1.1;
    this.view.start = Math.min(Math.max(lo, this.view.start), Math.max(lo, hi - span));
    const room = Math.max(0, hi - lo - span);
    this.scroll.hidden = room <= 0;
    this.scroll.min = String(Math.round(lo));
    this.scroll.max = String(Math.round(lo + room));
    this.scroll.value = String(Math.round(this.view.start));
  }

  // ---- tracé ----------------------------------------------------------------------------------------------------------------

  #geom() {
    return { dpr: pixelRatio(), w: Math.max(300, this.canvas.clientWidth || 700), h: LAYOUT.total, left: LABEL_W, pw: this.#plotW() };
  }

  #wave(x, clip, row, color, geom) {
    const { left, pw } = geom;
    const view = this.view;
    const s0 = Math.max(view.start, clip.start);
    const s1 = Math.min(xSample(view, pw), clip.start + clip.len);
    if (s1 <= s0) return;
    const mid = row.top + row.h / 2;
    const half = row.h / 2 - 2;
    x.save();
    x.beginPath();
    x.rect(left, row.top, pw, row.h);
    x.clip();
    x.fillStyle = color;
    for (const t of this.peaks.tilesFor(clip, view.spp, s0, s1)) {
      for (let i = 0; i < t.columns.length; i++) {
        const [lo, hi] = t.columns[i];
        const px = Math.floor(left + sampleX(view, t.from + i * t.spp));
        if (px < left - 1 || px > left + pw) continue;
        x.fillRect(px, mid - hi * half, 1, Math.max(1, (hi - lo) * half));
      }
    }
    x.restore();
  }

  #draw() {
    if (!this.dlg.open) return;
    const g = this.#geom();
    const c = this.canvas;
    if (c.width !== Math.round(g.w * g.dpr) || c.height !== Math.round(g.h * g.dpr)) {
      c.width = Math.round(g.w * g.dpr);
      c.height = Math.round(g.h * g.dpr);
    }
    this.#clamp();
    const x = c.getContext("2d");
    x.setTransform(g.dpr, 0, 0, g.dpr, 0, 0);
    x.clearRect(0, 0, g.w, g.h);
    const bg = css("--bg") || "#111";
    const line = css("--line") || "#444";
    const muted = css("--muted") || "#888";
    const accent = css("--accent") || "#4ea1ff";
    const R = LAYOUT.rows;
    x.fillStyle = bg;
    x.fillRect(0, 0, g.w, g.h);
    x.font = "11px sans-serif";
    // Règle (secondes depuis le début de la fenêtre du guide).
    const guide = this.#guide();
    const sr = this.#sr();
    const win = guide ? { start: guide.start, len: guide.len } : null;
    x.fillStyle = muted;
    if (win) {
      const secPerPx = (this.view.spp / sr) * 1;
      const stepS = [0.1, 0.25, 0.5, 1, 2, 5, 10, 20, 30, 60].find((s) => s / secPerPx >= 70) ?? 120;
      const t0 = (this.view.start - win.start) / sr;
      for (let t = Math.ceil(t0 / stepS) * stepS; t <= (xSample(this.view, g.pw) - win.start) / sr; t += stepS) {
        const px = g.left + sampleX(this.view, win.start + t * sr);
        x.fillRect(px, R.ruler.top + 12, 1, 6);
        x.fillText(`${Number.isInteger(t) ? t : t.toFixed(2)} s`, px + 3, R.ruler.top + 11);
      }
    }
    // Rangées : fond, étiquette.
    for (const r of ROWS.slice(1)) {
      const row = R[r.id];
      x.fillStyle = css("--panel-2") || "#1b1b1b";
      x.fillRect(g.left, row.top, g.pw, row.h);
      x.fillStyle = muted;
      x.fillText(r.label, 6, row.top + Math.min(14, row.h - 4));
    }
    const project = this.state.project;
    const cur = this.#current();
    const cfg = this.#cfg();
    const detail = this.#detail();
    if (guide) {
      for (const gid of guide.ids ?? [guide.id]) {
        const gc = clipById(project, gid);
        if (gc) this.#wave(x, gc, R.guide, "#e9a23b", g);
      }
      if (cur) {
        for (const o of originalsOn(project, cur.node, win)) this.#wave(x, o, R.orig, "#8aa0b8", g);
        const a = cur.aligned != null ? clipById(project, cur.aligned) : null;
        if (a) {
          this.#wave(x, a, R.aligned, "#3ecf8e", g);
        } else {
          x.fillStyle = muted;
          x.fillText("Pas encore calé — règle les réglages puis « Caler les doubles cochés ».", g.left + 10, R.aligned.top + 34);
        }
      } else {
        x.fillStyle = muted;
        x.fillText("Choisis un double dans la liste.", g.left + 10, R.orig.top + 34);
      }
    }
    // Consonnes (placées d'après le chemin : du temps du double au temps du guide), décalage appliqué, force.
    if (guide && detail && cfg) {
      const toX = (t) => g.left + sampleX(this.view, win.start + t * sr);
      x.save();
      x.beginPath();
      x.rect(g.left, 0, g.pw, g.h);
      x.clip();
      cfg.consonants.forEach(([a, b], i) => {
        const ga = guideTimeOf(detail.path, detail.hop, a);
        const gb = Math.max(ga + 0.01, guideTimeOf(detail.path, detail.hop, b));
        const on = i === this.selCons;
        x.fillStyle = on ? "rgba(233,211,74,0.85)" : "rgba(233,211,74,0.55)";
        x.fillRect(toX(ga), R.cons.top, Math.max(2, toX(gb) - toX(ga)), R.cons.h);
        x.fillStyle = on ? "rgba(233,211,74,0.2)" : "rgba(233,211,74,0.12)";
        x.fillRect(toX(ga), R.guide.top, Math.max(2, toX(gb) - toX(ga)), R.aligned.top + R.aligned.h - R.guide.top);
        if (on) {
          // Poignées des bords de la consonne sélectionnée.
          x.fillStyle = "#fff";
          x.fillRect(toX(ga) - 1, R.cons.top - 1, 3, R.cons.h + 2);
          x.fillRect(toX(gb) - 1, R.cons.top - 1, 3, R.cons.h + 2);
        }
      });
      if (this.consNew) {
        const ga = guideTimeOf(detail.path, detail.hop, Math.min(...this.consNew));
        const gb = guideTimeOf(detail.path, detail.hop, Math.max(...this.consNew));
        x.fillStyle = "rgba(233,211,74,0.4)";
        x.fillRect(toX(ga), R.cons.top, Math.max(2, toX(gb) - toX(ga)), R.cons.h);
      }
      // Décalage appliqué (aire entre le minimum et le maximum de chaque colonne de pixels).
      const range = Math.max(20, Math.ceil(maxShiftMs(detail.shift) / 10) * 10);
      const sy = (ms) => R.shift.top + R.shift.h / 2 - (ms / range) * (R.shift.h / 2 - 2);
      x.strokeStyle = line;
      x.beginPath();
      x.moveTo(g.left, sy(0));
      x.lineTo(g.left + g.pw, sy(0));
      x.stroke();
      x.fillStyle = accent;
      x.globalAlpha = 0.6;
      const hop = detail.hop;
      for (let px = 0; px < g.pw; px++) {
        const t0 = (xSample(this.view, px) - win.start) / sr;
        const t1 = (xSample(this.view, px + 1) - win.start) / sr;
        const i0 = Math.max(0, Math.floor(t0 / hop));
        const i1 = Math.min(detail.shift.length - 1, Math.max(i0, Math.floor(t1 / hop)));
        if (i0 >= detail.shift.length || t1 < 0) continue;
        let lo = Infinity;
        let hi = -Infinity;
        for (let i = i0; i <= i1; i++) {
          lo = Math.min(lo, detail.shift[i] * 1000);
          hi = Math.max(hi, detail.shift[i] * 1000);
        }
        x.fillRect(g.left + px, sy(hi), 1, Math.max(1, sy(lo) - sy(hi)));
      }
      x.globalAlpha = 1;
      x.restore();
      x.fillStyle = muted;
      x.fillText(`±${range} ms`, 6, R.shift.top + 28);
      // Force : ligne pointillée (constante) ou courbe avec ses points.
      const dur = win.len / sr;
      const curve = cfg.settings.local.strength_curve;
      const base = cfg.settings.general.strength;
      const fy = (v) => R.force.top + R.force.h - 3 - v * (R.force.h - 6);
      x.save();
      x.beginPath();
      x.rect(g.left, R.force.top, g.pw, R.force.h);
      x.clip();
      x.strokeStyle = "#3ecf8e";
      x.lineWidth = 2;
      x.beginPath();
      if (!curve.length) {
        x.setLineDash([5, 4]);
        x.moveTo(g.left, fy(base));
        x.lineTo(g.left + g.pw, fy(base));
      } else {
        x.moveTo(g.left, fy(strengthAt(curve, base, 0)));
        for (const [t, v] of curve) x.lineTo(toX(t), fy(v));
        x.lineTo(toX(dur), fy(strengthAt(curve, base, dur)));
      }
      x.stroke();
      x.setLineDash([]);
      curve.forEach(([t, v], i) => {
        x.fillStyle = i === this.selectedPoint ? "#fff" : "#3ecf8e";
        x.beginPath();
        x.arc(toX(t), fy(v), 5, 0, Math.PI * 2);
        x.fill();
      });
      x.restore();
      x.fillStyle = muted;
      x.fillText("1", g.left - 12, R.force.top + 10);
      x.fillText("0", g.left - 12, R.force.top + R.force.h - 2);
      this.curveInfo.textContent = curve.length ? `Courbe de force : ${curve.length} point(s)` : "Force constante (clique sur la rangée « Force » pour poser des points)";
      this._map = { g, toX, fy, dur, win, sr };
    } else {
      this._map = null;
      x.fillStyle = muted;
      x.fillText(cur ? "Le décalage et la force apparaissent après le calage." : "", g.left + 10, R.shift.top + 30);
    }
    // Tête de lecture pendant l'écoute.
    if (this.listening && win) {
      const px = g.left + sampleX(this.view, this.state.pos ?? 0);
      if (px >= g.left && px <= g.left + g.pw) {
        x.fillStyle = "#fff";
        x.fillRect(px - 1, R.guide.top, 2, g.h - R.guide.top);
      }
    }
  }

  // ---- souris et doigts -----------------------------------------------------------------------------------------------------

  #bindCanvas() {
    const c = this.canvas;
    const point = (e) => {
      const r = c.getBoundingClientRect();
      const k = (c.clientWidth || r.width) / (r.width || 1);
      return { x: (e.clientX - r.left) * k, y: (e.clientY - r.top) * k };
    };
    const inForce = (p) => {
      const R = LAYOUT.rows.force;
      return this._map && p.x >= LABEL_W - 6 && p.y >= R.top - 4 && p.y <= R.top + R.h + 4;
    };
    const toData = (p) => {
      const m = this._map;
      const R = LAYOUT.rows.force;
      const t = (xSample(this.view, p.x - m.g.left) - m.win.start) / m.sr;
      return { t: Math.min(m.dur, Math.max(0, t)), v: Math.min(1, Math.max(0, (R.top + R.h - 3 - p.y) / (R.h - 6))) };
    };
    const hit = (p, touch) => {
      const cfg = this.#cfg();
      if (!cfg || !this._map) return -1;
      const rad = touch ? 16 : 9;
      let best = -1;
      let bd = rad;
      cfg.settings.local.strength_curve.forEach(([t, v], i) => {
        const dd = Math.hypot(this._map.toX(t) - p.x, this._map.fy(v) - p.y);
        if (dd <= bd) {
          bd = dd;
          best = i;
        }
      });
      return best;
    };
    // Consonnes (rangée du milieu) : glisser un bord ou le corps, tirer dans le vide pour en créer une, double-clic pour la supprimer.
    const consRow = LAYOUT.rows.cons;
    const inCons = (p) => this._map && this.#detail() && p.x >= LABEL_W - 6 && p.y >= consRow.top - 3 && p.y <= consRow.top + consRow.h + 3;
    const dubT = (p) => {
      const m = this._map;
      const d = this.#detail();
      return doubleTimeOf(d.path, d.hop, (xSample(this.view, p.x - m.g.left) - m.win.start) / m.sr);
    };
    const consHit = (p, touch) => {
      const d = this.#detail();
      const cfg = this.#cfg();
      if (!d || !cfg || !this._map) return null;
      const tol = touch ? 14 : 7;
      for (let i = 0; i < cfg.consonants.length; i++) {
        const [a, b] = cfg.consonants[i];
        const xa = this._map.toX(guideTimeOf(d.path, d.hop, a));
        const xb = this._map.toX(guideTimeOf(d.path, d.hop, b));
        if (Math.abs(p.x - xa) <= tol && Math.abs(p.x - xb) > tol) return { i, mode: "l" };
        if (Math.abs(p.x - xb) <= tol) return { i, mode: "r" };
        if (p.x > xa && p.x < xb) return { i, mode: "move" };
      }
      return null;
    };
    c.addEventListener("pointerdown", (e) => {
      const p = point(e);
      if (inCons(p)) {
        const cfg = this.#cfg();
        const hit = consHit(p, e.pointerType === "touch");
        const base = cfg.consonants.map((q) => [...q]);
        this.consDrag = hit ? { ...hit, from: dubT(p), base } : { mode: "new", t0: dubT(p), base };
        this.selCons = hit ? hit.i : -1;
        c.focus({ preventScroll: true });
        c.setPointerCapture(e.pointerId);
        this.#draw();
        e.preventDefault();
        return;
      }
      if (!inForce(p)) return;
      const cfg = this.#cfg();
      let i = hit(p, e.pointerType === "touch");
      if (i < 0) {
        const { t, v } = toData(p);
        const a = addPoint(cfg.settings.local.strength_curve, t, v);
        cfg.settings.local.strength_curve = a.curve;
        i = a.index;
      }
      this.selectedPoint = i;
      this.drag = { pointer: e.pointerId };
      c.setPointerCapture(e.pointerId);
      this.#draw();
      e.preventDefault();
    });
    c.addEventListener("pointermove", (e) => {
      if (this.consDrag) {
        const t = dubT(point(e));
        const cd = this.consDrag;
        const cfg = this.#cfg();
        if (cd.mode === "new") this.consNew = [cd.t0, t];
        else {
          cfg.consonants = editConsonant(cd.base, cd.i, cd.mode, t, cd.from);
          cfg.edited = true;
        }
        this.#draw();
        return;
      }
      if (!this.drag || this.selectedPoint < 0) return;
      const { t, v } = toData(point(e));
      const cfg = this.#cfg();
      cfg.settings.local.strength_curve = movePoint(cfg.settings.local.strength_curve, this.selectedPoint, t, v, 0.01, this._map.dur);
      this.#draw();
    });
    const end = () => {
      this.drag = null;
      const cd = this.consDrag;
      this.consDrag = null;
      if (!cd) return;
      const cfg = this.#cfg();
      if (cd.mode === "new" && this.consNew) {
        const a = addConsonant(cd.base, this.consNew[0], this.consNew[1]);
        if (a.index >= 0) {
          cfg.consonants = a.list;
          cfg.edited = true;
          this.selCons = a.index;
        }
      }
      this.consNew = null;
      this.#renderConsonants();
      this.#draw();
    };
    c.addEventListener("pointerup", end);
    c.addEventListener("pointercancel", end);
    c.addEventListener("dblclick", (e) => {
      const pc = point(e);
      if (inCons(pc)) {
        const h = consHit(pc, false);
        const cfg = this.#cfg();
        if (h && cfg) {
          cfg.consonants.splice(h.i, 1);
          cfg.edited = true;
          this.selCons = -1;
          this.#renderConsonants();
          this.#draw();
        }
        return;
      }
      const i = hit(point(e), false);
      if (i >= 0) {
        const cfg = this.#cfg();
        cfg.settings.local.strength_curve = removePoint(cfg.settings.local.strength_curve, i);
        this.selectedPoint = -1;
        this.#draw();
      }
    });
    c.addEventListener("keydown", (k) => {
      if ((k.key === "Delete" || k.key === "Backspace") && this.selCons >= 0) {
        const cfg = this.#cfg();
        cfg?.consonants.splice(this.selCons, 1);
        if (cfg) cfg.edited = true;
        this.selCons = -1;
        this.#renderConsonants();
        this.#draw();
        return;
      }
      if ((k.key === "Delete" || k.key === "Backspace") && this.selectedPoint >= 0) {
        const cfg = this.#cfg();
        cfg.settings.local.strength_curve = removePoint(cfg.settings.local.strength_curve, this.selectedPoint);
        this.selectedPoint = -1;
        this.#draw();
      }
    });
    c.addEventListener(
      "wheel",
      (e) => {
        if (e.ctrlKey || e.metaKey) {
          e.preventDefault();
          this.#zoom(e.deltaY < 0 ? 1 : -1, point(e).x - LABEL_W);
        } else if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
          e.preventDefault();
          this.view.start += (e.deltaX || e.deltaY) * this.view.spp;
          this.#draw();
        }
      },
      { passive: false },
    );
    // Deux doigts : zoom horizontal (par crans) et défilement ; un doigt qui reste ne pose plus de point.
    new TouchPinch(c, {
      onStart: () => {
        this.drag = null;
        this.pinchAcc = 1;
      },
      onMove: ({ factor, mode, dx, x: cx }) => {
        const r = c.getBoundingClientRect();
        const anchor = cx - r.left - LABEL_W;
        if (mode === "x" || mode === "y") {
          // Un seul zoom ici, le temps : tout pincement le règle (la hauteur des rangées est fixe).
          this.pinchAcc *= mode === "x" ? factor : 1;
        }
        while (this.pinchAcc >= STEP) {
          this.#zoom(1, anchor);
          this.pinchAcc /= STEP;
        }
        while (this.pinchAcc <= 1 / STEP) {
          this.#zoom(-1, anchor);
          this.pinchAcc *= STEP;
        }
        this.view.start -= dx * this.view.spp;
        this.#draw();
      },
    });
  }
}
