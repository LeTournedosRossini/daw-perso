// Calculs purs du potentiomètre (testés sans page) : angle d'affichage, valeur d'après un glissement, pas au clavier.

/** Balayage du potentiomètre : de −135° à +135° autour du haut (0° = midi). */
export const SWEEP = 270;

const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));

/** Position de `value` entre 0 (minimum) et 1 (maximum). */
export const fraction = (value, min, max) => (max > min ? clamp((value - min) / (max - min), 0, 1) : 0);

/** Angle (degrés, 0 = en haut, sens horaire) de l'aiguille pour `value`. */
export const angleOf = (value, min, max) => -SWEEP / 2 + fraction(value, min, max) * SWEEP;

/**
 * Valeur après un glissement vertical **relatif** : la valeur ne saute jamais sous le doigt. `dy` > 0 = vers le haut.
 * `travel` = nombre de pixels pour parcourir toute la plage ; `fine` (Maj, ou second doigt) divise la vitesse par 5.
 */
export function dragValue(start, dy, { min, max, step = 0, travel = 160, fine = false }) {
  const span = max - min;
  const raw = start + (dy / (fine ? travel * 5 : travel)) * span;
  return snap(clamp(raw, min, max), min, max, step);
}

/** Arrondit au pas (en gardant les bornes exactes). */
export function snap(v, min, max, step) {
  if (!(step > 0)) return v;
  if (v <= min) return min;
  if (v >= max) return max;
  const r = min + Math.round((v - min) / step) * step;
  return clamp(Number(r.toFixed(6)), min, max);
}

/** Valeur après une touche : flèches (pas, ×10 avec Maj), PageUp/PageDown (×10), Début / Fin ; `null` si la touche n'agit pas. */
export function keyValue(key, value, { min, max, step = 0.1, shift = false }) {
  const big = step * 10;
  switch (key) {
    case "ArrowUp":
    case "ArrowRight":
      return clamp(value + (shift ? big : step), min, max);
    case "ArrowDown":
    case "ArrowLeft":
      return clamp(value - (shift ? big : step), min, max);
    case "PageUp":
      return clamp(value + big, min, max);
    case "PageDown":
      return clamp(value - big, min, max);
    case "Home":
      return min;
    case "End":
      return max;
    default:
      return null;
  }
}

/** Valeur après un cran de molette (`deltaY` > 0 = vers le bas) : un pas par cran, ×10 avec Maj. */
export function wheelValue(value, deltaY, { min, max, step = 0.1, shift = false }) {
  if (!deltaY) return value;
  const n = (deltaY < 0 ? 1 : -1) * (shift ? 10 : 1);
  return clamp(value + n * step, min, max);
}

/**
 * Tracé de l'arc de valeur : de `origin` (valeur de départ de l'arc : le minimum pour un volume, le centre pour un panoramique)
 * à `value`. Rend `{ a0, a1 }` en degrés, a0 ≤ a1.
 */
export function arcSpan(value, origin, min, max) {
  const a = angleOf(origin, min, max);
  const b = angleOf(value, min, max);
  return a <= b ? { a0: a, a1: b } : { a0: b, a1: a };
}

/** Point du cercle de rayon `r` centré en (cx, cy) à l'angle `deg` (0 = haut, sens horaire). */
export function polar(cx, cy, r, deg) {
  const t = (deg * Math.PI) / 180;
  return [cx + r * Math.sin(t), cy - r * Math.cos(t)];
}

/** Chemin SVG d'un arc entre deux angles ; vide si l'arc est quasi nul. */
export function arcPath(cx, cy, r, a0, a1) {
  if (a1 - a0 < 0.5) return "";
  const [x0, y0] = polar(cx, cy, r, a0);
  const [x1, y1] = polar(cx, cy, r, a1);
  const large = a1 - a0 > 180 ? 1 : 0;
  return `M${x0.toFixed(2)} ${y0.toFixed(2)}A${r} ${r} 0 ${large} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
}
