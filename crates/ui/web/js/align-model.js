// Calage de prises : calculs purs de la page (fiche de réglages, courbe de force, consonnes, tracés, textes). La fiche est celle de la
// bibliothèque de calage (JSON « flibuste-align-settings ») : un bloc `general` (se copie d'un double à l'autre) et un bloc `local`
// (courbe de force : ne se copie jamais). Une valeur absente prend la valeur par défaut côté moteur.

export const ENGINES = [
  ["signalsmith", "Signalsmith (rapide, recommandé)"],
  ["rubberband_r2", "Rubber Band R2"],
  ["rubberband_r3", "Rubber Band R3 (lent)"],
];
export const MODES = [
  ["same_pitch", "Même hauteur (défaut)"],
  ["harmony", "Harmonie (une autre hauteur : tierce, quinte, octave)"],
];

export const DEFAULT_GENERAL = { analysis_mode: "same_pitch", tightness: 0.95, max_shift_seconds: 1.5, engine: "signalsmith", strength: 1, protect: 1 };

/** Réglages qui exigent de **refaire l'analyse** quand ils changent (les autres ne demandent que le rendu). */
export const ANALYSIS_KEYS = ["analysis_mode", "tightness", "max_shift_seconds"];

/** Fiche complète d'après un texte JSON (valeurs absentes ou invalides : défauts). Jamais d'exception. */
export function parseSettings(text) {
  let v = {};
  try {
    v = JSON.parse(text ?? "{}") ?? {};
  } catch {
    v = {};
  }
  const g = { ...DEFAULT_GENERAL, ...(v.general ?? {}) };
  const num = (x, lo, hi, d) => (typeof x === "number" && Number.isFinite(x) ? Math.min(hi, Math.max(lo, x)) : d);
  const general = {
    analysis_mode: MODES.some((m) => m[0] === g.analysis_mode) ? g.analysis_mode : DEFAULT_GENERAL.analysis_mode,
    tightness: num(g.tightness, 0, 1, DEFAULT_GENERAL.tightness),
    max_shift_seconds: num(g.max_shift_seconds, 0.1, 5, DEFAULT_GENERAL.max_shift_seconds),
    engine: ENGINES.some((e) => e[0] === g.engine) ? g.engine : DEFAULT_GENERAL.engine,
    strength: num(g.strength, 0, 1, DEFAULT_GENERAL.strength),
    protect: num(g.protect, 0, 1, DEFAULT_GENERAL.protect),
  };
  return { format: "flibuste-align-settings", version: 1, general, local: { strength_curve: cleanCurve(v.local?.strength_curve) } };
}

export const stringifySettings = (s) => JSON.stringify({ format: "flibuste-align-settings", version: 1, general: s.general, local: { strength_curve: s.local.strength_curve } });

/** Fiche avec le bloc général de `src` et le bloc local de `dst` (même règle que la bibliothèque : la courbe de force ne se copie jamais). */
export function mergeGeneral(dst, src) {
  return { ...dst, general: { ...src.general }, local: { strength_curve: dst.local.strength_curve.map((p) => [...p]) } };
}

/** Courbe de force : paires `[temps s, force 0..1]` triées par temps, valeurs bornées, temps distincts. */
export function cleanCurve(c) {
  if (!Array.isArray(c)) return [];
  const pts = c.filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1])).map((p) => [Math.max(0, p[0]), Math.min(1, Math.max(0, p[1]))]);
  pts.sort((a, b) => a[0] - b[0]);
  return pts.filter((p, i) => i === 0 || p[0] > pts[i - 1][0]);
}

/** Force à l'instant `t` : la valeur constante `base` s'il n'y a pas de courbe, sinon la courbe (interpolée, plate avant et après). */
export function strengthAt(curve, base, t) {
  if (!curve.length) return base;
  if (t <= curve[0][0]) return curve[0][1];
  for (let i = 1; i < curve.length; i++) {
    if (t <= curve[i][0]) {
      const [t0, v0] = curve[i - 1];
      const [t1, v1] = curve[i];
      return v0 + ((v1 - v0) * (t - t0)) / (t1 - t0);
    }
  }
  return curve.at(-1)[1];
}

