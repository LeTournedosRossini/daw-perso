// Géométrie des clips à l'écran : où ils se trouvent, quelle zone une souris touche, et ce que devient un clip pendant un
// glissement. Pur JavaScript, sans DOM (testé avec `node --test`). Les mesures reprennent celles du dessin de la timeline.

import { sampleToX } from "../time.js";

export const CLIP_MARGIN_TOP = 5; // écart entre le haut de la ligne et le clip
export const CLIP_MARGIN_BOTTOM = 6;
export const TITLE_BAR = 17; // bandeau du nom
export const EDGE_PX = 6; // largeur de la zone de rognage à chaque bord
export const HANDLE_PX = 6; // rayon d'une poignée de fondu
export const MIN_LEN = 64; // un clip fait au moins 64 échantillons

/** Rectangle d'un clip dans sa ligne (`rowTop` = haut de la ligne, en pixels). */
export function clipRect(vp, rowH, rowTop, clip) {
  const x0 = sampleToX(vp, clip.start);
  const x1 = sampleToX(vp, clip.start + clip.len);
  return { x0, x1, y: rowTop + CLIP_MARGIN_TOP, h: rowH - CLIP_MARGIN_TOP - CLIP_MARGIN_BOTTOM, bodyTop: rowTop + CLIP_MARGIN_TOP + TITLE_BAR };
}

/** Position des poignées de fondu : au sommet du corps du clip, à la longueur du fondu. */
export function fadeHandles(vp, rect, clip) {
  return { inX: rect.x0 + (clip.fade_in ?? 0) / vp.spp, outX: rect.x1 - (clip.fade_out ?? 0) / vp.spp, y: rect.bodyTop };
}

/**
 * Zone touchée par la souris en (`px`, `py`) : `{ node, clip, zone }` ou `null`.
 * Zones : `fade-in`, `fade-out` (poignées), `trim-start`, `trim-end` (bords), `body` (déplacement) — pour les clips audio comme
 * pour les clips de notes (leurs fondus sont des fondus de vélocité).
 * À chevauchement, le clip le plus récent dans la liste (dessiné en dernier, donc au-dessus) l'emporte.
 */
export function hitTest(nodes, vp, rowH, px, py) {
  const row = Math.floor(py / rowH);
  const node = nodes[row];
  if (!node) return null;
  const rowTop = row * rowH;
  for (let i = node.clips.length - 1; i >= 0; i--) {
    const clip = node.clips[i];
    const r = clipRect(vp, rowH, rowTop, clip);
    if (py < r.y || py > r.y + r.h || px < r.x0 - 2 || px > r.x1 + 2) continue;
    const width = r.x1 - r.x0;
    if (width >= 40) {
      const h = fadeHandles(vp, r, clip);
      if (Math.hypot(px - h.inX, py - h.y) <= HANDLE_PX + 2) return { node, clip, zone: "fade-in" };
      if (Math.hypot(px - h.outX, py - h.y) <= HANDLE_PX + 2) return { node, clip, zone: "fade-out" };
    }
    if (width >= 24) {
      if (Math.abs(px - r.x0) <= EDGE_PX) return { node, clip, zone: "trim-start" };
      if (Math.abs(px - r.x1) <= EDGE_PX) return { node, clip, zone: "trim-end" };
    }
    if (px >= r.x0 && px <= r.x1) return { node, clip, zone: "body" };
  }
  return null;
}

/**
 * Aperçu d'un clip pendant un glissement de `delta` échantillons. `snap(sample)` ramène un bord sur la grille (ou le
 * laisse tel quel). Les bornes sont celles du moteur : jamais de début négatif, pas de rognage au-delà des données du
 * fichier (`source_len`), un clip garde au moins `MIN_LEN` échantillons, les fondus restent dans le clip.
 * Retourne `{ start, len, offset, fade_in, fade_out }`.
 */
