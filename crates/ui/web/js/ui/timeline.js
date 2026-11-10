// Timeline : règle et pistes dessinées sur deux canevas (clips, formes d'onde, notes, fondus, grille, boucle, tête de
// lecture) avec défilement, zoom et déplacement de la tête de lecture à la souris. Aucune logique de projet ici : tout vient
// de l'état et des événements du moteur.

import { TouchPinch } from "../touch-pinch.js";
import { gridLines, sampleToX, xToSample, zoomAt, clampView, snapSample } from "../time.js";
import { hitTest, previewDrag, commandFor, fadeHandles, targetTrackAt, crossfadeAt, crossfadeOf, CURSORS } from "./clip-geometry.js";
import { fadeGain } from "../fades.js";
import { canvasPoint, pixelRatio } from "../ui-scale.js";
import { TILE_COLUMNS } from "./peaks-cache.js";
import { orderedRows } from "./order.js";
import { StretchMarkers } from "./stretch-markers.js";
import { clickSelect, clipsInRect, clampGroupDelta, prune, groupDragCommand, trackIds, clampNodeDelta, shiftedTrack } from "../selection.js";

export const ROW_MIN = 80;
export const ROW_MAX = 200;
export const ROW_DEFAULT = 88;
const ZOOM_STEP = 2 ** 0.25; // zoom par crans de quart d'octave : les tuiles de crêtes restent réutilisables
export const snapSpp = (spp) => 2 ** (Math.round(Math.log2(spp) * 4) / 4);

const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const luminance = (hex) => {
  const n = parseInt(hex.slice(1), 16);
  return (0.299 * (n >> 16) + 0.587 * ((n >> 8) & 255) + 0.114 * (n & 255)) / 255;
};

/** Hauteur (px) de la bande de boucle, en haut de la règle, et largeur de la zone de saisie d'un bord de boucle. */
export const LOOP_H = 12;
const LOOP_EDGE = 6;

/** Ce que le point (x, y) de la règle désigne dans la bande de boucle : `start`, `end` (bords), `body`, `new` (vide), ou `null` (hors bande). */
export function loopHit(state, x, y) {
  if (y >= LOOP_H) return null;
  const { loop, view } = state;
  if (!loop) return "new";
  const x0 = sampleToX(view, loop.start);
  const x1 = sampleToX(view, loop.end);
  if (Math.abs(x - x0) <= LOOP_EDGE && Math.abs(x - x0) <= Math.abs(x - x1)) return "start";
  if (Math.abs(x - x1) <= LOOP_EDGE) return "end";
  return x > x0 && x < x1 ? "body" : "new";
}

export class Timeline {
  constructor({ tracks, ruler, scroller, state, client, peaks }) {
    this.tracks = tracks;
    this.ruler = ruler;
    this.scroller = scroller;
    this.state = state;
    this.client = client;
    this.peaks = peaks;
    this.dirty = true;
    this.width = 100;
    this.colors = {};
    this.drag = null;
    this.edit = null; // glissement de clip en cours : { hit, startX, moved, preview, committed }
    this.hover = null;
    this.band = null; // rectangle de sélection en cours
    this.takeDrag = null; // glissement sur une voie de prises (composer)
    this.state.selected ??= new Set();
    this.stretch = new StretchMarkers(this); // marqueurs d'étirement des clips audio
    this.autoFit = true; // tant que l'utilisateur n'a pas zoomé ni défilé, la vue suit la taille de la fenêtre
    this.#bind();
  }

  get rowH() {
    return parseInt(css("--row-h"), 10) || 92;
  }
  get rulerH() {
    return parseInt(css("--ruler-h"), 10) || 36;
  }

  /** Lignes dans l'ordre d'affichage : pistes et bus, la sortie principale en dernier, et sous chaque nœud dont la voie d'automation
   * est dépliée une ligne `kind: "lane"` (sans clips). */
  nodes() {
    return orderedRows(this.state.project, this.state.autoOpen, this.state.rollDockNode ?? null, this.state.dockRows ?? this.state.rollRows ?? 4, this.state.takesOpen);
  }

  refreshColors() {
    for (const k of ["bg", "row-a", "row-b", "line", "text", "muted", "accent", "panel"]) this.colors[k] = css(`--${k}`);
    this.dirty = true;
  }

  resize() {
    const w = Math.max(100, Math.floor(this.tracks.parentElement.clientWidth));
    this.width = w;
    const dpr = pixelRatio();
    const th = this.nodes().length * this.rowH;
    for (const [c, h] of [[this.tracks, th], [this.ruler, this.rulerH]]) {
      c.style.height = `${h}px`;
      c.width = Math.round(w * dpr);
      c.height = Math.round(h * dpr);
    }
    this.chordLane?.resize();
    if (this.autoFit && this.state.project) this.fit();
    this.dirty = true;
  }

  length() {
    let end = 0;
    for (const n of this.state.project.nodes) for (const c of n.clips) end = Math.max(end, c.start + c.len);
    return end + this.state.tempo.samplesAt(1);
  }

  limits() {
    return { minSpp: 1, maxSpp: snapSpp((this.state.project.sample_rate * 3600) / this.width) };
  }

  /** « Tout voir » : le bouton de zoom rend aussi à la vue son ajustement automatique. */
  fit() {
    this.state.view = { start: 0, spp: Math.min(this.limits().maxSpp, snapSpp(this.length() / this.width)) };
    this.dirty = true;
  }

  zoom(direction, anchorX = this.width / 2) {
    this.autoFit = false;
    const factor = direction > 0 ? 1 / ZOOM_STEP : ZOOM_STEP;
    this.state.view = clampView(zoomAt(this.state.view, anchorX, factor, this.limits()), this.width, this.length());
    this.dirty = true;
  }

  /** Hauteur des pistes (zoom vertical de la timeline), bornée et mémorisée ; les en-têtes suivent (même variable CSS). */
  setRowHeight(px, light = false) {
    const v = Math.max(ROW_MIN, Math.min(ROW_MAX, Math.round(px)));
    if (v === this.rowH) return;
    document.documentElement.style.setProperty("--row-h", `${v}px`);
    try {
      localStorage.setItem("daw.rowH", String(v));
    } catch {
      /* stockage indisponible : la hauteur n'est pas mémorisée */
    }
    // `light` : pendant qu'on tire la poignée d'une piste, les en-têtes suivent seuls (même variable CSS) : on ne les reconstruit pas.
    if (light) this.resize();
    else this.onLayout?.();
    this.dirty = true;
  }

