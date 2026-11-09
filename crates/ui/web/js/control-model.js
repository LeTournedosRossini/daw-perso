// Voies de contrôle du piano roll (modulation, volume, pédale, molette de hauteur, pression…) : calculs purs, sans DOM.
// Un point = un événement MIDI de contrôle du clip : `{ start: [n, d] (depuis le début de la matière), channel, value }`. La page édite la
// voie entière sur une copie et l'envoie d'un coup (`set_events`) : une seule action annulable par geste.

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
