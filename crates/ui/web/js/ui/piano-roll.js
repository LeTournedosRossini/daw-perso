// Piano roll : fenêtre flottante non modale pour éditer les notes d'un clip. Les positions sont en fractions exactes de ronde
// (voir `notes-model.js`) ; chaque geste est une seule commande (un pas d'annulation), envoyée au relâchement.
//
// Souris : clic dans le vide = ajouter une note (durée de la grille) ; clic sur une note = la sélectionner (Maj : ajouter à la
// sélection) ; glisser = déplacer (temps et touche) ; bord droit = durée ; Maj + glisser dans le vide = rectangle de sélection (au doigt : glisser dans le vide) ;
// bande du bas = vélocité. Alt pendant un geste : sans magnétisme. Clavier : Suppr, Ctrl+A, flèches (Maj = octave), Échap.

import { NoteProps } from "./note-props.js";
import { LANE_DEFS, laneById, isVelocity, eventsOf, valueOfY, yOfValue, withPoint, withoutPoints, nearestPoint, laneCommand, pointId, valueText, copyPoints, pastePoints, thinPoints } from "../control-model.js";
import { canvasPoint, pixelRatio } from "../ui-scale.js";
import { sampleToX, xToSample } from "../time.js";
import { getScale } from "../ui-scale.js";
import { makeFloatable } from "./float-frame.js";
import { pinchFactors, pinchMode, GRIDS, gridById, snapAbs, matToAbs, absToMat, moveGroup, resizeGroup, diffEdits, keyName, isBlack, noteId, fnum, fadd, fsub, fcmp } from "../notes-model.js";

const KEYS_W = 56;
export const ROLL_ROWS_MIN = 2;
export const ROLL_ROWS_MAX = 10;
export const ROLL_ROWS_DEFAULT = 4; // nombre de lignes de la timeline que prend le piano roll ancré (réglable)

/** Nombre de lignes demandé, ramené dans les bornes (une valeur absurde ou absente donne la valeur par défaut). */
export const clampRows = (n) => (Number.isFinite(n) ? Math.max(ROLL_ROWS_MIN, Math.min(ROLL_ROWS_MAX, Math.round(n))) : ROLL_ROWS_DEFAULT);
const EDGE = 6; // largeur (px) de la zone de redimensionnement au bord droit d'une note
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

let laneClipboard = null; // points copiés d'une voie de contrôle (partagé entre les fenêtres de piano roll)

