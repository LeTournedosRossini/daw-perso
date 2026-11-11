// Ce que la barre d'édition doit montrer selon la sélection (fonction pure, testée) : combien de clips audio et de clips de notes sont
// sélectionnés, le texte du contexte, et quels groupes d'outils sont utiles.

/** `selected` : ensemble des identifiants de clips sélectionnés ; `project` : l'événement `project`. */
export function selectionContext(project, selected) {
  const kinds = new Map();
  for (const n of project?.nodes ?? []) for (const c of n.clips ?? []) kinds.set(c.id, c.kind);
  let audio = 0;
  let notes = 0;
  for (const id of selected ?? []) {
    const k = kinds.get(id);
    if (k === "audio") audio++;
    else if (k === "midi") notes++;
  }
  return { audio, notes, total: audio + notes, label: contextLabel(audio, notes), showAudioTools: audio > 0 };
}

const plural = (n, one, many) => `${n} ${n > 1 ? many : one}`;

export function contextLabel(audio, notes) {
  if (audio === 0 && notes === 0) return "Aucun clip sélectionné";
  const parts = [];
  if (audio) parts.push(plural(audio, "clip audio", "clips audio"));
  if (notes) parts.push(plural(notes, "clip de notes", "clips de notes"));
  return parts.join(" + ");
}
