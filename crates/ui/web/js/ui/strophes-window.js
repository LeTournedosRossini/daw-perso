// Fenêtre « Texte des paroles » : on écrit ou on colle tout le texte, strophe par strophe, avec des marqueurs « [Couplet 1] », « [Refrain] ».
// Aucune mélodie n'est nécessaire. Chaque strophe est une carte colorée liée aux cartes de la structure (par son nom, ou à la main) :
// un « Refrain » couvre tous les refrains du morceau. « Appliquer » enregistre le texte (une action annulable) ; le placer sur des notes
// se fait ensuite dans la partition (réserve de paroles, choix « Strophe »).

import { CATEGORIES, CATEGORY_LABELS, sectionColor } from "../section-model.js";
import { parseStrophesText, strophesToText, withMeta, sectionsFor, uncoveredSections, strophesCommand, layStrophe } from "../strophes-model.js";
import { fnum, fcmp } from "../notes-model.js";
import { noteOnsets } from "../lyrics-model.js";

const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

const EXAMPLE = "[Couplet 1]\nMa maman est la plus belle\nElle chante dans la cuisine\n\n[Refrain]\nLa la la la la\n\n[Couplet 2]\nMon papa joue de la guitare";

export class StrophesWindow {
  constructor({ state, client, say }) {
    this.state = state;
    this.client = client;
    this.say = say;
    this.dlg = null;
    this.draft = [];
    client.on("chords", () => this.dlg && this.#renderCards());
  }

  open() {
    if (this.dlg) return this.dlg.querySelector("textarea")?.focus();
    this.draft = structuredClone(this.state.strophes ?? []);
    const dlg = h("dialog", "strophes-dialog");
    dlg.setAttribute("aria-label", "Texte des paroles");
    const form = h("div", "sd-body");
    const top = h("header", "sd-top");
    const x = h("button", "close", "✕");
    x.type = "button";
    x.setAttribute("aria-label", "Fermer");
    x.addEventListener("click", () => this.close());
    top.append(x, h("h2", "", "Texte des paroles"));
    form.append(top);
    form.append(h("p", "sd-help", "Écris ou colle le texte : une ligne « [Couplet 1] », « [Refrain] »… commence une strophe, chaque ligne qui suit est une phrase. Pas besoin de mélodie. Les cartes de droite se lient aux cartes de la structure."));
    const cols = h("div", "sd-cols");
    const ta = h("textarea", "sd-text");
    ta.setAttribute("aria-label", "Texte des paroles");
    ta.spellcheck = false;
    ta.rows = 16;
    ta.placeholder = EXAMPLE;
    ta.value = strophesToText(this.draft);
    ta.addEventListener("input", () => {
      this.draft = withMeta(parseStrophesText(ta.value), this.draft);
      this.#renderCards();
    });
    this.ta = ta;
    this.cards = h("div", "sd-cards");
    cols.append(ta, this.cards);
    this.foot = h("div", "sd-foot");
    const lay = h("div", "sd-lay");
    lay.append(h("span", "", "Poser sur la mélodie de la piste :"));
    this.track = h("select");
    this.track.setAttribute("aria-label", "Piste de la mélodie");
    this.track.addEventListener("change", () => (this.trackId = this.track.value));
    const all = h("button", "", "Poser tout sur les notes");
    all.title = "Pose chaque strophe sur les notes de la piste, dans chacune des cartes de la structure qui lui correspondent";
    all.addEventListener("click", () => this.#lay(null));
    lay.append(this.track, all);
    const btns = h("div", "sd-buttons");
    const apply = h("button", "", "Appliquer");
    apply.addEventListener("click", () => this.#apply());
    const example = h("button", "", "Exemple");
    example.title = "Remplit le texte avec un modèle (couplets et refrain)";
    example.addEventListener("click", () => {
      ta.value = EXAMPLE;
      ta.dispatchEvent(new Event("input"));
    });
    const close = h("button", "", "Fermer");
    close.addEventListener("click", () => this.close());
    btns.append(example, close, apply);
    form.append(cols, this.foot, lay, btns);
    dlg.append(form);
    dlg.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Escape") this.close();
    });
    dlg.addEventListener("cancel", (e) => (e.preventDefault(), this.close()));
    document.body.append(dlg);
    this.dlg = dlg;
    dlg.show();
    this.#fillTracks();
    this.#renderCards();
    ta.focus();
  }

  close() {
    this.dlg?.close?.();
    this.dlg?.remove();
    this.dlg = null;
  }

  #barText(sec) {
    const m = this.state.meters_;
    try {
      return m ? `mes. ${m.locate(fnum(sec.at)).bar + 1}` : "";
    } catch {
      return "";
    }
  }

