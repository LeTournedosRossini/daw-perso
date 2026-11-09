// Fenêtre de saisie d'un accord (comme dans Carte et Scorsaire) : trois colonnes — fondamentale, qualité, basse — avec le nom
// et les notes de l'accord en aperçu. La saisie au clavier reste possible dans le champ du haut (`Dm7`, `G7/B`, `N.C.`).
// Enter valide, Échap abandonne. L'écriture dièse / bémol est conservée : une touche noire a deux boutons.

import { ROOT_CELLS, chordNotes, draftFromText, prettyPitch, resolveQuality, textFromDraft } from "../chord-model.js";

const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

/**
 * @param {{catalog:{qualities:[string,number[]][],aliases:[string,string][]}|null, initial?:string, title:string,
 *   onOk:(text:string)=>void, onCancel?:()=>void}} o
 * @returns {{el:HTMLElement, close:()=>void}}
 */
export function createChordDialog(o) {
  const cat = o.catalog;
  const qualities = cat?.qualities ?? [];
  let draft = (o.initial && draftFromText(o.initial)) || { nc: false, root: null, quality: "", bass: null };
  if (cat && draft.quality) draft.quality = resolveQuality(draft.quality, cat) ?? draft.quality;

  const root = h("div", "chord-dlg");
  root.setAttribute("role", "dialog");
  root.setAttribute("aria-label", o.title);
  const head = h("div", "cd-head");
  head.append(h("span", "cd-title", o.title));
  const text = h("input", "cd-text");
  text.placeholder = "Dm7, G7/B, Bb, N.C. …";
  text.setAttribute("aria-label", "Nom de l'accord");
  text.autocomplete = "off";
  text.spellcheck = false;
  const preview = h("div", "cd-preview");
  const big = h("b", "cd-name");
  const notes = h("span", "cd-notes");
  preview.append(big, notes);
  head.append(text, preview);

  const cols = h("div", "cd-cols");
  const rootBtns = []; // { btn, name }
  const bassBtns = [];
  const noteGrid = (list, kind) => {
    const g = h("div", `cd-roots ${kind}`);
    for (const cell of ROOT_CELLS) {
      const c = h("div", `cd-cell${cell.names.length > 1 ? " black" : ""}`);
      for (const name of cell.names) {
        const b = h("button", "cd-note", prettyPitch(name));
        b.type = "button";
        b.dataset.name = name;
        b.addEventListener("click", () => {
          if (kind === "root") draft = { ...draft, nc: false, root: name };
          else draft = { ...draft, nc: false, bass: draft.bass === name ? null : name };
          refresh();
        });
        c.append(b);
        list.push({ btn: b, name });
      }
      g.append(c);
    }
    return g;
  };
  const section = (title, ...kids) => {
    const s = h("section", "cd-col");
    s.append(h("h4", "", title), ...kids);
    return s;
  };
  const qList = h("div", "cd-quals");
  const qBtns = [];
  const known = new Set(qualities.map(([id]) => id));
  const addQuality = (id, tip) => {
    const b = h("button", "cd-qual", id === "" ? "maj" : id);
    b.type = "button";
    b.title = tip;
    b.addEventListener("click", () => {
      draft = { ...draft, nc: false, quality: id };
      refresh();
    });
    qList.append(b);
    qBtns.push({ btn: b, id });
  };
  for (const [id, iv] of qualities) addQuality(id, `${id === "" ? "majeur" : id} : ${iv.join(" ")}`);
  const free = h("button", "cd-qual free", "libre");
  free.type = "button";
  free.hidden = true;
  qList.append(free);
  const noBass = h("button", "cd-nobass", "Sans basse");
  noBass.type = "button";
  noBass.addEventListener("click", () => {
    draft = { ...draft, bass: null };
    refresh();
  });
  cols.append(section("Fondamentale", noteGrid(rootBtns, "root")), section("Qualité", qList), section("Basse", noteGrid(bassBtns, "bass"), noBass));

  const foot = h("div", "cd-foot");
  const nc = h("button", "cd-nc", "N.C.");
  nc.type = "button";
  nc.title = "Pas d'accord";
  nc.addEventListener("click", () => {
    draft = { nc: true, root: null, quality: "", bass: null };
    refresh();
  });
  const cancel = h("button", "cd-cancel", "Annuler");
  cancel.type = "button";
  const ok = h("button", "cd-ok", "OK");
  ok.type = "button";
  foot.append(nc, h("span", "grow"), cancel, ok);
  root.append(head, cols, foot);

  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    root.remove();
  };
  const validate = () => {
    const t = textFromDraft(draft);
    if (!t) return;
    close();
    o.onOk(t);
  };
  const abort = () => {
    close();
    o.onCancel?.();
  };
  ok.addEventListener("click", validate);
  cancel.addEventListener("click", abort);

  function refresh(fromText = false) {
    const t = textFromDraft(draft);
    if (!fromText) text.value = t;
    text.classList.remove("bad");
    big.textContent = draft.nc ? "N.C." : draft.root ? `${prettyPitch(draft.root)}${draft.quality ?? ""}${draft.bass ? `/${prettyPitch(draft.bass)}` : ""}` : "—";
    const q = qualities.find(([id]) => id === draft.quality);
    const iv = q ? q[1] : /^\[[\d,\s]*\]$/.test(draft.quality ?? "") ? JSON.parse(draft.quality) : null;
    notes.textContent = draft.root && !draft.nc && iv ? chordNotes(draft.root, iv, draft.bass).map(prettyPitch).join("  ") : "";
    for (const { btn, name } of rootBtns) btn.setAttribute("aria-pressed", String(!draft.nc && draft.root === name));
    for (const { btn, name } of bassBtns) btn.setAttribute("aria-pressed", String(draft.bass === name));
    for (const { btn, id } of qBtns) btn.setAttribute("aria-pressed", String(!draft.nc && draft.quality === id));
    const isFree = !draft.nc && !!draft.quality && !known.has(draft.quality);
    free.hidden = !isFree;
    free.textContent = isFree ? draft.quality : "libre";
    free.setAttribute("aria-pressed", String(isFree));
    nc.setAttribute("aria-pressed", String(draft.nc));
    ok.disabled = !t;
  }

  text.addEventListener("input", () => {
    const d = draftFromText(text.value);
    if (d) {
      draft = d;
      refresh(true);
    } else {
      text.classList.add("bad");
      ok.disabled = true;
    }
  });
  root.addEventListener("keydown", (k) => {
    k.stopPropagation();
    if (k.key === "Escape") {
      k.preventDefault();
      abort();
    } else if (k.key === "Enter" && !(k.target instanceof HTMLButtonElement && k.target !== ok)) {
      k.preventDefault();
      validate();
    }
  });
  refresh();
  return { el: root, close, focus: () => text.focus(), select: () => text.select() };
}
