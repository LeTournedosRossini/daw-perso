// Écoute de contrôle (AFL) d'un bus ou d'un retour : à la place du solo (qui n'a pas de sens pour un bus), le bouton casque envoie ce bus seul dans
// la sortie principale, par son fader et ses effets ; les sorties physiques des retours (ce que les musiciens entendent) ne changent pas.

const HEADPHONES = '<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" focusable="false"><path d="M2.5 10V8a5.5 5.5 0 0 1 11 0v2" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/><rect x="1.8" y="9.5" width="3" height="4.5" rx="1.2" fill="currentColor"/><rect x="11.2" y="9.5" width="3" height="4.5" rx="1.2" fill="currentColor"/></svg>';

export const LISTEN_TITLE = "Écoute de contrôle (AFL) : n'entendre que ce bus dans la sortie principale, par son fader et ses effets. Les musiciens continuent d'entendre leur retour.";

/** Un bus ou un retour (jamais la sortie principale ni une piste). */
export const isListenable = (n) => n.kind === "bus";

/** Transforme le bouton « S » d'une tranche de bus en bouton d'écoute. */
export function setupListenButton(btn) {
  btn.classList.add("listen");
  btn.innerHTML = HEADPHONES;
  btn.title = LISTEN_TITLE;
  btn.setAttribute("aria-label", "Écoute de contrôle");
}

/** Allume l'écoute de ce bus, ou l'éteint si c'est lui qui est déjà écouté. */
export function toggleListen(client, project, id) {
  client.send({ type: "set_listen", node: project?.listen === id ? null : id });
}

/** Nom du bus écouté, pour le voyant de la barre (« » si aucun). */
export function listenedName(project) {
  if (project?.listen == null) return "";
  return project.nodes.find((n) => n.id === project.listen)?.name ?? "";
}
