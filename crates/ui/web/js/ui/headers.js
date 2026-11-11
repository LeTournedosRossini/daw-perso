// En-têtes de pistes : nom, muet / solo / armement, fader, panoramique et vumètre. Chaque geste devient une commande
// `set_node` ; l'affichage se recale sur ce que le moteur confirme (événement `project`).

import { dbToGain, gainToDb } from "../protocol.js";
import { formatDb } from "../time.js";
import { orderedRows } from "./order.js";
import { shownValues } from "../automation-model.js";
import { LANE_MODES, trackGesture } from "./gesture.js";
import { getScale, pixelRatio } from "../ui-scale.js";
import { openTrackMenu, openAddMenu, openInputMenu, openOutputMenu, bindInputMenu } from "./track-menu.js";
import { openReturnDialog } from "./return-dialog.js";
import { isListenable, setupListenButton, toggleListen } from "./listen.js";
import { parseReturnChoice, returnChoices } from "./mixer.js";
import { createWitness, paintWitness } from "./midi-witness.js";
import { knobControl } from "./knob.js";

const KIND_LABEL = { stereo: "stéréo", mono: "mono", bus: "bus", master: "sortie" };
const DB_MIN = -60;
const DB_MAX = 6;

export class Headers {
  constructor(listEl, state, client) {
    this.el = listEl;
    this.state = state;
    this.client = client;
    this.rows = new Map(); // id → { root, refs }
    this.laneRows = new Map(); // id du nœud → en-tête de sa voie d'automation
    this.automation = null; // éditeur d'automation (donné par main.js)
    this.chordLane = null; // voie d'accords (donnée par main.js)
    this.pianoRoll = null; // piano roll (donné par main.js) : son en-tête ancré s'insère dans la liste
    this.peakHold = new Map(); // id → [l, r] pour la retenue de crête
    this.drag = null; // déplacement d'une piste en cours : { id, ... }
    document.getElementById("btn-add-track")?.addEventListener("click", (e) => openAddMenu(e.currentTarget, client, () => {
      const project = this.state.project;
      openReturnDialog({ client, project, choices: returnChoices(project, { id: -1, is_return: true }), parse: parseReturnChoice });
    }));
  }

  /** En-têtes des voies d'accords : épinglés sous le coin de la règle (ils ne défilent pas avec les pistes). */
  #renderChordHeads() {
    const host = document.getElementById("chord-heads");
    if (!host) return;
    const key = this.state.chordsOpen ? "open" : "closed";
    if (key === this.chordKey) return;
    this.chordKey = key;
    host.replaceChildren();
    if (!this.state.chordsOpen) return;
    for (const kind of ["chords", "keys", "scales", "structure", "lyrics"]) host.append(this.#buildChordHead({ kind, color: "#a78bfa" }));
  }

  render() {
    this.#renderChordHeads();
    const rows = orderedRows(this.state.project, this.state.autoOpen, this.state.rollDockNode ?? null, this.state.dockRows ?? this.state.rollRows ?? 4, this.state.takesOpen);
    const keys = rows.map((r) => r.rowKey ?? String(r.id));
    // Reconstruction seulement si la liste de lignes a changé ; sinon mise à jour sur place (pour ne pas casser un glissement).
    const dockEl = this.state.dockHeadEl ?? this.pianoRoll?.dockHead ?? null;
    const same = dockEl === this.dockEl && keys.length === this.keys?.length && keys.every((k, i) => k === this.keys[i]);
    if (!same) {
      this.el.replaceChildren();
      this.rows.clear();
      this.laneRows.clear();
      for (const r of rows) {
        if (r.kind === "lane") this.#buildLane(r.node);
        else if (r.kind === "roll") this.el.append(dockEl); // l'en-tête de la vue ancrée (piano roll ou partition)
        else if (r.kind === "take") this.el.append(this.#buildTake(r));
        else if (r.kind !== "roll-cont") this.#build(r);
      }
      this.keys = keys;
      this.dockEl = dockEl;
    }
    for (const r of rows) {
      if (r.kind === "lane") this.#syncLane(r.node);
      else if (r.kind !== "roll" && r.kind !== "roll-cont" && r.kind !== "take") this.#sync(r);
    }
  }

  #relayout() {
    this.automation.timeline.resize();
    this.automation.timeline.settle?.();
    this.render();
  }

