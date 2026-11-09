// Voies de contrôle du piano roll (modulation, volume, pédale, molette de hauteur, pression…) : calculs purs, sans DOM.
// Un point = un événement MIDI de contrôle du clip : `{ start: [n, d] (depuis le début de la matière), channel, value }`. La page édite la
// voie entière sur une copie et l'envoie d'un coup (`set_events`) : une seule action annulable par geste.

import { fadd, fsub, fcmp } from "./notes-model.js";

export const LANE_DEFS = [
  { id: "vel", label: "Vélocité" },
  { id: "cc1", kind: "cc", controller: 1, label: "Modulation (CC 1)", min: 0, max: 127, rest: 0 },
  { id: "cc7", kind: "cc", controller: 7, label: "Volume (CC 7)", min: 0, max: 127, rest: 100 },
  { id: "cc11", kind: "cc", controller: 11, label: "Expression (CC 11)", min: 0, max: 127, rest: 127 },
  { id: "cc64", kind: "cc", controller: 64, label: "Pédale de maintien (CC 64)", min: 0, max: 127, rest: 0, steps: true },
  { id: "bend", kind: "bend", label: "Molette de hauteur", min: -8192, max: 8191, rest: 0 },
  { id: "pressure", kind: "pressure", label: "Pression de canal (aftertouch)", min: 0, max: 127, rest: 0 },
];

/** La voie d'identifiant `id` ; `cc<n>` désigne n'importe quel contrôleur 0 à 127 ; inconnu → la vélocité. */
export function laneById(id) {
  const known = LANE_DEFS.find((l) => l.id === id);
  if (known) return known;
  const m = /^cc(\d{1,3})$/.exec(String(id));
  if (m && Number(m[1]) <= 127) return { id: `cc${m[1]}`, kind: "cc", controller: Number(m[1]), label: `Contrôleur ${m[1]}`, min: 0, max: 127, rest: 0 };
  return LANE_DEFS[0];
}

export const isVelocity = (lane) => lane.id === "vel";

/** Les événements de l'événement `notes` qui appartiennent à la voie. */
export const eventsOf = (events, lane) => (events ?? []).filter((e) => e.kind === lane.kind && (lane.kind !== "cc" || e.controller === lane.controller));

const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
const PAD = 4; // marge (px) en haut et en bas de la voie

/** Valeur (entière) d'une position verticale `y` dans une voie de hauteur `H` : le haut = le maximum. */
export function valueOfY(lane, y, H) {
  const t = clamp((y - PAD) / Math.max(1, H - 2 * PAD), 0, 1);
  return Math.round(lane.max - t * (lane.max - lane.min));
}

/** Position verticale d'une valeur. */
export function yOfValue(lane, v, H) {
  const t = (clamp(v, lane.min, lane.max) - lane.min) / (lane.max - lane.min);
  return PAD + (1 - t) * (H - 2 * PAD);
}

export const timeKey = (p) => `${p.start[0]}/${p.start[1]}`;
export const pointId = (p) => `${timeKey(p)}:${p.channel ?? 0}`;
const num = (p) => p.start[0] / p.start[1];

/** Liste triée par temps, avec le point `p` posé (il remplace celui du même instant). */
export function withPoint(list, p) {
  const out = list.filter((q) => timeKey(q) !== timeKey(p));
  out.push(p);
  return out.sort((a, b) => num(a) - num(b));
}

/** Liste sans les points dont l'identifiant est dans `ids`. */
export const withoutPoints = (list, ids) => list.filter((p) => !ids.has(pointId(p)));

/** Le point le plus proche de `x` (à `radius` pixels près) : `xOf(point)` donne son abscisse ; rend l'indice ou -1. */
export function nearestPoint(list, x, xOf, radius = 8) {
  let best = -1;
  let bd = radius + 1e-9;
  list.forEach((p, i) => {
    const d = Math.abs(xOf(p) - x);
    if (d <= bd) {
      bd = d;
      best = i;
    }
  });
  return best;
}

/** Les points au format de la commande `set_events`. */
export const toSpecs = (list) => list.map((p) => ({ start: p.start, channel: p.channel ?? 0, value: p.value }));

/** La commande qui remplace toute la voie du clip par `list`. */
export function laneCommand(clip, lane, list, label) {
  const cmd = { type: "set_events", clip, kind: lane.kind, events: toSpecs(list), label };
  if (lane.kind === "cc") cmd.controller = lane.controller;
  return cmd;
}

