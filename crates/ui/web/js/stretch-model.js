// Marqueurs d'étirement d'un clip : calculs purs (aucun accès au DOM), partagés par la page et le faux moteur ; mêmes règles que l'hôte.
//
// Tout est en échantillons, depuis le début du clip. Un clip étiré est décrit par `{ engine, src_len, markers: [[sortie, source], …] }` :
// `sortie` = position du marqueur dans le clip, `source` = position dans la partie de la source que couvre le clip ; les extrémités
// (0, 0) et (longueur du clip, src_len) sont implicites et ne bougent pas. Entre deux marqueurs le rapport sortie / source est constant.
// Un clip ordinaire n'a pas de description : sa source est lui-même (`src_len` = sa longueur), sans marqueur.

/** Rapport permis pour un tronçon (sortie / source) et durée minimale de source d'un tronçon : ceux de la bibliothèque d'étirement. */
export const RATIO_MIN = 0.25;
export const RATIO_MAX = 4;
export const MIN_SEGMENT_SECONDS = 0.01;
export const MAX_MARKERS = 256;
export const ENGINES = ["signalsmith", "rubberband_r2", "rubberband_r3"];
export const ENGINE_LABELS = { signalsmith: "Signalsmith", rubberband_r2: "Rubber Band R2", rubberband_r3: "Rubber Band R3" };

/** Description d'un clip qui n'est pas étiré : sa source est lui-même. */
export function plain(len) {
  return { engine: "signalsmith", src_len: len, markers: [] };
}

/** Clés complètes `[[source, sortie], …]` avec les deux extrémités. */
export function keysOf(info, len) {
  return [[0, 0], ...info.markers.map(([out, src]) => [src, out]), [info.src_len, len]];
}

/** Position de la source (échantillons) qui tombe à la position de sortie `out`, par interpolation entre marqueurs. */
export function srcAt(info, len, out) {
  const k = keysOf(info, len);
  if (out <= 0) return 0;
  if (out >= len) return info.src_len;
  let i = 1;
  while (i < k.length - 1 && k[i][1] <= out) i++;
  const [s0, o0] = k[i - 1];
  const [s1, o1] = k[i];
  return s0 + ((out - o0) / (o1 - o0)) * (s1 - s0);
}

/** Rapports sortie / source de chaque tronçon (un de plus que de marqueurs). */
export function ratios(info, len) {
  const k = keysOf(info, len);
  const out = [];
  for (let i = 1; i < k.length; i++) out.push((k[i][1] - k[i - 1][1]) / (k[i][0] - k[i - 1][0]));
  return out;
}

/** Rapport du tronçon qui contient la position de sortie `out`. */
export function ratioAt(info, len, out) {
  const k = keysOf(info, len);
  let i = 1;
  while (i < k.length - 1 && k[i][1] <= out) i++;
  return (k[i][1] - k[i - 1][1]) / (k[i][0] - k[i - 1][0]);
}

/** Les mêmes règles que l'hôte : message de refus, ou `null` si les marqueurs sont permis. */
export function validate(markers, len, srcLen, rate) {
  if (markers.length > MAX_MARKERS) return `trop de marqueurs (${MAX_MARKERS} au plus)`;
  const pts = [[0, 0]];
  for (const [out, src] of markers) {
    if (!Number.isInteger(out) || !Number.isInteger(src) || out <= 0 || out >= len || src <= 0 || src >= srcLen) return "un marqueur doit être à l'intérieur du clip (ni au début ni à la fin)";
    const last = pts[pts.length - 1];
    if (out <= last[1] || src <= last[0]) return "les marqueurs doivent être strictement dans l'ordre, dans le clip comme dans la source";
    pts.push([src, out]);
  }
  pts.push([srcLen, len]);
  const minSrc = Math.ceil(MIN_SEGMENT_SECONDS * rate);
  for (let i = 1; i < pts.length; i++) {
    const ds = pts[i][0] - pts[i - 1][0];
    const dOut = pts[i][1] - pts[i - 1][1];
    if (ds < minSrc) return `le tronçon ${i} de la source est trop court (au moins 10 ms)`;
    const r = dOut / ds;
    if (r < RATIO_MIN || r > RATIO_MAX) return `le tronçon ${i} serait étiré à ${Math.round(r * 100)} % : permis de ${RATIO_MIN * 100} % à ${RATIO_MAX * 100} %`;
  }
  return null;
}

/**
 * Ajoute un marqueur à la position de sortie `out` : sa position dans la source est celle que la carte actuelle y met (le son ne bouge donc pas
 * à la pose). Rend la nouvelle liste, ou `null` si la position est au bord ou trop près d'un marqueur existant.
 */
