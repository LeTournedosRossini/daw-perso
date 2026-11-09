// Fenêtre « Plugins de la piste » : la chaîne de plugins (contourner, monter, descendre, retirer), le sélecteur de plugins
// installés et les réglages du plugin choisi (liste de paramètres avec recherche). Panneau flottant non modal : on peut
// lancer la lecture et régler en écoutant. Chaque geste est une commande du protocole.

import { valueAtWhole } from "../automation-model.js";
import { trackGesture } from "./gesture.js";

const ONLY_MINE = "Le Tournedos Rossini";
const MAX_PAGE = 16; // paramètres d'une page de contrôle (deux groupes de huit encodeurs)
const MAX_ROWS = 80; // paramètres affichés à la fois (un plugin peut en avoir des milliers : on affine par la recherche)

const norm = (t) => t.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase();

export class PluginsPanel {
  constructor(state, client) {
    this.state = state;
    this.client = client;
    this.root = document.getElementById("plugins-panel");
    this.node = null; // identifiant du nœud affiché
    this.selected = null; // identifiant du plugin dont on règle les paramètres
    this.catalog = null; // liste des plugins installés (null : pas encore demandée)
    this.scanning = false;
    this.showAll = false;
    this.choosing = false;
    this.params = new Map(); // insert → liste de paramètres
    this.paramSearch = "";
    this.catalogSearch = "";
    this.pending = new Map(); // insert|param → { timer, value } pour limiter le débit des curseurs
    this.bank = new Map(); // identifiant CLAP → { plugin_id, name, params [{ id, name }] } : la banque personnelle de pages de contrôle
    this.bankError = null;
    this.pluginIds = new Map(); // insert → identifiant CLAP du plugin (donné par `insert_page`)
    client.on("control_pages", (e) => {
      this.bank = new Map((e.entries ?? []).map((x) => [x.plugin_id, x]));
      this.bankError = e.error ?? null;
      if (this.isOpen()) this.renderParams();
    });
    client.on("insert_page", (e) => {
      this.pluginIds.set(e.insert, e.plugin_id);
      if (this.isOpen() && this.selected === e.insert) this.renderParams();
    });
    client.on("plugins", (e) => {
      this.scanning = e.scanning;
      if (!e.scanning || e.plugins.length) this.catalog = e.plugins;
      if (this.isOpen()) this.render();
    });
    // Un plugin qu'on vient d'ajouter ouvre aussitôt sa propre interface (s'il en a une) ; sinon on reste sur sa liste de paramètres.
    client.on("project", (p) => {
      const w = this.openOnAdd;
      if (!w) return;
      this.openOnAdd = null;
      const fresh = (p?.nodes ?? []).find((n) => n.id === w.node)?.inserts?.find((i) => !w.before.has(i.id));
      if (fresh?.gui && !fresh.ui_open) this.client.send({ type: "open_plugin_ui", insert: fresh.id });
    });
    client.on("plugin_params", (e) => {
      this.params.set(e.insert, e.params);
      if (this.isOpen() && this.selected === e.insert) this.renderParams();
    });
    client.on("plugin_param", (e) => {
      const list = this.params.get(e.insert);
      const p = list?.find((x) => x.id === e.param);
      if (p) {
        p.value = e.value;
        p.text = e.text;
      }
      const out = this.root.querySelector(`[data-param="${e.param}"] output`);
      if (out && this.selected === e.insert) out.textContent = e.text;
    });
    // Texte d'une valeur automatisée, tel que le plugin l'affiche (réponse à `get_plugin_param_text`).
    client.on("plugin_param_text", (e) => {
      const out = this.root.querySelector(`[data-param="${e.param}"] output`);
      if (out && this.selected === e.insert) out.textContent = e.text;
    });
    this.root.querySelector(".close").addEventListener("click", () => this.close());
    this.root.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.close();
    });
  }

  isOpen() {
    return !this.root.hidden && this.node !== null;
  }

  open(nodeId) {
    this.node = nodeId;
    this.choosing = false;
    const chain = this.chain();
    this.selected = chain.some((i) => i.id === this.selected) ? this.selected : (chain[0]?.id ?? null);
    this.root.hidden = false;
    this.render();
    if (this.selected !== null) this.#fetch(this.selected);
  }

  /** Les paramètres d'un plugin et son identifiant CLAP (pour retrouver sa page de contrôle dans la banque). */
  #fetch(insert) {
    this.client.send({ type: "get_plugin_params", insert });
    this.client.send({ type: "get_insert_page", insert });
  }

  close() {
    this.root.hidden = true;
    this.node = null;
  }

  nodeInfo() {
    return this.state.project.nodes.find((n) => n.id === this.node);
  }

  chain() {
    return this.nodeInfo()?.inserts ?? [];
  }

  /** Le projet a changé : la chaîne affichée suit (ou la fenêtre se ferme si la piste a disparu). */
  refresh() {
    if (!this.isOpen()) return;
    if (!this.nodeInfo()) return this.close();
    const chain = this.chain();
    if (!chain.some((i) => i.id === this.selected)) {
      this.selected = chain[0]?.id ?? null;
      if (this.selected !== null) this.#fetch(this.selected);
    }
    this.render();
  }

  select(id) {
    this.selected = id;
    this.paramSearch = "";
    this.render();
    this.#fetch(id);
  }

  render() {
    const n = this.nodeInfo();
    if (!n) return;
    this.root.querySelector("h2").textContent = `Plugins de « ${n.name} »`;
    this.renderChain();
    this.renderParams();
    this.root.classList.toggle("choosing", this.choosing);
    if (this.choosing) this.renderChooser();
  }

  renderChain() {
    const chain = this.chain();
    const box = this.root.querySelector(".chain");
    box.replaceChildren();
    chain.forEach((ins, i) => {
      const row = document.createElement("div");
      row.className = `slot${ins.id === this.selected ? " selected" : ""}${ins.bypass ? " bypassed" : ""}`;
      row.innerHTML = `
        <button class="byp" title="Contourner / réactiver" aria-pressed="${ins.bypass}" aria-label="Contourner">⏻</button>
        <button class="pick" title="Régler ce plugin"><span class="nm"></span><span class="vd"></span></button>
        <button class="gui" title="${ins.gui ? "Ouvrir / fermer l'interface du plugin (fenêtre à part)" : "Ce plugin n'a pas d'interface native ici"}" aria-pressed="${!!ins.ui_open}" aria-label="Interface du plugin" ${ins.gui ? "" : "disabled"}>▣</button>
        <button class="up" title="Monter" aria-label="Monter" ${i === 0 ? "disabled" : ""}>↑</button>
        <button class="down" title="Descendre" aria-label="Descendre" ${i === chain.length - 1 ? "disabled" : ""}>↓</button>
        <button class="rm" title="Retirer le plugin" aria-label="Retirer">✕</button>`;
      row.querySelector(".nm").textContent = ins.name;
      row.querySelector(".vd").textContent = [ins.vendor, ins.latency ? `${ins.latency} éch.` : ""].filter(Boolean).join(" · ");
      row.querySelector(".byp").addEventListener("click", () => this.client.send({ type: "set_plugin_bypass", insert: ins.id, bypass: !ins.bypass }));
      row.querySelector(".pick").addEventListener("click", () => this.select(ins.id));
      row.querySelector(".gui").addEventListener("click", () => this.client.send({ type: ins.ui_open ? "close_plugin_ui" : "open_plugin_ui", insert: ins.id }));
      row.querySelector(".up").addEventListener("click", () => this.client.send({ type: "move_plugin", insert: ins.id, before: chain[i - 1].id }));
      row.querySelector(".down").addEventListener("click", () => this.client.send(chain[i + 2] ? { type: "move_plugin", insert: ins.id, before: chain[i + 2].id } : { type: "move_plugin", insert: ins.id }));
      row.querySelector(".rm").addEventListener("click", () => this.client.send({ type: "remove_plugin", insert: ins.id }));
      box.append(row);
    });
    if (!chain.length) {
      const empty = document.createElement("p");
      empty.className = "empty";
      empty.textContent = "Aucun plugin sur cette piste.";
      box.append(empty);
    }
    const add = this.root.querySelector(".add");
    add.onclick = () => {
      this.choosing = !this.choosing;
      if (this.choosing && this.catalog === null) this.client.send({ type: "list_plugins" });
      this.render();
    };
    add.textContent = this.choosing ? "← Retour aux réglages" : "＋ Ajouter un plugin…";
  }

  renderChooser() {
    const box = this.root.querySelector(".chooser .list");
    const search = this.root.querySelector(".chooser input[type=search]");
    const all = this.root.querySelector(".chooser .all");
    all.checked = this.showAll;
    all.onchange = () => {
      this.showAll = all.checked;
      this.renderChooser();
    };
    search.oninput = () => {
      this.catalogSearch = search.value;
      this.renderChooser();
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
        this.choosing = false;
        this.openOnAdd = { node: this.node, before: new Set(this.chain().map((i) => i.id)) };
        this.client.send({ type: "add_plugin", node: this.node, clap_id: p.clap_id, file: p.file });
        this.render();
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

  /** Chaîne latérale du plugin sélectionné : la piste ou le bus dont le signal (avant son fader) alimente son second port d'entrée. */
  sidechainRow(ins) {
    const row = document.createElement("label");
    row.className = "sc-row";
    row.title = "Le signal de cette piste ou de ce bus (avant son fader) commande le plugin : par exemple un compresseur qui réagit à la grosse caisse. La piste source n'a pas besoin d'être entendue.";
    const text = document.createElement("span");
    text.textContent = "Chaîne latérale";
    const sel = document.createElement("select");
    sel.setAttribute("aria-label", "Source de la chaîne latérale");
    const none = document.createElement("option");
    none.value = "";
    none.textContent = "Aucune (signal interne)";
    sel.append(none);
    for (const n of sidechainSources(this.state.project, this.node)) {
      const o = document.createElement("option");
      o.value = String(n.id);
      o.textContent = n.name;
      sel.append(o);
    }
    sel.value = ins.sidechain == null ? "" : String(ins.sidechain);
    sel.addEventListener("change", () => this.client.send({ type: "set_insert_sidechain", insert: ins.id, source: sel.value === "" ? null : Number(sel.value) }));
    row.append(text, sel);
    return row;
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
    const box = this.root.querySelector(".params .list");
    const title = this.root.querySelector(".params h3");
    const ins = this.chain().find((i) => i.id === this.selected);
    box.replaceChildren();
    const search = this.root.querySelector(".params input[type=search]");
    search.hidden = !ins;
    if (!ins) {
      title.textContent = "Réglages";
      const p = document.createElement("p");
      p.className = "empty";
      p.textContent = "Ajoute un plugin, puis choisis-le pour le régler.";
      box.append(p);
      return;
    }
    title.textContent = `Réglages : ${ins.name}`;
    if (ins.has_sidechain) box.append(this.sidechainRow(ins));
    const pid = this.pluginIds.get(ins.id);
    if (pid) box.append(this.pageBlock(ins, pid));
    search.value = this.paramSearch;
    search.oninput = () => {
      this.paramSearch = search.value;
      this.renderParams();
      this.root.querySelector(".params input[type=search]").focus();
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
    if (!this.isOpen() || this.selected === null || !this.state.tempo) return;
    const node = this.nodeInfo();
    const lanes = node?.automation.filter((l) => l.target.kind === "insert" && l.target.insert === this.selected && l.mode !== "off") ?? [];
    if (!lanes.length) return;
    const whole = this.state.tempo.wholeAt(Math.max(0, this.state.pos));
    const now = performance.now();
    for (const lane of lanes) {
      const row = this.root.querySelector(`[data-param="${lane.target.param}"]`);
      const value = valueAtWhole(lane.points, whole);
      if (!row || value === null) continue;
      const range = row.querySelector("input");
      row.classList.add("automated");
      range.value = String(value);
      const key = `${this.selected}|${lane.target.param}`;
      const last = this.autoAsk?.get(key);
      if (!last || (now - last.t > 120 && Math.abs(value - last.value) > 1e-9)) {
        (this.autoAsk ??= new Map()).set(key, { t: now, value });
        this.client.send({ type: "get_plugin_param_text", insert: this.selected, param: lane.target.param, value });
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

/** Les nœuds qu'on peut prendre pour chaîne latérale d'un plugin du nœud `node` : pistes et bus (pas la sortie principale, ni le nœud lui-même, ni un retour). */
export function sidechainSources(project, node) {
  return project.nodes.filter((n) => n.kind !== "master" && n.id !== node && !(n.kind === "bus" && n.phys));
}
