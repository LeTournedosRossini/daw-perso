// Mixeur : une tranche par piste et par bus (nom, sortie, plugins, envois, panoramique, fader, vumètre, muet / solo / armement)
// et la sortie principale à droite. Le même élément a trois présentations, au choix : panneau fixe sous la timeline, plein écran
// (remplace la timeline) ou fenêtre flottante. Chaque geste est une commande du protocole ; l'affichage suit l'événement `project`.

import { createWitness, paintWitness } from "./midi-witness.js";
import { openMonitorPopup } from "./monitor-popup.js";
import { dbToGain, gainToDb } from "../protocol.js";
import { formatDb } from "../time.js";
import { orderedNodes } from "./order.js";
import { shownValues } from "../automation-model.js";
import { trackGesture } from "./gesture.js";
import { getScale, pixelRatio } from "../ui-scale.js";
import { LevelSheet, linearToDb } from "./level-sheet.js";
import { menuEl, showMenu } from "./track-menu.js";
import { openReturnDialog } from "./return-dialog.js";
import { isListenable, setupListenButton, toggleListen } from "./listen.js";
import { openGroupDialog } from "./group-dialog.js";
import { OFFSET_RANGE, effectiveGroups, anySendSolo, formatOffset, groupColor, groupFlags, groupMembers, groupOffset } from "../groups-model.js";

const DB_MIN = -60;
const DB_MAX = 6;
const MODES = ["dock", "full", "float"];
const KEY_MODE = "daw-mixer-mode";
const KEY_OPEN = "daw-mixer-open";
const KEY_H = "daw-mixer-h";
const KEY_WIDE = "daw-mixer-wide";
const KEY_ROWS = "daw-mixer-rows";
const KEY_VIEW = "daw-mixer-view";
const KEY_GROUPS_OPEN = "daw-mixer-groups-open"; // groupes dépliés dans la vue « Groupes » (liste d'identifiants)

const get = (k) => {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
};
const put = (k, v) => {
  try {
    localStorage.setItem(k, v);
  } catch {
    // Pas de mémorisation possible : le réglage vaut pour cette session.
  }
};

const parseIds = (text) => {
  try {
    const v = JSON.parse(text ?? "[]");
    return Array.isArray(v) ? v.filter(Number.isInteger) : [];
  } catch {
    return [];
  }
};

const dbOf = (gain) => (gain <= 0 ? DB_MIN : Math.max(DB_MIN, Math.min(DB_MAX, gainToDb(gain))));
const gainOfDb = (db) => (db <= DB_MIN ? 0 : dbToGain(Math.min(DB_MAX, db)));

/** Mode par défaut : panneau si l'écran est grand, plein écran sinon. */
export function defaultMode(width, height) {
  return width >= 1000 && height >= 720 ? "dock" : "full";
}

/** Destinations proposées à un nœud pour un envoi : la sortie principale et les bus, sauf lui-même et celles déjà utilisées. */
export function sendTargets(project, node) {
  const used = new Set(node.sends.map((s) => s.to));
  return project.nodes.filter((n) => (n.kind === "master" || n.kind === "bus") && n.id !== node.id && !used.has(n.id));
}

/** Destinations possibles pour la sortie d'un nœud : la sortie principale et les bus, sauf lui-même et les retours (ils ne reçoivent que des envois). */
export function outputTargets(project, node) {
  return project.nodes.filter((n) => (n.kind === "master" || n.kind === "bus") && n.id !== node.id && !n.is_return);
}

/** Un retour de monitoring : un bus d'un type à part (QR, onglet de mix, groupes propres). Un bus ordinaire peut sortir sur la carte son sans en être un. */
export const isReturn = (n) => n.kind === "bus" && !!n.is_return;

/**
 * Sorties que la carte son offre à un bus ou à un retour : `{ value, label, disabled }` ; la valeur est `"première:stéréo:mono"` (`""` :
 * pour un retour « aucune » — il est muet —, pour un bus la sortie principale). Les sorties 1 et 2 sont la sortie principale ; celles que
 * prend déjà un autre bus ou retour sont grisées, avec son nom.
 */
export function returnChoices(project, node) {
  const outputs = project.audio_outputs || 0;
  const taken = (ch) => project.nodes.find((o) => o.id !== node.id && o.phys && ch >= o.phys.first && ch < o.phys.first + (o.phys.stereo ? 2 : 1));
  const out = [{ value: "", label: node.is_return ? "Aucune (muet)" : "Sortie principale", disabled: false }];
  const push = (first, stereo, mono, label) => {
    const chans = stereo ? [first, first + 1] : [first];
    const owner = chans.map(taken).find(Boolean);
    out.push({ value: `${first}:${stereo ? 1 : 0}:${mono ? 1 : 0}`, label: owner ? `${label} (pris : ${owner.name})` : label, disabled: !!owner });
  };
  for (let a = 3; a + 1 <= outputs; a += 2) {
    push(a, true, false, `Sorties ${a}-${a + 1} stéréo`);
    push(a, true, true, `Sorties ${a}-${a + 1} somme mono`);
  }
  for (let a = 3; a <= outputs; a++) push(a, false, true, `Sortie ${a} mono`);
  return out;
}

/** Décode la valeur d'un choix de `returnChoices` pour la commande `set_return_output` (`null` : retour à la sortie principale). */
export function parseReturnChoice(value) {
  if (!value) return null;
  const [first, stereo, mono] = value.split(":").map(Number);
  return { first, stereo: stereo === 1, mono: mono === 1 };
}

/** Les nœuds que montre chaque vue du mixeur (`all`, `tracks`, `buses`, `returns`) ; la sortie principale reste toujours. */
export function viewNodes(project, view) {
  const all = orderedNodes(project);
  const keep = {
    all: () => true,
    tracks: (n) => n.kind === "mono" || n.kind === "stereo",
    buses: (n) => n.kind === "bus" && !n.is_return,
    returns: (n) => isReturn(n),
  }[view];
  return keep ? all.filter((n) => n.kind === "master" || keep(n)) : all;
}

/**
 * La vue « Groupes » : une tranche par groupe **du projet** (repliée : seulement elle), suivie de ses pistes membres quand le groupe est
 * déplié (`open` : ensemble d'identifiants de groupes) ; puis la sortie principale. Les groupes propres à un retour restent dans « Mix · X ».
 * Rend `[{ group }, { node }, …]` dans l'ordre d'affichage.
 */
export function groupsLayout(project, open) {
  const nodes = orderedNodes(project);
  const out = [];
  for (const g of project.groups ?? []) {
    out.push({ group: g });
    if (open.has(g.id)) for (const n of nodes) if (n.kind !== "master" && !isReturn(n) && (n.group ?? 0) === g.id) out.push({ node: n, groupId: g.id });
  }
  for (const n of nodes) if (n.kind === "master") out.push({ node: n });
  return out;
}

