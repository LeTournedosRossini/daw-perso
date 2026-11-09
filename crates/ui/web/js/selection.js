// Sélection multiple de clips : calculs purs (aucun accès à la page), testés dans tests/selection.test.mjs.

/** Nouvel ensemble après un clic sur `id` : avec Ctrl/Maj, on ajoute ou retire ; sinon, si le clip est déjà dans la sélection on la
 *  garde entière (pour glisser le groupe), sinon on ne garde que lui. */
export function clickSelect(selected, id, { toggle = false } = {}) {
  const next = new Set(selected);
  if (toggle) {
    if (next.has(id)) next.delete(id);
    else next.add(id);
    return next;
  }
  return next.has(id) ? next : new Set([id]);
}

/** Identifiants des clips qui touchent le rectangle (échantillons `s0`–`s1`, ordonnées `y0`–`y1`). `nodes` : lignes affichées,
 *  de hauteur `rowH` ; les voies d'automation et le piano roll ne portent pas de clips sélectionnables. */
export function clipsInRect(nodes, rowH, { s0, s1, y0, y1 }) {
  const [lo, hi] = [Math.min(s0, s1), Math.max(s0, s1)];
  const [top, bottom] = [Math.min(y0, y1), Math.max(y0, y1)];
  const out = [];
  nodes.forEach((n, i) => {
    if (n.kind === "lane" || n.kind === "take" || n.kind === "roll" || n.kind === "roll-cont") return;
    if ((i + 1) * rowH <= top || i * rowH >= bottom) return;
    for (const c of n.clips) if (c.start < hi && c.start + c.len > lo) out.push(c.id);
  });
  return out;
}

/** Tous les clips des lignes de clips (Ctrl+A). */
export function allClipIds(nodes) {
  return nodes.filter((n) => n.kind !== "lane" && n.kind !== "roll" && n.kind !== "roll-cont").flatMap((n) => n.clips.map((c) => c.id));
}

/** Le groupe s'arrête à 0 : décalage borné par le clip le plus à gauche (entier). */
export function clampGroupDelta(starts, delta) {
  const earliest = Math.min(...starts);
  return Math.round(Math.max(delta, -earliest)) || 0; // `|| 0` : pas de -0
}

/** Retire de la sélection les clips qui n'existent plus. */
export function prune(selected, nodes) {
  const alive = new Set(allClipIds(nodes));
  return new Set([...selected].filter((id) => alive.has(id)));
}

/** Pistes (ni bus, ni sortie principale, ni lignes d'automation) dans l'ordre d'affichage. */
export function trackIds(nodes) {
  return nodes.filter((n) => n.kind === "mono" || n.kind === "stereo").map((n) => n.id);
}

/** Décalage en pistes d'un groupe dont le clip saisi passe de la piste `from` à la piste `to`, borné pour que **tous** les clips
 *  (pistes d'origine `origins`) restent sur une piste : le groupe garde sa disposition et s'arrête aux bords. */
export function clampNodeDelta(tracks, origins, from, to) {
  const idx = origins.map((o) => tracks.indexOf(o)).filter((i) => i >= 0);
  if (!idx.length || tracks.indexOf(from) < 0 || tracks.indexOf(to) < 0) return 0;
  const raw = tracks.indexOf(to) - tracks.indexOf(from);
  return Math.max(-Math.min(...idx), Math.min(tracks.length - 1 - Math.max(...idx), raw)) || 0;
}

/** Piste d'arrivée d'un clip de la piste `origin` quand le groupe est décalé de `nodeDelta` pistes. */
export function shiftedTrack(tracks, origin, nodeDelta) {
  const i = tracks.indexOf(origin);
  return i < 0 ? origin : tracks[Math.max(0, Math.min(tracks.length - 1, i + nodeDelta))];
}

/** Commande qui réalise un glissement de groupe (`null` s'il n'y a rien à changer). `duplicate` : Alt maintenu. */
export function groupDragCommand(ids, delta, duplicate, nodeDelta = 0) {
  if (!ids.length) return null;
  const extra = nodeDelta ? { node_delta: nodeDelta } : {};
  if (duplicate) return { type: "duplicate_clips", clips: ids, delta, ...extra };
  return delta === 0 && nodeDelta === 0 ? null : { type: "move_clips", clips: ids, delta, ...extra };
}