export class PianoRoll {
  constructor(state, client) {
    this.state = state;
    this.client = client;
    this.root = document.getElementById("piano-roll");
    // Fenêtre flottante : déplaçable par son bandeau, redimensionnable par une poignée à nous (la poignée native ne marche pas sous WebKitGTK).
    makeFloatable(this.root, { key: "daw-pr-frame", minW: 360, minH: 260 });
    this.keys = document.getElementById("pr-keys");
    this.roll = document.getElementById("pr-roll");
    this.vel = document.getElementById("pr-vel");
    this.gridSel = document.getElementById("pr-grid");
    this.corner = this.root.querySelector(".pr-corner");
    // Ce que montre la bande du bas : vélocité des notes, ou une voie de contrôle (modulation, pédale, molette…) qu'on dessine à la souris.
    this.laneId = laneById(localStorageGet("daw-pr-lane") ?? "vel").id;
    this.laneSel = new Set(); // points sélectionnés (pointId)
    this.laneWork = null; // copie de la voie pendant un geste (aperçu) puis jusqu'à la réponse de l'hôte
    this.laneCtls = [];
    this.body = this.root.querySelector(".pr-body");
    this.state.rollRows = clampRows(Number(localStorageGet("daw-pr-rows")) || ROLL_ROWS_DEFAULT);
    document.documentElement.style.setProperty("--roll-rows", String(this.state.rollRows));
    this.docked = false; // présentation « dans la piste » : lignes sous la piste, sur l'axe du temps de la timeline
    this.dockHost = null; // { zoom(direction), layout() } donnés par main.js (la timeline et les en-têtes)
    this.shown = false;
    this.clipId = null;
    this.data = null; // dernier événement `notes`
    this.sel = new Set();
    this.view = { x: 0, ppw: 240, top: 96, keyH: 14 };
    this.grid = gridById(localStorageGet("daw-pr-grid") ?? "1/16");
    this.drag = null;
    this.dirty = true;
    this.lastLen = null;
    this.lastVel = 0.8;
    for (const g of GRIDS) this.gridSel.append(new Option(`${g.id === "free" ? "Libre" : g.id} — ${g.label}`, g.id));
    this.gridSel.value = this.grid.id;
    this.gridSel.addEventListener("change", () => this.setGrid(this.gridSel.value));
    document.getElementById("pr-quantize").addEventListener("click", () => this.quantize());
    document.getElementById("pr-zoom-in").addEventListener("click", () => this.zoom(1.25));
    document.getElementById("pr-zoom-out").addEventListener("click", () => this.zoom(0.8));
    document.getElementById("pr-zoomy-in").addEventListener("click", () => this.zoomY(1.2));
    document.getElementById("pr-zoomy-out").addEventListener("click", () => this.zoomY(1 / 1.2));
    this.touches = new Map(); // doigts posés (pointerId → position dans le repère de la zone touchée)
    this.pinch = null;
    this.root.querySelector(".close").addEventListener("click", () => this.close());
    document.getElementById("pr-dock").addEventListener("click", () => this.dock());
    // Propriétés des notes sélectionnées : une barre dans la fenêtre flottante, une dans l'ancrage.
    this.props = new NoteProps((patch) => this.#setProps(patch));
    this.dockProps = new NoteProps((patch) => this.#setProps(patch));
    this.root.querySelector("header").after(this.props.el);
    this.#buildDockElements();
    this.root.querySelector("header").insertBefore(this.#laneControl(), document.getElementById("pr-dock"));
    this.#cornerText();
    this.dockHead.querySelector(".rh-ctl").append(this.#laneControl("rh-row"));
    this.dockHead.querySelector(".rh-ctl").prepend(this.dockProps.el);
    new ResizeObserver(() => this.resize()).observe(this.body);
    new ResizeObserver(() => this.resize()).observe(this.dockBody);
    client.on("notes", (e) => this.onNotes(e));
    client.on("project", () => this.onProject());
    this.#bind();
  }

  isOpen() {
    return this.shown && this.clipId !== null;
  }

  /** Éléments de la présentation « dans la piste » : l'en-tête (clavier + commandes, dans la colonne des pistes) et le corps
   * (notes + vélocités, sur la largeur de la timeline). Créés une fois ; les canevas y sont déplacés au besoin. */
  #buildDockElements() {
    const head = document.createElement("div");
    head.className = "roll-head";
    head.innerHTML = `
      <div class="rh-keys"></div>
      <div class="rh-ctl">
        <div class="rh-title"></div>
        <select class="rh-grid" aria-label="Grille de magnétisme"></select>
        <div class="rh-row"><button class="rh-q" title="Aimante le début de toutes les notes du clip sur la grille">Quantifier</button></div>
        <div class="rh-row"><span aria-hidden="true">↕</span><button class="rh-zy-out" aria-label="Zoom vertical arrière">−</button><button class="rh-zy-in" aria-label="Zoom vertical avant">＋</button></div>
        <div class="rh-row"><span title="Hauteur du piano roll, en lignes de la timeline">Haut.</span><button class="rh-h-out" aria-label="Moins haut" title="Moins haut">−</button><button class="rh-h-in" aria-label="Plus haut" title="Plus haut">＋</button></div>
        <div class="rh-row"><button class="rh-float" title="Détacher en fenêtre flottante">⇱ Fenêtre</button><button class="rh-close" aria-label="Fermer le piano roll">✕</button></div>
      </div>`;
    this.dockHead = head;
    const body = document.createElement("div");
    body.className = "roll-dock";
    body.hidden = true;
    this.dockBody = body;
    document.getElementById("tracks-wrap").append(body);
    const sel = head.querySelector(".rh-grid");
    for (const g of GRIDS) sel.append(new Option(`${g.id === "free" ? "Libre" : g.id} — ${g.label}`, g.id));
    sel.value = this.grid.id;
    sel.addEventListener("change", () => this.setGrid(sel.value));
    head.querySelector(".rh-q").addEventListener("click", () => this.quantize());
    head.querySelector(".rh-zy-in").addEventListener("click", () => this.zoomY(1.2));
    head.querySelector(".rh-zy-out").addEventListener("click", () => this.zoomY(1 / 1.2));
    head.querySelector(".rh-h-in").addEventListener("click", () => this.setRows(this.state.rollRows + 1));
    head.querySelector(".rh-h-out").addEventListener("click", () => this.setRows(this.state.rollRows - 1));
    // Poignée en bas du piano roll : on tire pour changer la hauteur (par lignes entières de la timeline).
    const grip = document.createElement("div");
    grip.className = "roll-grip";
    grip.title = "Tirer pour changer la hauteur du piano roll";
    body.append(grip);
    let drag = null;
    grip.addEventListener("pointerdown", (e) => {
      grip.setPointerCapture?.(e.pointerId);
      drag = { y: e.clientY, rows: this.state.rollRows };
    });
    grip.addEventListener("pointermove", (e) => {
      if (!drag) return;
      const rowH = (this.dockHost?.timeline.rowH ?? 92) * (getScale() / 100);
      this.setRows(drag.rows + (e.clientY - drag.y) / rowH);
    });
    const end = () => (drag = null);
    grip.addEventListener("pointerup", end);
    grip.addEventListener("pointercancel", end);
    head.querySelector(".rh-float").addEventListener("click", () => this.undock());
    head.querySelector(".rh-close").addEventListener("click", () => this.close());
  }

  #setTitle(c) {
    document.getElementById("pr-title").textContent = `Piano roll — ${c.clip.name} (${c.node.name})`;
    this.dockHead.querySelector(".rh-title").textContent = `${c.clip.name} (${c.node.name})`;
  }

  /** Hauteur du piano roll ancré, en lignes de la timeline (mémorisée). */
  setRows(n) {
    const rows = clampRows(n);
    if (rows === this.state.rollRows) return;
    this.state.rollRows = rows;
    localStorageSet("daw-pr-rows", String(rows));
    document.documentElement.style.setProperty("--roll-rows", String(rows));
    if (this.docked) {
      this.state.dockRows = rows;
      this.#relayout();
    }
  }

  setGrid(id) {
    this.grid = gridById(id);
    this.gridSel.value = this.grid.id;
    const dock = this.dockHead.querySelector(".rh-grid");
    if (dock) dock.value = this.grid.id;
    localStorageSet("daw-pr-grid", this.grid.id);
    this.lastLen = null;
    this.dirty = true;
  }

  /** Le piano roll s'ancre sous la piste du clip : quatre lignes de la timeline, sur son axe du temps. */
  dock() {
    const c = this.#clip();
    if (!c) return;
    this.onDock?.(); // la partition ancrée passe en fenêtre : un seul ancrage à la fois
    this.docked = true;
    this.state.dockRows = this.state.rollRows;
    this.state.dockHeadEl = this.dockHead;
    localStorageSet("daw-pr-mode", "dock");
    this.state.rollDockNode = c.node.id;
    this.dockHead.querySelector(".rh-keys").append(this.keys);
    this.dockBody.append(this.roll, this.vel);
    this.root.hidden = true;
    this.dockBody.hidden = false;
    this.shown = true;
    this.#relayout();
  }

  undock() {
    this.docked = false;
    localStorageSet("daw-pr-mode", "float");
    this.state.rollDockNode = null;
    this.state.dockHeadEl = null;
    this.state.dockRows = null;
    this.body.prepend(this.keys);
    this.keys.after(this.roll);
    this.corner.after(this.vel);
    this.dockBody.hidden = true;
    this.root.hidden = false;
    this.#relayout();
    this.#fit();
  }

  #relayout() {
    this.dockHost?.layout();
    requestAnimationFrame(() => {
      this.resize();
      this.dockHost?.layout();
      this.dirty = true;
    });
  }

  /** Place le corps ancré sur ses lignes de la timeline (appelé à chaque image tant que le piano roll est ancré). */
  layoutDock() {
    const tl = this.dockHost?.timeline;
    if (!this.docked || !tl) return;
    const i = tl.nodes().findIndex((r) => r.kind === "roll");
    if (i < 0) return;
    const top = i * tl.rowH;
    const h = this.state.rollRows * tl.rowH;
    this.dockBody.style.top = `${top}px`;
    this.dockBody.style.height = `${h - 1}px`;
    const key = `${tl.width}|${h}`;
    if (key !== this.dockKey) {
      this.dockKey = key;
      this.resize();
    }
    const v = this.state.view;
    const vkey = `${v.start}|${v.spp}`;
    if (vkey !== this.viewKey) {
      this.viewKey = vkey;
      this.dirty = true;
    }
  }

  // ---- ouverture / fermeture -------------------------------------------------------------------------------------

  open(clipId, mode = localStorageGet("daw-pr-mode")) {
    if (!this.#clip(clipId)) return;
    this.clipId = clipId;
    this.laneSel = new Set();
    this.laneWork = null;
    this.data = null;
    this.sel.clear();
    this.shown = true;
    if (mode === "dock") this.dock();
    else {
      if (this.docked) this.undock();
      this.root.hidden = false;
    }
    this.resize();
    this.client.send({ type: "get_notes", clip: clipId });
    this.roll.focus();
  }

  close() {
    this.#releasePreview();
    this.root.hidden = true;
    this.shown = false;
    if (this.docked) {
      this.dockBody.hidden = true;
      this.docked = false;
      this.state.rollDockNode = null;
      this.state.dockHeadEl = null;
      this.state.dockRows = null;
      this.body.prepend(this.keys);
      this.keys.after(this.roll);
      this.corner.after(this.vel);
      this.#relayout();
    }
    this.clipId = null;
    this.drag = null;
  }

  #clip(id = this.clipId) {
    for (const n of this.state.project?.nodes ?? []) for (const c of n.clips) if (c.id === id && c.kind === "midi") return { clip: c, node: n };
    return null;
  }

  onProject() {
    if (!this.isOpen()) return;
    if (!this.#clip()) return this.close(); // le clip a disparu (supprimé, ou édition annulée)
    this.client.send({ type: "get_notes", clip: this.clipId });
    const c = this.#clip();
    this.#setTitle(c);
    this.dirty = true;
  }

  onNotes(e) {
    if (!this.isOpen() || e.clip !== this.clipId) return;
    const first = this.data === null;
    const model = { start: e.start, len: e.len, offset: e.offset };
    this.data = { ...e, clipModel: model };
    this.drag = null; // la réponse du moteur remplace l'aperçu du geste
    this.laneWork = null;
    if (this.laneSel.size) {
      const ids = new Set(eventsOf(e.events, laneById(this.laneId)).map(pointId));
      this.laneSel = new Set([...this.laneSel].filter((id) => ids.has(id)));
    }
    if (e.selection.length) this.sel = new Set(e.selection.map((i) => noteId(e.notes[i])).filter(Boolean));
    else {
      const ids = new Set(e.notes.map(noteId));
      this.sel = new Set([...this.sel].filter((id) => ids.has(id)));
    }
    if (first) this.#fit();
    const c = this.#clip();
    if (c) this.#setTitle(c);
    this.dirty = true;
  }

  // ---- vue ------------------------------------------------------------------------------------------------------

  /** Cadre la partie visible du clip en largeur et centre les notes en hauteur. */
  #fit() {
    if (!this.data) return;
    const m = this.data.clipModel;
    const lenW = fnum(m.len);
    this.view.ppw = Math.max(40, Math.min(2400, (this.#w(this.roll) - 24) / Math.max(lenW, 1 / 16)));
    this.view.x = fnum(m.offset) - 12 / this.view.ppw;
    const keys = this.data.notes.map((n) => n.key);
    const mid = keys.length ? (Math.min(...keys) + Math.max(...keys)) / 2 : 66;
    const rows = this.#h(this.roll) / this.view.keyH;
    this.view.top = Math.min(127, Math.max(rows - 1, mid + rows / 2));
  }

  zoom(f, anchorX = this.#w(this.roll) / 2) {
    if (this.docked) return this.dockHost?.zoom(f > 1 ? 1 : -1, anchorX); // le zoom horizontal est celui de la timeline
    const before = this.view.x + anchorX / this.view.ppw;
    this.view.ppw = Math.max(20, Math.min(4800, this.view.ppw * f));
    this.view.x = before - anchorX / this.view.ppw;
    this.dirty = true;
  }

  /** Zoom vertical : la touche sous `anchorY` reste sous le doigt. */
  zoomY(f, anchorY = this.#h(this.roll) / 2) {
    const key = this.view.top - anchorY / this.view.keyH;
    this.view.keyH = Math.max(8, Math.min(48, this.view.keyH * f));
    this.view.top = key + anchorY / this.view.keyH;
    this.#clampView();
    this.dirty = true;
  }

  /**
   * Gestes à deux doigts (écran tactile) sur n'importe quelle zone : pincer pour zoomer (horizontalement et/ou verticalement selon
   * la direction des doigts), glisser à deux pour défiler. Rend `true` si l'événement est consommé. Un geste en cours (déplacement
   * de note, ajout différé) est abandonné dès que le deuxième doigt se pose.
   */
  #touch(e, canvas, phase) {
    if (e.pointerType !== "touch") return false;
    const p = canvasPoint(canvas, e);
    if (phase === "down") {
      this.touches.set(e.pointerId, { ...p, canvas });
      if (this.touches.size >= 2) {
        this.#cancelGesture();
        this.#startPinch();
        return true;
      }
      return false;
    }
    if (phase === "move") {
      if (!this.touches.has(e.pointerId)) return this.pinch !== null;
      this.touches.set(e.pointerId, { ...p, canvas });
      if (this.pinch && this.touches.size >= 2) {
        this.#updatePinch();
        return true;
      }
      return this.pinch !== null;
    }
    // up / cancel
    const was = this.pinch !== null;
    this.touches.delete(e.pointerId);
    if (this.touches.size < 2) this.pinch = null;
    return was;
  }

  #cancelGesture() {
    this.pendingAdd = null;
    this.drag = null;
    this.#releasePreview();
    this.dirty = true;
  }

  #startPinch() {
    const [a, b] = [...this.touches.values()];
    // Les deux doigts sont ramenés dans le repère de la zone des notes (la bande de vélocité n'a pas la même hauteur).
    this.pinch = { a: this.#rollPoint(a), b: this.#rollPoint(b) };
    this.pinchStart = { a: this.pinch.a, b: this.pinch.b };
    this.pinchMode = null; // décidé après un mouvement suffisant : voir `pinchMode`
    this.dockAcc = 1;
  }

  #rollPoint(t) {
    const r = t.canvas.getBoundingClientRect();
    const rr = this.roll.getBoundingClientRect();
    const k = t.canvas.clientWidth / (r.width || 1);
    return { x: t.x + (r.left - rr.left) * k, y: t.canvas === this.roll ? t.y : (this.#h(this.roll) / 2) };
  }

  #updatePinch() {
    const [a, b] = [...this.touches.values()].map((t) => this.#rollPoint(t));
    const f = pinchFactors(this.pinch.a, this.pinch.b, a, b);
    const bothOnRoll = [...this.touches.values()].every((t) => t.canvas === this.roll);
    this.pinchMode ??= pinchMode(this.pinchStart.a, this.pinchStart.b, a, b);
    const mode = this.pinchMode;
    if (mode === "x") {
      if (!this.docked) {
        if (f.fx !== 1) this.zoom(f.fx, f.mid.x);
      } else {
        // Ancré : le zoom horizontal est celui de la timeline, par crans de quart d'octave ; on cumule le facteur des doigts.
        this.dockAcc *= f.fx;
        const step = 2 ** 0.25;
        while (this.dockAcc >= step) {
          this.dockHost?.zoom(1, f.mid.x);
          this.dockAcc /= step;
        }
        while (this.dockAcc <= 1 / step) {
          this.dockHost?.zoom(-1, f.mid.x);
          this.dockAcc *= step;
        }
      }
    }
    if (mode === "y" && f.fy !== 1 && bothOnRoll) this.zoomY(f.fy, f.mid.y);
    if (!this.docked) this.view.x -= f.dx / this.view.ppw;
    if (bothOnRoll) this.view.top += f.dy / this.view.keyH;
    this.#clampView();
    this.pinch = { a, b };
    this.dirty = true;
  }

  #w(c) {
    return c.clientWidth || 1;
  }
  #h(c) {
    return c.clientHeight || 1;
  }

  resize() {
    if (!this.shown) return;
    const dpr = pixelRatio();
    for (const c of [this.keys, this.roll, this.vel]) {
      c.width = Math.max(1, Math.round(this.#w(c) * dpr));
      c.height = Math.max(1, Math.round(this.#h(c) * dpr));
    }
    this.#clampView();
    this.dirty = true;
  }

  #clampView() {
    const rows = this.#h(this.roll) / this.view.keyH;
    this.view.top = Math.max(Math.min(127, rows - 1), Math.min(127, this.view.top));
  }

  // ---- conversions ----------------------------------------------------------------------------------------------

  // Axe du temps : en fenêtre flottante, le sien (zoom et défilement propres) ; ancré, celui de la timeline (en échantillons : la
  // carte des tempos s'applique, le piano roll reste aligné sur les clips).
  #mx = (m) => (this.docked ? sampleToX(this.state.view, this.state.tempo.samplesAt(this.#absF(m))) : (m - this.view.x) * this.view.ppw);
  #xm = (x) => (this.docked ? this.#matF(this.state.tempo.wholeAt(Math.max(0, xToSample(this.state.view, x)))) : this.view.x + x / this.view.ppw);
  /** Largeur en pixels d'une durée `len` (rondes) commençant à `start` (rondes, depuis le début de la matière). */
  #wlen = (start, len) => this.#mx(start + len) - this.#mx(start);
  #ky = (k) => (this.view.top - k) * this.view.keyH;
  #yk = (y) => Math.floor(this.view.top - y / this.view.keyH);
  #cm() {
    return this.data.clipModel;
  }
  /** Position absolue (rondes, flottant) d'une position de la matière (flottant). */
  #absF(m) {
    const c = this.#cm();
    return fnum(c.start) - fnum(c.offset) + m;
  }
  #matF(abs) {
    const c = this.#cm();
    return abs - fnum(c.start) + fnum(c.offset);
  }

  // ---- dessin ---------------------------------------------------------------------------------------------------

  draw() {
    this.layoutDock();
    if (!this.dirty || !this.isOpen()) return;
    this.dirty = false;
    if (!this.data) return;
    this.#drawKeys();
    this.#drawRoll();
    this.#drawVel();
    this.#updateProps();
  }

  /** Notes sélectionnées (celles du moteur). */
  #selectedNotes() {
    return this.data ? this.data.notes.filter((n) => this.sel.has(noteId(n))) : [];
  }

