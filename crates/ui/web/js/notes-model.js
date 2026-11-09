// Piano roll : fractions exactes de ronde, grilles et calculs de déplacement — pur JavaScript, sans DOM (testé avec `node --test`).
// Les positions voyagent en fractions `[n, d]` (comme le moteur) ; les nombres flottants ne servent qu'à l'affichage.

export const gcd = (a, b) => (b ? gcd(b, a % b) : Math.abs(a));
export const fnorm = ([n, d]) => {
  const g = gcd(n, d) || 1;
  return [n / g, d / g];
};
export const fadd = (a, b) => fnorm([a[0] * b[1] + b[0] * a[1], a[1] * b[1]]);
export const fsub = (a, b) => fnorm([a[0] * b[1] - b[0] * a[1], a[1] * b[1]]);
export const fnum = (f) => f[0] / f[1];
export const fcmp = (a, b) => a[0] * b[1] - b[0] * a[1];
export const fmax = (a, b) => (fcmp(a, b) >= 0 ? a : b);
export const fmin = (a, b) => (fcmp(a, b) <= 0 ? a : b);

/** Grilles de magnétisme proposées (fraction de ronde). « libre » = le tick de 1/3 840 de ronde (960 par noire). */
export const GRIDS = [
  { id: "1/1", label: "Ronde", f: [1, 1] },
  { id: "1/2", label: "Blanche", f: [1, 2] },
  { id: "1/4", label: "Noire", f: [1, 4] },
  { id: "1/8", label: "Croche", f: [1, 8] },
  { id: "1/16", label: "Double croche", f: [1, 16] },
  { id: "1/32", label: "Triple croche", f: [1, 32] },
  { id: "1/8T", label: "Triolet de croches", f: [1, 12] },
  { id: "1/16T", label: "Triolet de doubles", f: [1, 24] },
  { id: "free", label: "Libre", f: [1, 3840] },
];
export const gridById = (id) => GRIDS.find((g) => g.id === id) ?? GRIDS.find((g) => g.id === "1/16");

/** Position absolue `abs` (rondes, flottant) aimantée à la grille `g` : fraction exacte. `mode` : "round" ou "floor". */
export function snapAbs(abs, g, mode = "round") {
  const steps = (abs * g[1]) / g[0];
  const k = mode === "floor" ? Math.floor(steps + 1e-9) : Math.round(steps);
  return fnorm([Math.max(0, k) * g[0], g[1]]);
}

/**
 * Position de la matière du clip (`m`, depuis le début de la matière) ↔ position absolue sur la ligne de temps :
 * `abs = clip.start − clip.offset + m`.
 */
export const matToAbs = (clip, m) => fadd(fsub(clip.start, clip.offset), m);
export const absToMat = (clip, abs) => fadd(fsub(abs, clip.start), clip.offset);

/** Touche → nom (60 = Do4) et couleur de touche. */
const NAMES = ["Do", "Do♯", "Ré", "Ré♯", "Mi", "Fa", "Fa♯", "Sol", "Sol♯", "La", "La♯", "Si"];
export const keyName = (k) => `${NAMES[k % 12]}${Math.floor(k / 12) - 1}`;
export const isBlack = (k) => [1, 3, 6, 8, 10].includes(k % 12);

/** Identité d'une note (début, touche, canal) : sert à garder la sélection d'une réponse du moteur à la suivante. */
export const noteId = (n) => `${n.start[0]}/${n.start[1]}:${n.key}:${n.channel ?? 0}`;

/**
 * Déplacement d'un groupe de notes : le glissement a mis la note saisie à la position absolue `grabAbs` (flottant) et à la touche
 * `grabKey`. Le décalage de temps est exact (aimantation de la note saisie sur la grille), le même pour toutes les notes ;
 * les touches sont ramenées dans 0-127 sans écraser l'écart entre notes, et les débuts restent dans la partie visible du clip.
 */
export function moveGroup(clip, notes, grabbed, grabAbs, grabKey, g, snap = true) {
  const target = snap ? snapAbs(grabAbs, g) : fnorm([Math.round(grabAbs * 3840), 3840]);
  const grabbedAbs = matToAbs(clip, grabbed.start);
  let dt = fsub(target, grabbedAbs);
  let dk = grabKey - grabbed.key;
  const minKey = Math.min(...notes.map((n) => n.key));
  const maxKey = Math.max(...notes.map((n) => n.key));
  dk = Math.max(-minKey, Math.min(127 - maxKey, dk));
  const lo = clip.offset;
  const hi = fadd(clip.offset, clip.len);
  // Le début de chaque note doit rester dans [offset, offset + len[ : on borne le décalage commun.
  for (const n of notes) {
    const s = fadd(n.start, dt);
    if (fcmp(s, lo) < 0) dt = fsub(lo, n.start);
  }
  for (const n of notes) {
    const s = fadd(n.start, dt);
    if (fcmp(s, hi) >= 0) dt = fsub(fsub(hi, [1, 3840]), n.start);
  }
  return notes.map((n) => ({ ...n, start: fadd(n.start, dt), key: n.key + dk }));
}

