// Fenêtre « Plugins de la piste » : la chaîne de plugins (contourner, monter, descendre, retirer, ajouter) et, à côté, la vue du plugin
// choisi (`PluginView` : bande, paramètres, sorties, sélecteur). Dans l'application, un clic sur un plugin de la chaîne ouvre sa propre
// fenêtre (bande + interface native + paramètres) et la vue à côté disparaît. Panneau flottant non modal : on peut lancer la lecture et
// régler en écoutant. Chaque geste est une commande du protocole.

import { PluginView, sidechainSources } from "./plugin-view.js";

export { sidechainSources };

export class PluginsPanel {
  constructor(state, client) {
    this.state = state;
    this.client = client;
    this.root = document.getElementById("plugins-panel");
    this.node = null; // identifiant du nœud affiché
    this.view = new PluginView({ state, client, root: this.root.querySelector(".pv-host"), docked: false, onTarget: () => this.#layout() });
    this.root.querySelector(".close").addEventListener("click", () => this.close());
    this.root.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.close();
    });
  }

  isOpen() {
    return !this.root.hidden && this.node !== null;
  }

  /** L'application ouvre une fenêtre par plugin : la vue à côté de la chaîne devient inutile. */
  get windowed() {
    return !!this.state.project?.plugin_windows;
  }

  open(nodeId) {
    this.node = nodeId;
    const chain = this.chain();
    const keep = chain.find((i) => i.id === this.view.insert)?.id;
    this.root.hidden = false;
    this.view.setTarget(nodeId, keep ?? chain[0]?.id ?? null);
    this.render();
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
    this.view.refresh();
    this.render();
  }

  /** Dans l'application, les réglages d'un plugin ont leur propre fenêtre : à côté de la chaîne il ne reste que le sélecteur (emplacement vide). */
  #layout() {
    this.root.classList.toggle("windowed", this.windowed && this.view.view !== "picker");
    this.renderChain();
  }

  /** Appelé quand la position de lecture change : les réglages automatisés suivent leur courbe. */
  follow() {
    if (this.isOpen()) this.view.follow();
  }

  render() {
    const n = this.nodeInfo();
    if (!n) return;
    this.root.querySelector("h2").textContent = `Plugins de « ${n.name} »`;
    this.#layout();
  }

  select(ins) {
    if (this.windowed) {
      this.client.send({ type: "open_plugin_ui", insert: ins.id });
      return;
    }
    this.view.setTarget(this.node, ins.id);
  }

  renderChain() {
    const chain = this.chain();
    const box = this.root.querySelector(".chain");
    box.replaceChildren();
    chain.forEach((ins, i) => {
      const row = document.createElement("div");
      row.className = `slot${ins.id === this.view.insert ? " selected" : ""}${ins.bypass ? " bypassed" : ""}`;
      const guiTitle = this.windowed ? "Ouvrir / fermer la fenêtre du plugin" : ins.gui ? "Ouvrir / fermer l'interface du plugin (fenêtre à part)" : "Ce plugin n'a pas d'interface native ici";
      row.innerHTML = `
        <button class="byp" title="Contourner / réactiver" aria-pressed="${ins.bypass}" aria-label="Contourner">⏻</button>
        <button class="pick" title="${this.windowed ? "Ouvrir la fenêtre de ce plugin" : "Régler ce plugin"}"><span class="nm"></span><span class="vd"></span></button>
        <button class="gui" title="${guiTitle}" aria-pressed="${!!ins.ui_open}" aria-label="Fenêtre du plugin" ${ins.gui || this.windowed ? "" : "disabled"}>▣</button>
        <button class="up" title="Monter" aria-label="Monter" ${i === 0 ? "disabled" : ""}>↑</button>
        <button class="down" title="Descendre" aria-label="Descendre" ${i === chain.length - 1 ? "disabled" : ""}>↓</button>
        <button class="rm" title="Retirer le plugin" aria-label="Retirer">✕</button>`;
      row.querySelector(".nm").textContent = ins.name;
      row.querySelector(".vd").textContent = [ins.vendor, ins.latency ? `${ins.latency} éch.` : ""].filter(Boolean).join(" · ");
      row.querySelector(".byp").addEventListener("click", () => this.client.send({ type: "set_plugin_bypass", insert: ins.id, bypass: !ins.bypass }));
      row.querySelector(".pick").addEventListener("click", () => this.select(ins));
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
    // Emplacement vide : un champ à toucher pour ouvrir le sélecteur.
    const add = this.root.querySelector(".add");
    add.onclick = () => this.view.setTarget(this.node, null);
    add.textContent = "＋ Choisir un plugin…";
  }
}
