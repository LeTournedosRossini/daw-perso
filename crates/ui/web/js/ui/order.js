// Ordre d'affichage des pistes, commun aux en-têtes et à la timeline (ils doivent rester alignés ligne à ligne).

/** Pistes et bus dans l'ordre du projet, puis la sortie principale en dernier. */
export function orderedNodes(project) {
  // Les clips muets (prises inactives) ne sont pas sur la ligne de la piste : ils vivent dans les voies de prises.
  const n = project.nodes.map((x) => {
    const muted = (x.clips ?? []).filter((c) => c.muted);
    return muted.length ? { ...x, clips: x.clips.filter((c) => !c.muted), mutedClips: muted } : x;
  });
  return [...n.filter((x) => x.kind !== "master"), ...n.filter((x) => x.kind === "master")];
}

/** Clips muets d'une piste rangés en couches sans chevauchement (une couche = une voie de prises), dans l'ordre. */
export function takeLayers(node) {
  const layers = [];
  for (const c of [...(node.mutedClips ?? [])].sort((a, b) => a.start - b.start)) {
    const layer = layers.find((l) => l[l.length - 1].start + l[l.length - 1].len <= c.start);
    if (layer) layer.push(c);
    else layers.push([c]);
  }
  return layers;
}

/**
 * Lignes de la timeline et des en-têtes : pistes et bus, puis la sortie principale, avec **sous chaque nœud dont la voie
 * d'automation est dépliée** une ligne `kind: "lane"` (même hauteur qu'une piste : tout l'alignement ligne à ligne reste valable).
 */
export function orderedRows(project, open = new Set(), rollNode = null, rollRows = 4, takesOpen = new Set()) {
  const rows = [];
  for (const n of orderedNodes(project)) {
    rows.push(n);
    if (open.has(n.id)) rows.push({ id: n.id, kind: "lane", rowKey: `lane:${n.id}`, node: n, color: n.color, name: n.name, clips: [] });
    // Voies de prises : une ligne par couche de prises inactives, sous la piste (et sa voie d'automation).
    if (takesOpen.has(n.id)) {
      takeLayers(n).forEach((clips, k) => rows.push({ id: n.id, kind: "take", layer: k, rowKey: `take:${n.id}:${k}`, node: n, color: n.color, name: n.name, clips }));
    }
    // Piano roll ancré sous la piste : `rollRows` lignes d'un seul tenant (la première porte l'en-tête, les autres comblent l'espace).
    if (rollNode === n.id) {
      rows.push({ id: n.id, kind: "roll", rowKey: `roll:${n.id}`, node: n, color: n.color, name: n.name, clips: [] });
      for (let k = 1; k < rollRows; k++) rows.push({ id: n.id, kind: "roll-cont", rowKey: `roll:${n.id}:${k}`, node: n, color: n.color, name: n.name, clips: [] });
    }
  }
  return rows;
}