/** Redimensionnement d'un groupe : la fin de la note saisie est aimantée ; chaque note change de la même durée (mini : un pas de grille). */
export function resizeGroup(clip, notes, grabbed, endAbs, g, snap = true) {
  const target = snap ? snapAbs(endAbs, g) : fnorm([Math.round(endAbs * 3840), 3840]);
  const grabbedEnd = matToAbs(clip, fadd(grabbed.start, grabbed.len));
  const delta = fsub(target, grabbedEnd);
  const minLen = snap ? g : [1, 3840];
  let d = delta;
  for (const n of notes) {
    const l = fadd(n.len, d);
    if (fcmp(l, minLen) < 0) d = fsub(minLen, n.len);
  }
  return notes.map((n) => ({ ...n, len: fadd(n.len, d) }));
}

/** Les modifications à envoyer (`edit_notes`) entre un état d'origine et un état modifié, pour les indices donnés. */
export function diffEdits(indices, before, after) {
  const edits = [];
  indices.forEach((index, i) => {
    const a = before[i];
    const b = after[i];
    const e = { index };
    if (fcmp(a.start, b.start) !== 0) e.start = b.start;
    if (fcmp(a.len, b.len) !== 0) e.len = b.len;
    if (a.key !== b.key) e.key = b.key;
    if (a.velocity !== b.velocity) e.velocity = b.velocity;
    if (Object.keys(e).length > 1) edits.push(e);
  });
  return edits;
}

/**
 * Pincement à deux doigts : de `(a0, b0)` à `(a1, b1)` (positions des deux doigts, pixels). Rend le facteur de zoom horizontal et
 * vertical (écart horizontal et vertical entre les doigts d'avant et d'après ; un axe où les doigts sont trop proches — moins de
 * `min` pixels — n'est pas touché, sinon le rapport s'emballe), et le déplacement du milieu.
 */
export function pinchFactors(a0, b0, a1, b1, min = 28) {
  const axis = (p0, p1) => {
    const d0 = Math.abs(p0[0] - p0[1]);
    const d1 = Math.abs(p1[0] - p1[1]);
    return d0 >= min && d1 >= min ? Math.max(0.2, Math.min(5, d1 / d0)) : 1;
  };
  const fx = axis([a0.x, b0.x], [a1.x, b1.x]);
  const fy = axis([a0.y, b0.y], [a1.y, b1.y]);
  return {
    fx,
    fy,
    mid: { x: (a1.x + b1.x) / 2, y: (a1.y + b1.y) / 2 },
    dx: (a1.x + b1.x) / 2 - (a0.x + b0.x) / 2,
    dy: (a1.y + b1.y) / 2 - (a0.y + b0.y) / 2,
  };
}

/**
 * Propriétés de la sélection pour le panneau du piano roll. `notes` : les notes sélectionnées. Rend `null` si rien n'est
 * sélectionné ; sinon le nombre, et pour chaque propriété sa valeur commune (`null` si elles diffèrent) — canal de 1 à 16,
 * vélocité de 1 à 127, touche —, plus début et durée (en noires, depuis le début de la matière) pour une seule note.
 */
export function describeSelection(notes) {
  if (!notes.length) return null;
  const same = (f) => (notes.every((n) => f(n) === f(notes[0])) ? f(notes[0]) : null);
  const one = notes.length === 1 ? notes[0] : null;
  return {
    count: notes.length,
    key: same((n) => n.key),
    keyName: same((n) => n.key) === null ? null : keyName(notes[0].key),
    channel: same((n) => (n.channel ?? 0) + 1),
    velocity: same((n) => Math.max(1, Math.min(127, Math.round(n.velocity * 127)))),
    startBeats: one ? fnum(one.start) * 4 : null,
    lenBeats: one ? fnum(one.len) * 4 : null,
  };
}

/**
 * Sens d'un pincement, décidé **une fois par geste** d'après le mouvement cumulé depuis la pose des doigts (`a0`, `b0`) jusqu'à maintenant
 * (`a1`, `b1`) : `null` tant que l'écart entre les doigts a changé de moins de `dead` pixels (le bruit d'un doigt ne doit pas choisir) ;
 * ensuite `"y"` si le geste est **plutôt vertical** (l'écart vertical change plus d'une fois et demie plus que l'horizontal), sinon
 * `"x"` : le zoom horizontal est le plus utile et c'est le choix par défaut.
 */
export function pinchMode(a0, b0, a1, b1, dead = 14) {
  const cx = Math.abs(Math.abs(a1.x - b1.x) - Math.abs(a0.x - b0.x));
  const cy = Math.abs(Math.abs(a1.y - b1.y) - Math.abs(a0.y - b0.y));
  if (Math.max(cx, cy) < dead) return null;
  return cy > 1.5 * cx ? "y" : "x";
}

const isPow2 = (n) => Number.isInteger(n) && n > 0 && (n & (n - 1)) === 0;

/**
 * Grille d'**affichage** d'un clip pour la partition : la plus grosse grille binaire sur laquelle tombent le début et la durée de toutes
 * les notes, mais jamais plus grosse que la grille choisie. Sans cela, choisir « blanche » arrondirait (donc dessinerait) toutes les notes
 * en blanches. Notes hors grille binaire (triolets, tick libre) : la grille choisie, ou la double croche si elle est plus grosse.
 */
export function displayGrid(notes, chosen) {
  if (!isPow2(chosen[1]) || chosen[0] !== 1) return chosen;
  let need = null;
  for (const d of [1, 2, 4, 8, 16, 32]) {
    if (notes.every((n) => (n.start[0] * d) % n.start[1] === 0 && (n.len[0] * d) % n.len[1] === 0)) {
      need = d;
      break;
    }
  }
  if (need === null) return [1, Math.max(chosen[1], 16)];
  return [1, Math.max(need, chosen[1])];
}