/** Ajoute un point (ou en remplace un tout proche, à moins de `eps` secondes). Rend la nouvelle courbe et l'indice du point. */
export function addPoint(curve, t, v, eps = 0.02) {
  const next = curve.map((p) => [...p]);
  const near = next.findIndex((p) => Math.abs(p[0] - t) < eps);
  if (near >= 0) next[near][1] = Math.min(1, Math.max(0, v));
  else next.push([Math.max(0, t), Math.min(1, Math.max(0, v))]);
  const c = next.sort((a, b) => a[0] - b[0]);
  return { curve: c, index: c.findIndex((p) => p[0] === (near >= 0 ? next[near][0] : Math.max(0, t))) };
}

/** Déplace le point `i` ; il reste entre ses voisins (à `eps` près) et dans 0..1 pour la force. */
export function movePoint(curve, i, t, v, eps = 0.01, end = Infinity) {
  const next = curve.map((p) => [...p]);
  if (!next[i]) return next;
  const lo = i > 0 ? next[i - 1][0] + eps : 0;
  const hi = i < next.length - 1 ? next[i + 1][0] - eps : end;
  next[i] = [Math.min(hi, Math.max(lo, t)), Math.min(1, Math.max(0, v))];
  return next;
}

export const removePoint = (curve, i) => curve.filter((_, k) => k !== i).map((p) => [...p]);

/** Consonnes : paires `[début, fin]` en secondes dans le double, triées, valides (fin > début ≥ 0). */
export function cleanConsonants(list) {
  const out = (list ?? []).filter((p) => Array.isArray(p) && Number.isFinite(p[0]) && Number.isFinite(p[1]) && p[0] >= 0 && p[1] > p[0]).map((p) => [p[0], p[1]]);
  return out.sort((a, b) => a[0] - b[0]);
}

/** Temps du guide (secondes) qui correspond à une position `td` du double, d'après le chemin (`hop` secondes par point). */
export function guideTimeOf(path, hop, td) {
  if (!path.length) return 0;
  if (td <= path[0]) return 0;
  for (let i = 1; i < path.length; i++) {
    if (td <= path[i]) {
      const span = path[i] - path[i - 1];
      return (i - 1 + (span > 0 ? (td - path[i - 1]) / span : 0)) * hop;
    }
  }
  return (path.length - 1) * hop;
}

/** Instant du double (secondes) qui correspond à un instant `gt` du guide, d'après le chemin (l'inverse de `guideTimeOf`). */
export function doubleTimeOf(path, hop, gt) {
  if (!path.length) return 0;
  const f = Math.max(0, Math.min(path.length - 1, gt / hop));
  const i = Math.floor(f);
  const j = Math.min(path.length - 1, i + 1);
  return path[i] + (path[j] - path[i]) * (f - i);
}

/**
 * Modifie la consonne `i` (paires `[début, fin]` en secondes dans le double) : `mode` `"l"` (bord gauche), `"r"` (bord droit) ou `"move"`
 * (les deux, décalés de `t - from`). Elle reste entre ses voisines et garde au moins `min` secondes. Rend la nouvelle liste.
 */
export function editConsonant(list, i, mode, t, from = t, min = 0.01) {
  const next = list.map((p) => [...p]);
  const c = next[i];
  if (!c) return next;
  const lo = i > 0 ? next[i - 1][1] : 0;
  const hi = i < next.length - 1 ? next[i + 1][0] : Infinity;
  if (mode === "l") c[0] = Math.max(lo, Math.min(c[1] - min, t));
  else if (mode === "r") c[1] = Math.min(hi, Math.max(c[0] + min, t));
  else {
    const len = c[1] - c[0];
    const a = Math.max(lo, Math.min(hi - len, c[0] + (t - from)));
    c[0] = a;
    c[1] = a + len;
  }
  return next;
}

