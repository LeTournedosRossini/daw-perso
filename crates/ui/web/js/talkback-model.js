// Talkback de la régie : calculs purs de la page (retours visés, texte du bouton, entrées proposées).

/** Retours de monitoring du projet (les seuls qui peuvent entendre la voix). */
export const returnsOf = (project) => (project?.nodes ?? []).filter((n) => n.kind === "bus" && n.is_return);

/**
 * Retours à viser : tous, sauf ceux que l'utilisateur a décochés (retenus **par nom** : les numéros changent avec la structure).
 * @param {object} project
 * @param {string[]} unchecked noms des retours décochés
 * @returns {number[]} numéros triés
 */
export function desiredTargets(project, unchecked) {
  const off = new Set(unchecked ?? []);
  return returnsOf(project)
    .filter((n) => !off.has(n.name))
    .map((n) => n.id)
    .sort((a, b) => a - b);
}

/** Les deux listes de numéros sont-elles identiques (triées) ? */
export const sameTargets = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);

/** Message de la commande de réglage à envoyer, ou `null` si l'état de l'hôte est déjà celui qu'on veut. */
export function reconcile(host, wanted) {
  if (!wanted || wanted.channel == null) return null;
  const same = host && host.channel === wanted.channel && host.level_db === wanted.level_db && host.duck_db === wanted.duck_db && sameTargets(host.targets ?? [], wanted.targets);
  return same ? null : { type: "set_talkback", channel: wanted.channel, level_db: wanted.level_db, duck_db: wanted.duck_db, targets: wanted.targets };
}

/** Entrées de la carte proposées pour le micro (1 à `count`). */
export const inputChoices = (count) => Array.from({ length: Math.max(0, Math.min(32, count | 0)) }, (_, i) => i + 1);

/** Libellé du bouton « Parler » selon l'état : ce qu'on peut faire maintenant. */
export function talkLabel({ channel, targets, active }) {
  if (active) return "● Parle…";
  if (channel == null) return "Parler (choisir le micro)";
  if (!targets?.length) return "Parler (choisir un retour)";
  return "Parler";
}
