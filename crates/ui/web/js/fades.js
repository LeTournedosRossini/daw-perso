// Courbes de fondu : mêmes formules que le moteur (`engine::audio::Fade::curve`), pour dessiner ce que l'on entend.
// Une courbe = `{ shape, bend, smooth }` (voir docs/protocole-ui.md) ; `x` va de 0 (bord du clip, silence) à 1 (le clip est
// pleinement audible). Ordre : adoucissement en S, puis courbure, puis forme de base.

export const DEFAULT_CURVE = Object.freeze({ shape: "linear", bend: 0, smooth: 0 });

/** Réglages rapides : chacun est un point de l'espace des courbes. */
export const PRESETS = [
  { id: "linear", label: "Linéaire", hint: "Gain constant au point de croisement des deux clips (bon pour deux signaux identiques)", curve: { shape: "linear", bend: 0, smooth: 0 } },
  { id: "equal_power", label: "Puissance constante", hint: "Volume perçu constant (bon pour deux signaux sans rapport)", curve: { shape: "equal_power", bend: 0, smooth: 0 } },
  { id: "s_curve", label: "Courbe en S", hint: "Départ et arrivée doux, croisement rapide au milieu", curve: { shape: "linear", bend: 0, smooth: 1000 } },
  { id: "slow", label: "Lente", hint: "Convexe : le gain monte tard, le son s'efface lentement", curve: { shape: "linear", bend: 1000, smooth: 0 } },
  { id: "fast", label: "Rapide", hint: "Concave : le gain monte tôt, le son s'impose vite", curve: { shape: "linear", bend: -1000, smooth: 0 } },
];

export const sameCurve = (a, b) => (a?.shape ?? "linear") === (b?.shape ?? "linear") && (a?.bend ?? 0) === (b?.bend ?? 0) && (a?.smooth ?? 0) === (b?.smooth ?? 0);

/** Identifiant du réglage rapide qui correspond exactement à `curve`, ou `null` (courbe personnalisée). */
export const presetOf = (curve) => PRESETS.find((p) => sameCurve(p.curve, curve))?.id ?? null;

export function fadeGain(curve, x) {
  const c = curve ?? DEFAULT_CURVE;
  const s = (c.smooth ?? 0) / 1000;
  const u = s === 0 ? x : x + s * (x * x * (3 - 2 * x) - x);
  const k = (c.bend ?? 0) / 1000;
  const v = k === 0 ? u : u - k * u * (1 - u);
  return c.shape === "equal_power" ? Math.sqrt(Math.max(0, v)) : v;
}

/** Points `[x, gain]` d'une courbe, de `x = 0` à `x = 1` en `n + 1` échantillons. */
export function curvePoints(curve, n = 48) {
  return Array.from({ length: n + 1 }, (_, i) => [i / n, fadeGain(curve, i / n)]);
}

/**
 * Courbes d'un fondu enchaîné sur `x` de 0 à 1 le long de la zone commune : `out` (le clip qui sort : gain à `1 − x`),
 * `in` (celui qui entre : gain à `x`), `sum` (somme des gains : 1 = niveau constant) et `power` (somme des puissances :
 * 1 = volume perçu constant pour des signaux sans rapport).
 */
export function crossfadeCurves(curveOut, curveIn, n = 48) {
  return Array.from({ length: n + 1 }, (_, i) => {
    const x = i / n;
    const o = fadeGain(curveOut, 1 - x);
    const m = fadeGain(curveIn, x);
    return { x, out: o, in: m, sum: o + m, power: Math.sqrt(o * o + m * m) };
  });
}