  #updateProps() {
    const notes = this.#selectedNotes();
    this.props.update(notes);
    this.dockProps.update(notes);
  }

  /** Change le canal ou la vélocité de toute la sélection (une seule commande, un pas d'annulation). */
  #setProps(patch) {
    if (!this.data) return;
    const indices = this.#indicesOf(this.sel);
    const edits = indices.filter((i) => (patch.channel !== undefined && (this.data.notes[i].channel ?? 0) !== patch.channel) || (patch.velocity !== undefined && Math.round(this.data.notes[i].velocity * 127) !== Math.round(patch.velocity * 127))).map((index) => ({ index, ...patch }));
    this.#send(edits, patch.channel !== undefined ? "Canal des notes" : "Vélocité des notes");
  }

  #ctx(c) {
    const g = c.getContext("2d");
    const k = c.width / this.#w(c);
    g.setTransform(k, 0, 0, k, 0, 0);
    return g;
  }

  #shown() {
    // Notes affichées : celles du moteur, remplacées par l'aperçu du geste en cours.
    const notes = this.data.notes.map((n, i) => ({ ...n, i }));
    if (this.drag?.preview) for (const p of this.drag.preview) notes[p.index] = { ...notes[p.index], ...p.note };
    return notes;
  }

  #drawKeys() {
    const g = this.#ctx(this.keys);
    const W = this.#w(this.keys);
    const H = this.#h(this.keys);
    g.clearRect(0, 0, W, H);
    g.font = "10px system-ui, sans-serif";
    g.textBaseline = "middle";
    for (let k = Math.min(127, Math.ceil(this.view.top)); k >= 0; k--) {
      const y = this.#ky(k);
      if (y > H) break;
      g.fillStyle = isBlack(k) ? "#25282f" : "#e8eaee";
      g.fillRect(0, y, W, this.view.keyH);
      g.fillStyle = "rgba(0,0,0,.25)";
      g.fillRect(0, y + this.view.keyH - 1, W, 1);
      if (k % 12 === 0) {
        g.fillStyle = "#333";
        g.fillText(keyName(k), 4, y + this.view.keyH / 2);
      }
    }
    if (this.heldKey !== undefined) {
      g.fillStyle = css("--accent");
      g.fillRect(0, this.#ky(this.heldKey), W, this.view.keyH - 1);
    }
  }

  #drawRoll() {
    const g = this.#ctx(this.roll);
    const W = this.#w(this.roll);
    const H = this.#h(this.roll);
    const c = this.#cm();
    const [line, muted, accent, bg] = [css("--line"), css("--muted"), css("--accent"), css("--bg")];
    g.fillStyle = bg;
    g.fillRect(0, 0, W, H);
    // Rangées : touches noires un peu plus sombres, trait au-dessus de chaque Do.
    for (let k = Math.min(127, Math.ceil(this.view.top)); k >= 0; k--) {
      const y = this.#ky(k);
      if (y > H) break;
      g.fillStyle = isBlack(k) ? "rgba(0,0,0,.18)" : "rgba(255,255,255,.03)";
      g.fillRect(0, y, W, this.view.keyH);
      if (k % 12 === 0) {
        g.fillStyle = "rgba(128,128,128,.35)";
        g.fillRect(0, y + this.view.keyH - 1, W, 1);
      }
    }
    // Hors de la partie visible du clip : assombri.
    const x0 = this.#mx(fnum(c.offset));
    const x1 = this.#mx(fnum(c.offset) + fnum(c.len));
    g.fillStyle = "rgba(0,0,0,.35)";
    if (x0 > 0) g.fillRect(0, 0, Math.min(W, x0), H);
    if (x1 < W) g.fillRect(Math.max(0, x1), 0, W - Math.max(0, x1), H);
    this.#drawGrid(g, W, H, line, muted);
    // Notes.
    const shown = this.#shown();
    for (const n of shown) {
      const x = this.#mx(fnum(n.start));
      const w = Math.max(3, this.#wlen(fnum(n.start), fnum(n.len)));
      if (x + w < 0 || x > W) continue;
      const y = this.#ky(n.key);
      if (y + this.view.keyH < 0 || y > H) continue;
      const selected = this.sel.has(noteId(n));
      g.globalAlpha = 0.4 + 0.6 * n.velocity;
      g.fillStyle = selected ? "#ffd166" : accent;
      g.fillRect(x, y + 1, w, this.view.keyH - 2);
      g.globalAlpha = 1;
      g.strokeStyle = selected ? "#fff" : "rgba(0,0,0,.45)";
      g.strokeRect(x + 0.5, y + 1.5, w - 1, this.view.keyH - 3);
    }
    if (this.drag?.rubber) {
      const r = this.drag.rubber;
      g.strokeStyle = "#fff";
      g.fillStyle = "rgba(255,255,255,.12)";
      g.fillRect(Math.min(r.x0, r.x1), Math.min(r.y0, r.y1), Math.abs(r.x1 - r.x0), Math.abs(r.y1 - r.y0));
      g.strokeRect(Math.min(r.x0, r.x1) + 0.5, Math.min(r.y0, r.y1) + 0.5, Math.abs(r.x1 - r.x0), Math.abs(r.y1 - r.y0));
    }
    this.#drawPlayhead(g, H);
  }

  #drawGrid(g, W, H, line, muted) {
    const { tempo, meters_: meters } = this.state;
    const gf = this.grid.f[0] / this.grid.f[1];
    const left = this.#absF(this.#xm(0));
    const right = this.#absF(this.#xm(W));
    // Pas de la grille (si assez large pour rester lisible).
    if (this.#wlen(fnum(this.#cm().offset), gf) >= 5) {
      g.fillStyle = "rgba(128,128,128,.14)";
      for (let k = Math.floor(left / gf); k * gf <= right; k++) g.fillRect(Math.round(this.#mx(this.#matF(k * gf))), 0, 1, H);
    }
    // Temps et mesures d'après les cartes de tempo et de chiffrage du projet.
    if (!meters || !tempo) return;
    let bar = Math.max(0, meters.locate(Math.max(0, left)).bar);
    for (; bar < 100000; bar++) {
      const start = meters.barStart(bar);
      if (start > right) break;
      const { num, den } = meters.meterAtBar(bar);
      for (let b = 0; b < num; b++) {
        const pos = start + b / den;
        if (pos < left - 1 || pos > right) continue;
        const x = Math.round(this.#mx(this.#matF(pos)));
        g.fillStyle = b === 0 ? "rgba(200,200,200,.55)" : "rgba(160,160,160,.3)";
        g.fillRect(x, 0, 1, H);
        if (b === 0) {
          g.fillStyle = muted;
          g.font = "10px system-ui, sans-serif";
          g.textBaseline = "top";
          g.fillText(String(bar + 1), x + 3, 2);
        }
      }
    }
  }

  #drawPlayhead(g, H) {
    const { tempo, pos } = this.state;
    if (!tempo) return;
    const m = this.#matF(Math.max(0, tempo.wholeAt(pos)));
    const x = Math.round(this.#mx(m)) + 0.5;
    g.strokeStyle = "rgba(0,0,0,.7)";
    g.lineWidth = 3;
    g.beginPath();
    g.moveTo(x, 0);
    g.lineTo(x, H);
    g.stroke();
    g.strokeStyle = css("--play") || "#2ecc71";
    g.lineWidth = 1;
    g.stroke();
  }

  // ---- voies de contrôle ------------------------------------------------------------------------------------------

  /** Le choix de ce que montre la bande du bas (une copie dans la fenêtre, une dans l'ancrage) ; « Autre » demande un numéro de contrôleur. */
  #laneControl(cls = "") {
    const wrap = document.createElement("div");
    wrap.className = `pr-lane ${cls}`.trim();
    const sel = document.createElement("select");
    sel.className = "pr-lane-sel";
    sel.setAttribute("aria-label", "Ce que montre la bande du bas");
    sel.title = "Vélocité des notes ou voie de contrôle à dessiner (modulation, volume, pédale, molette…)";
    for (const l of LANE_DEFS) sel.add(new Option(l.label, l.id));
    sel.add(new Option("Autre contrôleur (numéro)…", "other"));
    const num = document.createElement("input");
    num.type = "number";
    num.min = "0";
    num.max = "127";
    num.step = "1";
    num.className = "pr-lane-num";
    num.setAttribute("aria-label", "Numéro du contrôleur (0 à 127)");
    num.hidden = true;
    const ctl = {
      sync: (id) => {
        const known = LANE_DEFS.some((l) => l.id === id);
        sel.value = known ? id : "other";
        num.hidden = known;
        if (!known) num.value = String(laneById(id).controller ?? "");
      },
    };
    sel.addEventListener("change", () => {
      if (sel.value === "other") {
        num.hidden = false;
        num.value ||= "74";
        this.#setLane(`cc${Math.min(127, Math.max(0, Math.round(Number(num.value) || 0)))}`);
      } else this.#setLane(sel.value);
    });
    num.addEventListener("change", () => this.#setLane(`cc${Math.min(127, Math.max(0, Math.round(Number(num.value) || 0)))}`));
    num.addEventListener("keydown", (e) => e.stopPropagation());
    const thin = document.createElement("button");
    thin.className = "pr-lane-thin";
    thin.textContent = "Amincir";
    thin.title = "Retire les points inutiles de la courbe (la sélection, ou toute la voie) sans changer son allure ; Ctrl+C / X / V copient, coupent et collent des points";
    thin.addEventListener("click", () => this.#laneThin());
    const oldSync = ctl.sync;
    ctl.sync = (id) => {
      oldSync(id);
      thin.hidden = id === "vel";
    };
    wrap.append(sel, num, thin);
    this.laneCtls.push(ctl);
    ctl.sync(this.laneId);
    return wrap;
  }

  /** Le petit texte du coin (56 px) : ce que montre la bande. */
  #cornerText() {
    const l = laneById(this.laneId);
    this.corner.textContent = isVelocity(l) ? "Vélocité" : l.kind === "cc" ? `CC ${l.controller}` : l.kind === "bend" ? "Molette" : "Pression";
    this.corner.title = l.label;
  }

  #setLane(id) {
    this.laneId = laneById(id).id;
    localStorageSet("daw-pr-lane", this.laneId);
    this.drag = null;
    this.laneWork = null;
    this.laneSel = new Set();
    for (const c of this.laneCtls) c.sync(this.laneId);
    this.#cornerText();
    this.dirty = true;
  }

  /** Les points de la voie : la copie en cours d'édition, sinon ceux de l'hôte. */
  #laneList(lane) {
    return this.laneWork ? this.laneWork.list : eventsOf(this.data?.events, lane).map((e) => ({ start: e.start, channel: e.channel, value: e.value }));
  }

  /** Canal des points neufs : celui des points déjà là, sinon celui de la première note, sinon le premier canal. */
  #newChannel(lane) {
    return eventsOf(this.data?.events, lane)[0]?.channel ?? this.data?.notes?.[0]?.channel ?? 0;
  }

  /** Instant d'un pixel, aimanté sur la grille absolue du projet (Alt : au tick) ; `null` hors de la partie visible du clip. */
  #laneTimeAt(x, alt) {
    const abs = this.#absF(this.#xm(x));
    const snapped = snapAbs(abs, alt ? [1, 3840] : this.grid.f, "round");
    const c = this.#cm();
    const start = absToMat({ start: c.start, offset: c.offset }, snapped);
    return fcmp(start, c.offset) >= 0 && fcmp(start, fadd(c.offset, c.len)) < 0 ? start : null;
  }

  #laneDown(e, lane) {
    const vel = this.vel;
    vel.setPointerCapture(e.pointerId);
    vel.focus?.({ preventScroll: true });
    const { x, y } = canvasPoint(vel, e);
    const list = this.#laneList(lane);
    const hit = nearestPoint(list, x, (p) => this.#mx(fnum(p.start)), e.pointerType === "touch" ? 14 : 8);
    if (hit >= 0) {
      const id = pointId(list[hit]);
      if (e.shiftKey) this.laneSel.has(id) ? this.laneSel.delete(id) : this.laneSel.add(id);
      else if (!this.laneSel.has(id)) this.laneSel = new Set([id]);
      this.laneWork = { mode: "move", list: list.map((p) => ({ ...p })), orig: list.map((p) => ({ ...p })), grab: { x, y }, ids: new Set(this.laneSel), moved: false };
    } else {
      this.laneSel = new Set();
      this.laneWork = { mode: "paint", list: list.map((p) => ({ ...p })), orig: list.map((p) => ({ ...p })), last: null, moved: false };
      this.#lanePaint(x, y, e.altKey, lane);
    }
    this.dirty = true;
  }

  /** Pose des points sous le geste : un par case de la grille entre la dernière position et celle-ci, valeurs en ligne droite. */
  #lanePaint(x, y, alt, lane) {
    const w = this.laneWork;
    const H = this.#h(this.vel);
    const value = valueOfY(lane, y, H);
    const channel = this.#newChannel(lane);
    const step = alt ? null : this.grid.f[0] / this.grid.f[1];
    const from = w.last;
    const to = { m: this.#xm(x), value };
    const marks = [];
    if (from && step) {
      const n = Math.min(512, Math.floor(Math.abs(to.m - from.m) / step));
      for (let i = 1; i <= n; i++) {
        const t = i / (n + 1);
        marks.push({ x: this.#mx(from.m + (to.m - from.m) * t), value: Math.round(from.value + (to.value - from.value) * t) });
      }
    }
    marks.push({ x, value });
    for (const mk of marks) {
      if (w.list.length >= 4000) break;
      const start = this.#laneTimeAt(mk.x, alt);
      if (!start) continue;
      w.list = withPoint(w.list, { start, channel, value: mk.value });
      w.moved = true;
    }
    w.last = to;
    this.dirty = true;
  }

  #laneMove(e, lane) {
    const w = this.laneWork;
    if (!w) return;
    const { x, y } = canvasPoint(this.vel, e);
    if (w.mode === "paint") return this.#lanePaint(x, y, e.altKey, lane);
    const H = this.#h(this.vel);
    const dv = valueOfY(lane, y, H) - valueOfY(lane, w.grab.y, H);
    const dm = this.#xm(x) - this.#xm(w.grab.x);
    let list = w.orig.filter((p) => !w.ids.has(pointId(p))).map((p) => ({ ...p }));
    const moved = [];
    for (const p of w.orig) {
      if (!w.ids.has(pointId(p))) continue;
      const m = fnum(p.start) + dm;
      const t = this.#laneTimeAt(this.#mx(m), e.altKey);
      moved.push({ ...p, start: t ?? p.start, value: Math.max(lane.min, Math.min(lane.max, p.value + dv)) });
    }
    for (const p of moved) list = withPoint(list, p);
    w.list = list;
    w.moved = Math.abs(x - w.grab.x) > 2 || Math.abs(y - w.grab.y) > 2;
    this.laneSel = new Set(moved.map(pointId)); // `w.ids` garde les identifiants d'origine : le geste repart toujours des positions de départ
    this.dirty = true;
  }

  #laneUp(commit, lane) {
    const w = this.laneWork;
    if (!w) return;
    if (commit && w.moved) {
      this.client.send(laneCommand(this.clipId, lane, w.list.filter((p) => fcmp(p.start, this.#cm().offset) >= 0), w.mode === "paint" ? "Dessiner une courbe" : "Déplacer des points"));
      w.moved = false; // la copie reste affichée jusqu'à la réponse de l'hôte (pas de clignotement)
    } else this.laneWork = null;
    this.dirty = true;
  }

  #laneKey(e) {
    const lane = laneById(this.laneId);
    if (isVelocity(lane) || !this.data) return;
    const mod = e.ctrlKey || e.metaKey;
    if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      e.stopPropagation();
      if (!this.laneSel.size) return;
      const list = withoutPoints(this.#laneList(lane), this.laneSel);
      this.laneSel = new Set();
      this.client.send(laneCommand(this.clipId, lane, list, "Supprimer des points"));
    } else if (mod && e.key.toLowerCase() === "a") {
      e.preventDefault();
      e.stopPropagation();
      this.laneSel = new Set(this.#laneList(lane).map(pointId));
      this.dirty = true;
    } else if (mod && (e.key.toLowerCase() === "c" || e.key.toLowerCase() === "x")) {
      e.preventDefault();
      e.stopPropagation();
      const list = this.#laneList(lane);
      const copied = copyPoints(list, lane, this.laneSel);
      if (!copied) return;
      laneClipboard = copied;
      if (e.key.toLowerCase() === "x") {
        const ids = this.laneSel.size ? this.laneSel : new Set(list.map(pointId));
        this.laneSel = new Set();
        this.client.send(laneCommand(this.clipId, lane, withoutPoints(list, ids), "Couper des points"));
      }
    } else if (mod && e.key.toLowerCase() === "v") {
      e.preventDefault();
      e.stopPropagation();
      this.#lanePaste(lane);
    } else if (e.key === "Escape" && this.laneWork) {
      e.preventDefault();
      e.stopPropagation();
      this.laneWork = null;
      this.dirty = true;
    }
  }

  /** Colle les points copiés à la tête de lecture (calée sur la grille) si elle est dans le clip, sinon au début de sa partie visible. */
  #lanePaste(lane) {
    if (!laneClipboard || !this.data) return;
    const c = this.#cm();
    const hi = fadd(c.offset, c.len);
    let at = c.offset;
    const { tempo, pos } = this.state;
    if (tempo) {
      const snapped = absToMat({ start: c.start, offset: c.offset }, snapAbs(tempo.wholeAt(Math.max(0, pos)), this.grid.f, "round"));
      if (fcmp(snapped, c.offset) >= 0 && fcmp(snapped, hi) < 0) at = snapped;
    }
    const { list, added } = pastePoints(this.#laneList(lane), lane, laneClipboard, at, c.offset, hi);
    if (!added.length) return;
    this.laneSel = new Set(added.map(pointId));
    this.client.send(laneCommand(this.clipId, lane, list, "Coller des points"));
  }

  /** Amincit la voie (les points sélectionnés, ou toute la voie) : retire répétitions et points qui suivent une droite, en une action annulable. */
  #laneThin() {
    const lane = laneById(this.laneId);
    if (isVelocity(lane) || !this.data) return;
    const list = this.#laneList(lane);
    const out = thinPoints(list, lane, this.laneSel);
    if (out.length === list.length) return;
    this.laneSel = new Set();
    this.client.send(laneCommand(this.clipId, lane, out, "Amincir une courbe"));
  }

  #drawLane(lane) {
    const g = this.#ctx(this.vel);
    const W = this.#w(this.vel);
    const H = this.#h(this.vel);
    g.fillStyle = css("--bg");
    g.fillRect(0, 0, W, H);
    g.fillStyle = "rgba(128,128,128,.25)";
    for (const f of [0.25, 0.5, 0.75]) g.fillRect(0, H * (1 - f), W, 1);
    // La valeur de repos en trait plein.
    g.fillStyle = "rgba(128,128,128,.55)";
    g.fillRect(0, Math.round(yOfValue(lane, lane.rest ?? lane.min, H)), W, 1);
    const list = this.#laneList(lane);
    const accent = css("--accent");
    const pts = list.map((p) => ({ p, x: this.#mx(fnum(p.start)), y: yOfValue(lane, p.value, H), sel: this.laneSel.has(pointId(p)) }));
    // Courbe en escalier : chaque valeur tient jusqu'au point suivant (c'est ce que fait le message MIDI).
    g.strokeStyle = accent;
    g.lineWidth = 2;
    g.beginPath();
    pts.forEach((q, i) => {
      const nx = i + 1 < pts.length ? pts[i + 1].x : Math.max(W, q.x);
      if (i === 0) g.moveTo(Math.min(q.x, W), q.y);
      g.lineTo(nx, q.y);
      if (i + 1 < pts.length) g.lineTo(nx, pts[i + 1].y);
    });
    g.stroke();
    for (const q of pts) {
      if (q.x < -6 || q.x > W + 6) continue;
      g.fillStyle = q.sel ? "#ffd166" : accent;
      g.beginPath();
      g.arc(q.x, q.y, q.sel ? 5 : 4, 0, Math.PI * 2);
      g.fill();
    }
    g.fillStyle = css("--muted") || "#888";
    g.font = "10px system-ui, sans-serif";
    g.textBaseline = "top";
    const sel = pts.filter((q) => q.sel);
    g.fillText(sel.length ? `${lane.label} — ${sel.length} point${sel.length > 1 ? "s" : ""} : ${valueText(lane, sel[0].p.value)}` : `${lane.label} — clic ou glisser pour dessiner · Suppr`, 6, 4);
  }

  #drawVel() {
    const lane = laneById(this.laneId);
    if (!isVelocity(lane)) return this.#drawLane(lane);
    const g = this.#ctx(this.vel);
    const W = this.#w(this.vel);
    const H = this.#h(this.vel);
    g.fillStyle = css("--bg");
    g.fillRect(0, 0, W, H);
    g.fillStyle = "rgba(128,128,128,.25)";
    for (const f of [0.25, 0.5, 0.75]) g.fillRect(0, H * (1 - f), W, 1);
    for (const n of this.#shown()) {
      const x = Math.round(this.#mx(fnum(n.start)));
      if (x < -2 || x > W + 2) continue;
      const selected = this.sel.has(noteId(n));
      g.fillStyle = selected ? "#ffd166" : css("--accent");
      const h = Math.max(2, n.velocity * (H - 6));
      g.fillRect(x, H - h, 3, h);
      g.fillRect(x - 2, H - h, 7, 3);
    }
  }

  // ---- souris ---------------------------------------------------------------------------------------------------

  #hitNote(x, y) {
    const shown = this.#shown();
    for (let i = shown.length - 1; i >= 0; i--) {
      const n = shown[i];
      const nx = this.#mx(fnum(n.start));
      const w = Math.max(3, this.#wlen(fnum(n.start), fnum(n.len)));
      const ny = this.#ky(n.key);
      if (x >= nx && x <= nx + w && y >= ny && y < ny + this.view.keyH) return { note: n, zone: x >= nx + w - Math.min(this.touchEdge ? EDGE * 2.5 : EDGE, w / 3) ? "resize" : "move" };
    }
    return null;
  }

  #indicesOf(ids) {
    return this.data.notes.map((n, i) => (ids.has(noteId(n)) ? i : -1)).filter((i) => i >= 0);
  }

  #send(edits, label) {
    if (edits.length) this.client.send({ type: "edit_notes", clip: this.clipId, edits, label });
  }

  #preview(key, velocity = this.lastVel) {
    this.#releasePreview();
    const node = this.#clip()?.node;
    if (!node) return;
    this.held = { node: node.id, key };
    this.client.send({ type: "preview_note", node: node.id, key, velocity, on: true });
    this.heldTimer = setTimeout(() => this.#releasePreview(), 400);
  }

  #releasePreview() {
    clearTimeout(this.heldTimer);
    if (this.held) this.client.send({ type: "preview_note", node: this.held.node, key: this.held.key, velocity: 0.5, on: false });
    this.held = null;
    if (this.heldKey !== undefined) {
      this.heldKey = undefined;
      this.dirty = true;
    }
  }

  #bind() {
    const roll = this.roll;
    roll.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || !this.data) return;
      if (this.#touch(e, roll, "down")) return;
      this.touchEdge = e.pointerType === "touch";
      roll.focus();
      roll.setPointerCapture(e.pointerId);
      const { x, y } = canvasPoint(roll, e);
      const hit = this.#hitNote(x, y);
      const abs = this.#absF(this.#xm(x));
      if (hit) {
        const id = noteId(hit.note);
        if (e.shiftKey) {
          if (this.sel.has(id)) this.sel.delete(id);
          else this.sel.add(id);
        } else if (!this.sel.has(id)) this.sel = new Set([id]);
        this.dirty = true;
        if (!this.sel.has(id)) return;
        const indices = this.#indicesOf(this.sel);
        const before = indices.map((i) => this.data.notes[i]);
        this.drag = { type: hit.zone, indices, before, grabbed: hit.note, pointerAbs: abs, pointerKey: this.#yk(y), moved: false, preview: null };
        if (hit.zone === "move") this.#preview(hit.note.key, hit.note.velocity);
        return;
      }
      if (e.shiftKey) {
        this.drag = { type: "rubber", rubber: { x0: x, y0: y, x1: x, y1: y }, base: new Set(this.sel) };
        return;
      }
      // Clic dans le vide : une note neuve (début aimanté vers le bas, durée de la grille ou de la dernière note).
      const key = this.#yk(y);
      if (key < 0 || key > 127) return;
      const g = this.grid.f;
      const absSnap = e.altKey ? snapAbs(abs, [1, 3840], "floor") : snapAbs(abs, g, "floor");
      const c = this.#cm();
      const start = absToMat({ start: c.start, offset: c.offset }, absSnap);
      this.sel = new Set();
      this.lastLen = this.lastLen ?? g;
      const add = { type: "add_notes", clip: this.clipId, notes: [{ start, len: this.lastLen, key, velocity: this.lastVel }] };
      if (e.pointerType === "touch") {
        // Au doigt, la note n'est posée qu'au relevé (et pas si un deuxième doigt arrive : c'était un pincement).
        this.pendingAdd = { add, key, x, y };
        return;
      }
      this.client.send(add);
      this.#preview(key);
    });
    roll.addEventListener("pointermove", (e) => {
      if (this.#touch(e, roll, "move")) return;
      const { x, y } = canvasPoint(roll, e);
      if (this.pendingAdd && Math.hypot(x - this.pendingAdd.x, y - this.pendingAdd.y) > 10) {
        // Le doigt glisse dans le vide : ce n'est plus un tap (pas de note posée) mais un rectangle de sélection, comme Maj + glisser à la souris.
        const { x: x0, y: y0 } = this.pendingAdd;
        this.pendingAdd = null;
        this.drag = { type: "rubber", rubber: { x0, y0, x1: x, y1: y }, base: new Set() };
        this.dirty = true;
        return;
      }
      if (!this.drag) {
        const hit = this.#hitNote(x, y);
        roll.style.cursor = hit ? (hit.zone === "resize" ? "ew-resize" : "grab") : "crosshair";
        return;
      }
      const d = this.drag;
      if (d.type === "rubber") {
        d.rubber.x1 = x;
        d.rubber.y1 = y;
        this.dirty = true;
        return;
      }
      const abs = this.#absF(this.#xm(x));
      const clip = this.#cm();
      const notesBefore = d.before;
      if (d.type === "move") {
        const key = d.grabbed.key + (this.#yk(y) - d.pointerKey);
        const grabAbs = abs - d.pointerAbs + matAbsF(clip, d.grabbed.start);
        const after = moveGroup(clip, notesBefore, d.grabbed, grabAbs, key, this.grid.f, !e.altKey);
        d.preview = d.indices.map((index, i) => ({ index, note: after[i] }));
        if (after.some((n, i) => fcmp(n.start, notesBefore[i].start) !== 0 || n.key !== notesBefore[i].key)) d.moved = true;
        const grabbedAfter = after[notesBefore.indexOf(d.grabbed)];
        if (grabbedAfter && grabbedAfter.key !== this.held?.key && d.moved) this.#preview(grabbedAfter.key, d.grabbed.velocity);
      } else if (d.type === "resize") {
        const endAbs = abs - d.pointerAbs + matAbsF(clip, fadd(d.grabbed.start, d.grabbed.len));
        const after = resizeGroup(clip, notesBefore, d.grabbed, endAbs, this.grid.f, !e.altKey);
        d.preview = d.indices.map((index, i) => ({ index, note: after[i] }));
        if (after.some((n, i) => fcmp(n.len, notesBefore[i].len) !== 0)) d.moved = true;
      }
      this.dirty = true;
    });
    const end = (commit) => {
      const d = this.drag;
      if (!d) return;
      this.#releasePreview();
      if (d.type === "rubber") {
        if (commit) {
          const r = d.rubber;
          const [xa, xb] = [Math.min(r.x0, r.x1), Math.max(r.x0, r.x1)];
          const [ya, yb] = [Math.min(r.y0, r.y1), Math.max(r.y0, r.y1)];
          const inside = new Set(d.base);
          for (const n of this.data.notes) {
            const nx = this.#mx(fnum(n.start));
            const nw = Math.max(3, this.#wlen(fnum(n.start), fnum(n.len)));
            const ny = this.#ky(n.key);
            if (nx <= xb && nx + nw >= xa && ny <= yb && ny + this.view.keyH >= ya) inside.add(noteId(n));
          }
          this.sel = inside;
        }
        this.drag = null;
      } else if (commit && d.moved && d.preview) {
        // L'aperçu reste affiché jusqu'à la réponse du moteur (qui le remplace) : pas de saut visuel.
        const after = d.preview.map((p) => p.note);
        const edits = diffEdits(d.indices, d.before, after);
        if (d.type === "resize") this.lastLen = after[d.before.indexOf(d.grabbed)]?.len ?? this.lastLen;
        this.#send(edits, d.type === "move" ? "Déplacer des notes" : "Changer la durée des notes");
      } else this.drag = null;
      this.dirty = true;
    };
    roll.addEventListener("pointerup", (e) => {
      if (this.#touch(e, roll, "up")) return;
      if (this.pendingAdd) {
        const { add, key } = this.pendingAdd;
        this.pendingAdd = null;
        this.client.send(add);
        this.#preview(key);
        return;
      }
      end(true);
    });
    roll.addEventListener("pointercancel", (e) => {
      this.pendingAdd = null;
      if (this.#touch(e, roll, "up")) return;
      end(false);
    });
    roll.addEventListener("wheel", (e) => this.#wheel(e, roll), { passive: false });
    this.vel.addEventListener("wheel", (e) => this.#wheel(e, this.vel), { passive: false });
    roll.addEventListener("keydown", (e) => this.#key(e));

    // Bande du bas : vélocité (on tire une barre vers le haut ou le bas) ou voie de contrôle (on dessine des points).
    const vel = this.vel;
    vel.tabIndex = 0;
    vel.addEventListener("keydown", (e) => this.#laneKey(e));
    vel.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || !this.data) return;
      if (this.#touch(e, vel, "down")) return;
      const lane = laneById(this.laneId);
      if (!isVelocity(lane)) return this.#laneDown(e, lane);
      vel.setPointerCapture(e.pointerId);
      const { x, y } = canvasPoint(vel, e);
      let best = null;
      for (const n of this.#shown()) {
        const dx = Math.abs(this.#mx(fnum(n.start)) + 1 - x);
        if (dx <= 6 && (best === null || dx <= best.dx)) best = { n, dx };
      }
      if (!best) return;
      const id = noteId(best.n);
      if (e.shiftKey) this.sel.add(id);
      else if (!this.sel.has(id)) this.sel = new Set([id]);
      const indices = this.#indicesOf(this.sel);
      const before = indices.map((i) => this.data.notes[i]);
      this.drag = { type: "velocity", indices, before, grabbed: best.n, startY: y, moved: false, preview: null };
      this.#velocityMove(y);
    });
    vel.addEventListener("pointermove", (e) => {
      if (this.#touch(e, vel, "move")) return;
      if (this.laneWork?.mode) return this.#laneMove(e, laneById(this.laneId));
      if (this.drag?.type === "velocity") this.#velocityMove(canvasPoint(vel, e).y);
    });
    vel.addEventListener("pointerup", (e) => {
      if (this.#touch(e, vel, "up")) return;
      if (this.laneWork?.mode) return this.#laneUp(true, laneById(this.laneId));
      this.#endVelocity(true);
    });
    vel.addEventListener("pointercancel", (e) => {
      if (this.#touch(e, vel, "up")) return;
      if (this.laneWork?.mode) return this.#laneUp(false, laneById(this.laneId));
      this.#endVelocity(false);
    });

    // Clavier à gauche : on entend la touche tant qu'on appuie.
    this.keys.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      this.keys.setPointerCapture(e.pointerId);
      const key = this.#yk(canvasPoint(this.keys, e).y);
      if (key < 0 || key > 127) return;
      this.#releasePreview();
      const node = this.#clip()?.node;
      if (!node) return;
      this.held = { node: node.id, key };
      this.heldKey = key;
      this.client.send({ type: "preview_note", node: node.id, key, velocity: this.lastVel, on: true });
      this.dirty = true;
    });
    const upKey = () => this.#releasePreview();
    this.keys.addEventListener("pointerup", upKey);
    this.keys.addEventListener("pointercancel", upKey);
  }

  #velocityMove(y) {
    const d = this.drag;
    const H = this.#h(this.vel);
    const target = Math.max(1 / 127, Math.min(1, 1 - y / H));
    const delta = target - d.grabbed.velocity;
    d.preview = d.indices.map((index, i) => ({ index, note: { ...d.before[i], velocity: Math.max(1 / 127, Math.min(1, d.before[i].velocity + delta)) } }));
    d.moved = true;
    this.dirty = true;
  }

  #endVelocity(commit) {
    const d = this.drag;
    if (!d || d.type !== "velocity") return;
    if (commit && d.moved && d.preview) {
      const after = d.preview.map((p) => p.note);
      this.lastVel = after[d.before.indexOf(d.grabbed)]?.velocity ?? this.lastVel;
      this.#send(diffEdits(d.indices, d.before, after), "Changer la vélocité");
    } else this.drag = null;
    this.dirty = true;
  }

  #wheel(e, canvas) {
    e.preventDefault();
    const { x } = canvasPoint(canvas, e);
    if (e.ctrlKey || e.metaKey) {
      if (e.altKey) this.view.keyH = Math.max(8, Math.min(30, this.view.keyH * (e.deltaY < 0 ? 1.1 : 0.9)));
      else this.zoom(e.deltaY < 0 ? 1.15 : 1 / 1.15, canvas === this.vel ? x : x);
    } else if (e.shiftKey) {
      if (!this.docked) this.view.x += (e.deltaY || e.deltaX) / this.view.ppw;
    }
    else this.view.top += e.deltaY / this.view.keyH / 1.5;
    this.#clampView();
    this.dirty = true;
  }

  // ---- actions ----------------------------------------------------------------------------------------------------

  quantize() {
    if (!this.data) return;
    this.client.send({ type: "quantize_notes", clip: this.clipId, grid: this.grid.f });
  }

  deleteSelected() {
    const indices = this.#indicesOf(this.sel);
    if (indices.length) this.client.send({ type: "delete_notes", clip: this.clipId, indices });
  }

  #key(e) {
    if (!this.data) return;
    const mod = e.ctrlKey || e.metaKey;
    const handled = () => (e.preventDefault(), e.stopPropagation());
    if (e.key === "Escape") {
      handled();
      if (this.drag) {
        this.drag = null;
        this.#releasePreview();
        this.dirty = true;
      } else this.close();
    } else if (e.key === "Delete" || e.key === "Backspace") {
      handled();
      this.deleteSelected();
    } else if (mod && e.key.toLowerCase() === "a") {
      handled();
      this.sel = new Set(this.data.notes.map(noteId));
      this.dirty = true;
    } else if (!mod && ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(e.key)) {
      handled();
      const indices = this.#indicesOf(this.sel);
      if (!indices.length) return;
      const before = indices.map((i) => this.data.notes[i]);
      const c = this.#cm();
      const step = e.altKey ? [1, 3840] : this.grid.f;
      let after = before;
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        const sign = e.key === "ArrowRight" ? 1 : -1;
        const t = sign > 0 ? fadd(before[0].start, step) : fsub(before[0].start, step);
        after = moveGroup(c, before, before[0], fnum(matToAbs(c, t)), before[0].key, this.grid.f, false);
      } else {
        const dk = (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 12 : 1);
        after = moveGroup(c, before, before[0], fnum(matToAbs(c, before[0].start)), before[0].key + dk, this.grid.f, false);
      }
      this.#send(diffEdits(indices, before, after), "Déplacer des notes");
      this.#preview(after[0].key, after[0].velocity);
    }
  }
}

/** Position absolue (flottant) d'une position de la matière donnée en fraction. */
function matAbsF(clip, m) {
  return fnum(clip.start) - fnum(clip.offset) + fnum(m);
}

function localStorageGet(k) {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
}
function localStorageSet(k, v) {
  try {
    localStorage.setItem(k, v);
  } catch {
    // Pas de mémorisation possible : le réglage vaut pour cette session.
  }
}