/** Les envois d'un retour « Mix de X » : pour chaque piste et bus qui n'est pas X ni un retour, le niveau de son envoi vers X (silence s'il n'y en a pas). */
export function mixSources(project, retour) {
  return orderedNodes(project)
    .filter((n) => n.kind !== "master" && n.id !== retour.id && !isReturn(n))
    .map((n) => ({ node: n, send: n.sends.find((s) => s.to === retour.id) ?? null }));
}

/** Signature de ce qui change la structure des tranches (pas les valeurs : celles-ci se mettent à jour sans tout reconstruire). */
export function structureKey(project) {
  return JSON.stringify(project.nodes.map((n) => [n.id, n.name, n.kind, n.color, n.output, n.group ?? 0, n.return_groups ? JSON.stringify([n.return_groups.groups.map((g) => [g.id, g.name, g.color]), n.return_groups.members]) : null, n.sends.map((s) => s.to), n.inserts.map((i) => i.name), !!n.is_return, n.phys ? [n.phys.first, n.phys.stereo, n.phys.mono] : null]).concat([project.audio_outputs, (project.groups ?? []).map((g) => [g.id, g.name, g.color])]));
}

function closeMenuSafe() {
  document.querySelector(".track-menu")?.remove();
}

export class Mixer {
  constructor(state, client) {
    this.state = state;
    this.client = client;
    this.root = document.getElementById("mixer");
    this.app = document.getElementById("app");
    this.strips = this.root.querySelector(".mx-strips");
    this.refs = new Map(); // id → éléments de la tranche
    this.hold = new Map(); // id → retenue de crête
    this.key = "";
    this.dragging = null;
    this.onPlugins = null;
    this.sheet = new LevelSheet();
    this.sheetFor = null; // ce que règle la feuille de réglage fin : { node, to }
    this.view = get(KEY_VIEW) || "all"; // vue : all | tracks | buses | returns | mix:<numéro du retour>
    this.mixRefs = new Map(); // vue « Mix de X » : id de la source → éléments de sa tranche
    this.groupRefs = new Map(); // id de groupe → éléments de sa tranche (dans la vue courante)
    this.groupsOpen = new Set(parseIds(get(KEY_GROUPS_OPEN))); // groupes dépliés dans la vue « Groupes »
    this.tabs = this.root.querySelector(".mx-tabs");
    this.wide = get(KEY_WIDE) === "1";
    this.rows2 = get(KEY_ROWS) === "1";
    const saved = get(KEY_MODE);
    this.mode = MODES.includes(saved) ? saved : defaultMode(window.innerWidth / (getScale() / 100), window.innerHeight / (getScale() / 100));
    this.open_ = get(KEY_OPEN) === "1";
    const h = Number(get(KEY_H));
    if (h >= 160) this.root.style.setProperty("--mixer-h", `${h}px`);
    for (const b of this.root.querySelectorAll(".mx-modes button")) b.addEventListener("click", () => this.setMode(b.dataset.mode));
    this.root.querySelector(".close").addEventListener("click", () => this.close());
    document.getElementById("btn-mixer")?.addEventListener("click", () => this.toggle());
    document.getElementById("mx-add-bus")?.addEventListener("click", () => this.client.send({ type: "add_track", kind: "bus" }));
    document.getElementById("mx-add-return")?.addEventListener("click", () => {
      const project = this.state.project;
      openReturnDialog({ client: this.client, project, choices: returnChoices(project, { id: -1, is_return: true }), parse: parseReturnChoice });
    });
    document.getElementById("mx-add-group")?.addEventListener("click", () => openGroupDialog({ client: this.client, project: this.state.project }));
    document.getElementById("mx-add-track")?.addEventListener("click", (e) => {
      const m = menuEl();
      for (const [label, kind] of [["Piste stéréo", "stereo"], ["Piste mono", "mono"]]) {
        const b = document.createElement("button");
        b.className = "item";
        b.textContent = label;
        b.addEventListener("click", () => {
          closeMenuSafe();
          this.client.send({ type: "add_track", kind });
        });
        m.append(b);
      }
      showMenu(m, e.currentTarget);
    });
    document.getElementById("mx-width")?.addEventListener("click", () => {
      this.wide = !this.wide;
      put(KEY_WIDE, this.wide ? "1" : "0");
      this.#apply();
    });
    document.getElementById("mx-rows")?.addEventListener("click", () => {
      this.rows2 = !this.rows2;
      put(KEY_ROWS, this.rows2 ? "1" : "0");
      if (this.rows2 && this.mode === "dock" && this.root.getBoundingClientRect().height / (getScale() / 100) < 460) this.root.style.setProperty("--mixer-h", "520px");
      this.#apply();
    });
    this.#bindGrip();
    this.#bindFloat();
    this.#apply();
    if (this.open_ && state.project) this.render(true); // le projet est déjà là : le mixeur démarre avec ses pistes
  }

  isOpen() {
    return this.open_;
  }

  /** Les valeurs affichées suivent la position de lecture (courbes en mode lecture). */
  follow() {
    if (this.open_ && (this.refs.size || this.mixRefs.size || this.groupRefs.size)) this.#values();
  }

  open() {
    this.open_ = true;
    put(KEY_OPEN, "1");
    this.#apply();
    this.render(true);
  }

  close() {
    this.open_ = false;
    put(KEY_OPEN, "0");
    this.#apply();
  }

  toggle() {
    if (this.open_) this.close();
    else this.open();
  }

  setMode(mode) {
    if (!MODES.includes(mode)) return;
    this.mode = mode;
    put(KEY_MODE, mode);
    this.#apply();
  }

