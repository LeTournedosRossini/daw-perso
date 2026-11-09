// Position d'une fenêtre flottante et hauteur d'une vue ancrée : calculs purs, sans DOM (testés avec `node --test`).
// Toutes les valeurs de position sont en pixels CSS de la page (donc avant le facteur de taille de l'interface).

export const DOCK_ROWS_MIN = 3;
export const DOCK_ROWS_MAX = 12;
export const DOCK_ROWS_DEFAULT = 5;

/** Nombre de lignes de la timeline que prend une vue ancrée sous une piste. */
export const clampDockRows = (n) => (Number.isFinite(n) ? Math.max(DOCK_ROWS_MIN, Math.min(DOCK_ROWS_MAX, Math.round(n))) : DOCK_ROWS_DEFAULT);

/**
 * Ramène une fenêtre dans l'écran : au moins `keepX` pixels de sa largeur et son bandeau de titre (`keepY`) restent visibles et atteignables.
 * `view` : { vw, vh } taille de l'écran, `w` : largeur de la fenêtre.
 */
export function clampWindowPos({ left, top }, { vw, vh, w }, keepX = 80, keepY = 40) {
  const l = Number.isFinite(left) ? left : 40;
  const t = Number.isFinite(top) ? top : 100;
  return {
    left: Math.round(Math.min(Math.max(l, keepX - w), Math.max(keepX - w, vw - keepX))),
    top: Math.round(Math.min(Math.max(t, 0), Math.max(0, vh - keepY))),
  };
}

/** Nouvelle position pendant un glissement : départ + déplacement de la souris ramené à l'échelle de l'interface. */
export function dragPos(start, dx, dy, zoom, view) {
  const z = zoom > 0 ? zoom : 1;
  return clampWindowPos({ left: start.left + dx / z, top: start.top + dy / z }, view);
}
