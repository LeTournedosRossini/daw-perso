// Fenêtre « Texte des paroles » : on écrit ou on colle tout le texte, strophe par strophe, avec des marqueurs « [Couplet 1] », « [Refrain] ».
// Aucune mélodie n'est nécessaire. Chaque strophe est une carte colorée liée aux cartes de la structure (par son nom, ou à la main) :
// un « Refrain » couvre tous les refrains du morceau. « Appliquer » enregistre le texte (une action annulable) ; le placer sur des notes
// se fait ensuite dans la partition (réserve de paroles, choix « Strophe »).

import { CATEGORIES, CATEGORY_LABELS, sectionColor } from "../section-model.js";
import { parseStrophesText, strophesToText, withMeta, sectionsFor, uncoveredSections, strophesCommand } from "../strophes-model.js";
import { fnum } from "../notes-model.js";

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
    form.append(h("h2", "", "Texte des paroles"));
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
    form.append(cols, this.foot, btns);
    dlg.append(form);
    dlg.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Escape") this.close();
    });
    dlg.addEventListener("cancel", (e) => (e.preventDefault(), this.close()));
    document.body.append(dlg);
    this.dlg = dlg;
    dlg.show();
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
      card.append(head, h("div", "sd-ctl"), info, lines);
      card.querySelector(".sd-ctl").append(cat, link);
      this.cards.append(card);
    });
    const loose = uncoveredSections(this.draft, structure);
    this.foot.textContent = loose.length && this.draft.length ? `Cartes de la structure sans texte : ${[...new Set(loose.map((s) => s.name))].join(", ")}.` : "";
  }

  #apply() {
    this.draft = withMeta(parseStrophesText(this.ta.value), this.draft);
    const names = this.draft.map((s) => s.name.toLowerCase());
    if (new Set(names).size !== names.length) this.say("Deux strophes portent le même nom : renomme-les (« Couplet 1 », « Couplet 2 »…) pour les distinguer.");
    this.client.send(strophesCommand(this.draft, this.state.strophes?.length ? "Modifier le texte des paroles" : "Écrire le texte des paroles"));
    this.close();
  }
}