  /** En-tête d'une voie d'accords (un quart de hauteur) : son nom et un bouton ⋯ qui ouvre ses réglages dans une fenêtre. */
  #buildChordHead(r) {
    const lane = this.chordLane;
    const root = document.createElement("div");
    root.className = "track-head lane-head chord-head";
    root.style.setProperty("--c", r.color);
    const names = { keys: ["Tonalités", "Double-clic : poser une tonalité (elle vaut jusqu'à la suivante). Glisser : déplacer d'une mesure à l'autre ; Suppr : supprimer. Elle donne l'armure de la partition."], chords: ["Accords", "Double-clic dans la voie : saisir un accord. Glisser : déplacer ; bords : changer la durée ; Suppr : supprimer."], scales: ["Gammes", "Double-clic : poser ou changer une gamme (les gammes qui se chevauchent sont empilées). Le trait du bas montre les appels de motifs ; les tonalités y sont aussi."], structure: ["Structure", "Double-clic : poser une section (intro, couplet, refrain…). Indépendante des accords."], lyrics: ["Paroles", "Double-clic : saisir un texte (ma-man est la plus bel-le), une syllabe par pas de la grille. Glisser : déplacer une syllabe ; Suppr : supprimer. Elles s'écrivent sous les notes de la partition."] };
    root.innerHTML = `<div class="stripe"></div><div class="body"><span class="name"></span><button class="more" title="Réglages de la voie" aria-label="Réglages de la voie" aria-haspopup="dialog">⋯</button></div>`;
    const [label, tip] = names[r.kind];
    root.querySelector(".name").textContent = label;
    root.title = tip;
    root.querySelector(".more").addEventListener("click", (e) => lane.openSettings(e.currentTarget, r.kind));
    if (r.kind === "lyrics") {
      const grip = document.createElement("div");
      grip.className = "chord-grip";
      grip.title = "Glisser pour régler la hauteur des voies";
      grip.setAttribute("role", "separator");
      grip.addEventListener("pointerdown", (e) => lane.startResize(e));
      root.append(grip);
    }
    return root;
  }

  /** En-tête d'une voie de prises (prises inactives de la piste ; un clic sur une prise dans la timeline la rend active). */
  #buildTake(r) {
    const root = document.createElement("div");
    root.className = "track-head lane-head";
    root.style.setProperty("--c", r.color);
    root.innerHTML = `<div class="stripe"></div><div class="body"><div class="title"><span class="name"></span><button class="x" title="Replier les prises" aria-label="Replier les prises">✕</button></div></div>`;
    root.querySelector(".name").textContent = `↳ Prises ${r.layer + 1}`;
    root.title = "Prises inactives de la piste : clic sur une prise (dans la timeline) pour l'activer";
    root.querySelector(".x").addEventListener("click", () => {
      this.state.takesOpen.delete(r.id);
      this.#relayout();
    });
    return root;
  }

  /** En-tête d'une voie d'automation : paramètre automatisé, mode (lecture / trim / désactivée), effacement de la courbe. */
  #buildLane(node) {
    const root = document.createElement("div");
    root.className = "track-head lane-head";
    root.style.setProperty("--c", node.color);
    root.innerHTML = `
      <div class="stripe"></div>
      <div class="body">
        <div class="title"><span class="name"></span><button class="x" title="Replier la voie d'automation" aria-label="Replier la voie d'automation">✕</button></div>
        <select class="target" aria-label="Paramètre automatisé"></select>
        <div class="ctl"><select class="mode" aria-label="Mode de la courbe">${LANE_MODES.map(([v, t]) => `<option value="${v}">${t}</option>`).join("")}</select><button class="clear" title="Supprimer la courbe de ce paramètre">Effacer</button></div>
      </div>`;
    const q = (s) => root.querySelector(s);
    const refs = { name: q(".name"), target: q(".target"), mode: q(".mode"), clear: q(".clear"), sig: "" };
    refs.name.textContent = `Automation — ${node.name}`;
    q(".x").addEventListener("click", () => this.automation.toggle(node.id));
    refs.target.addEventListener("change", () => {
      const v = refs.target.value;
      const [kind, a, b] = v.split(":");
      if (kind === "insert" && b === undefined) {
        const cur = this.automation.target(node.id);
        refs.target.value = refs.cur ?? "gain"; // la liste ne change qu'une fois le paramètre choisi
        void cur;
        this.automation.chooseParam(refs.target, node.id, Number(a));
      } else this.automation.setTarget(node.id, kind === "send" ? { kind: "send", to: Number(a) } : kind === "insert" ? { kind: "insert", insert: Number(a), param: Number(b) } : { kind });
    });
    refs.mode.addEventListener("change", () => this.automation.setMode(node.id, refs.mode.value));
    refs.clear.addEventListener("click", () => {
      const t = this.automation.target(node.id);
      if (this.automation.lane(node.id, t)) this.client.send({ type: "set_lane", node: node.id, target: t, points: [], label: "Supprimer l'automation" });
    });
    this.el.append(root);
    this.laneRows.set(node.id, { root, refs });
  }

  #syncLane(node) {
    const row = this.laneRows.get(node.id);
    if (!row) return;
    const { refs } = row;
    const ed = this.automation;
    const target = ed.target(node.id);
    const key = (t) => (t.kind === "send" ? `send:${t.to}` : t.kind === "insert" ? `insert:${t.insert}:${t.param}` : t.kind);
    const has = (t) => !!ed.lane(node.id, t);
    const opts = [{ v: "gain", t: { kind: "gain" } }, { v: "pan", t: { kind: "pan" } }];
    for (const s of node.sends) opts.push({ v: `send:${s.to}`, t: { kind: "send", to: s.to } });
    for (const i of node.inserts) {
      opts.push({ v: `insert:${i.id}`, t: null, label: `${i.name} — choisir un paramètre…` });
    }
    if (target.kind === "insert") opts.push({ v: key(target), t: target });
    const labelOf = (o) => (o.label ?? `${has(o.t) ? "● " : ""}${ed.label(node.id, o.t)}`);
    const sig = JSON.stringify(opts.map((o) => [o.v, labelOf(o)]));
    if (sig !== refs.sig) {
      refs.sig = sig;
      refs.target.replaceChildren(...opts.map((o) => new Option(labelOf(o), o.v)));
    }
    refs.cur = key(target);
    if (document.activeElement !== refs.target) refs.target.value = refs.cur;
    const mode = ed.mode(node.id, target);
    if (document.activeElement !== refs.mode) refs.mode.value = mode;
    refs.clear.disabled = !has(target);
    row.root.classList.toggle("armed", !!ed.writeMode(node.id, target));
  }

  #build(n) {
    const root = document.createElement("div");
    root.className = "track-head";
    root.style.setProperty("--c", n.color);
    root.innerHTML = `
      <div class="stripe"></div>
      <div class="body">
        <div class="title"><span class="grip" title="Glisser pour déplacer la piste" aria-hidden="true">⠿</span><span class="name"></span><span class="kind"></span><span class="midiw-slot"></span><button class="fx" title="Plugins de la piste" aria-label="Plugins de la piste">fx</button><button class="phase" title="Phase inversée (cliquer pour rétablir)" aria-label="Phase inversée" hidden>Ø</button>
          <span class="btns">
            <button class="m" title="Muet" aria-label="Muet">M</button>
            <button class="s" title="Solo" aria-label="Solo">S</button>
            <button class="r" title="Armer pour l'enregistrement" aria-label="Armer">●</button><button class="rin" title="Entrée d'enregistrement : canal, MIDI, dynamique…" aria-label="Entrée d'enregistrement" aria-haspopup="menu">▾</button>
            <button class="t" title="Prises : afficher ou replier les prises inactives de la piste" aria-label="Prises" hidden>▤</button>
            <button class="a" title="Voie d'automation (volume, panoramique, envois, plugins)" aria-label="Voie d'automation">A</button>
            <button class="more" title="Menu de la piste" aria-label="Menu de la piste" aria-haspopup="menu">⋮</button>
          </span></div>
        <div class="knobs"></div>
      </div>`;
    const q = (s) => root.querySelector(s);
    const refs = { witness: createWitness(), t: q(".t"), a: q(".a"), grip: q(".grip"), fx: q(".fx"), rin: q(".rin"), phase: q(".phase"), more: q(".more"), name: q(".name"), kind: q(".kind"), m: q(".m"), s: q(".s"), r: q(".r"), };
    // Volume et panoramique : deux potentiomètres (plus compacts que des curseurs, utilisables au toucher) ; les faders linéaires sont dans le mixeur.
    const send = (patch) => this.client.send({ type: "set_node", node: n.id, ...patch });
    const vol = knobControl("Vol", {
      min: DB_MIN, max: DB_MAX, step: 0.1, value: 0, rest: 0, travel: 180,
      text: (db) => formatDb(db <= DB_MIN ? -Infinity : db),
      onInput: (db) => {
        refs.gainOut.textContent = formatDb(db <= DB_MIN ? -Infinity : db);
        send({ gain: dbToGain(db <= DB_MIN ? -Infinity : db) });
      },
    });
    const pan = knobControl("Pan", {
      min: -1, max: 1, step: 0.01, value: 0, rest: 0, origin: 0, travel: 140,
      text: (p) => this.#panText(p),
      onInput: (p) => {
        refs.panOut.textContent = this.#panText(p);
        send({ pan: p });
      },
    });
    q(".knobs").append(vol.root, pan.root);
    Object.assign(refs, { gain: vol.knob.el, gainKnob: vol.knob, gainOut: vol.out, pan: pan.knob.el, panKnob: pan.knob, panOut: pan.out });
    // Le vumètre est sous le titre : on l'ajoute à la fin du corps pour garder la grille à trois lignes lisible.
    q(".midiw-slot").replaceWith(refs.witness);
    refs.meter = document.createElement("canvas");
    refs.meter.className = "meter";
    refs.meter.setAttribute("aria-hidden", "true");
    q(".body").append(refs.meter);
    // Poignée du bas : tirer règle la hauteur de toutes les pistes ; double-clic : hauteur par défaut.
    const grip = document.createElement("div");
    grip.className = "row-grip";
    grip.title = "Tirer pour changer la hauteur des pistes (double-clic : hauteur par défaut)";
    let resize = null;
    grip.addEventListener("pointerdown", (e) => {
      if (!this.rowControl) return;
      resize = { y: e.clientY, h: this.rowControl.get(), scale: getScale() / 100 };
      grip.setPointerCapture(e.pointerId);
      grip.classList.add("active");
      e.preventDefault();
    });
    grip.addEventListener("pointermove", (e) => {
      if (resize) this.rowControl.set(resize.h + (e.clientY - resize.y) / resize.scale, true);
    });
    const endResize = () => {
      resize = null;
      grip.classList.remove("active");
    };
    grip.addEventListener("pointerup", endResize);
    grip.addEventListener("pointercancel", endResize);
    grip.addEventListener("dblclick", () => this.rowControl?.reset());
    root.append(grip);
    root.style.gridTemplateRows = "";
    refs.m.addEventListener("click", () => send({ mute: !this.#node(n.id).mute }));
    if (isListenable(n)) {
      setupListenButton(refs.s);
      refs.s.addEventListener("click", () => toggleListen(this.client, this.state.project, n.id));
    } else {
      refs.s.addEventListener("click", () => send({ solo: !this.#node(n.id).solo }));
    }
    bindInputMenu(refs.r, () => ({ node: this.#node(n.id), project: this.state.project, client: this.client }));
    refs.rin.addEventListener("click", () => openInputMenu(refs.rin, { node: this.#node(n.id), project: this.state.project, client: this.client }));
    refs.r.addEventListener("click", () => send({ arm: !this.#node(n.id).arm }));
    refs.a.addEventListener("click", () => this.automation.toggle(n.id));
    refs.t.addEventListener("click", () => {
      const open = this.state.takesOpen;
      if (open.has(n.id)) open.delete(n.id);
      else open.add(n.id);
      this.#relayout();
    });
    trackGesture(refs.gain, this.client, () => ({ node: n.id, target: { kind: "gain" } }));
    trackGesture(refs.pan, this.client, () => ({ node: n.id, target: { kind: "pan" } }));
    if (n.kind === "master") {
      refs.r.hidden = true;
      refs.rin.hidden = true;
      refs.fx.hidden = true;
      refs.s.hidden = true;
      refs.more.hidden = true;
      refs.grip.hidden = true;
    } else {
      refs.more.addEventListener("click", () => openTrackMenu(refs.more, { node: this.#node(n.id), project: this.state.project, client: this.client }));
      // Le routage de l'en-tête (« → Bus 1 ») ouvre directement le choix de la sortie.
      refs.kind.addEventListener("click", () => openOutputMenu(refs.kind, { node: this.#node(n.id), project: this.state.project, client: this.client }));
      refs.fx.addEventListener("click", () => this.onPlugins?.(n.id));
      refs.phase.addEventListener("click", () => send({ invert: false }));
      refs.name.addEventListener("dblclick", () => this.#rename(n.id));
      refs.grip.addEventListener("pointerdown", (e) => this.#startDrag(e, n.id));
    }
    if (n.kind === "bus") {
      refs.r.hidden = true;
      refs.rin.hidden = true;
    }
    this.el.append(root);
    this.rows.set(n.id, { root, refs });
  }

  #node(id) {
    return this.state.project.nodes.find((x) => x.id === id);
  }

  /** Renomme sur place : le nom devient un champ ; Entrée ou perte du focus valide, Échap abandonne. */
  #rename(id) {
    const row = this.rows.get(id);
    const node = this.#node(id);
    if (!row || !node || row.renaming) return;
    row.renaming = true;
    const input = document.createElement("input");
    input.className = "rename";
    input.value = node.name;
    input.maxLength = 60;
    input.setAttribute("aria-label", "Nom de la piste");
    row.refs.name.replaceWith(input);
    input.focus();
    input.select();
    let done = false;
    const finish = (commit) => {
      if (done) return;
      done = true;
      row.renaming = false;
      const name = input.value.trim();
      if (input.isConnected) input.replaceWith(row.refs.name);
      if (commit && name && name !== node.name) this.client.send({ type: "rename_track", node: id, name });
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") finish(true);
      else if (e.key === "Escape") finish(false);
      e.stopPropagation();
    });
    input.addEventListener("blur", () => finish(true));
  }

  /** Déplacement d'une piste par sa poignée : un trait montre où elle ira ; une seule commande au relâchement. */
  #startDrag(e, id) {
    e.preventDefault();
    const movable = [...this.rows.keys()].filter((k) => this.#node(k)?.kind !== "master");
    const rects = new Map(movable.map((k) => [k, this.rows.get(k).root.getBoundingClientRect()]));
    const grip = e.currentTarget;
    grip.setPointerCapture(e.pointerId);
    const row = this.rows.get(id).root;
    row.classList.add("dragging");
    const others = movable.filter((k) => k !== id);
    const targetAt = (y) => {
      // Index d'insertion parmi les autres pistes : devant la première dont le milieu est sous le pointeur.
      const i = others.findIndex((k) => {
        const r = rects.get(k);
        return y < r.top + r.height / 2;
      });
      return i < 0 ? others.length : i;
    };
    const mark = (index) => {
      for (const { root } of this.rows.values()) root.classList.remove("drop-before", "drop-end");
      if (index < others.length) this.rows.get(others[index]).root.classList.add("drop-before");
      else if (others.length) this.rows.get(others.at(-1)).root.classList.add("drop-end");
    };
    this.drag = { id, index: targetAt(e.clientY) };
    mark(this.drag.index);
    const move = (ev) => {
      this.drag.index = targetAt(ev.clientY);
      mark(this.drag.index);
    };
    const end = (ev, commit) => {
      grip.removeEventListener("pointermove", move);
      grip.removeEventListener("pointerup", up);
      grip.removeEventListener("pointercancel", cancel);
      row.classList.remove("dragging");
      for (const { root } of this.rows.values()) root.classList.remove("drop-before", "drop-end");
      const { index } = this.drag;
      this.drag = null;
      if (!commit) return;
      // Pas de commande si la piste reste à sa place.
      const current = movable.indexOf(id);
      if (index === current) return;
      const before = others[index];
      this.client.send(before === undefined ? { type: "move_track", node: id } : { type: "move_track", node: id, before });
      void ev;
    };
    const up = (ev) => end(ev, true);
    const cancel = (ev) => end(ev, false);
    grip.addEventListener("pointermove", move);
    grip.addEventListener("pointerup", up);
    grip.addEventListener("pointercancel", cancel);
  }

  #panText(p) {
    if (Math.abs(p) < 0.005) return "C";
    return `${Math.abs(Math.round(p * 100))} ${p < 0 ? "G" : "D"}`;
  }

  #sync(n) {
    const { refs } = this.rows.get(n.id);
    refs.name.textContent = n.name;
    const bus = n.output ? this.#node(n.output) : null;
    // Sur la ligne du titre : seulement ce qui change quelque chose (bus, sortie, routage) ; le format est dans l'info-bulle.
    const shown = n.is_return ? "retour" : n.kind === "bus" || n.kind === "master" ? KIND_LABEL[n.kind] : "";
    // Une piste routée sur la sortie principale montre une simple flèche « → » (cliquable : choix de la sortie).
    const plain = !bus && !n.is_return && n.kind !== "master" && n.kind !== "bus";
    refs.kind.textContent = [shown, bus ? `→ ${bus.name}` : plain ? "→" : ""].filter(Boolean).join(" ");
    refs.name.title = `${n.name} — ${n.is_return ? "retour de monitoring" : KIND_LABEL[n.kind] ?? n.kind}${bus ? ` → ${bus.name}` : ""}`;
    this.rows.get(n.id).root.style.setProperty("--c", n.color);
    // Pastille « fx N » quand la piste a des plugins ; grisée si tous sont contournés.
    const count = n.inserts?.length ?? 0;
    // Toujours visible (sauf la sortie principale) : un clic ouvre les plugins de la piste, ou le sélecteur si elle n'en a pas encore.
    refs.fx.textContent = count ? `fx ${count}` : "fx";
    refs.fx.classList.toggle("empty", count === 0);
    refs.fx.classList.toggle("off", count > 0 && n.inserts.every((i) => i.bypass));
    refs.fx.title = count ? `Plugins de la piste (${count}) : ouvrir leur fenêtre` : "Aucun plugin : choisir un plugin";
    refs.kind.classList.toggle("clickable", n.kind !== "master" && !n.is_return);
    refs.kind.title = refs.kind.classList.contains("clickable") ? `Sortie : ${bus ? bus.name : "sortie principale"} — cliquer pour la changer` : "";
    refs.phase.hidden = !n.invert;
    paintWitness(refs.witness, n, this.state.midiAct?.[n.id] ?? 0);
    refs.a.setAttribute("aria-pressed", String(this.state.autoOpen.has(n.id)));
    const takes = n.mutedClips?.length ?? 0;
    refs.t.hidden = takes === 0;
    refs.t.textContent = `▤ ${takes}`;
    refs.t.setAttribute("aria-pressed", String(this.state.takesOpen.has(n.id)));
    refs.a.classList.toggle("has", (n.automation?.length ?? 0) > 0);
    for (const [btn, on] of [[refs.m, n.mute], [refs.s, isListenable(n) ? this.state.project.listen === n.id : n.solo], [refs.r, n.arm]]) btn.setAttribute("aria-pressed", String(on));
    // Armement : l'entrée de la piste dans l'info-bulle ; figé pendant une prise (le moteur refuserait).
    const inp = n.input ?? { kind: "audio", channels: [1] };
    const where = inp.kind === "midi" ? "entrée MIDI" : inp.channels.length === 2 ? `entrées ${inp.channels.join("-")}` : `entrée ${inp.channels[0]}`;
    refs.r.title = `Armer pour l'enregistrement — ${where}`;
    refs.r.disabled = !!this.state.recording?.active;
    this.#showFaders(n, refs);
  }

  /** Fader et panoramique affichés : ceux d'une courbe en mode lecture à la position de lecture, sinon le réglage manuel. */
  #showFaders(n, refs) {
    const { tempo, pos } = this.state;
    const s = shownValues(n, tempo ? tempo.wholeAt(Math.max(0, pos)) : 0);
    const db = s.gain <= 0 ? DB_MIN : Math.max(DB_MIN, Math.min(DB_MAX, gainToDb(s.gain)));
    refs.gainKnob.set(db);
    refs.gainOut.textContent = formatDb(gainToDb(s.gain));
    refs.panKnob.set(s.pan);
    refs.panOut.textContent = this.#panText(s.pan);
    refs.gain.classList.toggle("automated", s.gainAuto);
    refs.pan.classList.toggle("automated", s.panAuto);
    refs.gain.title = s.gainAuto ? "Volume commandé par la courbe d'automation (mode lecture)" : "";
  }

  /** Ouvre les voies de prises de ces pistes (après un calage : l'original apparaît sous la prise calée, un clic le réactive). */
  openTakes(nodeIds) {
    for (const id of nodeIds) this.state.takesOpen.add(id);
    this.#relayout();
  }

  /** Témoin d'entrée MIDI : appelé quand l'ensemble des pistes qui reçoivent du MIDI change. */
  setMidiActivity() {
    for (const [id, { refs }] of this.rows) paintWitness(refs.witness, this.#node(id), this.state.midiAct?.[id] ?? 0);
  }

  /** Appelé quand la position de lecture change : les faders des courbes en lecture suivent. */
  follow() {
    for (const [id, { refs }] of this.rows) {
      const n = this.#node(id);
      if (n && (n.automation?.length ?? 0) > 0) this.#showFaders(n, refs);
    }
  }

  /** Dessine les vumètres (appelé à chaque image). Échelle en dB, retenue de crête qui retombe lentement. */
  drawMeters(dt) {
    const dpr = pixelRatio();
    if (!this.holdColor || this.colorAge++ > 120) {
      this.holdColor = getComputedStyle(document.documentElement).getPropertyValue("--text") || "#fff";
      this.colorAge = 0;
    }
    for (const [id, { refs }] of this.rows) {
      const levels0 = this.state.meters[id] ?? [0, 0];
      const hold0 = this.peakHold.get(id) ?? [0, 0];
      // Rien à dessiner : vumètre déjà vide et sans retenue de crête — on ne touche pas au canevas.
      if (levels0[0] === 0 && levels0[1] === 0 && hold0[0] < 1e-4 && hold0[1] < 1e-4 && this.cleared?.has(id)) continue;
      const c = refs.meter;
      const w = Math.max(1, Math.round(c.clientWidth * dpr));
      const h = Math.max(1, Math.round(c.clientHeight * dpr));
      if (c.width !== w || c.height !== h) {
        c.width = w;
        c.height = h;
      }
      const g = c.getContext("2d");
      g.clearRect(0, 0, w, h);
      const levels = this.state.meters[id] ?? [0, 0];
      const hold = this.peakHold.get(id) ?? [0, 0];
      for (let ch = 0; ch < 2; ch++) {
        hold[ch] = Math.max(levels[ch], hold[ch] - dt * 0.0006);
        const y = Math.round((ch * h) / 2) + (ch ? 1 : 0);
        const bh = Math.round(h / 2) - 1;
        const pos = (v) => (v <= 0 ? 0 : Math.max(0, Math.min(1, (gainToDb(v) - DB_MIN) / (0 - DB_MIN + 6))));
        const full = Math.round(w * pos(levels[ch]));
        const grad = g.createLinearGradient(0, 0, w, 0);
        grad.addColorStop(0, "#3ecf8e");
        grad.addColorStop(0.72, "#e9d34a");
        grad.addColorStop(0.9, "#ff4d5e");
        g.fillStyle = grad;
        g.fillRect(0, y, full, bh);
        const hx = Math.round(w * pos(hold[ch]));
        if (hx > 1) {
          g.fillStyle = this.holdColor;
          g.fillRect(Math.max(0, hx - 2), y, 2, bh);
        }
      }
      this.peakHold.set(id, hold);
      (this.cleared ??= new Set())[levels[0] === 0 && levels[1] === 0 && hold[0] < 1e-4 && hold[1] < 1e-4 ? "add" : "delete"](id);
    }
  }
}