/** Résumé d'une valeur pour l'affichage : « 64 », « +2048 » (molette), « enfoncée » pour une pédale ≥ 64. */
export function valueText(lane, v) {
  if (lane.kind === "bend") return `${v > 0 ? "+" : ""}${v}`;
  if (lane.steps) return v >= 64 ? `enfoncée (${v})` : `relâchée (${v})`;
  return String(v);
}

/** Ce que copie Ctrl+C : les points choisis (`ids`, tous si vide) avec leur écart exact au premier, et la voie d'où ils viennent. */
export function copyPoints(list, lane, ids) {
  const picked = (ids?.size ? list.filter((p) => ids.has(pointId(p))) : list).slice().sort((a, b) => fcmp(a.start, b.start));
  if (!picked.length) return null;
  const t0 = picked[0].start;
  return { lane: { id: lane.id, kind: lane.kind, min: lane.min, max: lane.max }, points: picked.map((p) => ({ dt: fsub(p.start, t0), channel: p.channel ?? 0, value: p.value })) };
}

/**
 * Colle `clip` dans `list` : le premier point arrive en `at`, les autres gardent leurs écarts ; ceux qui tombent hors de [lo, hi[ sont laissés de
 * côté. Une voie d'une autre sorte reçoit les valeurs à la même hauteur relative (modulation → molette : 64 devient le milieu de la plage).
 * Rend { list, added : les points posés } ; un point déjà à cet instant est remplacé.
 */
export function pastePoints(list, lane, clip, at, lo, hi) {
  const same = clip.lane.id === lane.id;
  const scale = (v) => (same ? v : Math.round(lane.min + ((v - clip.lane.min) / (clip.lane.max - clip.lane.min)) * (lane.max - lane.min)));
  const added = clip.points
    .map((q) => ({ start: fadd(at, q.dt), channel: q.channel, value: Math.min(lane.max, Math.max(lane.min, scale(q.value))) }))
    .filter((p) => fcmp(p.start, lo) >= 0 && fcmp(p.start, hi) < 0);
  let out = list;
  for (const p of added) out = withPoint(out, p);
  return { list: out, added };
}

/**
 * Amincit une courbe : retire d'abord les points qui répètent la valeur précédente (sans aucune perte : un événement MIDI tient jusqu'au suivant),
 * puis ceux qui s'écartent de moins de `tol` (fraction de la plage) de la droite entre leurs voisins conservés (Douglas-Peucker). Une voie « en
 * marches » (pédale) ne perd que les répétitions. `ids` : ne toucher qu'à ces points (tous si vide) ; le premier et le dernier restent.
 */
export function thinPoints(list, lane, ids, tol = 0.015) {
  const sorted = list.slice().sort((a, b) => num(a) - num(b));
  const inScope = (p) => !ids?.size || ids.has(pointId(p));
  const scope = sorted.filter(inScope);
  if (scope.length < 3) return sorted;
  // 1. répétitions
  const lossless = [];
  for (const p of scope) {
    const prev = lossless[lossless.length - 1];
    if (prev && prev.value === p.value && (prev.channel ?? 0) === (p.channel ?? 0)) continue;
    lossless.push(p);
  }
  let kept = lossless;
  if (!lane.steps && lossless.length > 2) {
    // 2. Douglas-Peucker sur la valeur
    const eps = tol * (lane.max - lane.min);
    const keep = new Array(lossless.length).fill(false);
    keep[0] = keep[lossless.length - 1] = true;
    const stack = [[0, lossless.length - 1]];
    while (stack.length) {
      const [a, b] = stack.pop();
      const ta = num(lossless[a]);
      const tb = num(lossless[b]);
      let worst = -1;
      let wd = eps;
      for (let i = a + 1; i < b; i++) {
        const t = tb === ta ? 0 : (num(lossless[i]) - ta) / (tb - ta);
        const d = Math.abs(lossless[i].value - (lossless[a].value + t * (lossless[b].value - lossless[a].value)));
        if (d > wd) {
          wd = d;
          worst = i;
        }
      }
      if (worst >= 0) {
        keep[worst] = true;
        stack.push([a, worst], [worst, b]);
      }
    }
    kept = lossless.filter((_, i) => keep[i]);
  }
  const drop = new Set(scope.filter((p) => !kept.includes(p)).map(pointId));
  return sorted.filter((p) => !drop.has(pointId(p)));
}