  #apply() {
    this.root.hidden = !this.open_;
    for (const m of MODES) this.root.classList.toggle(m, m === this.mode);
    this.app.classList.toggle("mx-full", this.open_ && this.mode === "full");
    for (const b of this.root.querySelectorAll(".mx-modes button")) b.setAttribute("aria-pressed", String(b.dataset.mode === this.mode));
    document.getElementById("btn-mixer")?.setAttribute("aria-pressed", String(this.open_));
    this.root.classList.toggle("wide", this.wide);
    this.strips.classList.toggle("rows2", this.rows2);
    document.getElementById("mx-width")?.setAttribute("aria-pressed", String(this.wide));
    document.getElementById("mx-rows")?.setAttribute("aria-pressed", String(this.rows2));
    window.dispatchEvent(new Event("resize")); // la timeline reprend la place disponible
  }

  // ---- construction ---------------------------------------------------------------------------------------------

  /** Nouvel événement `project` : reconstruit si la structure a changé, sinon met seulement les valeurs à jour. */
  render(force = false) {
    if (!this.open_ || !this.state.project) return;
    const key = structureKey(this.state.project);
    if (force || key !== this.key) {
      this.key = key;
      this.#build();
    }
    this.#values();
  }

  #node(id) {
    return this.state.project.nodes.find((n) => n.id === id);
  }

  #build() {
    const project = this.state.project;
    this.strips.replaceChildren();
    this.refs.clear();
    this.mixRefs.clear();
    this.groupRefs.clear();
    // Une vue « Mix de X » dont le retour a disparu (ou n'est plus un retour) revient à « Tout ».
    const mixId = this.view.startsWith("mix:") ? Number(this.view.slice(4)) : null;
    const retour = mixId === null ? null : this.#node(mixId);
    if (mixId !== null && !(retour && isReturn(retour))) this.view = "all";
    if (this.view === "groups" && !(project.groups ?? []).length) this.view = "all"; // plus aucun groupe : retour à « Tout »
    this.#renderTabs(project);
    if (retour && this.view.startsWith("mix:")) {
      this.strips.append(this.#groupBar(retour));
      for (const g of effectiveGroups(project, retour.id).groups) this.strips.append(this.#groupStrip(g, retour.id));
      for (const { node, send } of mixSources(project, retour)) this.strips.append(this.#mixStrip(node, send, retour));
      return;
    }
    if (this.view === "groups") {
      for (const item of groupsLayout(project, this.groupsOpen)) {
        if (item.group) this.strips.append(this.#groupStrip(item.group, null, true));
        else {
          const el = this.#strip(item.node, project);
          if (item.groupId) {
            el.classList.add("grouped");
            el.style.setProperty("--gc", groupColor(project, project.groups.find((g) => g.id === item.groupId), project.groups));
          }
          this.strips.append(el);
        }
      }
      return;
    }
    if (this.view === "all") for (const g of project.groups ?? []) this.strips.append(this.#groupStrip(g, null));
    for (const n of viewNodes(project, this.view)) {
      const el = this.#strip(n, project);
      this.strips.append(el);
    }
  }

  setView(view) {
    this.view = view;
    put(KEY_VIEW, view);
    this.key = "";
    this.render(true);
  }

  /** Onglets : Tout, Pistes, Bus, Retours, puis « Mix · X » pour chaque retour. */
  #renderTabs(project) {
    this.tabs.replaceChildren();
    const mk = (view, label, title) => {
      const b = document.createElement("button");
      b.textContent = label;
      b.title = title;
      b.setAttribute("role", "tab");
      b.setAttribute("aria-selected", String(this.view === view));
      b.addEventListener("click", () => this.setView(view));
      this.tabs.append(b);
    };
    mk("all", "Tout", "Toutes les tranches");
    mk("tracks", "Pistes", "Seulement les pistes");
    mk("buses", "Bus", "Seulement les bus (sans les retours de monitoring)");
    const returns = project.nodes.filter(isReturn);
    if ((project.groups ?? []).length) mk("groups", "Groupes", "Les groupes du projet (muet, solo, niveau) ; déplie un groupe pour voir ses pistes");
    if (returns.length) mk("returns", "Retours", "Les bus qui sortent sur la carte son (retours de monitoring des musiciens)");
    for (const r of returns) mk(`mix:${r.id}`, `Mix · ${r.name}`, `Ce qu'entend « ${r.name} » : les faders règlent le niveau envoyé à son retour`);
  }

  /**
   * Tranche d'un groupe : mute, solo et niveau communs à tous ses membres. Le curseur **reste à sa place** et affiche sa valeur : il
   * décale d'autant (de −12 à +12 dB) le niveau de chaque membre en gardant les écarts entre eux ; le remettre à 0 (ou double-clic) rend
   * les niveaux d'avant. Avec `to`, la tranche agit sur les envois des membres vers ce retour (vue « Mix · X ») ; sans, sur les pistes.
   */
  #groupStrip(g, to, foldable = false) {
    const project = this.state.project;
    const eff = effectiveGroups(project, to);
    const root = document.createElement("div");
    root.className = "mx-strip group";
    root.style.setProperty("--c", groupColor(project, g, eff.groups));
    root.innerHTML = `<div class="mx-name"></div><div class="mx-gcount"></div><div class="mx-gtitle">Niveau du groupe</div><output class="mx-goff" aria-live="off"></output><div class="mx-fader-wrap"><div class="mx-fader centered" role="slider" aria-label="Décaler le niveau de tout le groupe (dB)" aria-valuemin="-${OFFSET_RANGE}" aria-valuemax="${OFFSET_RANGE}" tabindex="0"><div class="zero"></div><div class="thumb"></div></div></div><div class="mx-btns"><button class="m" title="Couper tout le groupe" aria-label="Muet du groupe">M</button><button class="s" title="Solo du groupe" aria-label="Solo du groupe">S</button><button class="edit" title="Modifier le groupe" aria-label="Modifier le groupe">⚙</button></div>`;
    const q = (s) => root.querySelector(s);
    q(".mx-name").textContent = g.name;
    if (foldable) {
      // Vue « Groupes » : le compteur devient le bouton qui déplie les pistes du groupe (mémorisé).
      const fold = document.createElement("button");
      fold.className = "mx-gcount fold";
      fold.setAttribute("aria-expanded", String(this.groupsOpen.has(g.id)));
      fold.title = "Montrer ou cacher les pistes de ce groupe";
      fold.addEventListener("click", () => {
        if (!this.groupsOpen.delete(g.id)) this.groupsOpen.add(g.id);
        put(KEY_GROUPS_OPEN, JSON.stringify([...this.groupsOpen]));
        this.key = "";
        this.render(true);
      });
      q(".mx-gcount").replaceWith(fold);
    }
    const refs = { root, count: q(".mx-gcount"), foldable, gid: g.id, m: q(".m"), s: q(".s"), fader: q(".mx-fader"), thumb: q(".thumb"), zero: q(".zero"), off: q(".mx-goff"), to };
    this.groupRefs.set(g.id, refs);
    const flags = () => groupFlags(this.state.project, g.id, to);
    const toPart = to === null ? {} : { to };
    const act = (extra) => this.client.send({ type: "set_group", group: g.id, ...toPart, ...extra });
    refs.m.addEventListener("click", () => act({ mute: !flags().mute }));
    refs.s.addEventListener("click", () => act({ solo: !flags().solo }));
    q(".edit").addEventListener("click", () => {
      const now = effectiveGroups(this.state.project, to);
      openGroupDialog({ client: this.client, project: this.state.project, group: now.groups.find((x) => x.id === g.id), ret: now.custom ? to : null });
    });
    // Fader vertical comme les autres réglages : zéro au centre, de −12 à +12 dB, il reste où on le met ; double-clic = 0.
    const current = () => groupOffset(effectiveGroups(this.state.project, to).groups.find((x) => x.id === g.id) ?? g, to);
    const setOffset = (db) => this.client.send({ type: "set_group_offset", group: g.id, db: Math.max(-OFFSET_RANGE, Math.min(OFFSET_RANGE, Math.round(db * 2) / 2)), ...toPart });
    this.#bindFaderWith(refs.fader, `group:${g.id}:${to}`, setOffset, () => setOffset(0), current, { max: OFFSET_RANGE, min: -OFFSET_RANGE });
    return root;
  }

  /**
   * Petite carte en tête de la vue « Mix · X » : de quels groupes ce mix dépend, au choix et de façon visible. « Groupes du projet » (les
   * mêmes pour tous les retours) ou « Propres à ce retour » (une copie qu'on change à volonté, qui **remplace** ceux du projet dans ce mix
   * seulement). Avec des groupes propres : « ＋ Groupe » et « Copier d'un autre retour… ».
   */
  #groupBar(retour) {
    const root = document.createElement("div");
    root.className = "mx-strip gbar";
    const custom = !!retour.return_groups;
    root.innerHTML = `<div class="mx-name">Groupes</div><div class="gb-choice" role="group" aria-label="Groupes de ce retour"></div><p class="gb-state"></p><div class="gb-buttons"></div>`;
    root.querySelector(".gb-state").textContent = custom ? `Propres à « ${retour.name} » : ils remplacent ceux du projet dans ce mix.` : "Ceux du projet : les mêmes pour tous les retours.";
    const choice = root.querySelector(".gb-choice");
    const seg = (label, title, pressed, fn) => {
      const b = document.createElement("button");
      b.textContent = label;
      b.title = title;
      b.setAttribute("aria-pressed", String(pressed));
      if (!pressed) b.addEventListener("click", fn);
      choice.append(b);
    };
    seg("Projet", "Ce retour suit les groupes du projet", !custom, () => this.client.send({ type: "reset_return_groups", ret: retour.id }));
    seg("Propres", "Copie les groupes du projet en groupes propres à ce retour : tu peux les changer sans toucher aux autres retours", custom, () => this.client.send({ type: "customize_return_groups", ret: retour.id }));
    const box = root.querySelector(".gb-buttons");
    if (custom) {
      const add = document.createElement("button");
      add.textContent = "＋ Groupe";
      add.title = "Créer un groupe propre à ce retour";
      add.addEventListener("click", () => openGroupDialog({ client: this.client, project: this.state.project, ret: retour.id }));
      box.append(add);
      const others = this.state.project.nodes.filter((n) => isReturn(n) && n.id !== retour.id);
      if (others.length) {
        const sel = document.createElement("select");
        sel.setAttribute("aria-label", "Copier les groupes d'un autre retour");
        sel.title = "Remplace les groupes propres de ce retour par une copie de ceux d'un autre retour";
        sel.append(new Option("Copier d'un retour…", ""));
        for (const o of others) sel.append(new Option(o.name, String(o.id)));
        sel.addEventListener("change", () => {
          if (sel.value !== "") this.client.send({ type: "copy_return_groups", ret: retour.id, from: Number(sel.value) });
          sel.value = "";
        });
        box.append(sel);
      }
    }
    return root;
  }

  /** Tranche de la vue « Mix de X » : le fader règle le niveau de l'envoi de cette source vers le retour. */
  #mixStrip(n, send, retour) {
    const root = document.createElement("div");
    root.className = "mx-strip mix";
    root.style.setProperty("--c", n.color || "var(--muted)");
    root.innerHTML = `<div class="mx-name"></div><div class="mx-fader-wrap"><div class="mx-fader" role="slider" aria-label="Niveau envoyé au retour" aria-valuemin="${DB_MIN}" aria-valuemax="${DB_MAX}" tabindex="0"><div class="zero"></div><div class="thumb"></div></div></div><div class="mx-db"></div><div class="mx-btns"><button class="m" title="Couper cette source dans ce retour seulement" aria-label="Muet dans ce retour">M</button><button class="s" title="Solo dans ce retour : le musicien n'entend que les sources en solo" aria-label="Solo dans ce retour">S</button><button class="pre" title="Avant le fader : le fader, le muet et le solo de la régie ne changent pas ce que le musicien entend" aria-label="Avant le fader">P</button><button class="off" title="Couper l'envoi (retirer)" aria-label="Retirer l'envoi">✕</button></div>`;
    const q = (s) => root.querySelector(s);
    q(".mx-name").textContent = n.name;
    this.#addGrip(root, n.id);
    const refs = { root, fader: q(".mx-fader"), thumb: q(".thumb"), zero: q(".zero"), db: q(".mx-db"), pre: q(".pre"), off: q(".off"), m: q(".m"), s: q(".s"), retour: retour.id };
    this.mixRefs.set(n.id, refs);
    const setDb = (db) => {
      const cur = this.#node(n.id)?.sends.find((s) => s.to === retour.id);
      const gain = gainOfDb(Math.round(db * 10) / 10);
      if (!cur) this.client.send({ type: "add_send", node: n.id, to: retour.id });
      this.client.send({ type: "set_send", node: n.id, to: retour.id, gain });
    };
    this.#bindFaderWith(refs.fader, `mix:${n.id}`, setDb, () => setDb(0), () => dbOf(this.#node(n.id)?.sends.find((s) => s.to === retour.id)?.gain ?? 0));
    trackGesture(refs.fader, this.client, () => ({ node: n.id, target: { kind: "send", to: retour.id } }));
    refs.db.title = "Réglage fin (grand fader)";
    refs.db.addEventListener("click", () => {
      if (!this.#node(n.id)?.sends.some((s) => s.to === retour.id)) this.client.send({ type: "add_send", node: n.id, to: retour.id });
      this.#openSheet({ node: n.id, to: retour.id });
    });
    refs.pre.addEventListener("click", () => {
      const cur = this.#node(n.id)?.sends.find((s) => s.to === retour.id);
      if (!cur) this.client.send({ type: "add_send", node: n.id, to: retour.id });
      this.client.send({ type: "set_send", node: n.id, to: retour.id, pre: !cur?.pre });
    });
    // Muet / solo : propres à ce retour. Une source sans envoi en reçoit un à niveau nul, pour qu'on puisse le couper ou l'isoler.
    const flag = (key) => () => {
      const cur = this.#node(n.id)?.sends.find((s) => s.to === retour.id);
      if (!cur) {
        this.client.send({ type: "add_send", node: n.id, to: retour.id });
        this.client.send({ type: "set_send", node: n.id, to: retour.id, gain: 0 });
      }
      this.client.send({ type: "set_send", node: n.id, to: retour.id, [key]: !cur?.[key] });
    };
    refs.m.addEventListener("click", flag("mute"));
    refs.s.addEventListener("click", flag("solo"));
    return root;
  }

  /** Poignée en haut d'une tranche : on la glisse pour changer la place de la piste (le même ordre que dans la timeline). */
  #addGrip(root, id) {
    const grip = document.createElement("div");
    grip.className = "mx-reorder";
    grip.textContent = "⠿";
    grip.title = "Glisser pour changer l'ordre des pistes (aussi dans la timeline)";
    grip.setAttribute("role", "button");
    grip.setAttribute("aria-label", "Déplacer la piste");
    grip.addEventListener("pointerdown", (e) => this.#startStripDrag(e, id));
    root.prepend(grip);
  }

  /**
   * Déplacement d'une tranche : un trait montre où elle ira, une seule commande `move_track` au relâchement. L'ordre est celui du
   * projet entier (timeline comprise) : dans une vue filtrée (pistes seules, mix d'un retour), « avant la tranche X » se traduit par
   * « avant X » dans la liste complète, et « à la fin » par « avant ce qui suit la dernière tranche visible ».
   */
  #startStripDrag(e, id) {
    e.preventDefault();
    const shown = [...this.refs, ...this.mixRefs].filter(([k]) => k !== 0).map(([k, r]) => [k, r.root]);
    const others = shown.filter(([k]) => k !== id);
    const me = shown.find(([k]) => k === id)?.[1];
    if (!me) return;
    const grip = e.currentTarget;
    grip.setPointerCapture(e.pointerId);
    me.classList.add("dragging");
    const rects = others.map(([k, el]) => [k, el.getBoundingClientRect()]);
    // Lieu d'insertion : la tranche la plus proche du pointeur, avant elle si le pointeur est à sa gauche, après sinon.
    const targetAt = (x, y) => {
      let best = null;
      for (const [i, [, r]] of rects.entries()) {
        const d = Math.hypot(x - (r.left + r.width / 2), y - (r.top + r.height / 2));
        if (!best || d < best.d) best = { d, i, after: x >= r.left + r.width / 2 };
      }
      return best ? (best.after ? best.i + 1 : best.i) : 0;
    };
    const clear = () => {
      for (const [, el] of shown) el.classList.remove("drop-before", "drop-end");
    };
    const mark = (index) => {
      clear();
      if (index < others.length) others[index][1].classList.add("drop-before");
      else if (others.length) others.at(-1)[1].classList.add("drop-end");
    };
    let index = targetAt(e.clientX, e.clientY);
    mark(index);
    const move = (ev) => {
      index = targetAt(ev.clientX, ev.clientY);
      mark(index);
    };
    const end = (commit) => {
      grip.removeEventListener("pointermove", move);
      grip.removeEventListener("pointerup", up);
      grip.removeEventListener("pointercancel", cancel);
      me.classList.remove("dragging");
      clear();
      if (!commit) return;
      const order = orderedNodes(this.state.project).filter((n) => n.kind !== "master").map((n) => n.id);
      const rest = order.filter((k) => k !== id);
      const before = index < others.length ? others[index][0] : rest[rest.indexOf(others.at(-1)?.[0]) + 1];
      if (before === order[order.indexOf(id) + 1]) return; // elle est déjà juste devant cette tranche (ou déjà à la fin)
      this.client.send(before === undefined ? { type: "move_track", node: id } : { type: "move_track", node: id, before });
    };
    const up = () => end(true);
    const cancel = () => end(false);
    grip.addEventListener("pointermove", move);
    grip.addEventListener("pointerup", up);
    grip.addEventListener("pointercancel", cancel);
  }

  /** Renomme sur place (comme dans l'en-tête de piste) : Entrée valide, Échap abandonne. Le mixeur se reconstruit au nouveau nom. */
  #rename(nameEl, n) {
    if (nameEl.querySelector("input")) return;
    const input = document.createElement("input");
    input.className = "mx-rename";
    input.value = n.name;
    input.maxLength = 60;
    input.setAttribute("aria-label", "Nom");
    const saved = [...nameEl.childNodes];
    nameEl.replaceChildren(input);
    input.focus();
    input.select();
    let done = false;
    const finish = (commit) => {
      if (done) return;
      done = true;
      const v = input.value.trim();
      if (input.isConnected) nameEl.replaceChildren(...saved);
      if (commit && v && v !== n.name) this.client.send({ type: "rename_track", node: n.id, name: v });
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") finish(true);
      else if (e.key === "Escape") finish(false);
      e.stopPropagation();
    });
    input.addEventListener("blur", () => finish(true));
  }

  #strip(n, project) {
    const root = document.createElement("div");
    root.className = `mx-strip${n.kind === "master" ? " master" : ""}`;
    root.style.setProperty("--c", n.color || "var(--muted)");
    const isTrack = n.kind === "mono" || n.kind === "stereo";
    const label = n.is_return ? "retour" : { stereo: "stéréo", mono: "mono", bus: "bus", master: "" }[n.kind];
    root.innerHTML = `
      <div class="mx-name"></div>
      <select class="mx-out" aria-label="Sortie" ${n.kind === "master" || isReturn(n) ? "hidden" : ""}></select>
      <select class="mx-ret" aria-label="Sortie sur la carte son" title="${n.is_return ? "Sortie de ce retour sur la carte son (sorties 3 et suivantes) : muet tant qu'aucune n'est choisie" : "Sortir ce bus sur la carte son (sorties 3 et suivantes) au lieu de la sortie principale"}" ${n.kind === "bus" ? "" : "hidden"}></select>
      <button class="mx-qr" title="Adresse et code QR de ce retour : la tablette du musicien règle son mix" aria-label="Code QR du retour" ${isReturn(n) ? "" : "hidden"}>QR</button>
      <div class="mx-top"><div class="mx-fx"></div><div class="mx-sends"></div></div>
      <div class="mx-pan"><input type="range" min="-1" max="1" step="0.01" aria-label="Panoramique"><span class="pan-out"></span></div>
      <div class="mx-fader-wrap"><div class="mx-fader" role="slider" aria-label="Volume" aria-valuemin="${DB_MIN}" aria-valuemax="${DB_MAX}" tabindex="0"><div class="zero"></div><div class="thumb"></div></div><canvas class="mx-meter" aria-hidden="true"></canvas></div>
      <div class="mx-db"></div>
      <div class="mx-btns"><button class="m" title="Muet" aria-label="Muet">M</button><button class="s" title="Solo" aria-label="Solo">S</button>${isTrack ? '<button class="r" title="Armer pour l\'enregistrement" aria-label="Armer">●</button>' : ""}<button class="p" title="Phase inversée" aria-label="Phase inversée">Ø</button></div>`;
    const q = (s) => root.querySelector(s);
    root.querySelector(".mx-name").innerHTML = "";
    const name = q(".mx-name");
    name.textContent = n.name;
    let witness = null;
    if (isTrack) {
      witness = createWitness();
      name.append(witness);
    }
    if (label) {
      const small = document.createElement("small");
      small.textContent = label;
      name.append(small);
    }
    const grp = n.group ? this.state.project.groups?.find((g) => g.id === n.group) : null;
    if (grp) {
      const tag = document.createElement("span");
      tag.className = "mx-gtag";
      tag.textContent = grp.name;
      tag.title = `Groupe « ${grp.name} »`;
      tag.style.setProperty("--g", groupColor(this.state.project, grp));
      root.prepend(tag);
    }
    if (n.kind !== "master") {
      name.title = "Double-clic : renommer";
      name.addEventListener("dblclick", () => this.#rename(name, n));
      this.#addGrip(root, n.id);
    }
    q(".mx-qr").addEventListener("click", (e) => openMonitorPopup(e.currentTarget, this.state, this.client, n));
    const refs = { witness, root, out: q(".mx-out"), ret: q(".mx-ret"), fx: q(".mx-fx"), sends: q(".mx-sends"), pan: q(".mx-pan input"), panOut: q(".pan-out"), fader: q(".mx-fader"), thumb: q(".thumb"), zero: q(".zero"), meter: q(".mx-meter"), db: q(".mx-db"), m: q(".m"), s: q(".s"), r: q(".r"), p: q(".p"), sendRows: new Map() };
    this.refs.set(n.id, refs);
    const send = (patch) => this.client.send({ type: "set_node", node: n.id, ...patch });

    // Sortie.
    if (n.kind !== "master") {
      for (const t of outputTargets(project, n)) refs.out.append(new Option(t.kind === "master" ? "→ Sortie" : `→ ${t.name}`, String(t.id)));
      refs.out.value = String(n.output ?? 0);
      refs.out.addEventListener("change", () => this.client.send({ type: "set_track_output", node: n.id, output: Number(refs.out.value) }));
    }
    // Retour de monitoring : sorties de la carte son (bus seulement).
    if (n.kind === "bus") {
      const choices = returnChoices(project, n);
      for (const c of choices) {
        const o = new Option(c.label, c.value);
        o.disabled = c.disabled;
        refs.ret.append(o);
      }
      const cur = n.phys ? `${n.phys.first}:${n.phys.stereo ? 1 : 0}:${n.phys.mono ? 1 : 0}` : "";
      refs.ret.value = [...refs.ret.options].some((o) => o.value === cur) ? cur : "";
      refs.ret.disabled = (project.audio_outputs || 0) <= 2 && !n.phys;
      if (refs.ret.disabled) refs.ret.title = "Le pilote n'a ouvert que la sortie principale : pas de sortie sur la carte son possible";
      if (n.is_return && !n.phys) {
        refs.ret.classList.add("silent");
        refs.ret.title = "Ce retour est muet : choisis sa sortie sur la carte son";
      }
      refs.ret.addEventListener("change", () => this.client.send({ type: "set_return_output", node: n.id, output: parseReturnChoice(refs.ret.value) }));
    }
    // Plugins : un bouton par plugin (grisé s'il est contourné), « ＋ » pour en ajouter ; tous ouvrent la fenêtre des plugins.
    for (const ins of n.inserts) {
      const b = document.createElement("button");
      b.textContent = ins.name;
      b.title = `${ins.name} — ouvrir les plugins de « ${n.name} »${ins.gui ? " (double-clic : l'interface du plugin)" : ""}`;
      b.dataset.insert = String(ins.id);
      b.addEventListener("click", () => this.onPlugins?.(n.id));
      b.addEventListener("dblclick", () => ins.gui && this.client.send({ type: "open_plugin_ui", insert: ins.id }));
      refs.fx.append(b);
    }
    const add = document.createElement("button");
    add.textContent = "＋ fx";
    add.title = "Ajouter un plugin";
    add.addEventListener("click", () => this.onPlugins?.(n.id));
    refs.fx.append(add);
    // Envois : un par destination, avec son niveau, « P » = avant le fader, ✕ pour le retirer.
    if (n.kind !== "master") {
      for (const s of n.sends) this.#sendRow(n, s, refs, project);
      const targets = sendTargets(project, n);
      if (targets.length) {
        const sel = document.createElement("select");
        sel.setAttribute("aria-label", "Ajouter un envoi");
        sel.append(new Option("＋ Envoi…", ""));
        for (const t of targets) sel.append(new Option(t.kind === "master" ? "Sortie" : t.name, String(t.id)));
        sel.addEventListener("change", () => {
          if (sel.value !== "") this.client.send({ type: "add_send", node: n.id, to: Number(sel.value) });
          sel.value = "";
        });
        refs.sends.append(sel);
      }
    }
    // Panoramique.
    refs.pan.addEventListener("input", () => send({ pan: Number(refs.pan.value) }));
    refs.pan.addEventListener("dblclick", () => send({ pan: 0 }));
    // Fader.
    this.#bindFader(n.id, refs);
    trackGesture(refs.fader, this.client, () => ({ node: n.id, target: { kind: "gain" } }));
    trackGesture(refs.pan, this.client, () => ({ node: n.id, target: { kind: "pan" } }));
    refs.db.title = "Réglage fin (grand fader)";
    refs.db.addEventListener("click", () => this.#openSheet({ node: n.id, to: null }));
    // Boutons.
    refs.m.addEventListener("click", () => send({ mute: !this.#node(n.id).mute }));
    if (isListenable(n)) {
      setupListenButton(refs.s);
      refs.s.addEventListener("click", () => toggleListen(this.client, this.state.project, n.id));
    } else {
      refs.s.addEventListener("click", () => send({ solo: !this.#node(n.id).solo }));
    }
    if (n.kind === "master") refs.s.hidden = true; // le solo n'a pas de sens pour la sortie principale
    refs.r?.addEventListener("click", () => send({ arm: !this.#node(n.id).arm }));
    refs.p.addEventListener("click", () => send({ invert: !this.#node(n.id).invert }));
    return root;
  }

  #sendRow(n, s, refs, project) {
    const dest = project.nodes.find((x) => x.id === s.to);
    const row = document.createElement("div");
    row.className = "mx-send";
    row.innerHTML = `<span class="dest"></span><input type="range" min="${DB_MIN}" max="${DB_MAX}" step="0.5" aria-label="Niveau de l'envoi"><button class="pre" aria-label="Avant le fader" title="Avant le fader (le fader, le muet et le solo de la piste ne touchent pas cet envoi)">P</button><button class="del" aria-label="Retirer l'envoi" title="Retirer cet envoi">✕</button>`;
    const label = `→ ${dest?.kind === "master" ? "Sortie" : (dest?.name ?? s.to)}`;
    row.querySelector(".dest").textContent = label;
    const slider = row.querySelector("input");
    trackGesture(slider, this.client, () => ({ node: n.id, target: { kind: "send", to: s.to } }));
    // Au doigt, le petit curseur est trop brusque : on ouvre la feuille de réglage fin (grand fader) à la place.
    slider.addEventListener("pointerdown", (e) => {
      if (e.pointerType !== "touch") return;
      e.preventDefault();
      this.#openSheet({ node: n.id, to: s.to });
    });
    row.querySelector(".dest").addEventListener("click", () => this.#openSheet({ node: n.id, to: s.to }));
    row.querySelector(".dest").title = "Réglage fin (grand fader)";
    slider.addEventListener("input", () => this.client.send({ type: "set_send", node: n.id, to: s.to, gain: gainOfDb(Number(slider.value)) }));
    slider.addEventListener("dblclick", () => this.client.send({ type: "set_send", node: n.id, to: s.to, gain: 1 }));
    row.querySelector(".pre").addEventListener("click", () => {
      const cur = this.#node(n.id).sends.find((x) => x.to === s.to);
      this.client.send({ type: "set_send", node: n.id, to: s.to, pre: !cur?.pre });
    });
    row.querySelector(".del").addEventListener("click", () => this.client.send({ type: "remove_send", node: n.id, to: s.to }));
    refs.sends.append(row);
    refs.sendRows.set(s.to, { row, slider, label, dest: row.querySelector(".dest"), pre: row.querySelector(".pre") });
  }

  /** Ouvre la feuille de réglage fin pour le fader d'une tranche (`to` = null) ou pour l'un de ses envois. */
  #openSheet(target) {
    const n = this.#node(target.node);
    if (!n) return;
    const s = target.to === null ? null : n.sends.find((x) => x.to === target.to);
    if (target.to !== null && !s) return;
    const dest = target.to === null ? null : this.state.project.nodes.find((x) => x.id === target.to);
    this.sheetFor = target;
    this.sheet.open({
      title: s ? `${n.name} → ${dest?.kind === "master" ? "Sortie" : dest?.name}` : `Volume de ${n.name}`,
      gain: s ? s.gain : n.gain,
      onChange: (gain) => this.client.send(s ? { type: "set_send", node: n.id, to: target.to, gain } : { type: "set_node", node: n.id, gain }),
      onGesture: (active) => this.client.send({ type: "gesture", node: n.id, target: s ? { kind: "send", to: target.to } : { kind: "gain" }, active }),
    });
  }

  #bindFader(id, refs) {
    const set = (db) => this.client.send({ type: "set_node", node: id, gain: gainOfDb(Math.round(db * 10) / 10) });
    this.#bindFaderWith(refs.fader, id, set, () => this.client.send({ type: "set_node", node: id, gain: 1 }), () => dbOf(this.#node(id)?.gain ?? 1));
  }

  /** Fader vertical : `set(db)` à chaque geste, `reset()` au double-clic ou à Début, `current()` donne le niveau pour les flèches. */
  #bindFaderWith(f, id, set, reset, current, range = { max: DB_MAX, min: DB_MIN }) {
    const dbAt = (e) => {
      const r = f.getBoundingClientRect();
      const k = f.clientHeight / (r.height || 1);
      const y = (e.clientY - r.top) * k;
      const frac = Math.max(0, Math.min(1, (y - 4) / Math.max(1, f.clientHeight - 8)));
      return range.max - frac * (range.max - range.min);
    };
    f.addEventListener("pointerdown", (e) => {
      f.setPointerCapture?.(e.pointerId);
      this.dragging = id;
      set(dbAt(e));
    });
    f.addEventListener("pointermove", (e) => {
      if (this.dragging === id) set(dbAt(e));
    });
    const end = () => {
      if (this.dragging === id) this.dragging = null;
    };
    f.addEventListener("pointerup", end);
    f.addEventListener("pointercancel", end);
    f.addEventListener("dblclick", reset);
    f.addEventListener("keydown", (e) => {
      if (!["ArrowUp", "ArrowDown", "Home"].includes(e.key)) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Home") return reset();
      set(current() + (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 0.1 : 1));
    });
  }

  /** Recale tous les affichages sur l'état du projet (sans toucher à ce que l'utilisateur est en train de manipuler). */
  #values() {
    if (this.sheet.isOpen() && this.sheetFor) {
      const n = this.#node(this.sheetFor.node);
      const g = this.sheetFor.to === null ? n?.gain : n?.sends.find((x) => x.to === this.sheetFor.to)?.gain;
      if (g === undefined) this.sheet.close();
      else this.sheet.update(g);
    }
    for (const [id, r] of this.mixRefs) {
      const n = this.#node(id);
      const s = n?.sends.find((x) => x.to === r.retour);
      if (!n) continue;
      const db = s ? dbOf(s.gain) : DB_MIN;
      const H = r.fader.clientHeight || 120;
      r.thumb.style.top = `${4 + ((DB_MAX - db) / (DB_MAX - DB_MIN)) * (H - 8)}px`;
      r.fader.style.setProperty("--y", r.thumb.style.top);
      r.zero.style.top = `${4 + (DB_MAX / (DB_MAX - DB_MIN)) * (H - 8)}px`;
      r.fader.setAttribute("aria-valuenow", String(Math.round(db)));
      r.db.textContent = s ? `${formatDb(gainToDb(s.gain))} dB` : "—";
      r.pre.setAttribute("aria-pressed", String(!!s?.pre));
      r.m.setAttribute("aria-pressed", String(!!s?.mute));
      r.s.setAttribute("aria-pressed", String(!!s?.solo));
      r.root.classList.toggle("silent", !s || !!s.mute || (anySendSolo(this.state.project, r.retour) && !s.solo));
    }
    for (const [id, r] of this.groupRefs) {
      const g = effectiveGroups(this.state.project, r.to).groups.find((x) => x.id === id);
      if (!g) continue;
      const f = groupFlags(this.state.project, id, r.to);
      r.m.setAttribute("aria-pressed", String(f.mute));
      r.s.setAttribute("aria-pressed", String(f.solo));
      const off = groupOffset(g, r.to);
      const H = r.fader.clientHeight || 120;
      const y = 4 + ((OFFSET_RANGE - off) / (2 * OFFSET_RANGE)) * (H - 8);
      const zy = 4 + 0.5 * (H - 8);
      r.thumb.style.top = `${y}px`;
      r.zero.style.top = `${zy}px`;
      r.fader.style.setProperty("--y", `${Math.min(y, zy)}px`);
      r.fader.style.setProperty("--y2", `${Math.max(y, zy)}px`);
      r.fader.setAttribute("aria-valuenow", String(off));
      r.off.textContent = formatOffset(off);
      r.off.classList.toggle("shifted", off !== 0);
      const n = groupMembers(this.state.project, id, r.to).length;
      const label = n === 1 ? "1 membre" : `${n} membres`;
      r.count.textContent = r.foldable ? `${this.groupsOpen.has(r.gid) ? "▾" : "▸"} ${label}` : label;
    }
    for (const [id, r] of this.refs) {
      const n = this.#node(id);
      if (!n) continue;
      const { tempo, pos } = this.state;
      const shown = shownValues(n, tempo ? tempo.wholeAt(Math.max(0, pos)) : 0);
      const db = dbOf(shown.gain);
      const H = r.fader.clientHeight || 120;
      const y = 4 + ((DB_MAX - db) / (DB_MAX - DB_MIN)) * (H - 8);
      r.thumb.style.top = `${y}px`;
      r.fader.style.setProperty("--y", `${y}px`);
      r.zero.style.top = `${4 + (DB_MAX / (DB_MAX - DB_MIN)) * (H - 8)}px`;
      r.fader.setAttribute("aria-valuenow", String(Math.round(db)));
      r.db.textContent = `${formatDb(gainToDb(shown.gain))} dB`;
      r.fader.classList.toggle("automated", shown.gainAuto);
      if (document.activeElement !== r.pan) r.pan.value = String(shown.pan);
      r.pan.classList.toggle("automated", shown.panAuto);
      r.panOut.textContent = shown.pan === 0 ? "C" : `${Math.abs(Math.round(shown.pan * 100))} ${shown.pan < 0 ? "G" : "D"}`;
      r.m.setAttribute("aria-pressed", String(n.mute));
      r.s.setAttribute("aria-pressed", String(isListenable(n) ? this.state.project.listen === id : n.solo));
      r.r?.setAttribute("aria-pressed", String(n.arm));
      paintWitness(r.witness, n, this.state.midiAct?.[id] ?? 0);
      r.p.setAttribute("aria-pressed", String(!!n.invert));
      if (r.out && document.activeElement !== r.out) r.out.value = String(n.output ?? 0);
      for (const ins of n.inserts) r.fx.querySelector(`[data-insert="${ins.id}"]`)?.classList.toggle("off", !!ins.bypass);
      for (const s of n.sends) {
        const row = r.sendRows.get(s.to);
        if (!row) continue;
        const sg = shown.send(s.to, s.gain);
        if (document.activeElement !== row.slider) row.slider.value = String(dbOf(sg));
        row.slider.classList.toggle("automated", shown.sendAuto(s.to));
        row.slider.title = `${formatDb(gainToDb(sg))} dB`;
        row.dest.textContent = `${row.label}  ${formatDb(gainToDb(sg))} dB`;
        row.pre.setAttribute("aria-pressed", String(s.pre));
      }
    }
  }

  // ---- vumètres -------------------------------------------------------------------------------------------------

  /** Témoin d'entrée MIDI : appelé quand l'ensemble des pistes qui reçoivent du MIDI change. */
  setMidiActivity() {
    for (const [id, r] of this.refs) paintWitness(r.witness, this.#node(id), this.state.midiAct?.[id] ?? 0);
  }

  drawMeters(dt) {
    if (!this.open_) return;
    const dpr = pixelRatio();
    for (const [id, r] of this.refs) {
      const c = r.meter;
      const w = Math.max(1, Math.round(c.clientWidth * dpr));
      const h = Math.max(1, Math.round(c.clientHeight * dpr));
      if (c.width !== w || c.height !== h) {
        c.width = w;
        c.height = h;
      }
      const g = c.getContext("2d");
      g.clearRect(0, 0, w, h);
      const levels = this.state.meters[id] ?? [0, 0];
      const hold = this.hold.get(id) ?? [0, 0];
      const pos = (v) => (v <= 0 ? 0 : Math.max(0, Math.min(1, (gainToDb(v) - DB_MIN) / (DB_MAX - DB_MIN))));
      for (let ch = 0; ch < 2; ch++) {
        hold[ch] = Math.max(levels[ch], hold[ch] - dt * 0.0006);
        const x = Math.round((ch * w) / 2) + (ch ? 1 : 0);
        const bw = Math.round(w / 2) - 1;
        const top = Math.round(h * (1 - pos(levels[ch])));
        const grad = g.createLinearGradient(0, h, 0, 0);
        grad.addColorStop(0, "#3ecf8e");
        grad.addColorStop(0.72, "#e9d34a");
        grad.addColorStop(0.9, "#ff4d5e");
        g.fillStyle = grad;
        g.fillRect(x, top, bw, h - top);
        const hy = Math.round(h * (1 - pos(hold[ch])));
        if (hy < h - 1) {
          g.fillStyle = "#fff";
          g.fillRect(x, hy, bw, 2);
        }
      }
      this.hold.set(id, hold);
    }
  }

  // ---- hauteur du panneau et déplacement de la fenêtre flottante -----------------------------------------------------

  #bindGrip() {
    const grip = this.root.querySelector(".mx-grip");
    let drag = null;
    grip.addEventListener("pointerdown", (e) => {
      grip.setPointerCapture?.(e.pointerId);
      drag = { y: e.clientY, h: this.root.getBoundingClientRect().height / (getScale() / 100) };
    });
    grip.addEventListener("pointermove", (e) => {
      if (!drag) return;
      const k = getScale() / 100;
      const h = Math.max(160, Math.min(window.innerHeight / k - 140, drag.h + (drag.y - e.clientY) / k));
      this.root.style.setProperty("--mixer-h", `${Math.round(h)}px`);
      this.#values();
    });
    const end = () => {
      if (!drag) return;
      drag = null;
      put(KEY_H, String(Math.round(parseFloat(this.root.style.getPropertyValue("--mixer-h")) || 330)));
      window.dispatchEvent(new Event("resize"));
      this.#values();
    };
    grip.addEventListener("pointerup", end);
    grip.addEventListener("pointercancel", end);
  }

  #bindFloat() {
    const head = this.root.querySelector("header");
    let drag = null;
    head.addEventListener("pointerdown", (e) => {
      if (this.mode !== "float" || e.target.closest("button")) return;
      head.setPointerCapture?.(e.pointerId);
      const r = this.root.getBoundingClientRect();
      const k = getScale() / 100;
      drag = { x: e.clientX, y: e.clientY, left: r.left / k, top: r.top / k };
    });
    head.addEventListener("pointermove", (e) => {
      if (!drag) return;
      const k = getScale() / 100;
      this.root.style.left = `${Math.max(0, drag.left + (e.clientX - drag.x) / k)}px`;
      this.root.style.top = `${Math.max(0, drag.top + (e.clientY - drag.y) / k)}px`;
    });
    const end = () => (drag = null);
    head.addEventListener("pointerup", end);
    head.addEventListener("pointercancel", end);
    new ResizeObserver(() => this.#values()).observe(this.strips);
  }
}
