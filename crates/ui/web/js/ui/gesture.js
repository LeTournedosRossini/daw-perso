// Gestes de l'utilisateur sur un réglage (curseur, fader) : l'hôte a besoin de savoir quand on **prend** et quand on **lâche** un
// réglage pour le mode d'écriture d'automation « Toucher » (message `gesture`).

/** Prévient l'hôte quand `el` est pris (pointeur enfoncé) puis lâché ; `getTarget()` rend `{ node, target }`. */
export function trackGesture(el, client, getTarget) {
  let on = false;
  const end = () => {
    if (!on) return;
    on = false;
    window.removeEventListener("pointerup", end, true);
    window.removeEventListener("pointercancel", end, true);
    client.send({ type: "gesture", ...getTarget(), active: false });
  };
  el.addEventListener("pointerdown", () => {
    if (on) return;
    on = true;
    client.send({ type: "gesture", ...getTarget(), active: true });
    window.addEventListener("pointerup", end, true);
    window.addEventListener("pointercancel", end, true);
  });
}

/** Libellés des modes d'une voie d'automation, dans l'ordre du menu. */
export const LANE_MODES = [
  ["read", "Lecture"],
  ["trim", "Trim (le fader s'ajoute)"],
  ["off", "Désactivée"],
  ["touch", "Toucher (écrit quand on tient)"],
  ["latch", "Latch (écrit jusqu'à l'arrêt)"],
  ["write", "Écriture (écrit en continu)"],
];

export const isWriteMode = (m) => m === "touch" || m === "latch" || m === "write";
