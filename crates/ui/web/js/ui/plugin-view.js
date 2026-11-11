// Vue d'un plugin : une bande en haut (contourner, nom ou champ « choisir un plugin », sorties, bascule Interface / Paramètres) et, dessous,
// la vue choisie : l'interface native du plugin (sa zone est alors réservée par l'application), la liste de ses paramètres (avec la page de
// contrôle de la télécommande), le routage de ses sorties multiples, ou le sélecteur de plugins installés.
//
// Utilisée à deux endroits : dans le panneau des plugins de la page principale (navigateur, ou plateforme sans fenêtres natives) et dans la
// fenêtre de plugin de l'application (`plugin.html`, où `docked` est vrai : l'interface native est logée par l'application sous la bande).

import { valueAtWhole } from "../automation-model.js";
import { trackGesture } from "./gesture.js";

const ONLY_MINE = "Le Tournedos Rossini";
const MAX_PAGE = 16; // paramètres d'une page de contrôle (deux groupes de huit encodeurs)
const MAX_ROWS = 80; // paramètres affichés à la fois (un plugin peut en avoir des milliers : on affine par la recherche)

const norm = (t) => t.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();

/** Les nœuds qu'on peut prendre pour chaîne latérale d'un plugin du nœud `node` : pistes et bus (pas la sortie principale, ni le nœud lui-même, ni un retour). */
export function sidechainSources(project, node) {
  return project.nodes.filter((n) => n.kind !== "master" && n.id !== node && !(n.kind === "bus" && n.is_return));
}

/** Texte du nombre de sorties d'un plugin, pour la bande (« stéréo », « 32 sorties »). */
export function outputsLabel(ins) {
  const n = (ins?.aux_ports?.length ?? 0) + 1;
  return n > 1 ? `${n} sorties` : "stéréo";
}

export class PluginView {
  /**
   * @param {{ state: object, client: object, root: HTMLElement, docked?: boolean, onTarget?: (insert: number|null) => void }} o
   * `state.project` est tenu à jour par l'appelant (qui appelle `refresh()` à chaque événement `project`).
   */
  constructor({ state, client, root, docked = false, onTarget = null }) {
    this.state = state;
    this.client = client;
    this.docked = docked;
    this.onTarget = onTarget;
    this.node = null;
    this.insert = null; // le plugin affiché (null : emplacement vide, on propose le sélecteur)
    this.view = "params"; // "native" | "params" | "outputs" | "picker"
    this.catalog = null;
    this.scanning = false;
    this.showAll = false;
    this.paramSearch = "";
    this.catalogSearch = "";
    this.params = new Map(); // insert → liste de paramètres
    this.pending = new Map(); // insert|param → { timer, value } pour limiter le débit des curseurs
    this.bank = new Map(); // identifiant CLAP → { plugin_id, name, params [{ id, name }] } : la banque personnelle de pages de contrôle
    this.bankError = null;
    this.pluginIds = new Map(); // insert → identifiant CLAP du plugin (donné par `insert_page`)
    this.root = root;
    root.classList.add("pv");
    root.innerHTML = `
      <div class="pv-strip">
        <button type="button" class="pv-byp" aria-label="Contourner" title="Contourner / réactiver le plugin">⏻</button>
        <button type="button" class="pv-name"></button>
        <span class="pv-meta"></span>
        <button type="button" class="pv-outs" title="Sorties du plugin"></button>
        <div class="pv-toggle" role="group" aria-label="Affichage">
          <button type="button" data-view="native">Interface</button>
          <button type="button" data-view="params">Paramètres</button>
        </div>
      </div>
      <div class="pv-body">
        <div class="pv-native"></div>
        <div class="pv-params"><input type="search" placeholder="Chercher un paramètre…" aria-label="Chercher un paramètre"><div class="list"></div></div>
        <div class="pv-outputs"></div>
        <div class="pv-picker"><input type="search" placeholder="Chercher un plugin…" aria-label="Chercher un plugin"><label class="allbox"><input type="checkbox" class="all"> Tout afficher</label><div class="list"></div></div>
      </div>`;
    this.$ = (sel) => root.querySelector(sel);
    this.$(".pv-byp").addEventListener("click", () => {
      const ins = this.ins();
      if (ins) this.client.send({ type: "set_plugin_bypass", insert: ins.id, bypass: !ins.bypass });
    });
    this.$(".pv-name").addEventListener("click", () => {
      if (!this.ins()) this.show("picker");
    });
    this.$(".pv-outs").addEventListener("click", () => this.show(this.view === "outputs" ? "params" : "outputs"));
    for (const b of root.querySelectorAll(".pv-toggle button")) b.addEventListener("click", () => this.show(b.dataset.view));
    this.#subscribe();
  }