  pan(pixels) {
    this.autoFit = false;
    const v = this.state.view;
    this.state.view = clampView({ spp: v.spp, start: v.start + pixels * v.spp }, this.width, this.length());
    this.dirty = true;
  }

  /** Suit la tête de lecture pendant la lecture : tourne la « page » quand elle sort de la fenêtre. */
  follow() {
    const { view, pos, playing } = this.state;
    if (!playing || this.drag) return;
    const x = sampleToX(view, pos);
    if (x > this.width * 0.94 || x < 0) {
      this.state.view = clampView({ spp: view.spp, start: pos - this.width * view.spp * 0.06 }, this.width, this.length());
    }
  }

  // ---- interactions -------------------------------------------------------------------------------------------

  #bind() {
    // Règle : la bande du haut est la bande de boucle (glisser = poser la boucle, bords = la redimensionner, corps = la déplacer,
    // double-clic = la supprimer) ; le reste de la règle déplace la tête de lecture. Maj + glisser pose aussi la boucle (comme avant).
    // La boucle se cale sur la ligne de grille visible la plus proche (mesures, puis temps quand on zoome) ; Alt : sans magnétisme.
    const pt = (e) => canvasPoint(this.ruler, e);
    const sampleAt = (e) => this.#clampSample(xToSample(this.state.view, pt(e).x));
    const snap = (e, v) => (e.altKey ? Math.round(v) : snapSample(this.state.view, this.width, this.state.tempo, this.state.meters_, v, 1e6));
    const sendLoop = (s, t) => this.client.send({ type: "loop", start: s, end: t });
    this.ruler.addEventListener("pointerdown", (e) => {
      this.ruler.setPointerCapture(e.pointerId);
      const { x, y } = pt(e);
      const sample = sampleAt(e);
      const hit = e.shiftKey ? "new" : loopHit(this.state, x, y);
      const loop = this.state.loop;
      if (hit === "start" || hit === "end") {
        this.drag = { kind: `loop-${hit}`, fixed: hit === "start" ? loop.end : loop.start };
      } else if (hit === "body") {
        this.drag = { kind: "loop-move", grab: sample - loop.start, len: loop.end - loop.start, x0: x, moved: false, sample };
      } else if (hit === "new") {
        // Un simple clic dans la bande ne pose rien : il déplace la tête ; la boucle naît quand on glisse (ou tout de suite avec Maj).
        this.drag = { kind: "loop-new", anchor: snap(e, sample), x0: x, active: !!e.shiftKey, sample };
        if (e.shiftKey) sendLoop(this.drag.anchor, this.drag.anchor + 1);
      } else {
        this.drag = { kind: "seek" };
        this.client.send({ type: "seek", pos: Math.round(sample) });
      }
    });
    this.ruler.addEventListener("pointermove", (e) => {
      if (!this.drag) {
        const { x, y } = pt(e);
        this.ruler.style.cursor = { start: "ew-resize", end: "ew-resize", body: "grab", new: "crosshair" }[loopHit(this.state, x, y)] ?? "pointer";
        return;
      }
      const d = this.drag;
      const sample = sampleAt(e);
      switch (d.kind) {
        case "seek":
          this.client.send({ type: "seek", pos: Math.round(sample) });
          break;
        case "loop-new": {
          if (!d.active && Math.abs(pt(e).x - d.x0) < 4) break;
          d.active = true;
          const b = snap(e, sample);
          const [s, t] = b >= d.anchor ? [d.anchor, b] : [b, d.anchor];
          sendLoop(s, Math.max(s + 1, t));
          break;
        }
        case "loop-start":
        case "loop-end": {
          const b = snap(e, sample);
          const [s, t] = d.kind === "loop-start" ? [b, d.fixed] : [d.fixed, b];
          if (s < t) sendLoop(s, t); // une extrémité ne dépasse pas l'autre
          break;
        }
        case "loop-move": {
          if (!d.moved && Math.abs(pt(e).x - d.x0) < 4) break;
          d.moved = true;
          this.ruler.style.cursor = "grabbing";
          const s = Math.max(0, snap(e, sample - d.grab));
          sendLoop(s, s + d.len);
          break;
        }
      }
    });
    const end = (e) => {
      const d = this.drag;
      this.drag = null;
      // Clic sans glisser dans la bande de boucle (hors du glissement d'un bord) : la tête de lecture va là.
      if (e?.type === "pointerup" && d && (d.kind === "loop-new" || d.kind === "loop-move") && !d.active && !d.moved) this.client.send({ type: "seek", pos: Math.round(d.sample) });
    };
    this.ruler.addEventListener("pointerup", end);
    this.ruler.addEventListener("pointercancel", () => (this.drag = null));
    this.ruler.addEventListener("dblclick", (e) => {
      const { x, y } = pt(e);
      if (this.state.loop && y < LOOP_H && !e.shiftKey && loopHit(this.state, x, y) !== null) sendLoop(null, null);
    });
    this.ruler.title = "Bande du haut : glisser pour poser la boucle, tirer ses bords pour la régler, la saisir pour la déplacer, double-clic pour la supprimer (Alt : sans magnétisme). La zone bleue est aussi la zone de punch quand le bouton Punch est allumé : l'enregistrement commence et finit exactement à ses bords. Reste de la règle : déplacer la tête de lecture.";

