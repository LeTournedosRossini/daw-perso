// Courbe de réponse de la vélocité MIDI d'une piste : mêmes formules que le moteur (`engine::velocity`), pour dessiner la courbe et
// montrer en direct ce que devient une note. Le moteur reste la référence ; ici l'arrondi peut différer d'une unité dans de rares cas.

export const DEFAULT_CURVE = { enabled: false, bend: 0, smooth: 0, min: 1, max: 127, fixed: 0 };

/** Vélocité de sortie (1 à 127) pour une vélocité d'entrée (1 à 127). */
export function mapVelocity(c, velocity) {
  if (c.fixed > 0) return c.fixed;
  const i = Math.max(1, Math.min(127, Math.round(velocity)));
  const x = (i - 1) / 126;
  const s = c.smooth / 1000;
  const u = c.smooth === 0 ? x : x + s * (x * x * (3 - 2 * x) - x);
  const k = c.bend / 1000;
  const v = c.bend === 0 ? u : u - k * u * (1 - u);
  return Math.max(1, Math.min(127, Math.floor(c.min + v * (c.max - c.min) + 0.5)));
}

/** Résumé pour le menu et l'info-bulle. */
export function describeCurve(c) {
  if (!c?.enabled) return "désactivée";
  if (c.fixed > 0) return `vélocité fixe ${c.fixed}`;
  const shape = c.bend === 0 && c.smooth === 0 ? "droite" : c.bend > 0 ? "convexe" : c.bend < 0 ? "concave" : "en S";
  return `${shape} · ${c.min} à ${c.max}`;
}

/** Valeurs valides d'après la saisie : bornes ramenées dans 1-127, courbure et adoucissement dans leurs plages. */
export function normalizeCurve(c) {
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, Math.round(Number.isFinite(v) ? v : lo)));
  return {
    enabled: !!c.enabled,
    bend: clamp(c.bend ?? 0, -1000, 1000),
    smooth: clamp(c.smooth ?? 0, 0, 1000),
    min: clamp(c.min ?? 1, 1, 127),
    max: clamp(c.max ?? 127, 1, 127),
    fixed: clamp(c.fixed ?? 0, 0, 127),
  };
}

/** Poignée touchée par un point `(x, y)` en pixels dans un tracé carré de côté `size` ; `radius` = zone de saisie. `null` si aucune. */
export function hitHandle(c, size, x, y, radius = 16) {
  const py = (v) => size - ((v - 1) / 126) * size;
  const handles = [
    { id: "min", x: 0, y: py(c.min) },
    { id: "max", x: size, y: py(c.max) },
  ];
  let best = null;
  for (const h of handles) {
    const d = Math.hypot(h.x - x, h.y - y);
    if (d <= radius && (!best || d < best.d)) best = { id: h.id, d };
  }
  return best?.id ?? null;
}

/** Vélocité de sortie correspondant à une hauteur `y` (pixels) dans le tracé. */
export function yToVelocity(size, y) {
  return Math.max(1, Math.min(127, Math.round(1 + (1 - y / size) * 126)));
}