/** Ajoute une consonne entre `a` et `b` (dans un ordre quelconque) : refusée (liste inchangée) si elle est trop courte ou recouvre une autre. */
export function addConsonant(list, a, b, min = 0.01) {
  const [s, e] = a <= b ? [a, b] : [b, a];
  if (!(e - s >= min) || s < 0 || list.some((p) => s < p[1] && p[0] < e)) return { list: list.map((p) => [...p]), index: -1 };
  const next = [...list.map((p) => [...p]), [s, e]].sort((x, y) => x[0] - y[0]);
  return { list: next, index: next.findIndex((p) => p[0] === s) };
}

/** Tracé du décalage (ms) en `columns` colonnes : pour chacune, le minimum et le maximum du décalage de ses points. */
export function shiftColumns(shift, columns) {
  const cols = [];
  const n = shift.length;
  for (let c = 0; c < columns; c++) {
    const a = Math.floor((c * n) / columns);
    const b = Math.max(a + 1, Math.floor(((c + 1) * n) / columns));
    let lo = Infinity;
    let hi = -Infinity;
    for (let i = a; i < Math.min(b, n); i++) {
      lo = Math.min(lo, shift[i] * 1000);
      hi = Math.max(hi, shift[i] * 1000);
    }
    cols.push(Number.isFinite(lo) ? [lo, hi] : [0, 0]);
  }
  return cols;
}

/** Écart de décalage maximal (ms, en valeur absolue) : sert à régler l'échelle verticale. */
export const maxShiftMs = (shift) => shift.reduce((m, s) => Math.max(m, Math.abs(s) * 1000), 0);

/** Texte d'avancement : « 2 / 4 — Voix 2 · 37 % ». */
export function alignProgressText(e) {
  if (e.active) return `Calage ${Math.min(e.done + 1, e.total)} / ${e.total} — ${e.current} · ${Math.round(e.progress * 100)} %`;
  if (e.cancelled) return "Calage annulé";
  if (e.error) return `Calage impossible — ${e.error}`;
  const n = e.clips?.length ?? 0;
  return `${n} prise${n > 1 ? "s" : ""} calée${n > 1 ? "s" : ""}${e.warnings?.length ? ` (${e.warnings.length} avertissement${e.warnings.length > 1 ? "s" : ""})` : ""}`;
}

/** Tous les clips audio du projet : `{ id, name, track, start, len, aligned, muted }`. */
export function audioClips(project) {
  const out = [];
  for (const n of project?.nodes ?? []) {
    if (n.kind === "master" || n.kind === "bus") continue;
    for (const c of n.clips ?? []) if (c.kind === "audio") out.push({ id: c.id, name: c.name, node: n.id, track: n.name, start: c.start, len: c.len, aligned: !!c.aligned, muted: !!c.muted });
  }
  return out;
}

/** Les deux clips se recouvrent-ils dans le temps (le double doit recouvrir la fenêtre du guide) ? */
export const overlaps = (a, b) => a.start < b.start + b.len && b.start < a.start + a.len;

/** Guide proposé et doubles préselectionnés d'après la sélection : le clip principal est le guide, les autres clips audio sélectionnés les doubles. */
export function defaultRoles(project, selection, selected) {
  const clips = audioClips(project);
  const sel = clips.filter((c) => selected?.has?.(c.id) || c.id === selection);
  const guide = clips.find((c) => c.id === selection) ?? sel[0] ?? null;
  return { guide, doubles: sel.filter((c) => c.id !== guide?.id && !c.muted).map((c) => c.id) };
}

// ---- les doubles d'un guide, leurs prises d'origine et leurs prises calées (pour la grande fenêtre de calage) ----------------------

const audioOn = (project, nodeId) => (project?.nodes ?? []).find((n) => n.id === nodeId)?.clips?.filter((c) => c.kind === "audio") ?? [];
const overlap = (c, ws, wl) => Math.max(0, Math.min(c.start + c.len, ws + wl) - Math.max(c.start, ws));

