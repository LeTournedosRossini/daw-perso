// Boutons de durée de la partition : glyphes de notes dessinés en SVG (ronde à triple croche) et bouton cyclique à deux états
// dont le choix n'est validé qu'un court instant après le dernier clic. Calculs purs, sans DOM (testés avec `node --test`).

/** Durées proposées en boutons directs, dans l'ordre d'affichage (identifiants de `GRIDS`). */
export const DIRECT_DURATIONS = ["1/2", "1/4", "1/8", "1/16"];
/** Les deux états du bouton cyclique : ronde et triple croche. */
export const CYCLE_PAIR = ["1/1", "1/32"];
/** Délai (ms) après le dernier clic du bouton cyclique avant que le choix soit pris en compte. */
export const CYCLE_DELAY_MS = 300;

const KINDS = {
  "1/1": { head: "open", stem: false, flags: 0, label: "Ronde" },
  "1/2": { head: "open", stem: true, flags: 0, label: "Blanche" },
  "1/4": { head: "solid", stem: true, flags: 0, label: "Noire" },
  "1/8": { head: "solid", stem: true, flags: 1, label: "Croche" },
  "1/16": { head: "solid", stem: true, flags: 2, label: "Double croche" },
  "1/32": { head: "solid", stem: true, flags: 3, label: "Triple croche" },
};

export const durationLabel = (id) => KINDS[id]?.label ?? id;

/** Glyphe d'une durée en SVG (couleur du texte courant) ; `null` si la durée n'a pas de glyphe. */
export function glyphSvg(id) {
  const k = KINDS[id];
  if (!k) return null;
  const head = k.head === "solid"
    ? `<ellipse cx="8" cy="17" rx="4.4" ry="3.1" transform="rotate(-22 8 17)" fill="currentColor"/>`
    : k.stem
      ? `<ellipse cx="8" cy="17" rx="4.2" ry="2.9" transform="rotate(-22 8 17)" fill="none" stroke="currentColor" stroke-width="1.5"/>`
      : `<ellipse cx="12" cy="12" rx="6.4" ry="4.1" fill="none" stroke="currentColor" stroke-width="2"/><ellipse cx="12" cy="12" rx="2.2" ry="3.4" transform="rotate(-30 12 12)" fill="currentColor"/>`;
  const stem = k.stem ? `<line x1="11.8" y1="16" x2="11.8" y2="3" stroke="currentColor" stroke-width="1.4"/>` : "";
  let flags = "";
  for (let i = 0; i < k.flags; i++) flags += `<path d="M11.8 ${3 + i * 3.4} c0.3 3.6 5 4.4 4.5 8.4" fill="none" stroke="currentColor" stroke-width="1.5"/>`;
  return `<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true" focusable="false">${head}${stem}${flags}</svg>`;
}

/**
 * État suivant du bouton cyclique. `pending` : état déjà choisi mais pas encore validé (clics rapides) ; sinon on part de la grille actuelle.
 * Si ni l'un ni l'autre n'est dans la paire, le premier clic donne la ronde, le suivant la triple croche.
 */
export function nextCycle(pending, current, pair = CYCLE_PAIR) {
  const base = pending ?? current;
  const i = pair.indexOf(base);
  return i < 0 ? pair[0] : pair[(i + 1) % pair.length];
}