  #subscribe() {
    const c = this.client;
    c.on("control_pages", (e) => {
      this.bank = new Map((e.entries ?? []).map((x) => [x.plugin_id, x]));
      this.bankError = e.error ?? null;
      if (this.view === "params") this.renderParams();
    });
    c.on("insert_page", (e) => {
      this.pluginIds.set(e.insert, e.plugin_id);
      if (this.insert === e.insert && this.view === "params") this.renderParams();
    });
    c.on("plugins", (e) => {
      this.scanning = e.scanning;
      if (!e.scanning || e.plugins.length) this.catalog = e.plugins;
      if (this.view === "picker") this.renderPicker();
    });
    c.on("plugin_params", (e) => {
      this.params.set(e.insert, e.params);
      if (this.insert === e.insert && this.view === "params") this.renderParams();
    });
    c.on("plugin_param", (e) => {
      const p = this.params.get(e.insert)?.find((x) => x.id === e.param);
      if (p) {
        p.value = e.value;
        p.text = e.text;
      }
      const out = this.$(`[data-param="${e.param}"] output`);
      if (out && this.insert === e.insert) out.textContent = e.text;
    });
    // Texte d'une valeur automatisée, tel que le plugin l'affiche (réponse à `get_plugin_param_text`).
    c.on("plugin_param_text", (e) => {
      const out = this.$(`[data-param="${e.param}"] output`);
      if (out && this.insert === e.insert) out.textContent = e.text;
    });
  }

  nodeInfo() {
    return this.state.project?.nodes.find((n) => n.id === this.node);
  }

  chain() {
    return this.nodeInfo()?.inserts ?? [];
  }

  ins() {
    return this.chain().find((i) => i.id === this.insert) ?? null;
  }

  /** Choisit ce qu'on affiche : le plugin `insert` du nœud `node` (`insert` nul : emplacement vide, sélecteur proposé). */
  setTarget(node, insert) {
    this.node = node;
    this.insert = insert;
    this.paramSearch = "";
    this.view = insert === null ? "picker" : this.ins()?.gui && this.docked ? "native" : "params";
    if (this.view === "picker" && this.catalog === null) this.client.send({ type: "list_plugins" });
    if (insert !== null) this.#fetch(insert);
    this.#sendView();
    this.render();
    this.onTarget?.(insert);
  }

  #fetch(insert) {
    this.client.send({ type: "get_plugin_params", insert });
    this.client.send({ type: "get_insert_page", insert });
  }

  /** Change la vue. Dans la fenêtre de l'application, l'hôte montre ou cache la zone de l'interface native. */
  show(view) {
    if (view === "native" && !this.ins()?.gui) return;
    if (view === "picker" && this.catalog === null) this.client.send({ type: "list_plugins" });
    this.view = view;
    this.#sendView();
    this.render();
  }

  #sendView() {
    if (this.docked && this.insert !== null) this.client.send({ type: "set_plugin_view", insert: this.insert, view: this.view === "native" ? "native" : "web" });
  }

  /** Le projet a changé : la bande suit (un plugin qui disparaît laisse un emplacement vide ; un plugin tout juste ajouté s'affiche). */
  refresh() {
    if (this.node === null) return;
    const w = this.openOnAdd;
    if (w) {
      const fresh = this.chain().find((i) => !w.before.has(i.id));
      if (fresh) {
        this.openOnAdd = null;
        this.setTarget(w.node, fresh.id);
        // Un plugin tout juste ajouté ouvre aussitôt sa fenêtre (l'application) ou son interface (s'il en a une).
        if (!fresh.ui_open && !this.docked && (fresh.gui || this.state.project?.plugin_windows)) this.client.send({ type: "open_plugin_ui", insert: fresh.id });
        return;
      }
    }
    if (this.insert !== null && !this.ins()) {
      this.insert = null;
      this.view = "picker";
    }
    this.render();
  }

  render() {
    const ins = this.ins();
    const byp = this.$(".pv-byp");
    byp.disabled = !ins;
    byp.setAttribute("aria-pressed", String(!!ins?.bypass));
    const name = this.$(".pv-name");
    name.textContent = ins ? ins.name : "Choisir un plugin…";
    name.classList.toggle("empty", !ins);
    name.disabled = !!ins;
    name.title = ins ? ins.name : "Ouvrir le sélecteur de plugins";
    this.$(".pv-meta").textContent = ins ? [ins.vendor, ins.latency ? `${ins.latency} éch.` : ""].filter(Boolean).join(" · ") : "";
    const outs = this.$(".pv-outs");
    outs.hidden = !ins;
    outs.textContent = ins ? outputsLabel(ins) : "";
    const multi = !!ins?.aux_ports?.length;
    outs.disabled = !multi;
    outs.setAttribute("aria-pressed", String(this.view === "outputs"));
    outs.title = multi ? "Brancher les sorties multiples du plugin sur des bus" : "Ce plugin n'a qu'une sortie stéréo";
    const native = this.$('.pv-toggle [data-view="native"]');
    native.disabled = !ins?.gui;
    native.title = ins?.gui ? "Interface du plugin" : "Ce plugin n'a pas d'interface native ici";
    for (const b of this.root.querySelectorAll(".pv-toggle button")) b.setAttribute("aria-pressed", String(b.dataset.view === this.view));
    this.root.dataset.view = this.view;
    this.renderNative();
    if (this.view === "params") this.renderParams();
    if (this.view === "outputs") this.renderOutputs();
    if (this.view === "picker") this.renderPicker();
  }

  /** Zone de l'interface native : dans la fenêtre de l'application elle est vide (l'application y loge l'interface) ; ailleurs, un renvoi. */
  renderNative() {
    const box = this.$(".pv-native");
    box.replaceChildren();
    if (this.docked) return;
    const ins = this.ins();
    const p = document.createElement("p");
    p.className = "empty";
    p.textContent = "L'interface du plugin s'ouvre dans sa propre fenêtre.";
    box.append(p);
    if (ins?.gui) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "pv-open";
      b.textContent = ins.ui_open ? "Fermer l'interface" : "Ouvrir l'interface";
      b.addEventListener("click", () => this.client.send({ type: ins.ui_open ? "close_plugin_ui" : "open_plugin_ui", insert: ins.id }));
      box.append(b);
    }
  }

  renderPicker() {
    const box = this.$(".pv-picker .list");
    const search = this.$(".pv-picker input[type=search]");
    const all = this.$(".pv-picker .all");
    all.checked = this.showAll;
    all.onchange = () => {
      this.showAll = all.checked;
      this.renderPicker();
    };
    search.oninput = () => {
      this.catalogSearch = search.value;
      this.renderPicker();
    };
    box.replaceChildren();
    if (this.catalog === null || (this.scanning && !this.catalog.length)) {
      const p = document.createElement("p");
      p.className = "empty";
      p.textContent = "Recherche des plugins installés…";
      box.append(p);
      return;
    }
    const q = norm(this.catalogSearch);
    const items = this.catalog.filter((p) => (this.showAll || p.vendor === ONLY_MINE) && (!q || norm(`${p.name} ${p.vendor}`).includes(q)));
    for (const p of items) {
      const b = document.createElement("button");
      b.className = "plug";
      b.innerHTML = `<span class="nm"></span><span class="vd"></span>`;
      b.querySelector(".nm").textContent = p.name;
      b.querySelector(".vd").textContent = [p.vendor, p.features?.includes("instrument") ? "instrument" : ""].filter(Boolean).join(" · ");
      b.title = p.description || p.name;
      b.addEventListener("click", () => {
        this.openOnAdd = { node: this.node, before: new Set(this.chain().map((i) => i.id)) };
        this.client.send({ type: "add_plugin", node: this.node, clap_id: p.clap_id, file: p.file });
        this.view = this.insert === null ? "params" : this.view;
      });
      box.append(b);
    }
    if (!items.length) {
      const p = document.createElement("p");
      p.className = "empty";
      p.textContent = this.showAll ? "Aucun plugin ne correspond." : "Aucun de tes plugins ne correspond. Coche « Tout afficher » pour voir les autres.";
      box.append(p);
    }
  }

  /** Chaîne latérale du plugin : la piste ou le bus dont le signal (avant son fader) alimente son second port d'entrée. */
  sidechainRow(ins) {
    const row = document.createElement("label");
    row.className = "sc-row";
    row.title = "Le signal de cette piste ou de ce bus (avant son fader) commande le plugin : par exemple un compresseur qui réagit à la grosse caisse. La piste source n'a pas besoin d'être entendue.";
    const text = document.createElement("span");
    text.textContent = "Chaîne latérale";
    const sel = document.createElement("select");
    sel.setAttribute("aria-label", "Source de la chaîne latérale");
    sel.append(new Option("Aucune (signal interne)", ""));
    for (const n of sidechainSources(this.state.project, this.node)) sel.append(new Option(n.name, String(n.id)));
    sel.value = ins.sidechain == null ? "" : String(ins.sidechain);
    sel.addEventListener("change", () => this.client.send({ type: "set_insert_sidechain", insert: ins.id, source: sel.value === "" ? null : Number(sel.value) }));
    row.append(text, sel);
    return row;
  }

  /** Sorties multiples du plugin : un sélecteur par port de sortie auxiliaire (un bus, la sortie principale, ou rien). */
  renderOutputs() {
    const box = this.$(".pv-outputs");
    box.replaceChildren();
    const ins = this.ins();
    if (!ins?.aux_ports?.length) return;
    const title = document.createElement("div");
    title.className = "aux-title";
    title.textContent = `Sorties multiples (${ins.aux_ports.length})`;
    title.title = "Chaque sortie audio supplémentaire du plugin peut aller sur un bus (ou la sortie principale), comme un envoi avant le fader de niveau 1. Une sortie non branchée n'est pas entendue ; la première sortie suit la piste comme d'habitude.";
    box.append(title);
    const list = document.createElement("div");
    list.className = "aux-list";
    const dests = this.state.project.nodes.filter((n) => (n.kind === "bus" || n.kind === "master") && n.id !== this.node);
    ins.aux_ports.forEach((port, k) => {
      const row = document.createElement("label");
      row.className = "aux-row";
      const text = document.createElement("span");
      text.textContent = port.name;
      const sel = document.createElement("select");
      sel.setAttribute("aria-label", `Destination de la sortie « ${port.name} »`);
      sel.append(new Option("Non branchée", ""));
      for (const n of dests) sel.append(new Option(n.kind === "master" ? "Sortie principale" : n.name, String(n.id)));
      sel.value = port.to == null ? "" : String(port.to);
      sel.addEventListener("change", () => this.client.send({ type: "set_plugin_aux", insert: ins.id, port: k, to: sel.value === "" ? null : Number(sel.value) }));
      row.append(text, sel);
      list.append(row);
    });
    box.append(list);
  }

  /** Définit la page de contrôle du plugin dans la banque personnelle (valable pour tous les projets). */
  #savePage(ins, pid, params) {
    this.client.send({ type: "set_control_page", plugin_id: pid, name: ins.name, params });
  }

  /** La page de contrôle du plugin : ses paramètres dans l'ordre, qu'on peut monter, descendre ou retirer. */
  pageBlock(ins, pid) {
    const page = this.bank.get(pid)?.params ?? [];
    const exposed = this.params.get(ins.id) ?? [];
    const det = document.createElement("details");
    det.className = "ppage";
    det.open = this.pageOpen ?? page.length > 0;
    det.addEventListener("toggle", () => (this.pageOpen = det.open));
    const sum = document.createElement("summary");
    sum.textContent = `Page de contrôle (${page.length}/${MAX_PAGE})`;
    sum.title = "Les paramètres que la tablette ou la surface de contrôle montre pour ce plugin, huit par huit. Enregistrée sur cet ordinateur : elle sert à tous les projets.";
    det.append(sum);
    if (this.bankError) {
      const e = document.createElement("p");
      e.className = "empty";
      e.textContent = this.bankError;
      det.append(e);
    }
    if (!page.length) {
      const e = document.createElement("p");
      e.className = "empty";
      e.textContent = "Aucun paramètre choisi : touche ☆ devant un paramètre ci-dessous pour l'ajouter.";
      det.append(e);
    }
    page.forEach((p, i) => {
      const row = document.createElement("div");
      row.className = "ppage-row";
      const known = exposed.some((x) => x.id === p.id);
      const num = document.createElement("span");
      num.className = "ppage-n";
      num.textContent = String(i + 1);
      const name = document.createElement("span");
      name.className = "ppage-name";
      name.textContent = p.name || `#${p.id}`;
      if (exposed.length && !known) name.title = "Ce plugin n'expose plus ce paramètre (il sera retrouvé par son nom s'il a changé d'identifiant)";
      const mk = (label, title, fn, disabled) => {
        const b = document.createElement("button");
        b.type = "button";
        b.textContent = label;
        b.title = title;
        b.disabled = !!disabled;
        b.addEventListener("click", fn);
        return b;
      };
      const swap = (j) => {
        const next = [...page];
        [next[i], next[j]] = [next[j], next[i]];
        this.#savePage(ins, pid, next);
      };
      row.append(num, name, mk("↑", "Monter", () => swap(i - 1), i === 0), mk("↓", "Descendre", () => swap(i + 1), i === page.length - 1), mk("✕", "Retirer de la page", () => this.#savePage(ins, pid, page.filter((_, k) => k !== i))));
      det.append(row);
    });
    return det;
  }

  renderParams() {
    const box = this.$(".pv-params .list");
    const search = this.$(".pv-params input[type=search]");
    const ins = this.ins();
    box.replaceChildren();
    search.hidden = !ins;
    if (!ins) {
      const p = document.createElement("p");
      p.className = "empty";
      p.textContent = "Aucun plugin : touche le nom ci-dessus pour en choisir un.";
      box.append(p);
      return;
    }
    if (ins.has_sidechain) box.append(this.sidechainRow(ins));
    const pid = this.pluginIds.get(ins.id);
    if (pid) box.append(this.pageBlock(ins, pid));
    search.value = this.paramSearch;
    search.oninput = () => {
      this.paramSearch = search.value;
      this.renderParams();
      this.$(".pv-params input[type=search]").focus();
    };
    const list = this.params.get(ins.id);
    if (!list) {
      const p = document.createElement("p");
      p.className = "empty";
      p.textContent = "Lecture des paramètres…";
      box.append(p);
      return;
    }
    const q = norm(this.paramSearch);
    const shown = list.filter((p) => !q || norm(`${p.name} ${p.module}`).includes(q));
    for (const p of shown.slice(0, MAX_ROWS)) {
      const row = document.createElement("label");
      row.className = "param";
      row.dataset.param = String(p.id);
      const span = p.max - p.min;
      row.innerHTML = `<button type="button" class="pstar" aria-label="Page de contrôle"></button><span class="pn"></span><input type="range" min="${p.min}" max="${p.max}" step="${span > 0 ? span / 1000 : 1}" value="${p.value}"><output></output>`;
      row.querySelector(".pn").textContent = p.module ? `${p.name} · ${p.module}` : p.name;
      const star = row.querySelector(".pstar");
      const page = pid ? (this.bank.get(pid)?.params ?? []) : [];
      const inPage = page.some((x) => x.id === p.id);
      star.textContent = inPage ? "★" : "☆";
      star.setAttribute("aria-pressed", String(inPage));
      star.title = inPage ? "Retirer de la page de contrôle" : page.length >= MAX_PAGE ? `La page de contrôle est pleine (${MAX_PAGE})` : "Ajouter à la page de contrôle (tablette, surface)";
      star.disabled = !pid || (!inPage && page.length >= MAX_PAGE);
      star.addEventListener("click", (e) => {
        e.preventDefault();
        this.#savePage(ins, pid, inPage ? page.filter((x) => x.id !== p.id) : [...page, { id: p.id, name: p.name }]);
      });
      row.querySelector("output").textContent = p.text;
      const range = row.querySelector("input");
      trackGesture(range, this.client, () => ({ node: this.node, target: { kind: "insert", insert: ins.id, param: p.id } }));
      range.addEventListener("input", () => {
        p.value = Number(range.value);
        this.sendParam(ins.id, p.id, p.value);
      });
      range.addEventListener("dblclick", () => {
        range.value = String(p.default);
        p.value = p.default;
        this.sendParam(ins.id, p.id, p.default);
      });
      box.append(row);
    }
    if (shown.length > MAX_ROWS) {
      const more = document.createElement("p");
      more.className = "empty";
      more.textContent = `${shown.length - MAX_ROWS} autres paramètres : affine la recherche.`;
      box.append(more);
    }
    if (!shown.length) {
      const none = document.createElement("p");
      none.className = "empty";
      none.textContent = list.length ? "Aucun paramètre ne correspond." : "Ce plugin n'expose aucun paramètre.";
      box.append(none);
    }
  }

  /**
   * Appelé quand la position de lecture change : les réglages qui ont une courbe d'automation (mode lecture ou trim ; un plugin ne
   * reçoit que la courbe) suivent la courbe à la position de lecture — curseur et texte. Le texte vient du plugin lui-même, demandé
   * au plus toutes les 120 ms par paramètre.
   */
  follow() {
    if (this.insert === null || this.view !== "params" || !this.state.tempo) return;
    const lanes = this.nodeInfo()?.automation?.filter((l) => l.target.kind === "insert" && l.target.insert === this.insert && l.mode !== "off") ?? [];
    if (!lanes.length) return;
    const whole = this.state.tempo.wholeAt(Math.max(0, this.state.pos));
    const now = performance.now();
    for (const lane of lanes) {
      const row = this.$(`[data-param="${lane.target.param}"]`);
      const value = valueAtWhole(lane.points, whole);
      if (!row || value === null) continue;
      const range = row.querySelector("input");
      row.classList.add("automated");
      range.value = String(value);
      const key = `${this.insert}|${lane.target.param}`;
      const last = this.autoAsk?.get(key);
      if (!last || (now - last.t > 120 && Math.abs(value - last.value) > 1e-9)) {
        (this.autoAsk ??= new Map()).set(key, { t: now, value });
        this.client.send({ type: "get_plugin_param_text", insert: this.insert, param: lane.target.param, value });
      }
    }
  }

  /** Envoie un réglage en limitant le débit (un curseur produit beaucoup d'événements) : le premier tout de suite, puis au plus un tous les 40 ms. */
  sendParam(insert, param, value) {
    const key = `${insert}|${param}`;
    const slot = this.pending.get(key);
    if (slot) {
      slot.value = value;
      slot.dirty = true;
      return;
    }
    this.client.send({ type: "set_plugin_param", insert, param, value });
    const entry = { dirty: false, value, timer: null };
    entry.timer = setTimeout(() => {
      this.pending.delete(key);
      if (entry.dirty) this.sendParam(insert, param, entry.value);
    }, 40);
    this.pending.set(key, entry);
  }
}
