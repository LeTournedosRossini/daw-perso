// Voies d'accords épinglées sous la règle (un quart de hauteur de piste chacune) : accords, gammes et motifs, structure.
// Voie d'accords : deux lignes en haut de la timeline. « Accords » : les accords (blocs nommés) et les marqueurs de tonalité ;
// « Gammes » : les gammes (empilées quand elles se chevauchent) et les appels de motifs. Double-clic dans le vide : saisir un accord par
// son nom (Dm7, G7/B, Bb, N.C., C[0,4,7,13]) ou poser une gamme ; glisser : déplacer ; bords : changer la durée ; Suppr : supprimer.
// Le temps s'aimante sur la grille choisie (Alt : libre). Chaque geste = une commande `set_chords` (un pas d'annulation).

import { createChordDialog } from "./chord-dialog.js";
import { canvasPoint } from "../ui-scale.js";
import { sampleToX, xToSample, gridLines } from "../time.js";
import { pixelRatio } from "../ui-scale.js";
import { fnum, fadd, fsub, fcmp } from "../notes-model.js";
import { KEY_CHOICES, choiceOf, keyEntry, keyName } from "../chord-model.js";
import { parseLyricsText, syllableLabel, placeSyllables, mergeSyllables, verseSyllables, verseCount, MAX_VERSE } from "../lyrics-model.js";
import { parseChordText, chordLabel, keyLabel, scaleLabel, scaleLayers, snapWhole, newChordLength, resolveQuality, normalizePitch, pitchName, barLength } from "../chord-model.js";

const LANES = ["chords", "keys", "scales", "structure", "lyrics"]; // de haut en bas ; chacune fait un quart de hauteur de piste
import { CATEGORIES, CATEGORY_COLORS, CATEGORY_LABELS, SECTION_NAMES, categoryOf, sectionColor, suggestCategory } from "../section-model.js";
const SECTION_COLORS = [...new Set(Object.values(CATEGORY_COLORS))];
export { sectionColor };
const DEFAULT_LANE_H = 24; // hauteur d'une voie (px) : un peu moins d'un tiers de piste
const MIN_LANE_H = 14;
const MAX_LANE_H = 64;
const EDGE = 5; // zone de saisie d'un bord (px)
const PATTERN_BAND = 4; // bande des appels de motifs en bas de la ligne des gammes (px)
const NOTE_NAMES = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

export const GRID_CHOICES = [
  { id: "1/4", label: "Noire", f: [1, 4] },
  { id: "1/2", label: "Blanche", f: [1, 2] },
  { id: "1/1", label: "Ronde", f: [1, 1] },
  { id: "1/8", label: "Croche", f: [1, 8] },
  { id: "free", label: "Libre", f: null },
];

