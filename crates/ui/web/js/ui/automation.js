// Voie d'automation : sous une piste, une ligne (même hauteur) où l'on voit et édite la courbe d'un paramètre — volume, panoramique,
// niveau d'un envoi ou paramètre d'un plugin. Clic dans le vide : ajouter un point ; glisser : déplacer (le temps s'aimante sur la
// grille visible, Alt pour l'éviter) ; Maj + clic sur un point : palier ↔ ligne ; double-clic ou Suppr : supprimer. Chaque geste
// est une seule commande `set_lane` (un pas d'annulation).

import { canvasPoint } from "../ui-scale.js";
import { menuEl, showMenu, closeMenu } from "./track-menu.js";
import { isWriteMode } from "./gesture.js";
import { sampleToX, xToSample, snapSample } from "../time.js";
import { fnum } from "../notes-model.js";
import { rangeFor, valueToFrac, fracToValue, toPos, insertPoint, movePoint, removePoint, toggleCurve, targetKey, sameTarget, targetLabel, valueText } from "../automation-model.js";

const PAD = 7; // marge verticale de la zone de tracé (px)
const HIT = 9; // rayon de saisie d'un point (px)
const norm = (t) => t.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();

export class AutomationEditor {
  constructor(state, client, timeline) {
    this.state = state;
    this.client = client;
    this.timeline = timeline;
    this.params = new Map(); // plugin → liste de paramètres (événement `plugin_params`)
    this.live = new Map(); // nœud|cible → points [échantillon, valeur] enregistrés pour l'instant (aperçu de l'écriture)
    this.drag = null; // { node, target, points, index, created, moved }
    this.selected = null; // { node, index }
    this.onHeaders = null; // rappelé quand une cible ou un mode change (les en-têtes se redessinent)
    state.autoOpen ??= new Set(); // nœuds dont la voie d'automation est dépliée
    state.autoTarget ??= new Map(); // nœud → cible affichée
    state.autoMode ??= new Map(); // nœud+cible sans courbe encore → mode à donner à la première pose
    client.on("plugin_params", (e) => {
      this.params.set(e.insert, e.params);
      this.paramsMenu?.();
      this.timeline.dirty = true;
      this.onHeaders?.();
    });
    client.on("write_live", (e) => this.liveUpdate(e));
    const c = timeline.tracks;
    c.addEventListener("pointerdown", (e) => this.#down(e), true);
    c.addEventListener("pointermove", (e) => this.#move(e), true);
    c.addEventListener("pointerup", (e) => this.#up(e, true), true);
    c.addEventListener("pointercancel", (e) => this.#up(e, false), true);
    c.addEventListener("dblclick", (e) => this.#dbl(e), true);
  }

  /** Aperçu en direct de ce qui s'écrit : l'hôte envoie les nouveaux points au fil de l'eau ; `done` efface l'aperçu. */
  liveUpdate(e) {
    const key = `${e.node}|${targetKey(e.target)}`;
    if (e.done) this.live.delete(key);
    else {
      const pts = this.live.get(key) ?? [];
      pts.splice(e.index, pts.length - e.index, ...e.points);
      this.live.set(key, pts);
    }
    this.timeline.dirty = true;
  }

  // ---- état ------------------------------------------------------------------------------------------------------

  toggle(nodeId) {
    const open = this.state.autoOpen;
    if (open.has(nodeId)) open.delete(nodeId);
    else open.add(nodeId);
    this.timeline.resize();
    this.timeline.settle?.();
    this.onHeaders?.(true);
  }

  node(id) {
    return this.state.project.nodes.find((n) => n.id === id);
  }

  target(nodeId) {
    const node = this.node(nodeId);
    const t = this.state.autoTarget.get(nodeId) ?? { kind: "gain" };
    // La cible mémorisée peut avoir disparu (envoi retiré, plugin retiré) : retour au volume.
    if (t.kind === "send" && !node?.sends.some((s) => s.to === t.to)) return { kind: "gain" };
    if (t.kind === "insert" && !node?.inserts.some((i) => i.id === t.insert)) return { kind: "gain" };
    return t;
  }

  setTarget(nodeId, target) {
    this.state.autoTarget.set(nodeId, target);
    if (target.kind === "insert" && !this.params.has(target.insert)) this.client.send({ type: "get_plugin_params", insert: target.insert });
    this.selected = null;
    this.timeline.dirty = true;
    this.onHeaders?.();
  }

  lane(nodeId, target = this.target(nodeId)) {
    return this.node(nodeId)?.automation.find((l) => sameTarget(l.target, target)) ?? null;
  }

  /** Armement d'écriture (`touch`, `latch`, `write`) de ce paramètre, ou `null`. */
  writeMode(nodeId, target = this.target(nodeId)) {
    return this.state.project.write_arms?.find((a) => a.node === nodeId && sameTarget(a.target, target))?.mode ?? null;
  }

  /** Mode affiché : l'armement d'écriture s'il y en a un, sinon le mode de la courbe (ou celui de la future courbe). */
  mode(nodeId, target = this.target(nodeId)) {
    return this.writeMode(nodeId, target) ?? this.lane(nodeId, target)?.mode ?? this.state.autoMode.get(`${nodeId}|${targetKey(target)}`) ?? "trim";
  }

  setMode(nodeId, mode) {
    const target = this.target(nodeId);
    if (isWriteMode(mode)) {
      this.client.send({ type: "set_write_mode", node: nodeId, target, mode });
      return;
    }
    if (this.writeMode(nodeId, target)) this.client.send({ type: "set_write_mode", node: nodeId, target, mode: "none" });
    if (this.lane(nodeId, target)) this.client.send({ type: "set_lane_mode", node: nodeId, target, mode });
    else this.state.autoMode.set(`${nodeId}|${targetKey(target)}`, mode);
    this.onHeaders?.();
  }

  param(target) {
    return target.kind === "insert" ? this.params.get(target.insert)?.find((p) => p.id === target.param) ?? null : null;
  }

  label(nodeId, target) {
    const node = this.node(nodeId);
    return targetLabel(node, this.state.project, target, this.param(target)?.name ?? null);
  }

  /** Menu de choix d'un paramètre de plugin : recherche sans accents ni casse, au plus 60 lignes à la fois. */
  chooseParam(anchor, nodeId, insertId) {
    const node = this.node(nodeId);
    const ins = node?.inserts.find((i) => i.id === insertId);
    if (!ins) return;
    if (!this.params.has(insertId)) this.client.send({ type: "get_plugin_params", insert: insertId });
    const m = menuEl();
    m.classList.add("time-form", "param-chooser");
    const h = document.createElement("div");
    h.className = "heading";
    h.textContent = `Paramètre de « ${ins.name} »`;
    const search = document.createElement("input");
    search.type = "search";
    search.placeholder = "Rechercher…";
    search.setAttribute("aria-label", "Rechercher un paramètre");
    const list = document.createElement("div");
    list.className = "pc-list";
    m.append(h, search, list);
    const render = () => {
      list.replaceChildren();
      const params = this.params.get(insertId);
      if (!params) return void (list.textContent = "Chargement des paramètres…");
      const q = norm(search.value);
      const rows = params.filter((p) => !q || norm(`${p.module} ${p.name}`).includes(q)).slice(0, 60);
      for (const p of rows) {
        const b = document.createElement("button");
        b.className = "item";
        b.textContent = p.module ? `${p.module} › ${p.name}` : p.name;
        b.addEventListener("click", () => {
          closeMenu();
          this.setTarget(nodeId, { kind: "insert", insert: insertId, param: p.id });
        });
        list.append(b);
      }
      if (!rows.length) list.textContent = "Aucun paramètre ne correspond.";
    };
    search.addEventListener("input", render);
    this.paramsMenu = render;
    showMenu(m, anchor);
    render();
    search.focus();
  }

  /** Supprime le point sélectionné (touche Suppr) ; `true` si quelque chose a été fait. */
  deleteSelected() {
    const s = this.selected;
    if (!s) return false;
    const target = this.target(s.node);
    const lane = this.lane(s.node, target);
    if (!lane || !lane.points[s.index]) return false;
    this.#send(s.node, target, removePoint(lane.points, s.index), "Supprimer un point");
    this.selected = null;
    return true;
  }

  #send(nodeId, target, points, label) {
    const msg = { type: "set_lane", node: nodeId, target, points, label };
    const m = this.state.autoMode.get(`${nodeId}|${targetKey(target)}`);
    if (!this.lane(nodeId, target) && m) msg.mode = m;
    this.client.send(msg);
  }

  // ---- géométrie ------------------------------------------------------------------------------------------------

  #rowAt(y) {
    const rows = this.timeline.nodes();
    const i = Math.floor(y / this.timeline.rowH);
    const row = rows[i];
    return row?.kind === "lane" ? { row, top: i * this.timeline.rowH } : null;
  }

  #range(nodeId, target) {
    return rangeFor(target, this.param(target));
  }

  #xy(point, range, top) {
    const h = this.timeline.rowH;
    const sample = this.state.tempo.samplesAt(fnum(point.pos));
    return { x: sampleToX(this.state.view, sample), y: top + PAD + (1 - valueToFrac(range, point.value)) * (h - 2 * PAD) };
  }

  #hit(points, range, top, x, y) {
    let best = -1;
    let bestD = HIT;
    points.forEach((p, i) => {
      const q = this.#xy(p, range, top);
      const d = Math.hypot(q.x - x, q.y - y);
      if (d <= bestD) [best, bestD] = [i, d];
    });
    return best;
  }