/** Clips d'origine (non calés) de la piste `nodeId` qui recouvrent la fenêtre `{ start, len }`, du plus recouvrant au moins recouvrant (actifs d'abord). */
export function originalsOn(project, nodeId, win) {
  return audioOn(project, nodeId)
    .filter((c) => !c.aligned && overlap(c, win.start, win.len) > 0)
    .sort((a, b) => Number(!!a.muted) - Number(!!b.muted) || overlap(b, win.start, win.len) - overlap(a, win.start, win.len));
}

/** Prises calées de la piste qui recouvrent la fenêtre, la plus récente d'abord (identifiants croissants dans le temps). */
export function alignedOn(project, nodeId, win) {
  return audioOn(project, nodeId)
    .filter((c) => c.aligned && overlap(c, win.start, win.len) > 0)
    .sort((a, b) => b.id - a.id);
}

/** La prise calée à montrer : celle qui est active, sinon la plus récente ; `null` s'il n'y en a pas. */
export function bestAligned(project, nodeId, win) {
  const all = alignedOn(project, nodeId, win);
  return all.find((c) => !c.muted) ?? all[0] ?? null;
}

/**
 * Les doubles possibles d'un guide : une entrée par **piste** (autre que celle du guide) qui a de l'audio d'origine recouvrant la fenêtre
 * du guide. `clipId` : le clip d'origine à caler ; `aligned` : la prise calée à montrer (ou `null`).
 */
export function doublesFor(project, guide, pick) {
  if (!guide) return [];
  const win = { start: guide.start, len: guide.len };
  const out = [];
  for (const n of project?.nodes ?? []) {
    if (n.kind === "master" || n.kind === "bus" || n.id === guide.node) continue;
    const all = originalsOn(project, n.id, win);
    const orig = all[0];
    // Plusieurs clips choisis sur la même piste : ils forment ensemble le double (une seule bande, les trous sont du silence).
    const chosen = pick ? all.filter((c) => pick.has(c.id)).sort((a, b) => a.start - b.start) : [];
    const clipIds = chosen.length > 1 ? chosen.map((c) => c.id) : orig ? [orig.id] : [];
    const al = bestAligned(project, n.id, win);
    if (orig || al) out.push({ node: n.id, track: n.name, clipId: clipIds[0] ?? null, clipIds, aligned: al?.id ?? null });
  }
  return out;
}

/** Le clip audio `id` du projet (tel que la page le reçoit), ou `undefined`. */
export function clipById(project, id) {
  for (const n of project?.nodes ?? []) {
    const c = (n.clips ?? []).find((k) => k.id === id && k.kind === "audio");
    if (c) return c;
  }
  return undefined;
}

/** Clip guide avec sa piste : `{ id, name, node, start, len }` ou `null`. */
export function guideInfo(project, clipId, extra = []) {
  for (const n of project?.nodes ?? []) {
    const c = (n.clips ?? []).find((k) => k.id === clipId && k.kind === "audio");
    if (!c) continue;
    // Des clips de la même piste peuvent s'ajouter au guide : la fenêtre va du début du premier à la fin du dernier.
    const more = extra.map((id) => (n.clips ?? []).find((k) => k.id === id && k.kind === "audio")).filter(Boolean);
    const all = [c, ...more];
    const start = Math.min(...all.map((k) => k.start));
    const end = Math.max(...all.map((k) => k.start + k.len));
    return { id: c.id, ids: all.map((k) => k.id), more: more.map((k) => k.id), name: more.length ? `${c.name} + ${more.length}` : c.name, node: n.id, track: n.name, start, len: end - start };
  }
  return null;
}

/** Position horizontale (pixels) d'un échantillon pour une vue `{ start, spp }`, et l'inverse. */
export const sampleX = (view, s) => (s - view.start) / view.spp;
export const xSample = (view, x) => view.start + x * view.spp;

/** Instant du guide (secondes) → échantillon du projet, d'après le début de la fenêtre. */
export const guideSecondsToSample = (win, sampleRate, t) => win.start + t * sampleRate;