export function addMarker(info, len, out, rate) {
  const at = Math.round(out);
  const minGap = Math.ceil(MIN_SEGMENT_SECONDS * rate);
  const k = keysOf(info, len);
  for (const [, o] of k) if (Math.abs(o - at) < minGap) return null;
  const src = Math.round(srcAt(info, len, at));
  const next = [...info.markers, [at, src]].sort((a, b) => a[0] - b[0]);
  return validate(next, len, info.src_len, rate) ? null : next;
}

/** Retire le marqueur `index` (le tronçon voisin reprend la place) ; rend la nouvelle liste. */
export function removeMarker(info, index) {
  return info.markers.filter((_, i) => i !== index);
}

/**
 * Bornes dans lesquelles le marqueur `index` peut être glissé : les rapports des deux tronçons voisins restent dans [25 %, 400 %] et les
 * tronçons gardent au moins 10 ms de source. Rend `[min, max]` en position de sortie (entiers), ou `null` si le marqueur n'existe pas.
 */
export function dragBounds(info, len, index) {
  if (index < 0 || index >= info.markers.length) return null;
  const k = keysOf(info, len); // k[index + 1] est le marqueur
  const [sPrev, oPrev] = k[index];
  const [s, ] = k[index + 1];
  const [sNext, oNext] = k[index + 2];
  const dsL = s - sPrev;
  const dsR = sNext - s;
  const lo = Math.max(oPrev + RATIO_MIN * dsL, oNext - RATIO_MAX * dsR);
  const hi = Math.min(oPrev + RATIO_MAX * dsL, oNext - RATIO_MIN * dsR);
  return [Math.ceil(lo + 1e-9), Math.floor(hi - 1e-9)];
}

/** Déplace le marqueur `index` à `out` (ramené dans ses bornes) ; la position source ne change pas. Rend la nouvelle liste. */
export function moveMarker(info, len, index, out) {
  const b = dragBounds(info, len, index);
  if (!b || b[0] > b[1]) return info.markers;
  const at = Math.min(b[1], Math.max(b[0], Math.round(out)));
  return info.markers.map((m, i) => (i === index ? [at, m[1]] : m));
}

/** Position de sortie (carte actuelle) où tombe la position `src` de la source : l'inverse de `srcAt`. */
export function outAt(info, len, src) {
  const k = keysOf(info, len);
  if (src <= 0) return 0;
  if (src >= info.src_len) return len;
  let i = 1;
  while (i < k.length - 1 && k[i][0] <= src) i++;
  const [s0, o0] = k[i - 1];
  const [s1, o1] = k[i];
  return o0 + ((src - s0) / (s1 - s0)) * (o1 - o0);
}

/**
 * Pour l'aperçu (pose, glissement, retrait d'un marqueur) : chaque tronçon de la **nouvelle** carte montre le tronçon de source qu'il couvre,
 * lu là où la carte **actuelle** le met. On raisonne par position dans la source et non par rang de marqueur : un marqueur ajouté ou retiré
 * change le nombre de tronçons (défaut d'origine : un marqueur posé écrasait tout le clip dans sa moitié gauche).
 * Rend `[{ from: [a, b], to: [a', b'] }]` (positions de sortie, avant et après).
 */
export function previewSegments(info, len, markers) {
  const next = keysOf({ ...info, markers }, len);
  const out = [];
  for (let i = 1; i < next.length; i++) {
    const [sa, oa] = next[i - 1];
    const [sb, ob] = next[i];
    out.push({ from: [outAt(info, len, sa), outAt(info, len, sb)], to: [oa, ob] });
  }
  return out;
}

/** Texte « 92 % » d'un rapport. */
export function ratioText(r) {
  return `${Math.round(r * 100)} %`;
}

/**
 * La même description pour la fenêtre `[from, to)` de la position de sortie du clip (découpe, rognage) : positions ramenées au début de la
 * fenêtre, marqueurs intérieurs seulement, `src_len` = ce que la fenêtre couvre de la source. `null` si la fenêtre est vide.
 */
export function rewindow(info, len, from, to) {
  if (!(to > from)) return null;
  const s0 = Math.round(srcAt(info, len, from));
  const s1 = Math.round(srcAt(info, len, to));
  const markers = [];
  for (const [out, src] of info.markers) {
    const o = out - from;
    const s = src - s0;
    if (o >= 1 && o <= to - from - 1 && s >= 1 && s <= s1 - s0 - 1) markers.push([o, s]);
  }
  return { ...info, src_len: Math.max(1, s1 - s0), markers };
}