    const local = (e) => canvasPoint(this.tracks, e);
    this.tracks.addEventListener("pointerdown", (e) => {
      const { x, y } = local(e);
      const trow = this.nodes()[Math.floor(y / this.rowH)];
      if (trow?.kind === "take") {
        // Voie de prises : un clic sur une prise la rend active (rien d'autre ne s'édite ici).
        const s = xToSample(this.state.view, x);
        const c = trow.clips.find((k) => s >= k.start && s < k.start + k.len);
        // Un clic active toute la prise ; glisser dessus compose : seule la plage balayée est activée.
        if (c) {
          this.takeDrag = { clip: c, x0: x, x, y: Math.floor(y / this.rowH) * this.rowH, moved: false, alt: e.altKey };
          this.tracks.setPointerCapture(e.pointerId);
          this.dirty = true;
        }
        return;
      }
      // Un marqueur d'étirement touché : on le saisit (priorité sur le déplacement du clip).
      const marker = this.stretch.hit(this.nodes(), this.rowH, x, y);
      if (marker) {
        this.band = null;
        this.edit = null;
        this.#setSelected(new Set([marker.clip.id]), marker.clip.id);
        this.stretch.begin(marker, x);
        this.tracks.setPointerCapture(e.pointerId);
        this.dirty = true;
        return;
      }
      const hit = hitTest(this.nodes(), this.state.view, this.rowH, x, y);
      const additive = e.ctrlKey || e.metaKey || e.shiftKey;
      this.band = null;
      this.edit = null;
      if (hit && additive) {
        // Ctrl / Maj + clic : ajoute ou retire le clip de la sélection (pas de glissement).
        this.#setSelected(clickSelect(this.state.selected, hit.clip.id, { toggle: true }), hit.clip.id);
        this.dirty = true;
        return;
      }
      if (!hit) {
        // Dans le vide : un rectangle de sélection (Ctrl / Maj : on garde la sélection actuelle) ; un simple clic efface.
        this.band = { x0: x, y0: y, x, y, base: additive ? new Set(this.state.selected) : new Set(), moved: false };
        if (!additive) this.#setSelected(new Set(), null);
        // Un clic dans le vide d'une piste la désigne comme destination du prochain collage (un bus ou une autre ligne : aucune).
        const row = this.nodes()[Math.floor(y / this.rowH)];
        this.state.activeTrack = row && (row.kind === "mono" || row.kind === "stereo") ? row.id : null;
        this.tracks.setPointerCapture(e.pointerId);
        this.dirty = true;
        return;
      }
      const group = hit.zone === "body" ? clickSelect(this.state.selected, hit.clip.id) : new Set([hit.clip.id]);
      this.#setSelected(group, hit.clip.id);
      this.edit = { hit, startX: x, startY: y, moved: false, preview: null, committed: false, group: null, duplicate: false, delta: 0, nodeDelta: 0 };
      if (hit.zone === "body" && (group.size > 1 || e.altKey)) {
        this.edit.group = [...group];
        this.edit.duplicate = e.altKey; // Alt + glisser : on duplique au lieu de déplacer
      }
      this.autoFit = false; // dès qu'on édite, la vue ne se redimensionne plus d'elle-même
      this.tracks.setPointerCapture(e.pointerId);
      this.dirty = true;
    });
    this.tracks.addEventListener("pointermove", (e) => {
      const { x, y } = local(e);
      if (this.stretch.drag) {
        this.stretch.move(x, e.shiftKey);
        this.tracks.style.cursor = "ew-resize";
        return;
      }
      if (this.takeDrag) {
        this.takeDrag.x = x;
        this.takeDrag.alt = e.altKey;
        this.takeDrag.moved = this.takeDrag.moved || Math.abs(x - this.takeDrag.x0) > 4;
        this.dirty = true;
        return;
      }
      if (this.band) {
        this.band.x = x;
        this.band.y = y;
        this.band.moved = this.band.moved || Math.abs(x - this.band.x0) > 3 || Math.abs(y - this.band.y0) > 3;
        if (this.band.moved) {
          const { view } = this.state;
          const hits = clipsInRect(this.nodes(), this.rowH, { s0: xToSample(view, this.band.x0), s1: xToSample(view, x), y0: this.band.y0, y1: y });
          this.#setSelected(new Set([...this.band.base, ...hits]), hits.length ? hits[hits.length - 1] : this.state.selection);
        }
        this.dirty = true;
        return;
      }
      if (!this.edit) {
        if (this.stretch.hovering(this.nodes(), this.rowH, x, y)) {
          this.tracks.style.cursor = "ew-resize";
          return;
        }
        const hit = hitTest(this.nodes(), this.state.view, this.rowH, x, y);
        this.tracks.style.cursor = hit ? CURSORS[hit.zone] : "default";
        const h = hit ? { id: hit.clip.id, zone: hit.zone } : null;
        if (h?.id !== this.hover?.id || h?.zone !== this.hover?.zone) {
          this.hover = h;
          this.dirty = true;
        }
        return;
      }
      if (this.edit.committed) return;
      if (!this.edit.moved && Math.abs(x - this.edit.startX) < 3 && Math.abs(y - this.edit.startY) < 3) return;
      this.edit.moved = true;
      const { tempo, meters_, view } = this.state;
      const snap = e.shiftKey ? (s) => s : (s) => snapSample(view, this.width, tempo, meters_, s, 8);
      this.edit.preview = previewDrag(this.edit.hit.clip, this.edit.hit.zone, (x - this.edit.startX) * view.spp, snap);
      if (this.edit.group) {
        // Groupe ou duplication : décalage horizontal commun, borné pour que le plus à gauche ne passe pas sous 0.
        const clips = this.state.project.nodes.flatMap((n) => n.clips).filter((c) => this.edit.group.includes(c.id));
        const raw = this.edit.preview.start - this.edit.hit.clip.start;
        this.edit.delta = clampGroupDelta(clips.map((c) => c.start), raw);
        this.edit.preview = { ...this.edit.preview, start: this.edit.hit.clip.start + this.edit.delta };
        const owners = clips.map((c) => this.state.project.nodes.find((n) => n.clips.includes(c))?.id);
        const to = targetTrackAt(this.nodes(), this.rowH, y) ?? this.edit.hit.node.id;
        this.edit.nodeDelta = clampNodeDelta(trackIds(this.state.project.nodes), owners, this.edit.hit.node.id, to);
        this.edit.targetNode = this.edit.hit.node.id;
      } else if (this.edit.hit.zone === "body") this.edit.targetNode = targetTrackAt(this.nodes(), this.rowH, y) ?? this.edit.hit.node.id;
      this.tracks.style.cursor = this.edit.hit.zone === "body" ? "grabbing" : CURSORS[this.edit.hit.zone];
      this.dirty = true;
    });
    const finish = (commit) => {
      if (this.stretch.drag) {
        this.stretch.end(commit);
        return;
      }
      if (this.takeDrag) {
        const d = this.takeDrag;
        this.takeDrag = null;
        this.dirty = true;
        if (!commit) return;
        if (!d.moved || d.clip.kind === "midi") {
          this.client.send({ type: "set_active_take", clip: d.clip.id });
        } else {
          const { view, tempo, meters_ } = this.state;
          const snap = (px) => (d.alt ? Math.round(xToSample(view, px)) : snapSample(view, this.width, tempo, meters_, xToSample(view, px), 8));
          const [a, b] = [snap(d.x0), snap(d.x)].sort((p, q) => p - q);
          const start = Math.max(a, d.clip.start);
          const end = Math.min(b, d.clip.start + d.clip.len);
          if (end > start) this.client.send({ type: "activate_take_range", clip: d.clip.id, start: Math.round(start), end: Math.round(end) });
        }
        return;
      }
      if (this.band) {
        this.band = null;
        this.dirty = true;
        return;
      }
      const ed = this.edit;
      if (!ed) return;
      // Simple clic (sans glisser) sur un clip d'un groupe : la sélection se réduit à ce clip (le groupe n'était gardé que pour le glisser).
      if (commit && !ed.moved && ed.hit.zone === "body" && this.state.selected.size > 1) this.#setSelected(new Set([ed.hit.clip.id]), ed.hit.clip.id);
      if (commit && ed.moved && ed.preview && ed.group) {
        const cmd = groupDragCommand(ed.group, ed.delta, ed.duplicate, ed.nodeDelta);
        if (cmd) {
          // `committed` avant l'envoi : le faux moteur répond de façon synchrone, et la réponse doit trouver l'aperçu déjà « validé ».
          ed.committed = true;
          if (this.client.send(cmd)) return;
          ed.committed = false;
        }
      } else if (commit && ed.moved && ed.preview) {
        const cmd = commandFor(ed.hit.clip, ed.hit.zone, ed.preview, ed.targetNode ?? null, ed.hit.node.id);
        if (cmd) {
          // L'aperçu reste affiché jusqu'à la réponse du moteur (événement `project` ou `error`) ; `committed` d'abord : le faux
          // moteur répond de façon synchrone.
          ed.committed = true;
          if (this.client.send(cmd)) return;
          ed.committed = false;
        }
      }
      this.edit = null;
      this.dirty = true;
    };
    this.tracks.addEventListener("dblclick", (e) => {
      const { x, y } = local(e);
      const node = this.nodes()[Math.floor(y / this.rowH)];
      if (!node) return;
      const { view } = this.state;
      const cf = crossfadeAt(node, xToSample(view, x), 6 * view.spp);
      if (cf) return this.onFadeEditor?.({ kind: "crossfade", outId: cf.a.id, inId: cf.b.id });
      const marker = this.stretch.hit(this.nodes(), this.rowH, x, y);
      if (marker) return this.stretch.remove(marker.clip, marker.index); // double-clic sur un marqueur d'étirement : le supprime
      const hit = hitTest(this.nodes(), view, this.rowH, x, y);
      if (hit && hit.clip.kind === "midi") return this.onPianoRoll?.(hit.clip.id);
      if (hit && hit.clip.kind === "audio" && (hit.zone === "fade-in" || hit.zone === "fade-out")) {
        this.onFadeEditor?.({ kind: "single", clipId: hit.clip.id, side: hit.zone === "fade-in" ? "in" : "out" });
        return;
      }
      // Double-clic sur le corps d'un clip audio : pose un marqueur d'étirement.
      if (hit && hit.clip.kind === "audio" && hit.zone === "body") this.stretch.addAt(hit.clip, x, e.shiftKey);
    });
    // Deux doigts : zoom horizontal et défilement ; le geste d'un doigt en cours (clip, rectangle, boucle) est abandonné, et le doigt qui
    // reste ne déplace plus rien tant que tous les doigts ne sont pas levés.
    const pinch = {
      onStart: () => {
        finish(false);
        this.drag = null;
        this.pinchAcc = 1;
        this.rowAcc = null;
      },
      onMove: ({ factor, fy, mode, dx, x, y }) => {
        const rect = this.tracks.getBoundingClientRect();
        const k = this.tracks.clientWidth / (rect.width || 1);
        const { x: anchor } = canvasPoint(this.tracks, { clientX: x, clientY: y });
        // Le zoom avance par crans de quart d'octave (les tuiles de crêtes restent réutilisables) : on cumule le facteur des doigts.
        if (mode === "x") {
          this.pinchAcc = (this.pinchAcc ?? 1) * factor;
          while (this.pinchAcc >= ZOOM_STEP) {
            this.zoom(1, anchor);
            this.pinchAcc /= ZOOM_STEP;
          }
          while (this.pinchAcc <= 1 / ZOOM_STEP) {
            this.zoom(-1, anchor);
            this.pinchAcc *= ZOOM_STEP;
          }
        } else if (mode === "y" && fy !== 1) {
          // Geste plutôt vertical : la hauteur des pistes (zoom vertical de la timeline).
          this.rowAcc = (this.rowAcc ?? this.rowH) * fy;
          this.setRowHeight(this.rowAcc);
        }
        this.pan(-dx * k);
        this.dirty = true;
      },
    };
    new TouchPinch(this.tracks, pinch);
    new TouchPinch(this.ruler, pinch);
    this.tracks.addEventListener("pointerup", () => finish(true));
    this.tracks.addEventListener("pointercancel", () => finish(false));
    // Suppr sur un marqueur d'étirement sélectionné : il est retiré, la touche n'atteint pas « supprimer le clip ».
    window.addEventListener(
      "keydown",
      (e) => {
        if ((e.key !== "Delete" && e.key !== "Backspace") || !this.stretch.sel) return;
        const t = e.target;
        if (t instanceof HTMLElement && (t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName))) return;
        if (this.stretch.deleteSelected()) {
          e.preventDefault();
          e.stopImmediatePropagation();
        }
      },
      true,
    );
    window.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.stretch.escape();
      if (e.key === "Escape" && (this.band || this.takeDrag || (this.edit && !this.edit.committed))) finish(false);
    });

    this.scroller.addEventListener(
      "wheel",
      (e) => {
        if (e.ctrlKey || e.metaKey) {
          e.preventDefault();
          this.zoom(e.deltaY < 0 ? 1 : -1, canvasPoint(this.tracks, e).x);
        } else if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
          e.preventDefault();
          this.pan(e.deltaX || e.deltaY);
        }
      },
      { passive: false },
    );
  }

  /** Où un fichier déposé en (clientX, clientY) atterrirait : `{ node, sample }` (`node` : la piste visée, `null` = une nouvelle piste), ou `null` hors de la timeline. */
  dropTarget(clientX, clientY) {
    const box = this.scroller.getBoundingClientRect();
    if (clientX < box.left || clientX > box.right || clientY < box.top || clientY > box.bottom) return null;
    const { x, y } = canvasPoint(this.tracks, { clientX, clientY });
    const { view, tempo, meters_ } = this.state;
    const sample = snapSample(view, this.width, tempo, meters_, this.#clampSample(xToSample(view, x)), 8);
    const node = this.nodes()[Math.floor(y / this.rowH)];
    return { node: node && (node.kind === "mono" || node.kind === "stereo") ? node.id : null, sample: Math.round(sample) };
  }

  /** Repère de dépôt pendant un glisser de fichiers (`{ node, sample }` ou `null`). */
  setDropHint(hint) {
    this.dropHint = hint;
    this.dirty = true;
  }

  /** Remplace la sélection (le clip « principal » est celui des commandes à un seul clip : découper, fondu, piano roll). */
  #setSelected(set, primary) {
    this.state.selected = set;
    this.state.selection = primary !== null && set.has(primary) ? primary : set.size ? [...set].pop() : null;
    this.onSelection?.();
    this.dirty = true;
  }

  /** Sélectionne ces clips (collage, duplication, Ctrl+A) ; le dernier devient le principal. */
  selectClips(ids) {
    this.#setSelected(new Set(ids), ids.length ? ids[ids.length - 1] : null);
  }

  #clampSample(s) {
    return Math.max(0, s);
  }

  /** Le moteur a confirmé (nouveau projet) ou refusé (erreur) : l'aperçu du glissement n'a plus lieu d'être. */
  settle() {
    if (this.edit?.committed) {
      this.edit = null;
      this.dirty = true;
    }
    if (this.state.activeTrack != null && !this.state.project.nodes.some((n) => n.id === this.state.activeTrack)) this.state.activeTrack = null;
    // Les clips sélectionnés qui ont disparu (supprimés, ou édition annulée) sortent de la sélection.
    const alive = prune(this.state.selected, this.nodes());
    if (alive.size !== this.state.selected.size) {
      this.#setSelected(alive, alive.has(this.state.selection) ? this.state.selection : [...alive].pop() ?? null);
    }
  }

  // ---- dessin -------------------------------------------------------------------------------------------------

  draw() {
    if (!this.dirty) return;
    this.dirty = false;
    if (!this.colors.bg) this.refreshColors();
    this.#drawTracks();
    this.#drawRuler();
    this.chordLane?.paint(); // voies d'accords épinglées sous la règle
    this.onDraw?.();
  }

  #ctx(canvas) {
    const dpr = pixelRatio();
    const g = canvas.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    return g;
  }

  #drawTracks() {
    const g = this.#ctx(this.tracks);
    const { view, tempo, meters_, pos, loop } = this.state;
    const nodes = this.nodes();
    const rowH = this.rowH;
    const W = this.width;
    const H = nodes.length * rowH;
    g.clearRect(0, 0, W, H);
    nodes.forEach((n, i) => {
      g.fillStyle = n.kind === "roll" || n.kind === "roll-cont" ? "rgba(0,0,0,.28)" : i % 2 ? this.colors["row-b"] : this.colors["row-a"];
      g.fillRect(0, i * rowH, W, rowH);
    });
    // Piste de destination du collage (clic dans le vide d'une piste) : un liseré à gauche et une teinte légère.
    const active = this.state.activeTrack ?? null;
    if (active !== null) {
      const i = nodes.findIndex((n) => n.id === active);
      if (i >= 0) {
        g.fillStyle = this.colors.accent + "0f";
        g.fillRect(0, i * rowH, W, rowH);
        g.fillStyle = this.colors.accent;
        g.fillRect(0, i * rowH, 3, rowH);
      }
    }
    // Boucle
    if (loop) {
      const x0 = sampleToX(view, loop.start);
      const x1 = sampleToX(view, loop.end);
      g.fillStyle = this.colors.accent + "22";
      g.fillRect(Math.max(0, x0), 0, Math.min(W, x1) - Math.max(0, x0), H);
    }
    // Grille
    for (const l of gridLines(view, W, tempo, meters_)) {
      g.fillStyle = l.kind === "bar" ? this.colors.line : l.kind === "beat" ? this.colors.line + "88" : this.colors.line + "44";
      g.fillRect(Math.round(l.x), 0, 1, H);
    }
    g.fillStyle = this.colors.line;
    nodes.forEach((_, i) => g.fillRect(0, (i + 1) * rowH - 1, W, 1));
    // Clips
    const moving = this.edit?.preview && this.edit.hit.zone === "body" ? this.edit : null;
    const targetId = moving?.targetNode ?? null;
    const shiftedRows = moving?.group && moving.nodeDelta ? trackIds(this.state.project.nodes) : null; // groupe qui change de piste
    const inGroup = (id) => moving?.group?.includes(id);
    nodes.forEach((n, i) => {
      if (n.kind === "lane") this.automation?.draw(g, n.id, i * rowH, W);
      for (const c of n.clips) {
        if (moving && !moving.group && c.id === moving.hit.clip.id && targetId !== null && targetId !== n.id) continue; // il est dessiné dans la piste visée
        if (inGroup(c.id)) {
          if (moving.duplicate) {
            // Duplication : l'original reste en place, la copie suit la souris (translucide).
            this.#drawClip(g, n, c, i * rowH);
            if (!shiftedRows) {
              g.save();
              g.globalAlpha = 0.55;
              this.#drawClip(g, n, this.#shown(c), i * rowH);
              g.restore();
            }
            continue;
          }
          if (shiftedRows) continue; // il est dessiné dans sa nouvelle piste, plus bas
        }
        this.#drawClip(g, n, this.#shown(c), i * rowH);
      }
    });
    if (shiftedRows) {
      // Le groupe a changé de piste : chaque clip est dessiné dans la piste d'arrivée (translucide s'il s'agit d'une duplication).
      nodes.forEach((n) => {
        for (const c of n.clips) {
          if (!inGroup(c.id)) continue;
          const row = nodes.findIndex((x) => x.id === shiftedTrack(shiftedRows, n.id, moving.nodeDelta));
          if (row < 0) continue;
          g.fillStyle = this.colors.accent + "10";
          g.fillRect(0, row * rowH, W, rowH);
          g.save();
          if (moving.duplicate) g.globalAlpha = 0.55;
          this.#drawClip(g, nodes[row], this.#shown(c), row * rowH, n.color);
          g.restore();
        }
      });
    }
    if (moving && !moving.group && targetId !== null && targetId !== moving.hit.node.id) {
      const row = nodes.findIndex((n) => n.id === targetId);
      g.fillStyle = this.colors.accent + "18";
      g.fillRect(0, row * rowH, W, rowH);
      this.#drawClip(g, nodes[row], this.#shown(moving.hit.clip), row * rowH, moving.hit.node.color);
    }
    // Rectangle de sélection en cours.
    this.#drawTakeDrag(g);
    if (this.band?.moved) {
      const b = this.band;
      g.fillStyle = this.colors.accent + "22";
      g.strokeStyle = this.colors.accent;
      g.lineWidth = 1;
      g.fillRect(Math.min(b.x0, b.x), Math.min(b.y0, b.y), Math.abs(b.x - b.x0), Math.abs(b.y - b.y0));
      g.strokeRect(Math.min(b.x0, b.x) + 0.5, Math.min(b.y0, b.y) + 0.5, Math.abs(b.x - b.x0), Math.abs(b.y - b.y0));
    }
    // Dépôt de fichiers en cours : la piste visée est éclairée, sinon une bande « nouvelle piste » ; un trait marque la position.
    if (this.dropHint) {
      const { node, sample } = this.dropHint;
      const row = node === null ? nodes.length : nodes.findIndex((n) => n.id === node);
      if (row >= 0) {
        g.fillStyle = this.colors.accent + (node === null ? "14" : "26");
        g.fillRect(0, row * rowH, W, rowH);
        if (node === null) {
          g.fillStyle = this.colors.muted;
          g.font = "11px system-ui, sans-serif";
          g.textBaseline = "middle";
          g.fillText("Nouvelle piste", 10, row * rowH + rowH / 2);
        }
      }
      const hx = Math.round(sampleToX(view, sample)) + 0.5;
      g.fillStyle = this.colors.accent;
      g.fillRect(hx - 0.5, 0, 1.5, Math.max(H, (nodes.length + 1) * rowH));
    }
    // Prises en cours : un conteneur sur chaque piste qui enregistre, du début de la prise à la tête de lecture (sans forme d'onde ni
    // notes : on voit seulement qu'on est en train d'enregistrer).
    const rec = this.state.recording;
    if (rec?.active && rec.nodes?.length) {
      const x0 = Math.max(-2, sampleToX(view, rec.start));
      const x1 = Math.min(W + 2, sampleToX(view, Math.max(rec.start, pos)));
      if (x1 > x0) {
        const color = css("--rec") || "#ff4d5e";
        nodes.forEach((n, i) => {
          if (n.kind === "lane" || n.kind === "take" || !rec.nodes.includes(n.id)) return;
          const y = i * rowH + 4;
          g.fillStyle = color + "33";
          g.fillRect(x0, y, x1 - x0, rowH - 9);
          g.strokeStyle = color;
          g.lineWidth = 1.5;
          g.strokeRect(x0 + 0.75, y + 0.75, x1 - x0 - 1.5, rowH - 10.5);
          g.fillStyle = color;
          g.font = "600 11px system-ui, sans-serif";
          g.textBaseline = "top";
          g.fillText(rec.stopping ? "Écriture de la prise…" : "● Enregistrement", Math.max(x0, 0) + 8, y + 6);
        });
      }
    }
    // Tête de lecture
    const px = Math.round(sampleToX(view, pos)) + 0.5;
    if (px >= 0 && px <= W) {
      g.fillStyle = this.colors.accent;
      g.fillRect(px - 0.5, 0, 1.5, H);
    }
  }

  /** Ouvre l'éditeur de fondu du clip sélectionné : son fondu enchaîné s'il en a un, sinon son fondu d'entrée ou de sortie. */
  /** Ouvre le piano roll sur le clip de notes sélectionné (touche Entrée). */
  openPianoRollForSelection() {
    const id = this.state.selection;
    if (id === null) return;
    const clip = this.state.project.nodes.flatMap((n) => n.clips).find((c) => c.id === id);
    if (clip?.kind === "midi") this.onPianoRoll?.(id);
  }

  /** Touche N : le clip de notes sélectionné s'ouvre en partition ; plusieurs clips de notes sélectionnés s'y empilent (le clip principal d'abord). */
  openScoreForSelection() {
    const all = this.state.project.nodes.flatMap((n) => n.clips).filter((c) => c.kind === "midi");
    const main = this.state.selection;
    const ids = all.filter((c) => c.id === main || this.state.selected?.has?.(c.id)).map((c) => c.id);
    if (!ids.length) return;
    if (ids.includes(main)) ids.splice(ids.indexOf(main), 1), ids.unshift(main);
    this.onScore?.(ids);
  }

  openFadeEditorForSelection() {
    const id = this.state.selection;
    if (id === null) return;
    const node = this.nodes().find((n) => n.clips.some((c) => c.id === id));
    const clip = node?.clips.find((c) => c.id === id);
    if (!clip || clip.kind !== "audio") return;
    const cf = crossfadeOf(node, id);
    if (cf) this.onFadeEditor?.({ kind: "crossfade", outId: cf.a.id, inId: cf.b.id });
    else if (clip.fade_out > 0 || clip.fade_in > 0) this.onFadeEditor?.({ kind: "single", clipId: id, side: clip.fade_out > 0 ? "out" : "in" });
  }

  /** Le clip tel qu'on l'affiche : avec l'aperçu du glissement s'il est en cours d'édition. */
  #shown(clip) {
    const ed = this.edit;
    if (!ed?.preview) return clip;
    if (ed.group) return ed.group.includes(clip.id) ? { ...clip, start: clip.start + ed.delta } : clip;
    return ed.hit.clip.id === clip.id ? { ...clip, ...ed.preview } : clip;
  }

  /** Plage en cours de sélection par glissement dans une voie de prises (composer). */
  #drawTakeDrag(g) {
    const d = this.takeDrag;
    if (!d?.moved) return;
    const { view } = this.state;
    const x0 = Math.max(sampleToX(view, d.clip.start), Math.min(d.x0, d.x));
    const x1 = Math.min(sampleToX(view, d.clip.start + d.clip.len), Math.max(d.x0, d.x));
    if (x1 <= x0) return;
    g.fillStyle = this.colors.accent + "55";
    g.fillRect(x0, d.y + 3, x1 - x0, this.rowH - 7);
    g.strokeStyle = this.colors.accent;
    g.lineWidth = 1.5;
    g.strokeRect(x0, d.y + 3, x1 - x0, this.rowH - 7);
  }

  #drawClip(g, node, clip, top, colorOverride = null) {
    const { view } = this.state;
    const x0 = sampleToX(view, clip.start);
    const x1 = sampleToX(view, clip.start + clip.len);
    if (x1 < 0 || x0 > this.width) return;
    const y = top + 5;
    const h = this.rowH - 11;
    const w = Math.max(2, x1 - x0);
    const color = colorOverride ?? node.color;
    const r = Math.min(5, w / 2);
    const path = new Path2D();
    path.roundRect(x0, y, w, h, r);
    g.fillStyle = color + "33";
    g.fill(path);
    g.save();
    if (clip.muted) g.globalAlpha = 0.55; // prise inactive (le restore plus bas le rétablit)
    g.clip(path);
    const bar = 17;
    g.fillStyle = color + "dd";
    g.fillRect(x0, y, w, bar);
    // Nom
    if (w > 36) {
      g.font = "600 11px system-ui, sans-serif";
      g.textBaseline = "middle";
      const left = Math.max(x0, 0) + 6;
      let tx = left;
      if (clip.aligned && w > 80) {
        // Prise calée sur un guide : pastille verte « CALÉ » devant le nom, pour qu'on la reconnaisse tout de suite.
        g.font = "700 9px system-ui, sans-serif";
        const cw = g.measureText("CALÉ").width + 10;
        const chip = new Path2D();
        chip.roundRect(left - 2, y + 3, cw, bar - 6, 3);
        g.fillStyle = "#3ecf8e";
        g.fill(chip);
        g.fillStyle = "#0b2a1c";
        g.fillText("CALÉ", left + 3, y + bar / 2 + 0.5);
        tx = left + cw + 4;
        g.font = "600 11px system-ui, sans-serif";
      }
      g.fillStyle = luminance(color) > 0.6 ? "#111" : "#fff";
      g.fillText(clip.name, tx, y + bar / 2 + 0.5, Math.max(10, Math.min(x1, this.width) - tx - 4));
    }
    const bodyTop = y + bar;
    const bodyH = h - bar;
    if (clip.kind === "audio") {
      const warp = this.stretch.warpFor(clip);
      if (warp) this.#drawWarpedWave(g, clip, color, bodyTop, bodyH, warp);
      else this.#drawWave(g, clip, color, bodyTop, bodyH);
      this.stretch.draw(g, clip, { x0, x1, y, h, bodyTop, bodyH });
    } else this.#drawNotes(g, clip, color, bodyTop, bodyH);
    if (clip.missing) {
      // Fichier référencé en place introuvable : hachures et mention (le clip joue du silence).
      g.strokeStyle = "#ff6b6bcc";
      g.lineWidth = 1;
      for (let hx = x0 - bodyH; hx < x1; hx += 9) {
        g.beginPath();
        g.moveTo(hx, bodyTop + bodyH);
        g.lineTo(hx + bodyH, bodyTop);
        g.stroke();
      }
      if (w > 60) {
        g.fillStyle = "#ff8a8a";
        g.font = "600 10px system-ui, sans-serif";
        g.textBaseline = "middle";
        g.fillText("fichier manquant", Math.max(x0, 0) + 6, bodyTop + bodyH / 2);
      }
    }
    // Fondus : la zone assombrie est ce qui est retranché du son, la courbe suit la forme choisie.
    g.strokeStyle = this.colors.text + "aa";
    g.fillStyle = "#0000004d";
    g.lineWidth = 1;
    const bottom = y + h;
    const curve = (edge, fx, dir, shape) => {
      const n = Math.max(6, Math.min(32, Math.round(fx / 4)));
      g.beginPath();
      g.moveTo(edge, bodyTop);
      g.lineTo(edge + dir * fx, bodyTop);
      for (let i = n; i >= 0; i--) {
        const t = i / n;
        g.lineTo(edge + dir * t * fx, bottom - fadeGain(shape, t) * (bottom - bodyTop));
      }
      g.closePath();
      g.fill();
      g.beginPath();
      for (let i = 0; i <= n; i++) {
        const t = i / n;
        const px = edge + dir * t * fx;
        const py = bottom - fadeGain(shape, t) * (bottom - bodyTop);
        if (i) g.lineTo(px, py);
        else g.moveTo(px, py);
      }
      g.stroke();
    };
    if (clip.fade_in > 0) curve(x0, clip.fade_in / view.spp, 1, clip.fade_in_curve);
    if (clip.fade_out > 0) curve(x1, clip.fade_out / view.spp, -1, clip.fade_out_curve);
    g.restore();
    // Poignées de fondu et surbrillance des bords : sur le clip survolé ou sélectionné.
    const isSel = this.state.selected.has(clip.id);
    const active = this.state.selection === clip.id || this.hover?.id === clip.id;
    if (active && x1 - x0 >= 40) {
      const hd = fadeHandles(view, { x0, x1, bodyTop }, clip);
      g.fillStyle = this.colors.text;
      g.strokeStyle = this.colors.bg;
      g.lineWidth = 1;
      for (const hx of [hd.inX, hd.outX]) {
        g.beginPath();
        g.arc(hx, hd.y, 4.5, 0, Math.PI * 2);
        g.fill();
        g.stroke();
      }
    }
    if (this.hover?.id === clip.id && (this.hover.zone === "trim-start" || this.hover.zone === "trim-end")) {
      g.fillStyle = this.colors.accent;
      g.fillRect(this.hover.zone === "trim-start" ? x0 - 1 : x1 - 2, y + 2, 3, h - 4);
    }
    if (this.edit?.preview && this.edit.hit.clip.id === clip.id) {
      // Repère du bord déplacé : on voit où il se cale.
      const zone = this.edit.hit.zone;
      const mx = zone === "trim-end" ? x1 : x0;
      g.fillStyle = this.colors.accent;
      g.fillRect(Math.round(mx), 0, 1, this.nodes().length * this.rowH);
    }
    g.lineWidth = isSel ? 2 : 1;
    g.strokeStyle = isSel ? this.colors.accent : color;
    g.stroke(path);
  }

  #drawWave(g, clip, color, top, height) {
    const { view } = this.state;
    const s0 = view.start;
    const s1 = view.start + this.width * view.spp;
    const mid = top + height / 2;
    const half = height / 2 - 2;
    g.fillStyle = color;
    for (const t of this.peaks.tilesFor(clip, view.spp, s0, s1)) {
      for (let i = 0; i < t.columns.length; i++) {
        const [lo, hi] = t.columns[i];
        const x = Math.floor(sampleToX(view, t.from + i * t.spp));
        if (x < -1 || x > this.width) continue;
        const yTop = mid - hi * half;
        g.fillRect(x, yTop, 1, Math.max(1, (hi - lo) * half));
      }
    }
    void TILE_COLUMNS;
  }

  /**
   * Forme d'onde **déformée** pendant le glissement d'un marqueur d'étirement : chaque tronçon de la nouvelle carte montre le tronçon de
   * source qu'il montrait avant (`segs` : `{ from, to }` en échantillons absolus), en lisant les crêtes déjà chargées du clip.
   */
  #drawWarpedWave(g, clip, color, top, height, segs) {
    const { view } = this.state;
    const mid = top + height / 2;
    const half = height / 2 - 2;
    g.fillStyle = color;
    for (const seg of segs) {
      const xa = sampleToX(view, seg.to[0]);
      const xb = sampleToX(view, seg.to[1]);
      if (xb <= 0 || xa >= this.width || xb - xa < 0.5) continue;
      const [oa, ob] = seg.from;
      const lo = Math.max(0, Math.floor(xa));
      const hi = Math.min(this.width, Math.ceil(xb));
      const at = (px) => oa + ((px - xa) / (xb - xa)) * (ob - oa);
      const tiles = this.peaks.tilesFor(clip, view.spp, Math.min(at(lo), at(hi)), Math.max(at(lo), at(hi)));
      for (let px = lo; px < hi; px++) {
        const s0 = Math.min(at(px), at(px + 1));
        const s1 = Math.max(at(px), at(px + 1));
        let cmin = 1;
        let cmax = -1;
        for (const t of tiles) {
          const a = Math.max(0, Math.floor((s0 - t.from) / t.spp));
          const b = Math.min(t.columns.length - 1, Math.floor((Math.max(s1, s0 + 1) - t.from) / t.spp));
          for (let i = a; i <= b; i++) {
            const [l, h] = t.columns[i];
            if (l < cmin) cmin = l;
            if (h > cmax) cmax = h;
          }
        }
        if (cmax < cmin) continue;
        g.fillRect(px, mid - cmax * half, 1, Math.max(1, (cmax - cmin) * half));
      }
    }
  }

  #drawNotes(g, clip, color, top, height) {
    const { view } = this.state;
    if (clip.notes.length === 0) return;
    let lo = 127;
    let hi = 0;
    for (const n of clip.notes) {
      lo = Math.min(lo, n.key);
      hi = Math.max(hi, n.key);
    }
    if (hi - lo < 11) {
      const mid = (hi + lo) / 2;
      lo = Math.floor(mid - 6);
      hi = Math.ceil(mid + 6);
    }
    const nh = Math.max(2, Math.min(5, (height - 4) / (hi - lo + 1)));
    g.fillStyle = color;
    for (const n of clip.notes) {
      const x = sampleToX(view, clip.start + n.start);
      const w = Math.max(1, n.len / view.spp);
      if (x + w < 0 || x > this.width) continue;
      const y = top + 2 + ((hi - n.key) / (hi - lo + 1)) * (height - 4);
      g.globalAlpha = 0.45 + 0.55 * n.velocity;
      g.fillRect(x, y, w, nh);
    }
    g.globalAlpha = 1;
  }

  #drawRuler() {
    const g = this.#ctx(this.ruler);
    const { view, tempo, meters_, pos, loop } = this.state;
    const W = this.width;
    const H = this.rulerH;
    g.clearRect(0, 0, W, H);
    g.fillStyle = this.colors.panel;
    g.fillRect(0, 0, W, H);
    // Bande de boucle : toujours visible (légèrement teintée), pour qu'on sache où glisser.
    g.fillStyle = this.colors.line + "55";
    g.fillRect(0, 0, W, LOOP_H);
    if (loop) {
      const x0 = sampleToX(view, loop.start);
      const x1 = sampleToX(view, loop.end);
      const cx0 = Math.max(0, x0);
      const cx1 = Math.min(W, x1);
      g.fillStyle = this.colors.accent + "22";
      g.fillRect(cx0, LOOP_H, Math.max(0, cx1 - cx0), H - LOOP_H);
      g.fillStyle = this.colors.accent + "99";
      g.fillRect(cx0, 0, Math.max(0, cx1 - cx0), LOOP_H);
      g.fillStyle = this.colors.accent;
      if (x0 >= 0 && x0 <= W) g.fillRect(x0 - 1, 0, 4, LOOP_H); // poignées des bords
      if (x1 >= 0 && x1 <= W) g.fillRect(x1 - 3, 0, 4, LOOP_H);
    } else {
      g.fillStyle = this.colors.muted;
      g.font = "9px system-ui, sans-serif";
      g.textBaseline = "middle";
      g.fillText("boucle : glisser ici", 6, LOOP_H / 2 + 0.5);
    }
    g.font = "11px system-ui, sans-serif";
    g.textBaseline = "alphabetic";
    for (const l of gridLines(view, W, tempo, meters_)) {
      const x = Math.round(l.x) + 0.5;
      g.fillStyle = l.kind === "bar" ? this.colors.muted : this.colors.line;
      const h = l.kind === "bar" ? 14 : l.kind === "beat" ? 8 : 4;
      g.fillRect(x - 0.5, H - h, 1, h);
      if (l.label) {
        g.fillStyle = this.colors.text;
        g.fillText(l.label, x + 4, H - 16);
      }
    }
    const px = Math.round(sampleToX(view, pos)) + 0.5;
    if (px >= -6 && px <= W + 6) {
      g.fillStyle = this.colors.accent;
      g.beginPath();
      g.moveTo(px - 6, H - 11);
      g.lineTo(px + 6, H - 11);
      g.lineTo(px, H - 1);
      g.closePath();
      g.fill();
      g.fillRect(px - 0.5, 0, 1.5, H);
    }
  }
}