export function previewDrag(clip, zone, delta, snap = (s) => s) {
  delta = Math.round(delta); // le moteur n'accepte que des échantillons entiers
  const end = clip.start + clip.len;
  const dataStart = clip.start - clip.offset; // position sur la ligne de temps du premier échantillon du fichier
  const dataEnd = dataStart + (clip.source_len ?? end);
  let { start, len, offset, fade_in = 0, fade_out = 0 } = clip;
  switch (zone) {
    case "move":
    case "body": {
      // On cale sur la grille le bord (début ou fin) qui s'en approche le plus ; un bord qui ne tombe sur aucune ligne
      // n'est pas « calé » (il ne doit pas l'emporter parce qu'il n'a pas bougé).
      const s = Math.max(0, clip.start + delta);
      const a = snap(s);
      const e = snap(s + clip.len);
      const startSnapped = a !== s;
      const endSnapped = e !== s + clip.len;
      if (startSnapped && endSnapped) start = Math.abs(a - s) <= Math.abs(e - (s + clip.len)) ? a : e - clip.len;
      else if (startSnapped) start = a;
      else if (endSnapped) start = e - clip.len;
      else start = s;
      start = Math.max(0, start);
      break;
    }
    case "trim-start": {
      const ns = Math.min(end - MIN_LEN, Math.max(Math.max(0, dataStart), snap(clip.start + delta)));
      offset = clip.offset + (ns - clip.start);
      len = end - ns;
      start = ns;
      break;
    }
    case "trim-end": {
      const ne = Math.max(clip.start + MIN_LEN, Math.min(dataEnd, snap(end + delta)));
      len = ne - clip.start;
      break;
    }
    case "fade-in":
      fade_in = Math.max(0, Math.min(len, Math.round(fade_in + delta)));
      break;
    case "fade-out":
      fade_out = Math.max(0, Math.min(len, Math.round(fade_out - delta)));
      break;
    default:
      break;
  }
  fade_in = Math.min(fade_in, len);
  fade_out = Math.min(fade_out, len);
  return { start, len, offset, fade_in, fade_out };
}

/**
 * Piste visée par un déplacement vertical : la piste sous la souris si c'est une vraie piste (ni sortie principale, ni bus),
 * sinon `null` (on reste sur la piste d'origine). `rowH` : hauteur d'une ligne ; `py` : ordonnée dans le canevas.
 */
export function targetTrackAt(nodes, rowH, py) {
  const node = nodes[Math.max(0, Math.min(nodes.length - 1, Math.floor(py / rowH)))];
  return node && (node.kind === "mono" || node.kind === "stereo") ? node.id : null;
}

/** Commande du protocole qui réalise le glissement validé, ou `null` s'il n'y a rien à changer. */
export function commandFor(clip, zone, preview, targetNode = null, originNode = null) {
  const same = (k) => preview[k] === (clip[k] ?? 0);
  switch (zone) {
    case "move":
    case "body": {
      const changesTrack = targetNode !== null && originNode !== null && targetNode !== originNode;
      if (same("start") && !changesTrack) return null;
      return changesTrack ? { type: "move_clip", clip: clip.id, start: preview.start, node: targetNode } : { type: "move_clip", clip: clip.id, start: preview.start };
    }
    case "trim-start":
      return same("start") ? null : { type: "trim_clip", clip: clip.id, start: preview.start };
    case "trim-end":
      return same("len") ? null : { type: "trim_clip", clip: clip.id, end: preview.start + preview.len };
    case "fade-in":
      return same("fade_in") ? null : { type: "set_clip_fades", clip: clip.id, fade_in: preview.fade_in };
    case "fade-out":
      return same("fade_out") ? null : { type: "set_clip_fades", clip: clip.id, fade_out: preview.fade_out };
    default:
      return null;
  }
}

export const CURSORS = { body: "grab", "trim-start": "ew-resize", "trim-end": "ew-resize", "fade-in": "col-resize", "fade-out": "col-resize" };

/**
 * Fondu enchaîné sous la position `sample` d'une piste : une paire de clips audio « a puis b » qui se chevauchent (a commence
 * avant b, finit après le début de b et avant la fin de b). `tolerance` (échantillons) élargit la zone de chaque côté : une zone
 * étroite reste facile à viser. Les fondus peuvent ne pas correspondre à la zone commune (fondus désactivés, réglés à la main) :
 * l'éditeur de fondu les remet d'accord en réglant la longueur. Retourne `{ a, b, start, end }` ou `null`.
 */
export function crossfadeAt(node, sample, tolerance = 0) {
  const audio = node.clips.filter((c) => c.kind === "audio").sort((p, q) => p.start - q.start || p.id - q.id);
  for (let i = 0; i + 1 < audio.length; i++) {
    const [a, b] = [audio[i], audio[i + 1]];
    const end = a.start + a.len;
    if (a.start < b.start && b.start < end && end <= b.start + b.len && sample >= b.start - tolerance && sample <= end + tolerance) return { a, b, start: b.start, end };
  }
  return null;
}

/** Le fondu enchaîné auquel participe le clip `clipId` (comme sortant ou comme entrant) sur sa piste, ou `null`. */
export function crossfadeOf(node, clipId) {
  const audio = node.clips.filter((c) => c.kind === "audio").sort((p, q) => p.start - q.start || p.id - q.id);
  for (let i = 0; i + 1 < audio.length; i++) {
    const [a, b] = [audio[i], audio[i + 1]];
    const end = a.start + a.len;
    if ((a.id === clipId || b.id === clipId) && a.start < b.start && b.start < end && end <= b.start + b.len) return { a, b, start: b.start, end };
  }
  return null;
}