export class ChordLane {
  constructor(state, client, timeline, dialogs, say) {
    this.state = state;
    this.client = client;
    this.timeline = timeline;
    this.dialogs = dialogs;
    this.say = say;
    this.sel = null; // { kind: "chord" | "scale", index }
    this.drag = null; // { kind, index, zone, grabWhole, orig, moved, list }
    this.gridId = "1/4";
    this.input = null;
    this.pop = null;
    this.dlg = null;
    this.gen = null; // génération du projet vu en dernier : un nouveau projet rouvre la voie s'il a des accords
    this.autoPending = false;
    state.chordsOpen ??= false;
    state.structure ??= [];
    state.lyrics ??= [];
    state.lyricVerse ??= 1; // couplet montré et saisi dans la voie « Paroles »
    try {
      state.laneH ??= Number(localStorage.getItem("daw.laneH")) || DEFAULT_LANE_H;
    } catch {
      state.laneH ??= DEFAULT_LANE_H;
    }
    state.laneH = Math.max(MIN_LANE_H, Math.min(MAX_LANE_H, state.laneH));
    document.documentElement.style.setProperty("--lane-h", `${state.laneH}px`);
    state.chordTrack ??= null;
    state.chordCatalog ??= null;
    client.on("file", (f) => {
      if (f.generation !== this.gen) {
        this.gen = f.generation;
        this.autoPending = true;
      }
    });
    client.on("chords", (e) => {
      state.chordTrack = { revision: e.revision, track: e.track };
      state.structure = e.structure ?? [];
      state.lyrics = e.lyrics ?? [];
      // À l'ouverture d'un projet qui contient des accords ou des gammes, on affiche la voie (une seule fois par projet).
      if (this.autoPending) {
        this.autoPending = false;
        const t = e.track ?? {};
        if (!state.chordsOpen && ((t.chords?.length ?? 0) > 0 || (t.scales?.length ?? 0) > 0 || state.structure.length > 0 || state.lyrics.length > 0)) this.toggle();
      }
      if (e.skipped?.length) this.say(`Carte importée ; ignoré : ${e.skipped.slice(0, 3).join(" ; ")}${e.skipped.length > 3 ? ` (+${e.skipped.length - 3})` : ""}`);
      this.sel = null;
      this.drag = null;
      this.timeline.dirty = true;
    });
    client.on("chord_catalog", (e) => (state.chordCatalog = e));
    this.canvas = document.getElementById("chords");
    const c = this.canvas;
    c.addEventListener("pointerdown", (e) => this.#down(e), true);
    c.addEventListener("pointermove", (e) => this.#move(e), true);
    c.addEventListener("pointerup", (e) => this.#up(e, true), true);
    c.addEventListener("pointercancel", (e) => this.#up(e, false), true);
    c.addEventListener("dblclick", (e) => this.#dbl(e), true);
    window.addEventListener("keydown", (e) => this.#key(e), true);
  }

  get track() {
    const t = this.state.chordTrack?.track ?? {};
    return { chords: [], scales: [], keys: [], patterns: [], measures: [], ...t };
  }

  get grid() {
    return GRID_CHOICES.find((g) => g.id === this.gridId)?.f ?? null;
  }

  /** Taille du canevas épinglé : trois voies d'un quart de hauteur de piste, sur toute la largeur de la timeline. */
  resize() {
    const c = this.canvas;
    c.hidden = !this.state.chordsOpen;
    if (c.hidden) return;
    const dpr = pixelRatio();
    const w = this.timeline.width;
    const h = LANES.length * this.#h;
    c.style.height = `${h}px`;
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
    this.timeline.dirty = true;
  }

  /** Tracé des deux voies (appelé par la timeline à chaque image sale) : fond, grille de mesures, boucle, tête de lecture, puis les blocs. */
  paint() {
    if (!this.state.chordsOpen || this.canvas.hidden) return;
    const dpr = pixelRatio();
    const g = this.canvas.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const W = this.timeline.width;
    const h = this.#h;
    const c = this.timeline.colors;
    const total = LANES.length * h;
    g.clearRect(0, 0, W, total);
    LANES.forEach((_, i) => {
      g.fillStyle = i % 2 ? c["row-b"] : c["row-a"];
      g.fillRect(0, i * h, W, h);
    });
    const { view, tempo, meters_: meters, loop, pos } = this.state;
    if (loop) {
      const x0 = sampleToX(view, loop.start);
      const x1 = sampleToX(view, loop.end);
      g.fillStyle = c.accent + "22";
      g.fillRect(Math.max(0, x0), 0, Math.min(W, x1) - Math.max(0, x0), total);
    }
    for (const l of gridLines(view, W, tempo, meters)) {
      g.fillStyle = l.kind === "bar" ? c.line : l.kind === "beat" ? c.line + "88" : c.line + "44";
      g.fillRect(Math.round(l.x), 0, 1, total);
    }
    g.fillStyle = c.line;
    LANES.forEach((kind, i) => {
      if (i > 0) g.fillRect(0, i * h - 1, W, 1);
      this.draw(g, { kind }, i * h, W);
    });
    const px = Math.round(sampleToX(view, pos));
    if (px >= 0 && px <= W) {
      g.fillStyle = c.text;
      g.fillRect(px, 0, 1, total);
    }
  }

  toggle() {
    this.state.chordsOpen = !this.state.chordsOpen;
    this.sel = null;
    this.timeline.resize();
    this.timeline.settle?.();
    this.onHeaders?.();
  }

  // ---- géométrie ------------------------------------------------------------------------------------------------

  #x(whole) {
    return sampleToX(this.state.view, this.state.tempo.samplesAt(whole));
  }

  #xf(f) {
    return this.#x(fnum(f));
  }

  /** La ligne de la voie sous la position verticale `y` : `{ kind: "chords" | "scales" | "structure", top }` ou `null`. */
  #rowAt(y) {
    const h = this.#h;
    const i = Math.floor(y / h);
    return y >= 0 && i < LANES.length ? { kind: LANES[i], top: i * h } : null;
  }

  /** Hauteur d'une voie : un quart de hauteur de piste. */
  get #h() {
    return this.state.laneH;
  }

  /** Hauteur des trois voies (px, 14 à 64) ; mémorisée. */
  setLaneHeight(px) {
    const h = Math.max(MIN_LANE_H, Math.min(MAX_LANE_H, Math.round(px)));
    if (h === this.state.laneH) return;
    this.state.laneH = h;
    document.documentElement.style.setProperty("--lane-h", `${h}px`);
    try {
      localStorage.setItem("daw.laneH", String(h));
    } catch {
      /* stockage indisponible : la hauteur n'est pas mémorisée */
    }
    this.timeline.resize();
    this.timeline.settle?.();
  }

  /** Poignée sous la voie de structure : tirer vers le bas agrandit les voies (la hauteur suit la souris ou le doigt). */
  startResize(e) {
    e.preventDefault();
    const grip = e.currentTarget;
    grip.setPointerCapture?.(e.pointerId);
    const y0 = e.clientY;
    const h0 = this.state.laneH;
    const scale = this.canvas.getBoundingClientRect().height / (LANES.length * h0) || 1; // taille de l'interface comprise
    const move = (ev) => this.setLaneHeight(h0 + (ev.clientY - y0) / scale / LANES.length);
    const end = () => {
      grip.removeEventListener("pointermove", move);
      grip.removeEventListener("pointerup", end);
      grip.removeEventListener("pointercancel", end);
    };
    grip.addEventListener("pointermove", move);
    grip.addEventListener("pointerup", end);
    grip.addEventListener("pointercancel", end);
  }

  #wholeAt(e, free = false) {
    const { x } = canvasPoint(this.canvas, e);
    const whole = Math.max(0, this.state.tempo.wholeAt(Math.max(0, xToSample(this.state.view, x))));
    return { whole, snapped: snapWhole(whole, e.altKey || free ? null : this.grid) };
  }

  /** Position aimantée au début de mesure le plus proche (les sections commencent sur une mesure ; Alt : libre). */
  #barSnap(whole, alt) {
    const m = this.state.meters_;
    if (alt || !m) return snapWhole(whole, alt ? null : this.grid);
    const { bar, within } = m.locate(whole);
    const len = barLength(m, whole)[0] / barLength(m, whole)[1];
    return snapWhole(within >= len / 2 ? m.barStart(bar + 1) : m.barStart(bar), null);
  }

