// Fenêtre d'un groupe de pistes : nom, couleur, membres (pistes et bus). « Enregistrer » crée (ou modifie) le groupe en une action
// annulable ; « Supprimer » le défait (ses membres restent). Un nœud n'est que dans un groupe : le cocher ici l'enlève de l'ancien.

import { GROUP_COLORS, effectiveGroups, groupColor, groupable } from "../groups-model.js";

const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

/**
 * @param {{client:{send:(m:object)=>void}, project:object, group?:object|null, ret?:number|null}} o  `group` absent : création ; `ret` :
 *   le groupe est propre à ce retour (il remplace ceux du projet dans son mix).
 * @returns {HTMLDialogElement}
 */
export function openGroupDialog({ client, project, group = null, ret = null }) {
  const eff = effectiveGroups(project, ret);
  const scope = ret === null ? {} : { ret };
  const dlg = document.createElement("dialog");
  dlg.className = "group-dialog";
  const form = h("form");
  form.method = "dialog";
  const title = h("h2", "", (group ? "Modifier le groupe" : "Nouveau groupe") + (ret === null ? "" : " de ce retour"));

  const name = h("input");
  name.type = "text";
  name.maxLength = 40;
  name.placeholder = "Batterie, Cordes, Chœurs…";
  name.value = group?.name ?? "";
  name.setAttribute("aria-label", "Nom du groupe");

  let color = group ? groupColor(project, group, eff.groups) : GROUP_COLORS[eff.groups.length % GROUP_COLORS.length];
  const swatches = h("div", "gd-colors");
  const custom = h("input");
  custom.type = "color";
  custom.value = color;
  custom.setAttribute("aria-label", "Autre couleur");
  const paint = () => {
    for (const b of swatches.querySelectorAll("button")) b.setAttribute("aria-pressed", String(b.dataset.c === color));
    custom.value = color;
  };
  for (const c of GROUP_COLORS) {
    const b = h("button", "gd-swatch");
    b.type = "button";
    b.dataset.c = c;
    b.style.background = c;
    b.setAttribute("aria-label", c);
    b.addEventListener("click", () => {
      color = c;
      paint();
    });
    swatches.append(b);
  }
  custom.addEventListener("input", () => {
    color = custom.value;
    paint();
  });
  swatches.append(custom);

  const list = h("div", "gd-members");
  const checked = new Set((group ? project.nodes.filter((n) => eff.groupOf(n.id) === group.id) : []).map((n) => n.id));
  const groupName = (id) => eff.groups.find((g) => g.id === id)?.name;
  for (const n of groupable(project)) {
    const row = h("label", "gd-member");
    const cb = h("input");
    cb.type = "checkbox";
    cb.checked = checked.has(n.id);
    cb.addEventListener("change", () => (cb.checked ? checked.add(n.id) : checked.delete(n.id)));
    const text = h("span", "", n.name);
    row.append(cb, text);
    const inOther = eff.groupOf(n.id);
    if (inOther && inOther !== group?.id) row.append(h("small", "", `(déjà dans « ${groupName(inOther) ?? "?"} »)`));
    list.append(row);
  }
  if (!list.children.length) list.append(h("p", "gd-empty", "Aucune piste ni aucun bus à grouper."));

  const error = h("p", "gd-error");
  const buttons = h("div", "gd-buttons");
  const cancel = h("button", "", "Annuler");
  cancel.type = "button";
  cancel.addEventListener("click", () => dlg.close());
  const ok = h("button", "primary", "Enregistrer");
  ok.type = "submit";
  if (group) {
    const del = h("button", "", "Supprimer le groupe");
    del.type = "button";
    del.addEventListener("click", () => {
      client.send({ type: "delete_group", group: group.id, ...scope });
      dlg.close();
    });
    buttons.append(del);
  }
  buttons.append(h("span", "grow"), cancel, ok);

  form.append(title, labelled("Nom", name), labelled("Couleur", swatches), labelled("Membres", list), error, buttons);
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    if (!name.value.trim()) {
      error.textContent = "Donne un nom au groupe.";
      name.focus();
      return;
    }
    const nodes = [...checked];
    client.send(group ? { type: "edit_group", group: group.id, name: name.value.trim(), color, nodes, ...scope } : { type: "create_group", name: name.value.trim(), color, nodes, ...scope });
    dlg.close();
  });
  dlg.append(form);
  dlg.addEventListener("close", () => dlg.remove());
  dlg.addEventListener("keydown", (k) => k.stopPropagation()); // les raccourcis de la page ne doivent pas agir ici
  document.body.append(dlg);
  dlg.showModal();
  name.focus();
  paint();
  return dlg;
}

function labelled(text, control) {
  const row = h("div", "gd-field");
  row.append(h("span", "", text), control);
  return row;
}