  /** Position (fraction de ronde) et valeur sous le pointeur ; le temps est aimanté sur la grille visible sauf avec Alt. */
  #at(e, range, top) {
    const { x, y } = canvasPoint(this.timeline.tracks, e);
    let sample = Math.max(0, xToSample(this.state.view, x));
    if (!e.altKey) sample = snapSample(this.state.view, this.timeline.width, this.state.tempo, this.state.meters_, sample);
    const h = this.timeline.rowH;
    const frac = 1 - (y - top - PAD) / (h - 2 * PAD);
    return { pos: toPos(this.state.tempo.wholeAt(sample)), value: fracToValue(range, frac) };
  }

  // ---- souris ---------------------------------------------------------------------------------------------------

  #down(e) {
    if (e.button !== 0) return;
    const { x, y } = canvasPoint(this.timeline.tracks, e);
    const r = this.#rowAt(y);
    if (!r) return;
    e.stopImmediatePropagation();
    this.timeline.tracks.setPointerCapture?.(e.pointerId);
    const nodeId = r.row.id;
    const target = this.target(nodeId);
    const range = this.#range(nodeId, target);
    const points = this.lane(nodeId, target)?.points ?? [];
    const hit = this.#hit(points, range, r.top, x, y);
    this.state.selection = null;
    this.state.selected = new Set();
    this.timeline.onSelection?.();
    if (hit >= 0) {
      this.selected = { node: nodeId, index: hit };
      if (e.shiftKey) return this.#send(nodeId, target, toggleCurve(points, hit), "Palier / ligne");
      this.drag = { node: nodeId, target, top: r.top, range, points, index: hit, created: false, moved: false };
    } else {
      const { pos, value } = this.#at(e, range, r.top);
      const ins = insertPoint(points, pos, value);
      this.selected = { node: nodeId, index: ins.index };
      this.drag = { node: nodeId, target, top: r.top, range, points: ins.points, index: ins.index, created: true, moved: true };
    }
    this.timeline.dirty = true;
  }

  #move(e) {
    const d = this.drag;
    if (!d) {
      const { y } = canvasPoint(this.timeline.tracks, e);
      if (this.#rowAt(y)) {
        e.stopImmediatePropagation();
        this.timeline.tracks.style.cursor = "crosshair";
      }
      return;
    }
    e.stopImmediatePropagation();
    const { pos, value } = this.#at(e, d.range, d.top);
    const m = movePoint(d.points, d.index, pos, value);
    d.points = m.points;
    d.moved = true;
    this.timeline.dirty = true;
  }

  #up(e, commit) {
    const d = this.drag;
    if (!d) return;
    e.stopImmediatePropagation();
    this.drag = null;
    if (commit && d.moved) this.#send(d.node, d.target, d.points, d.created ? "Ajouter un point" : "Déplacer un point");
    this.timeline.dirty = true;
  }

  #dbl(e) {
    const { x, y } = canvasPoint(this.timeline.tracks, e);
    const r = this.#rowAt(y);
    if (!r) return;
    e.stopImmediatePropagation();
    const target = this.target(r.row.id);
    const lane = this.lane(r.row.id, target);
    if (!lane) return;
    const hit = this.#hit(lane.points, this.#range(r.row.id, target), r.top, x, y);
    if (hit >= 0) this.#send(r.row.id, target, removePoint(lane.points, hit), "Supprimer un point");
  }

  // ---- dessin ---------------------------------------------------------------------------------------------------

  /** Dessine la ligne d'automation du nœud `nodeId` dont le haut est à `top` (appelé par la timeline). */
  draw(g, nodeId, top, W) {
    const node = this.node(nodeId);
    if (!node) return;
    const rowH = this.timeline.rowH;
    const target = this.target(nodeId);
    const range = this.#range(nodeId, target);
    const lane = this.lane(nodeId, target);
    const mode = this.mode(nodeId, target);
    const colors = this.timeline.colors;
    g.fillStyle = "rgba(0,0,0,.22)";
    g.fillRect(0, top, W, rowH - 1);
    // Repères : 0 dB / centre, et bords de la plage.
    g.fillStyle = "rgba(128,128,128,.35)";
    const ref = range.kind === "db" ? valueToFrac(range, 1) : range.min === -1 ? 0.5 : null;
    if (ref !== null) g.fillRect(0, Math.round(top + PAD + (1 - ref) * (rowH - 2 * PAD)), W, 1);
    g.fillRect(0, top + PAD, W, 1);
    g.fillRect(0, top + rowH - PAD - 1, W, 1);
    const points = this.drag && this.drag.node === nodeId ? this.drag.points : (lane?.points ?? []);
    const live = this.live.get(`${nodeId}|${targetKey(target)}`);
    if (!points.length && live?.length) return this.#drawLive(g, live, range, top, W, rowH);
    if (!points.length) {
      g.fillStyle = colors.muted;
      g.font = "11px system-ui, sans-serif";
      g.textBaseline = "middle";
      g.fillText(`${this.label(nodeId, target)} — cliquer pour poser un point`, 10, top + rowH / 2);
      return;
    }
    const color = node.color || colors.accent;
    g.save();
    g.beginPath();
    g.rect(0, top, W, rowH - 1);
    g.clip();
    g.strokeStyle = color;
    g.globalAlpha = mode === "off" ? 0.35 : 1;
    g.lineWidth = 2;
    g.setLineDash(mode === "off" ? [5, 4] : []);
    g.beginPath();
    const first = this.#xy(points[0], range, top);
    g.moveTo(Math.min(0, first.x), first.y);
    g.lineTo(first.x, first.y);
    let prev = first;
    for (let i = 1; i < points.length; i++) {
      const q = this.#xy(points[i], range, top);
      if (points[i - 1].curve === "step") g.lineTo(q.x, prev.y);
      g.lineTo(q.x, q.y);
      prev = q;
    }
    g.lineTo(Math.max(W, prev.x), prev.y);
    g.stroke();
    g.setLineDash([]);
    points.forEach((p, i) => {
      const q = this.#xy(p, range, top);
      const sel = this.selected?.node === nodeId && this.selected.index === i;
      g.fillStyle = sel ? "#ffd166" : color;
      g.strokeStyle = "rgba(0,0,0,.6)";
      g.lineWidth = 1;
      g.beginPath();
      if (p.curve === "step") g.rect(q.x - 4, q.y - 4, 8, 8);
      else g.arc(q.x, q.y, 4.5, 0, Math.PI * 2);
      g.fill();
      g.stroke();
      if (sel || (this.drag?.node === nodeId && this.drag.index === i)) {
        g.fillStyle = colors.text;
        g.font = "11px system-ui, sans-serif";
        g.textBaseline = "bottom";
        g.fillText(valueText(range, p.value, null), q.x + 8, Math.max(top + 14, q.y - 6));
      }
    });
    g.restore();
    if (live?.length) this.#drawLive(g, live, range, top, W, rowH);
  }

  /** Ce qui s'écrit en ce moment (geste de l'utilisateur) : trait rouge épais par-dessus la courbe, avec un repère à la dernière valeur. */
  #drawLive(g, live, range, top, W, rowH) {
    g.save();
    g.beginPath();
    g.rect(0, top, W, rowH - 1);
    g.clip();
    const xy = ([sample, value]) => ({ x: sampleToX(this.state.view, sample), y: top + PAD + (1 - valueToFrac(range, value)) * (rowH - 2 * PAD) });
    g.strokeStyle = getComputedStyle(document.documentElement).getPropertyValue("--rec").trim() || "#ff4d5e";
    g.lineWidth = 3;
    g.beginPath();
    live.forEach((p, i) => {
      const q = xy(p);
      if (i === 0) g.moveTo(q.x, q.y);
      else g.lineTo(q.x, q.y);
    });
    g.stroke();
    const end = xy(live[live.length - 1]);
    g.fillStyle = g.strokeStyle;
    g.beginPath();
    g.arc(end.x, end.y, 5, 0, Math.PI * 2);
    g.fill();
    g.restore();
  }
}

export { norm };