  #renderCards() {
    const structure = this.state.structure ?? [];
    this.cards.replaceChildren();
    if (!this.draft.length) this.cards.append(h("p", "sd-empty", "Aucune strophe pour l'instant : écris le texte à gauche."));
    const names = [...new Set(structure.map((s) => s.name))];
    this.draft.forEach((st, i) => {
      const color = sectionColor({ name: st.name, category: st.category });
      const card = h("div", "sd-card");
      card.style.setProperty("--sc", color);
      const head = h("div", "sd-card-head");
      head.append(h("strong", "", st.name), h("span", "sd-count", `${st.lines.length} ligne${st.lines.length > 1 ? "s" : ""}`));
      const cat = h("select");
      cat.setAttribute("aria-label", `Catégorie de « ${st.name} »`);
      for (const c of CATEGORIES) cat.append(new Option(CATEGORY_LABELS[c] ?? c, c));
      cat.value = st.category ?? "custom";
      cat.addEventListener("change", () => ((st.category = cat.value), this.#renderCards()));
      const link = h("select");
      link.setAttribute("aria-label", `Carte liée à « ${st.name} »`);
      link.append(new Option("Auto (d'après le nom)", "__auto"), new Option("Aucune carte", "__none"));
      for (const n of names) link.append(new Option(`Carte « ${n} »`, n));
      link.value = st.link === undefined ? "__auto" : st.link === "" ? "__none" : names.includes(st.link) ? st.link : "__auto";
      link.addEventListener("change", () => {
        if (link.value === "__auto") delete st.link;
        else st.link = link.value === "__none" ? "" : link.value;
        this.#renderCards();
      });
      const secs = sectionsFor(st, structure);
      const info = h("div", "sd-link", secs.length ? `→ ${secs.map((s) => `${s.name}${this.#barText(s) ? ` (${this.#barText(s)})` : ""}`).join(", ")}` : structure.length ? "Aucune carte de la structure ne correspond." : "Pas encore de structure : les cartes se poseront dans la voie « Structure ».");
      info.classList.toggle("sd-none", !secs.length);
      const lines = h("div", "sd-lines", st.lines.slice(0, 4).join(" / ") + (st.lines.length > 4 ? " …" : ""));
      const one = h("button", "sd-lay-one", "Poser sur les notes");
      one.disabled = !secs.length;
      one.title = secs.length ? "Pose cette strophe sur les notes de la piste choisie, dans chaque carte liée" : "Aucune carte liée : rien à poser";
      one.addEventListener("click", () => this.#lay(i));
      card.append(head, h("div", "sd-ctl"), info, lines, one);
      card.querySelector(".sd-ctl").append(cat, link);
      this.cards.append(card);
    });
    const loose = uncoveredSections(this.draft, structure);
    this.foot.textContent = loose.length && this.draft.length ? `Cartes de la structure sans texte : ${[...new Set(loose.map((s) => s.name))].join(", ")}.` : "";
  }

  /** Les pistes qui portent des clips de notes (la mélodie chantée). */
  #fillTracks() {
    const nodes = (this.state.project?.nodes ?? []).filter((n) => (n.clips ?? []).some((c) => c.kind !== "audio"));
    this.track.replaceChildren(...nodes.map((n) => new Option(n.name, String(n.id))));
    if (!nodes.length) this.track.append(new Option("(aucune piste de notes)", ""));
    this.trackId = this.trackId && nodes.some((n) => String(n.id) === this.trackId) ? this.trackId : this.track.value;
    this.track.value = this.trackId ?? "";
  }

  /** Les instants de toutes les notes des clips de la piste (union triée, sans doublon). */
  async #onsetsOf(nodeId) {
    const node = this.state.project.nodes.find((n) => String(n.id) === String(nodeId));
    const clips = (node?.clips ?? []).filter((c) => c.kind !== "audio");
    const all = [];
    for (const c of clips) {
      const data = await new Promise((resolve) => {
        const off = this.client.on("notes", (e) => {
          if (e.clip !== c.id) return;
          off();
          resolve(e);
        });
        setTimeout(() => (off(), resolve(null)), 3000);
        this.client.send({ type: "get_notes", clip: c.id });
      });
      if (data) for (const o of noteOnsets(data)) if (!all.some((x) => fcmp(x, o) === 0)) all.push(o);
    }
    return all.sort(fcmp);
  }

  /** Pose une strophe (`index`) ou toutes (`null`) sur les notes de la piste choisie : le texte est d'abord enregistré, puis les syllabes (une action annulable). */
  async #lay(index) {
    if (!this.trackId) return this.say("Choisis d'abord une piste qui porte la mélodie (un clip de notes).");
    this.draft = withMeta(parseStrophesText(this.ta.value), this.draft);
    const structure = this.state.structure ?? [];
    const targets = index === null ? this.draft.map((_, i) => i) : [index];
    const onsets = await this.#onsetsOf(this.trackId);
    if (!onsets.length) return this.say("La piste choisie n'a aucune note.");
    this.client.send(strophesCommand(this.draft, "Écrire le texte des paroles"));
    let list = this.state.lyrics ?? [];
    const lines = [];
    for (const i of targets) {
      const st = this.draft[i];
      const secs = sectionsFor(st, structure);
      if (!secs.length) continue;
      const r = layStrophe(st, secs, onsets, list);
      list = r.list;
      const placed = r.report.reduce((a, x) => a + x.placed, 0);
      const extra = r.report.reduce((a, x) => a + x.extra, 0);
      lines.push(`${st.name} : ${placed} syllabe(s) sur ${secs.length} carte(s)${extra ? `, ${extra} sans note` : ""}`);
    }
    if (!lines.length) return this.say("Aucune strophe n'a de carte de la structure à laquelle se poser.");
    this.client.send({ type: "set_lyrics", syllables: list.map((x) => ({ at: x.at, text: x.text, hyphen: !!x.hyphen, verse: x.verse ?? 1, ...(x.end ? { end: x.end } : {}) })), label: "Poser les paroles sur les notes" });
    this.say(lines.join(" · "));
  }

  #apply() {
    this.draft = withMeta(parseStrophesText(this.ta.value), this.draft);
    const names = this.draft.map((s) => s.name.toLowerCase());
    if (new Set(names).size !== names.length) this.say("Deux strophes portent le même nom : renomme-les (« Couplet 1 », « Couplet 2 »…) pour les distinguer.");
    this.client.send(strophesCommand(this.draft, this.state.strophes?.length ? "Modifier le texte des paroles" : "Écrire le texte des paroles"));
    this.close();
  }
}