  /** Blocs de la ligne (avec leur rectangle) pour le tracé et la saisie. */
  #blocks(kind, top) {
    const h = this.#h;
    const t = this.track;
    if (kind === "chords") {
      return t.chords.map((c, index) => ({ index, item: c, x0: this.#xf(c.at), x1: this.#xf(fadd(c.at, c.dur)), y0: top + 1, y1: top + h - 1 }));
    }
    if (kind === "keys") {
      // Une tonalité dure jusqu'à la suivante (ou la fin de la vue) : pas de longueur propre.
      const W = this.timeline.width;
      const keys = t.keys;
      return keys.map((k, index) => ({ index, item: k, x0: this.#xf(k.at), x1: index + 1 < keys.length ? this.#xf(keys[index + 1].at) : Math.max(this.#xf(k.at) + 40, W + 2), y0: top + 1, y1: top + h - 1 }));
    }
    if (kind === "structure") {
      return (this.state.structure ?? []).map((sec, index) => ({ index, item: sec, x0: this.#xf(sec.at), x1: this.#xf(fadd(sec.at, sec.dur)), y0: top + 1, y1: top + h - 1 }));
    }
    if (kind === "lyrics") {
      // Une syllabe va jusqu'à la suivante du même couplet (ou garde une largeur minimale) ; le couplet montré est `state.lyricVerse`.
      const list = verseSyllables(this.state.lyrics ?? [], this.state.lyricVerse ?? 1);
      return list.map((s, i) => {
        const x0 = this.#xf(s.at);
        const next = list[i + 1] ? this.#xf(list[i + 1].at) : x0 + 60;
        return { index: s.index, item: s, x0, x1: Math.max(x0 + 8, next - 1), y0: top + 1, y1: top + h - 1 };
      });
    }
    const layers = scaleLayers(t.scales);
    const nl = Math.min(3, Math.max(1, ...layers.map((l) => l + 1)));
    const area = h - PATTERN_BAND - 2;
    const lh = area / nl;
    return t.scales.map((s, index) => {
      const l = Math.min(layers[index], nl - 1);
      return { index, item: s, x0: this.#xf(s.at), x1: this.#xf(fadd(s.at, s.dur)), y0: top + 1 + l * lh, y1: top + 1 + (l + 1) * lh - 1 };
    });
  }

  #hit(kind, top, x, y) {
    const list = this.#blocks(kind, top);
    for (let i = list.length - 1; i >= 0; i--) {
      const b = list[i];
      if (y < b.y0 || y > b.y1 || x < b.x0 - 2 || x > b.x1 + 2) continue;
      const w = b.x1 - b.x0;
      const zone = kind === "keys" || kind === "lyrics" ? "body" : x <= b.x0 + EDGE && w > 3 * EDGE ? "start" : x >= b.x1 - EDGE && w > 3 * EDGE ? "end" : "body";
      return { ...b, zone };
    }
    return null;
  }

  // ---- tracé ----------------------------------------------------------------------------------------------------

  draw(g, row, top, W) {
    const c = this.timeline.colors;
    const h = this.#h;
    const t = this.track;
    const color = "#a78bfa";
    g.save();
    g.beginPath();
    g.rect(0, top, W, h);
    g.clip();
    g.font = "600 10px system-ui, sans-serif";
    g.textBaseline = "middle";
    const block = (b, label, fill, kind, stroke = color) => {
      if (b.x1 < 0 || b.x0 > W) return;
      const d = this.drag && this.drag.kind === kind && this.drag.index === b.index && this.drag.preview ? this.drag.preview : null;
      const x0 = d ? this.#xf(d.at) : b.x0;
      const x1 = d ? (d.dur ? this.#xf(fadd(d.at, d.dur)) : x0 + (b.x1 - b.x0)) : b.x1;
      const w = Math.max(2, x1 - x0);
      const selected = this.sel && this.sel.kind === kind && this.sel.index === b.index;
      g.fillStyle = fill;
      g.strokeStyle = selected ? c.accent : stroke;
      g.lineWidth = selected ? 2 : 1;
      g.beginPath();
      g.roundRect(x0, b.y0, w, b.y1 - b.y0, 3);
      g.fill();
      g.stroke();
      if (w > 18 && b.y1 - b.y0 >= 9) {
        g.save();
        g.beginPath();
        g.rect(x0 + 2, b.y0, w - 4, b.y1 - b.y0);
        g.clip();
        g.fillStyle = c.text;
        g.fillText(label, x0 + 6, (b.y0 + b.y1) / 2);
        g.restore();
      }
    };
    if (row.kind === "chords") {
      for (const b of this.#blocks("chords", top)) block(b, chordLabel(b.item), b.item.nc ? "rgba(128,128,128,.25)" : color + "44", "chord");
    } else if (row.kind === "structure") {
      for (const b of this.#blocks("structure", top)) {
        const col = sectionColor(b.item);
        block(b, b.item.name, col + "55", "section", col);
      }
    } else if (row.kind === "lyrics") {
      if (!(this.state.lyrics ?? []).length) {
        g.fillStyle = c.muted;
        g.fillText("Paroles : double-clic pour saisir un texte (ma-man est la plus bel-le)", 6, top + h / 2);
      }
      for (const b of this.#blocks("lyrics", top)) block(b, syllableLabel(b.item), "#5eead433", "lyric", "#5eead4");
    } else if (row.kind === "keys") {
      // La tonalité du morceau (avant le premier marqueur) en fond discret, puis un bloc par marqueur, jusqu'au suivant.
      const first = t.keys.length ? this.#xf(t.keys[0].at) : W + 2;
      g.fillStyle = c.muted;
      if (t.globalKey && first > 0) g.fillText(keyName(t.globalKey), 6, top + h / 2);
      else if (!t.globalKey && !t.keys.length) g.fillText("Tonalités : double-clic pour en poser une (armure de la partition)", 6, top + h / 2);
      for (const b of this.#blocks("keys", top)) block(b, keyName(b.item), "#e9a23b33", "key", "#e9a23b");
    } else {
      for (const b of this.#blocks("scales", top)) block(b, scaleLabel(b.item), c.accent + "30", "scale");
      for (const p of t.patterns) {
        const x0 = this.#xf(p.at);
        const x1 = this.#xf(fadd(p.at, p.dur));
        g.fillStyle = color + "88";
        g.fillRect(x0, top + h - PATTERN_BAND, Math.max(2, x1 - x0), PATTERN_BAND - 1);
      }
    }
    g.restore();
  }

  // ---- souris ---------------------------------------------------------------------------------------------------

  #down(e) {
    const { x, y } = canvasPoint(this.canvas, e);
    const row = this.#rowAt(y);
    if (!row) {
      if (this.sel) ((this.sel = null), (this.timeline.dirty = true));
      return;
    }
    if (e.button !== 0) return;
    e.stopImmediatePropagation();
    this.closeEditors();
    this.state.selection = null;
    this.state.selected = new Set();
    this.timeline.onSelection?.();
    const kind = row.kind === "chords" ? "chord" : row.kind === "structure" ? "section" : row.kind === "keys" ? "key" : row.kind === "lyrics" ? "lyric" : "scale";
    const hit = this.#hit(row.kind, row.top, x, y);
    if (!hit) {
      this.sel = null;
      this.timeline.dirty = true;
      return;
    }
    this.sel = { kind, index: hit.index };
    this.canvas.setPointerCapture?.(e.pointerId);
    const list = kind === "chord" ? this.track.chords : kind === "section" ? this.state.structure : kind === "lyric" ? this.state.lyrics : kind === "key" ? this.track.keys : this.track.scales;
    this.drag = { kind, index: hit.index, zone: hit.zone, grab: this.#wholeAt(e).whole, orig: list[hit.index], list, moved: false, preview: null };
    this.timeline.dirty = true;
  }

  #move(e) {
    const d = this.drag;
    const { x, y } = canvasPoint(this.canvas, e);
    if (!d) {
      const row = this.#rowAt(y);
      if (row) {
        e.stopImmediatePropagation();
        const hit = this.#hit(row.kind, row.top, x, y);
        this.canvas.style.cursor = !hit ? "crosshair" : hit.zone === "body" ? "grab" : "ew-resize";
      }
      return;
    }
    e.stopImmediatePropagation();
    const { whole } = this.#wholeAt(e);
    const snapped = d.kind === "section" || d.kind === "key" ? this.#barSnap(whole, e.altKey) : this.#wholeAt(e).snapped;
    const o = d.orig;
    if (d.zone === "body") {
      const dx = whole - d.grab;
      const start = d.kind === "section" || d.kind === "key" ? this.#barSnap(Math.max(0, fnum(o.at) + dx), e.altKey) : snapWhole(Math.max(0, fnum(o.at) + dx), e.altKey ? null : this.grid);
      d.preview = { ...o, at: start };
    } else {
      const end = d.zone === "end" ? snapped : fadd(o.at, o.dur);
      const start = d.zone === "start" ? snapped : o.at;
      d.preview = fcmp(end, start) > 0 ? { ...o, at: start, dur: fsub(end, start) } : null;
    }
    d.moved = d.moved || Math.abs(fnum(d.preview?.at ?? o.at) - fnum(o.at)) > 1e-9 || (o.dur && Math.abs(fnum(d.preview?.dur ?? o.dur) - fnum(o.dur)) > 1e-9);
    this.timeline.dirty = true;
  }

  #up(e, commit) {
    const d = this.drag;
    if (!d) return;
    e.stopImmediatePropagation();
    this.drag = null;
    this.timeline.dirty = true;
    if (!commit || !d.moved || !d.preview) return;
    const p = d.preview;
    if (d.kind === "section") {
      const list = structuredClone(this.state.structure);
      list[d.index] = { ...list[d.index], at: p.at, dur: p.dur };
      this.#sendStructure(list, d.zone === "body" ? "Déplacer une section" : "Changer la durée d'une section");
      return;
    }
    if (d.kind === "lyric") {
      const list = structuredClone(this.state.lyrics);
      list[d.index] = { ...list[d.index], at: normFrac(p.at) };
      this.#sendLyrics(list, "Déplacer une syllabe");
      return;
    }
    const t = structuredClone(this.track);
    if (d.kind === "key") {
      t.keys[d.index] = { ...t.keys[d.index], at: normFrac(p.at) };
      t.keys.sort((a, b) => fnum(a.at) - fnum(b.at));
      this.#send(t, "Déplacer une tonalité");
      return;
    }
    const list = d.kind === "chord" ? t.chords : t.scales;
    list[d.index] = { ...list[d.index], at: p.at, dur: p.dur };
    this.#send(t, d.zone === "body" ? "Déplacer" : "Changer la durée");
  }

  #dbl(e) {
    const { x, y } = canvasPoint(this.canvas, e);
    const row = this.#rowAt(y);
    if (!row) return;
    e.stopImmediatePropagation();
    const hit = this.#hit(row.kind, row.top, x, y);
    const at = row.kind === "structure" || row.kind === "keys" ? this.#barSnap(this.#wholeAt(e).whole, e.altKey) : this.#wholeAt(e).snapped;
    if (row.kind === "chords") this.#chordInput(e, hit?.index ?? null, at);
    else if (row.kind === "keys") this.#keyPopup(e, hit?.index ?? null, at);
    else if (row.kind === "structure") this.#sectionPopup(e, hit?.index ?? null, at);
    else if (row.kind === "lyrics") this.#lyricPopup(e, hit?.index ?? null, at);
    else this.#scalePopup(e, hit?.index ?? null, at);
  }

  #key(e) {
    if (e.key === "Escape" && (this.drag || this.input || this.pop || this.dlg)) {
      this.drag = null;
      this.closeEditors();
      this.timeline.dirty = true;
      return;
    }
    if ((e.key === "Delete" || e.key === "Backspace") && this.sel && !this.dlg && !/^(INPUT|SELECT|TEXTAREA)$/.test(e.target?.tagName ?? "")) {
      e.preventDefault();
      e.stopImmediatePropagation();
      const { kind, index } = this.sel;
      if (kind === "section") {
        const list = structuredClone(this.state.structure);
        list.splice(index, 1);
        this.sel = null;
        this.#sendStructure(list, "Supprimer une section");
        return;
      }
      if (kind === "lyric") {
        const list = structuredClone(this.state.lyrics);
        list.splice(index, 1);
        this.sel = null;
        this.#sendLyrics(list, "Supprimer une syllabe");
        return;
      }
      const t = structuredClone(this.track);
      (kind === "chord" ? t.chords : kind === "key" ? t.keys : t.scales).splice(index, 1);
      this.sel = null;
      this.#send(t, kind === "scale" ? "Supprimer une gamme" : kind === "key" ? "Supprimer une tonalité" : "Supprimer un accord");
    }
  }

  // ---- saisie ---------------------------------------------------------------------------------------------------

  closeEditors() {
    // Les références d'abord : retirer le champ déclenche son `blur`, qui ne doit plus rien trouver à fermer.
    const [input, pop, dlg] = [this.input, this.pop, this.dlg];
    this.input = this.pop = this.dlg = null;
    input?.remove();
    pop?.remove();
    dlg?.close();
  }

  #host() {
    return this.canvas.parentElement;
  }

  #place(el, e, width = 190) {
    const wrap = this.#host();
    const r = wrap.getBoundingClientRect();
    el.style.position = "absolute";
    el.style.left = `${Math.max(0, Math.min(e.clientX - r.left, r.width - width))}px`;
    el.style.top = `${Math.max(0, e.clientY - r.top)}px`;
    el.style.zIndex = "30";
    wrap.appendChild(el);
  }

  /** Un nouvel accord à `at` ou le renommage de l'accord `index` : fenêtre à trois colonnes (fondamentale, qualité, basse). */
  #chordInput(e, index, at) {
    this.closeEditors();
    const t = this.track;
    const cur = index === null ? null : t.chords[index];
    if (index === null && newChordLength(t.chords, at, this.#barLen(at)) === null) return this.say("Il y a déjà un accord ici : double-clique dessus pour le modifier, ou choisis une place libre.");
    const dlg = createChordDialog({
      title: cur ? "Modifier l'accord" : "Nouvel accord",
      catalog: this.state.chordCatalog,
      initial: cur ? chordLabel(cur) : "",
      onOk: (text) => this.#applyChord(index, at, text),
      onCancel: () => this.closeEditors(),
    });
    this.dlg = dlg;
    this.#place(dlg.el, e, 440);
    dlg.focus();
    dlg.select();
  }

  /** Durée d'une mesure à la position `at` (fraction de ronde), d'après la carte des chiffrages du projet. */
  #barLen(at) {
    return barLength(this.state.meters_, fnum(at));
  }

  #applyChord(index, at, text) {
    const p = parseChordText(text);
    if (!p) return this.say(`« ${text} » : accord illisible (exemples : Dm7, G7/B, Bb, N.C.).`);
    const t = structuredClone(this.track);
    let item;
    if (p.nc) item = { nc: true };
    else {
      const quality = resolveQuality(p.quality, this.state.chordCatalog);
      if (quality === null) return this.say(`« ${p.quality} » : qualité inconnue (ni dans le dictionnaire de Scorsaire ni écrite [0,4,7,13]).`);
      item = { root: p.root, quality, bass: p.bass };
    }
    if (index === null) {
      const len = newChordLength(t.chords, at, this.#barLen(at));
      t.chords.push({ at: normFrac(at), dur: normFrac(len), ...item });
      this.#send(t, "Ajouter un accord");
    } else {
      const old = t.chords[index];
      t.chords[index] = { at: old.at, dur: old.dur, ...item };
      this.#send(t, "Renommer un accord");
    }
  }

  #scalePopup(e, index, at) {
    this.closeEditors();
    const cat = this.state.chordCatalog;
    if (!cat) return this.say("Le catalogue de gammes n'est pas encore arrivé.");
    const cur = index === null ? null : this.track.scales[index];
    const pop = document.createElement("div");
    pop.className = "chord-pop";
    const rootSel = document.createElement("select");
    for (const n of NOTE_NAMES) rootSel.add(new Option(n, n));
    const sel = document.createElement("select");
    for (const s of cat.scales) sel.add(new Option(s.fr, s.id));
    if (cur) {
      rootSel.value = NOTE_NAMES.includes(cur.root) ? cur.root : pitchName({ pc: 0, sharp: false });
      if (cur.scale) sel.value = cur.scale;
    }
    const lenIn = document.createElement("input");
    lenIn.type = "number";
    lenIn.min = "0.25";
    lenIn.step = "0.25";
    const bar = this.#barLen(at);
    lenIn.value = cur ? String(fnum(cur.dur) * 4) : String((bar[0] * 4) / bar[1]);
    lenIn.title = "Durée en noires (une mesure par défaut)";
    const mk = (label, fn) => {
      const b = document.createElement("button");
      b.textContent = label;
      b.addEventListener("click", fn);
      return b;
    };
    const ok = mk("OK", () => {
      const s = cat.scales.find((x) => x.id === sel.value);
      const dur = Math.max(1, Math.round(Number(lenIn.value) * 4)) / 16; // noires → rondes, au seizième de ronde
      const t = structuredClone(this.track);
      const entry = { at: normFrac(cur ? cur.at : at), dur: normFrac(cur ? (Number(lenIn.value) === fnum(cur.dur) * 4 ? cur.dur : toFrac(dur)) : toFrac(dur)), root: normalizePitch(rootSel.value), scale: s.id, intervals: s.intervals, name: s.fr };
      if (index === null) t.scales.push(entry);
      else t.scales[index] = entry;
      this.closeEditors();
      this.#send(t, index === null ? "Ajouter une gamme" : "Changer une gamme");
    });
    const cancel = mk("Annuler", () => this.closeEditors());
    pop.append(label("Fondamentale", rootSel), label("Gamme", sel), label("Durée (noires)", lenIn), ok, cancel);
    pop.addEventListener("keydown", (k) => {
      k.stopPropagation();
      if (k.key === "Escape") this.closeEditors();
    });
    this.pop = pop;
    this.#place(pop, e);
    sel.focus();
  }

  /** Pose (ou, avec `choice = null`, retire) la tonalité de la mesure qui commence à `at` : le geste commun à la voie « Tonalités » et à la partition. */
  setKeyAt(at, choice) {
    const t = structuredClone(this.track);
    const a = normFrac(at);
    t.keys = t.keys.filter((k) => fcmp(k.at, a) !== 0);
    if (choice) t.keys.push(keyEntry(choice, a));
    t.keys.sort((x, y) => fnum(x.at) - fnum(y.at));
    this.#send(t, choice ? "Poser une tonalité" : "Retirer une tonalité");
  }

  /** Pose une tonalité au **début de la mesure où se trouve la tête de lecture** : ouvre le choix des 30 tonalités (touche K, ou ⋯ de la voie). */
  placeKeyAtPlayhead(anchor = null) {
    if (!this.state.chordsOpen) this.toggle();
    const m = this.state.meters_;
    const bar = m.locate(Math.max(0, this.state.tempo.wholeAt(Math.max(0, this.state.pos)))).bar;
    const at = snapWhole(m.barStart(bar), null);
    const r = (anchor ?? this.canvas).getBoundingClientRect();
    this.#keyPopup({ clientX: anchor ? r.left : r.left + 40, clientY: anchor ? r.bottom : r.top + 40 }, null, at);
  }

  /** Une tonalité (armure) : on choisit parmi les 30 tonalités ; elle vaut jusqu'à la suivante et donne l'armure de la partition. */
  #keyPopup(e, index, at) {
    this.closeEditors();
    const t = structuredClone(this.track);
    const exact = index === null ? t.keys.findIndex((k) => fcmp(k.at, normFrac(at)) === 0) : -1; // un marqueur déjà posé là : on le modifie
    const idx = index ?? (exact >= 0 ? exact : null);
    const cur = idx === null ? null : t.keys[idx];
    const pop = document.createElement("div");
    pop.className = "chord-pop";
    const sel = document.createElement("select");
    for (const c of KEY_CHOICES) sel.append(new Option(c.label, c.id));
    sel.value = (choiceOf(cur) ?? choiceOf({ fifths: 0, mode: "major" })).id;
    const mk = (label, fn) => {
      const b = document.createElement("button");
      b.textContent = label;
      b.addEventListener("click", fn);
      return b;
    };
    const ok = mk("OK", () => {
      const choice = KEY_CHOICES.find((c) => c.id === sel.value);
      const entry = keyEntry(choice, normFrac(cur ? cur.at : at));
      if (idx === null) t.keys.push(entry);
      else t.keys[idx] = entry;
      t.keys.sort((a, b) => fnum(a.at) - fnum(b.at));
      this.closeEditors();
      this.#send(t, idx === null ? "Ajouter une tonalité" : "Changer une tonalité");
    });
    const row = document.createElement("div");
    row.className = "cd-foot";
    const del = idx === null ? null : mk("Supprimer", () => {
      t.keys.splice(idx, 1);
      this.closeEditors();
      this.#send(t, "Supprimer une tonalité");
    });
    row.append(...[del, mk("Annuler", () => this.closeEditors()), ok].filter(Boolean));
    pop.append(label("Tonalité à partir d'ici", sel), row);
    pop.addEventListener("keydown", (k) => {
      k.stopPropagation();
      if (k.key === "Escape") this.closeEditors();
      else if (k.key === "Enter") ok.click();
    });
    this.pop = pop;
    this.#place(pop, e, 260);
    sel.focus();
  }

  #sendLyrics(syllables, label) {
    this.client.send({ type: "set_lyrics", syllables, label });
  }

  /** Le pas entre deux syllabes saisies d'un coup : la grille de la voie (noire si elle est libre). */
  get #lyricStep() {
    return this.grid ?? [1, 4];
  }

  /**
   * Saisie des paroles : un texte (« ma-man est la plus bel-le ») posé à partir de `at`, une syllabe par pas de la grille de la voie, dans le couplet
   * choisi ; sur une syllabe existante, son texte est remplacé (les suivantes du texte se posent après). Des syllabes déjà posées au même
   * endroit dans le même couplet sont remplacées.
   */
  #lyricPopup(e, index, at) {
    this.closeEditors();
    const cur = index === null ? null : this.state.lyrics[index];
    const pop = document.createElement("div");
    pop.className = "chord-pop";
    const text = document.createElement("input");
    text.value = cur ? syllableLabel(cur) : "";
    text.placeholder = "ma-man est la plus bel-le";
    text.setAttribute("aria-label", "Texte des paroles");
    const verse = document.createElement("select");
    for (let v = 1; v <= verseCount(this.state.lyrics); v++) verse.append(new Option(`Couplet ${v}`, String(v)));
    verse.value = String(cur?.verse ?? this.state.lyricVerse ?? 1);
    const hint = document.createElement("div");
    hint.className = "cd-hint";
    const stepLabel = GRID_CHOICES.find((g) => g.id === this.gridId)?.label ?? "noire";
    hint.textContent = `Une syllabe par ${stepLabel.toLowerCase()} (réglable dans ⋯). Tiret = suite du mot, espace = mot suivant.`;
    const mk = (label, fn) => {
      const b = document.createElement("button");
      b.textContent = label;
      b.addEventListener("click", fn);
      return b;
    };
    const ok = mk("OK", () => {
      const items = parseLyricsText(text.value);
      const v = Number(verse.value);
      if (!items.length) return this.say("Écris au moins une syllabe (ou utilise Supprimer).");
      const base = cur ? this.state.lyrics.filter((_, i) => i !== index) : this.state.lyrics;
      const added = placeSyllables(items, normFrac(cur ? cur.at : at), this.#lyricStep, v);
      this.state.lyricVerse = v;
      this.closeEditors();
      this.#sendLyrics(mergeSyllables(base, added), cur ? "Changer une syllabe" : "Saisir des paroles");
    });
    const row = document.createElement("div");
    row.className = "cd-foot";
    const del = cur ? mk("Supprimer", () => {
      const list = structuredClone(this.state.lyrics);
      list.splice(index, 1);
      this.closeEditors();
      this.#sendLyrics(list, "Supprimer une syllabe");
    }) : null;
    row.append(...[del, mk("Annuler", () => this.closeEditors()), ok].filter(Boolean));
    pop.append(label("Texte", text), label("Couplet", verse), hint, row);
    pop.addEventListener("keydown", (k) => {
      k.stopPropagation();
      if (k.key === "Escape") this.closeEditors();
      else if (k.key === "Enter") ok.click();
    });
    this.pop = pop;
    this.#place(pop, e, 280);
    text.focus();
    text.select();
  }

  #sendStructure(sections, label) {
    this.client.send({ type: "set_structure", sections, label });
  }

  /** Une section (intro, couplet, refrain...) : nom au choix ou saisi, couleur, durée en mesures. */
  #sectionPopup(e, index, at) {
    this.closeEditors();
    const cur = index === null ? null : this.state.structure[index];
    const list = structuredClone(this.state.structure);
    const bar = this.#barLen(at);
    const pop = document.createElement("div");
    pop.className = "chord-pop";
    const name = document.createElement("input");
    name.setAttribute("list", "section-names");
    name.value = cur?.name ?? "";
    name.placeholder = "Intro, Couplet, Refrain…";
    name.maxLength = 60;
    const dl = document.createElement("datalist");
    dl.id = "section-names";
    for (const [n] of SECTION_NAMES) dl.append(new Option(n));
    const bars = document.createElement("input");
    bars.type = "number";
    bars.min = "1";
    bars.step = "1";
    bars.value = String(cur ? Math.max(1, Math.round(fnum(cur.dur) / (bar[0] / bar[1]))) : 4);
    // La catégorie suit le nom tant qu'on ne l'a pas choisie à la main ; elle donne la couleur par défaut (comme dans Scorsaire).
    const catSel = document.createElement("select");
    for (const c of CATEGORIES) catSel.append(new Option(CATEGORY_LABELS[c], c));
    catSel.value = cur ? categoryOf(cur) : suggestCategory(name.value);
    let catTouched = !!cur?.category;
    catSel.addEventListener("change", () => (catTouched = true));
    name.addEventListener("input", () => {
      if (!catTouched) catSel.value = suggestCategory(name.value);
    });
    let color = cur?.color ?? null;
    const swatches = document.createElement("div");
    swatches.className = "cd-swatches";
    const paint = () => swatches.querySelectorAll("button").forEach((b) => b.setAttribute("aria-pressed", String((b.dataset.c || null) === color)));
    for (const c of [null, ...SECTION_COLORS]) {
      const b = document.createElement("button");
      b.type = "button";
      b.dataset.c = c ?? "";
      b.title = c ? c : "Couleur automatique d'après la catégorie";
      b.style.background = c ?? "var(--panel-2)";
      b.textContent = c ? "" : "auto";
      b.addEventListener("click", () => ((color = c), paint()));
      swatches.append(b);
    }
    paint();
    const mk = (label, fn) => {
      const b = document.createElement("button");
      b.textContent = label;
      b.addEventListener("click", fn);
      return b;
    };
    const ok = mk("OK", () => {
      const nm = name.value.trim();
      if (!nm) return this.say("Donne un nom à la section.");
      const nBars = Math.max(1, Math.round(Number(bars.value) || 1));
      const dur = normFrac([bar[0] * nBars, bar[1]]);
      const entry = { at: normFrac(cur ? cur.at : at), dur, name: nm, category: catSel.value, ...(color ? { color } : {}) };
      if (index === null) {
        const free = this.#freeLength(list, entry.at, dur);
        if (free === null) return this.say("Il y a déjà une section ici : choisis une place libre.");
        entry.dur = free;
        list.push(entry);
      } else list[index] = entry;
      this.closeEditors();
      this.#sendStructure(list, index === null ? "Ajouter une section" : "Changer une section");
    });
    const row = document.createElement("div");
    row.className = "cd-foot";
    const del = index === null ? null : mk("Supprimer", () => {
      list.splice(index, 1);
      this.closeEditors();
      this.#sendStructure(list, "Supprimer une section");
    });
    row.append(...[del, mk("Annuler", () => this.closeEditors()), ok].filter(Boolean));
    pop.append(label("Nom", name), dl, label("Catégorie", catSel), label("Durée (mesures)", bars), swatches, row);
    pop.addEventListener("keydown", (k) => {
      k.stopPropagation();
      if (k.key === "Escape") this.closeEditors();
      else if (k.key === "Enter") ok.click();
    });
    this.pop = pop;
    this.#place(pop, e);
    name.focus();
    name.select();
  }

  /** Durée possible pour une section posée en `at` : la voulue, ou jusqu'à la suivante ; `null` si `at` tombe dans une section. */
  #freeLength(list, at, want) {
    let len = want;
    for (const s of list) {
      const end = fadd(s.at, s.dur);
      if (fcmp(at, s.at) >= 0 && fcmp(at, end) < 0) return null;
      if (fcmp(s.at, at) > 0 && fcmp(fsub(s.at, at), len) < 0) len = fsub(s.at, at);
    }
    return len;
  }

