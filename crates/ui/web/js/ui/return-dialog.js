// Fenêtre « Nouveau retour » : un nom et, tout de suite, la sortie de la carte son (le retour est muet tant qu'il n'en a pas). Réutilise
// l'aspect de la fenêtre des groupes. Un clic sur « Créer » envoie une seule commande `add_track` : un seul pas d'annulation.

const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

/**
 * @param {{client:{send:(m:object)=>void}, project:object, choices:{value:string,label:string,disabled:boolean}[], parse:(v:string)=>object|null}} o
 *   `choices` et `parse` : voir `returnChoices` et `parseReturnChoice` du mixeur (la première option est « aucune sortie »).
 * @returns {HTMLDialogElement}
 */
export function openReturnDialog({ client, project, choices, parse }) {
  const dlg = document.createElement("dialog");
  dlg.className = "group-dialog";
  const form = h("form");
  form.method = "dialog";
  const title = h("h2", "", "Nouveau retour");

  const name = h("input");
  name.type = "text";
  name.maxLength = 60;
  name.placeholder = "Retour Marie, Retour batteur…";
  name.setAttribute("aria-label", "Nom du retour");

  const out = h("select");
  out.setAttribute("aria-label", "Sortie sur la carte son");
  for (const c of choices) {
    const o = new Option(c.label, c.value);
    o.disabled = c.disabled;
    out.append(o);
  }
  const noOutputs = (project.audio_outputs || 0) <= 2;
  out.disabled = noOutputs;
  const hint = h("p", "gd-empty", noOutputs
    ? "Le pilote n'a ouvert que la sortie principale : le retour sera créé muet, tu lui donneras sa sortie plus tard."
    : "Un retour n'envoie jamais rien à la sortie principale. Sans sortie choisie, il reste muet jusqu'à ce que tu en donnes une.");

  const buttons = h("div", "gd-buttons");
  const cancel = h("button", "", "Annuler");
  cancel.type = "button";
  cancel.addEventListener("click", () => dlg.close());
  const ok = h("button", "primary", "Créer le retour");
  ok.type = "submit";
  buttons.append(h("span", "grow"), cancel, ok);

  const field = (text, control) => {
    const row = h("div", "gd-field");
    row.append(h("span", "", text), control);
    return row;
  };
  form.append(title, field("Nom", name), field("Sortie", out), hint, buttons);
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const msg = { type: "add_track", kind: "return" };
    if (name.value.trim()) msg.name = name.value.trim();
    const output = noOutputs ? null : parse(out.value);
    if (output) msg.output = output;
    client.send(msg);
    dlg.close();
  });
  dlg.append(form);
  dlg.addEventListener("close", () => dlg.remove());
  dlg.addEventListener("keydown", (k) => k.stopPropagation()); // les raccourcis de la page ne doivent pas agir ici
  document.body.append(dlg);
  dlg.showModal();
  name.focus();
  return dlg;
}
