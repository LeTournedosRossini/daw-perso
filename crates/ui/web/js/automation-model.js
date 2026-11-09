// Voies d'automation : cibles, plages de valeurs et édition de points — pur JavaScript, sans DOM (testé avec `node --test`).
// Les positions voyagent en fractions de ronde `[n, d]` (comme le moteur) ; ici on les garde sur la grille fine de 1/3 840 de ronde
// (960 par noire), précision largement suffisante pour une courbe.

import { fnorm, fadd, fsub, fnum, fcmp } from "./notes-model.js";
import { dbToGain, gainToDb } from "./protocol.js";

export const DB_MIN = -60;
export const DB_MAX = 6;
export const TICK = [1, 3840];

export const targetKey = (t) => (t.kind === "send" ? `send:${t.to}` : t.kind === "insert" ? `insert:${t.insert}:${t.param}` : t.kind);
export const sameTarget = (a, b) => targetKey(a) === targetKey(b);

/**
 * Plage d'affichage d'une cible : gain et niveau d'envoi en dB (de −60 à +6 ; la valeur envoyée est linéaire), panoramique de −1 à 1,
 * paramètre de plugin dans son unité (`param` : { min, max }).
 */
export function rangeFor(target, param = null) {
  if (target.kind === "gain" || target.kind === "send") return { kind: "db", min: DB_MIN, max: DB_MAX, unity: 0 };
  if (target.kind === "pan") return { kind: "linear", min: -1, max: 1, unity: 0 };
  return { kind: "linear", min: param?.min ?? 0, max: param?.max ?? 1, unity: param?.default ?? null };
}

/** Valeur de la courbe → position verticale 0 (bas) … 1 (haut). */
export function valueToFrac(range, v) {
  if (range.kind === "db") {
    const db = v <= 0 ? DB_MIN : Math.max(DB_MIN, Math.min(DB_MAX, gainToDb(v)));
    return (db - DB_MIN) / (DB_MAX - DB_MIN);
  }
  return Math.max(0, Math.min(1, (v - range.min) / (range.max - range.min || 1)));
}

/** Position verticale 0 … 1 → valeur de la courbe (gain linéaire pour les dB ; `-60 dB` et moins donnent 0). */
export function fracToValue(range, f) {
  const c = Math.max(0, Math.min(1, f));
  if (range.kind === "db") {
    const db = DB_MIN + c * (DB_MAX - DB_MIN);
    return db <= DB_MIN + 0.05 ? 0 : dbToGain(db);
  }
  return range.min + c * (range.max - range.min);
}

/** Position musicale flottante (rondes) → fraction `[n, d]` sur la grille fine, jamais négative. */
export function toPos(whole) {
  return fnorm([Math.max(0, Math.round(whole * TICK[1])), TICK[1]]);
}

const byPos = (a, b) => fcmp(a.pos, b.pos);

/** Ajoute un point (ou remplace celui qui est exactement à la même position) ; rend la nouvelle liste et l'indice du point. */
export function insertPoint(points, pos, value, curve = "linear") {
  const list = points.filter((p) => fcmp(p.pos, pos) !== 0).concat([{ pos, value, curve }]).sort(byPos);
  return { points: list, index: list.findIndex((p) => fcmp(p.pos, pos) === 0) };
}

/** Déplace le point `index` : la position reste strictement entre celles de ses voisins (à un tick près). */
export function movePoint(points, index, pos, value) {
  const prev = points[index - 1];
  const next = points[index + 1];
  let p = pos;
  if (prev && fcmp(p, fadd(prev.pos, TICK)) < 0) p = fadd(prev.pos, TICK);
  if (next && fcmp(p, fsub(next.pos, TICK)) > 0) p = fsub(next.pos, TICK);
  if (fcmp(p, [0, 1]) < 0) p = [0, 1];
  const list = points.map((q, i) => (i === index ? { ...q, pos: p, value } : q));
  return { points: list, index };
}

export function removePoint(points, index) {
  return points.filter((_, i) => i !== index);
}

export function toggleCurve(points, index) {
  return points.map((p, i) => (i === index ? { ...p, curve: p.curve === "step" ? "linear" : "step" } : p));
}

/** Valeur de la courbe à la position `whole` (rondes) : celle du premier point avant lui, du dernier après lui, interpolée entre. */
export function valueAtWhole(points, whole) {
  if (!points.length) return null;
  const xs = points.map((p) => fnum(p.pos));
  if (whole <= xs[0]) return points[0].value;
  const last = points.length - 1;
  if (whole >= xs[last]) return points[last].value;
  let i = 0;
  while (xs[i + 1] <= whole) i++;
  if (points[i].curve === "step") return points[i].value;
  const t = (whole - xs[i]) / (xs[i + 1] - xs[i]);
  return points[i].value + (points[i + 1].value - points[i].value) * t;
}

/** Libellé d'une cible pour les menus et les en-têtes. */
export function targetLabel(node, project, t, paramName = null) {
  if (t.kind === "gain") return "Volume";
  if (t.kind === "pan") return "Panoramique";
  if (t.kind === "send") {
    const dest = project.nodes.find((n) => n.id === t.to);
    return `Envoi → ${dest?.kind === "master" ? "Sortie" : (dest?.name ?? t.to)}`;
  }
  const ins = node.inserts.find((i) => i.id === t.insert);
  return `${ins?.name ?? "Plugin"} : ${paramName ?? `paramètre ${t.param}`}`;
}

/** Texte d'une valeur de la courbe pour une cible. */
export function valueText(range, v, paramText = null) {
  if (range.kind === "db") return v <= 0 ? "-∞ dB" : `${gainToDb(v) >= 0 ? "+" : ""}${gainToDb(v).toFixed(1)} dB`;
  if (range.min === -1 && range.max === 1) return v === 0 ? "C" : `${Math.round(Math.abs(v) * 100)} ${v < 0 ? "G" : "D"}`;
  return paramText ?? String(Math.round(v * 1000) / 1000);
}

/**
 * Valeurs **affichées** pour un nœud à la position `whole` (rondes) : si une courbe de volume / panoramique / niveau d'envoi est en
 * mode lecture, c'est elle qui commande (le fader suit) ; sinon (pas de courbe, trim, désactivée) c'est le réglage manuel.
 */
export function shownValues(node, whole) {
  const read = (target) => node.automation?.find((l) => l.mode === "read" && targetKey(l.target) === target);
  const g = read("gain");
  const p = read("pan");
  return {
    gain: g ? valueAtWhole(g.points, whole) : node.gain,
    pan: p ? valueAtWhole(p.points, whole) : node.pan,
    gainAuto: !!g,
    panAuto: !!p,
    send: (to, manual) => {
      const l = read(`send:${to}`);
      return l ? valueAtWhole(l.points, whole) : manual;
    },
    sendAuto: (to) => !!read(`send:${to}`),
  };
}