  /** Fenêtre de réglage d'une voie (le bouton ⋯ de son en-tête) : grille de la saisie, import / export, fermeture. */
  openSettings(anchor, kind) {
    this.closeEditors();
    const pop = document.createElement("div");
    pop.className = "chord-pop chord-settings";
    const mk = (label, fn) => {
      const b = document.createElement("button");
      b.textContent = label;
      b.addEventListener("click", () => {
        this.closeEditors();
        fn();
      });
      return b;
    };
    const sel = document.createElement("select");
    for (const g of GRID_CHOICES) sel.add(new Option(g.label, g.id));
    sel.value = this.gridId;
    sel.addEventListener("change", () => (this.gridId = sel.value));
    pop.append(label("Grille de la saisie (Alt : libre)", sel));
    const hs = document.createElement("input");
    hs.type = "range";
    hs.min = String(MIN_LANE_H);
    hs.max = String(MAX_LANE_H);
    hs.step = "1";
    hs.value = String(this.state.laneH);
    hs.setAttribute("aria-label", "Hauteur des voies");
    hs.addEventListener("input", () => this.setLaneHeight(Number(hs.value)));
    pop.append(label("Hauteur des voies (ou tirer le trait sous les paroles)", hs));
    if (kind === "lyrics") {
      const vs = document.createElement("select");
      for (let v = 1; v <= verseCount(this.state.lyrics); v++) vs.append(new Option(`Couplet ${v}`, String(v)));
      vs.value = String(this.state.lyricVerse ?? 1);
      vs.addEventListener("change", () => ((this.state.lyricVerse = Number(vs.value)), (this.timeline.dirty = true)));
      pop.append(label(`Couplet montré dans la voie (1 à ${MAX_VERSE})`, vs));
    }
    if (kind === "chords") pop.append(mk("Importer une carte…", () => this.importFile()), mk("Exporter la carte…", () => this.exportFile()));
    if (kind === "structure") {
      const add = document.createElement("div");
      add.className = "cd-swatches names";
      for (const [n, cat] of SECTION_NAMES) {
        const b = mk(n, () => {
          const at = snapWhole(this.state.tempo.wholeAt(Math.max(0, this.state.pos)), this.grid);
          const bar = this.#barLen(at);
          const list = structuredClone(this.state.structure);
          const dur = this.#freeLength(list, at, normFrac([bar[0] * 4, bar[1]]));
          if (dur === null) return this.say("Il y a déjà une section à la tête de lecture.");
          list.push({ at: normFrac(at), dur, name: n, category: cat });
          this.#sendStructure(list, "Ajouter une section");
        });
        add.append(b);
      }
      pop.append(label("Ajouter à la tête de lecture", add));
    }
    if (kind === "keys") pop.append(mk("Poser une tonalité à la mesure de la tête de lecture…", () => this.placeKeyAtPlayhead(anchor)));
    pop.append(mk("Fermer les voies", () => this.toggle()));
    pop.addEventListener("keydown", (k) => {
      k.stopPropagation();
      if (k.key === "Escape") this.closeEditors();
    });
    this.pop = pop;
    const r = anchor.getBoundingClientRect();
    this.#place(pop, { clientX: r.right, clientY: r.bottom });
  }

  #send(track, label) {
    this.client.send({ type: "set_chords", track, label });
  }

  // ---- fichiers -------------------------------------------------------------------------------------------------

  async importFile() {
    const path = await this.dialogs.pickJson?.();
    if (path) this.client.send({ type: "import_chords", path });
  }

  async exportFile() {
    const path = await this.dialogs.pickSaveJson?.("carte.json");
    if (path) this.client.send({ type: "export_chords", path });
  }
}

function label(text, el) {
  const l = document.createElement("label");
  l.textContent = text;
  l.append(el);
  return l;
}

const normFrac = ([n, d]) => {
  const g = (a, b) => (b ? g(b, a % b) : a);
  const k = g(Math.abs(n), Math.abs(d)) || 1;
  return [n / k, d / k];
};
const toFrac = (x) => normFrac([Math.round(x * 3840), 3840]);
