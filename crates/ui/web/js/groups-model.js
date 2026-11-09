// Groupes de pistes : calculs purs (testés sans page). Un groupe a un nom, une couleur et des membres (pistes et bus) ; un nœud n'est
// que dans un groupe au plus (`node.group`, 0 = aucun). Dans la vue d'un retour, un groupe agit sur les envois de ses membres vers ce retour.

/** Couleurs proposées d'office quand un groupe n'en a pas (dans l'ordre de création). */
export const GROUP_COLORS = ["#e5484d", "#f5a524", "#46a758", "#3e9bff", "#8e4ec6", "#12a594", "#d6409f", "#978365"];

/**
 * Les groupes qui comptent dans une vue : ceux du projet, ou — dans le mix d'un retour qui a ses propres groupes — ceux de ce retour,
 * qui **remplacent** ceux du projet. `groupOf(id)` : groupe d'un nœud dans cette vue (0 : aucun) ; `custom` : groupes propres au retour.
 */
export function effectiveGroups(project, ret = null) {
  const node = ret === null || ret === undefined ? null : project.nodes.find((n) => n.id === ret);
  if (node?.return_groups) {
    const of = new Map(node.return_groups.members.map(([n, g]) => [n, g]));
    return { groups: node.return_groups.groups, groupOf: (id) => of.get(id) ?? 0, custom: true };
  }
  return { groups: project.groups ?? [], groupOf: (id) => project.nodes.find((n) => n.id === id)?.group ?? 0, custom: false };
}

/** Couleur d'un groupe : la sienne, sinon une couleur de la palette d'après sa place dans `list` (par défaut les groupes du projet). */
export function groupColor(project, group, list = project.groups ?? []) {
  if (group.color) return group.color;
  const i = list.findIndex((g) => g.id === group.id);
  return GROUP_COLORS[(i < 0 ? 0 : i) % GROUP_COLORS.length];
}

/** Ce qui peut être membre d'un groupe : pistes et bus, pas la sortie principale ni un retour de monitoring. */
export const groupable = (project) => project.nodes.filter((n) => n.kind !== "master" && !(n.kind === "bus" && n.phys));

/** Membres d'un groupe dans une vue (`ret` : le mix d'un retour, sinon le projet). */
export const groupMembers = (project, id, ret = null) => {
  const { groupOf } = effectiveGroups(project, ret);
  return project.nodes.filter((n) => n.kind !== "master" && groupOf(n.id) === id);
};

/**
 * Coupé / en solo pour un groupe : vrai quand **tous** ses membres le sont. Sans `to` : les pistes elles-mêmes ; avec `to` : leurs envois
 * vers ce nœud (un membre sans envoi n'est ni coupé ni en solo : il n'est pas entendu de toute façon, mais il ne compte pas comme coupé).
 */
export function groupFlags(project, id, to = null) {
  const members = groupMembers(project, id, to);
  if (!members.length) return { mute: false, solo: false, any: false };
  const pick = (n, k) => (to === null ? !!n[k] : !!n.sends.find((s) => s.to === to)?.[k]);
  return { mute: members.every((n) => pick(n, "mute")), solo: members.every((n) => pick(n, "solo")), any: true };
}

/** Un solo est actif dans la destination `to` (un envoi en solo) : les autres envois ne s'y entendent pas. */
export function anySendSolo(project, to) {
  return project.nodes.some((n) => n.sends.some((s) => s.to === to && s.solo));
}

/** Niveau moyen (dB) des membres d'un groupe dans un retour, pour l'affichage ; `null` si aucun n'a d'envoi. */
export function groupMeanDb(project, id, to, gainToDb) {
  const levels = groupMembers(project, id, to).map((n) => n.sends.find((s) => s.to === to)).filter((s) => s && s.gain > 0).map((s) => gainToDb(s.gain));
  return levels.length ? levels.reduce((a, b) => a + b, 0) / levels.length : null;
}

/** Plage du curseur de niveau d'un groupe (dB, de part et d'autre de 0). */
export const OFFSET_RANGE = 12;

/** Décalage de niveau actuel d'un groupe vers une destination (`to` : un retour, `null` : les pistes) ; 0 s'il n'y en a pas. */
export function groupOffset(group, to = null) {
  return (group.offsets ?? []).find((o) => (o.to ?? null) === to)?.db ?? 0;
}

/** Texte d'un décalage : « +3,0 dB », « −6,5 dB », « 0 dB » (signe typographique, virgule décimale). */
export function formatOffset(db) {
  const v = Math.round(db * 10) / 10;
  if (v === 0) return "0 dB";
  return `${v > 0 ? "+" : "−"}${Math.abs(v).toFixed(1).replace(".", ",")} dB`;
}
