// Taille de l'interface : un facteur appliqué à toute la page (propriété CSS `zoom` sur la racine), mémorisé d'une session à
// l'autre. Les canevas tiennent compte du facteur (`pixelRatio`) pour rester nets, et les coordonnées de la souris sont
// ramenées dans le repère du canevas (`canvasPoint`) : tout reste juste quelle que soit la taille choisie.

export const SCALE_STEPS = [60, 70, 80, 90, 100, 110, 125, 150, 175];
const KEY = "daw-ui-scale";
let percent = 70;
let manual = false; // vrai : réglage de l'utilisateur (mémorisé), qui prime sur le calcul d'après l'écran

/** Écran de référence (pixels CSS) sur lequel l'interface fait 70 %. */
const REF = { w: 1920, h: 1080, percent: 70 };

const nearestStep = (p) => SCALE_STEPS.reduce((best, s) => (Math.abs(s - p) < Math.abs(best - p) ? s : best), 100);

function apply() {
  const root = document.documentElement;
  root.style.zoom = percent === 100 ? "" : String(percent / 100);
  root.style.setProperty("--ui-zoom", String(percent / 100));
  window.dispatchEvent(new CustomEvent("ui-scale-change", { detail: percent }));
}

/** Taille choisie d'après l'écran : 70 % sur 1920×1080, proportionnelle au côté le plus limitant, dans les bornes des crans. */
export function autoScale(width, height) {
  if (!(width > 0) || !(height > 0)) return REF.percent;
  const p = REF.percent * Math.min(width / REF.w, height / REF.h);
  return nearestStep(Math.max(SCALE_STEPS[0], Math.min(125, p)));
}

const screenScale = () => autoScale(screen.availWidth, screen.availHeight);

/** Lit le réglage mémorisé (ou `?scale=80` dans l'adresse) ; sans réglage, calcule la taille d'après l'écran. */
export function initScale() {
  let p = 0;
  try {
    p = Number(new URLSearchParams(location.search).get("scale")) || Number(localStorage.getItem(KEY)) || 0;
  } catch {
    // Stockage indisponible (fenêtre privée, données bloquées) : taille automatique.
  }
  manual = p > 0;
  percent = manual ? nearestStep(p) : screenScale();
  apply();
  return percent;
}

/** Revient au calcul automatique et oublie le réglage mémorisé. */
export function resetScale() {
  manual = false;
  try {
    localStorage.removeItem(KEY);
  } catch {
    // Rien à oublier si le stockage est indisponible.
  }
  percent = screenScale();
  apply();
  return percent;
}

export const isManualScale = () => manual;

/** En mode automatique, suit un changement d'écran (fenêtre déplacée sur un autre moniteur). */
export function refreshAutoScale() {
  if (manual) return percent;
  const p = screenScale();
  if (p !== percent) {
    percent = p;
    apply();
  }
  return percent;
}

export const getScale = () => percent;

export function setScale(p) {
  manual = true;
  percent = nearestStep(p);
  try {
    localStorage.setItem(KEY, String(percent));
  } catch {
    // Pas de mémorisation possible : le réglage reste valable pour cette session.
  }
  apply();
  return percent;
}

/** Cran suivant (+1) ou précédent (−1). */
export function stepScale(direction) {
  const i = SCALE_STEPS.indexOf(percent);
  return setScale(SCALE_STEPS[Math.max(0, Math.min(SCALE_STEPS.length - 1, i + direction))]);
}

/** Pixels du canevas par pixel CSS : densité de l'écran × taille choisie (sinon un canevas agrandi serait flou). */
export const pixelRatio = () => (window.devicePixelRatio || 1) * (percent / 100);

/** Position d'un événement de souris dans le repère (en pixels CSS non zoomés) du canevas. */
export function canvasPoint(canvas, e) {
  const r = canvas.getBoundingClientRect();
  const kx = r.width > 0 ? canvas.clientWidth / r.width : 1;
  const ky = r.height > 0 ? canvas.clientHeight / r.height : 1;
  return { x: (e.clientX - r.left) * kx, y: (e.clientY - r.top) * ky };
}
