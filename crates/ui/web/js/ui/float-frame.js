// Fenêtre flottante déplaçable par son bandeau et redimensionnable par une poignée à nous (coin bas droit). La poignée native du navigateur
// (`resize: both`) ne marche pas partout (WebKitGTK sous Linux : le curseur change mais rien ne bouge dès qu'un canevas recouvre le coin) ;
// celle-ci passe par des événements de pointeur (souris et doigt), à l'échelle de l'interface, et mémorise position et taille.

import { dragPos } from "../window-pos.js";

const zoom = () => parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--ui-zoom")) || 1;
const store = {
  get: (k) => { try { return localStorage.getItem(k); } catch { return null; } },
  set: (k, v) => { try { localStorage.setItem(k, v); } catch { /* stockage indisponible : la place ne sera pas mémorisée */ } },
};

/**
 * `root` : la fenêtre (position fixe). Options : `drag` (déplacement par le bandeau `header`, vrai par défaut), `key` (mémorise position et taille),
 * `minW` / `minH`, `onResize` (appelé pendant le redimensionnement).
 */
export function makeFloatable(root, { drag = true, key = null, minW = 360, minH = 240, onResize = () => {} } = {}) {
  const grip = document.createElement("div");
  grip.className = "float-grip";
  grip.setAttribute("aria-hidden", "true");
  grip.title = "Redimensionner";
  root.append(grip);

  const place = (f) => {
    if (Number.isFinite(f.left)) root.style.left = `${f.left}px`;
    if (Number.isFinite(f.top)) root.style.top = `${f.top}px`;
    if (Number.isFinite(f.width)) root.style.width = `${Math.max(minW, f.width)}px`;
    if (Number.isFinite(f.height)) root.style.height = `${Math.max(minH, f.height)}px`;
  };
  const save = () => {
    if (!key) return;
    const z = zoom();
    const r = root.getBoundingClientRect();
    store.set(key, JSON.stringify({ left: r.left / z, top: r.top / z, width: r.width / z, height: r.height / z }));
  };
  if (key) {
    try {
      const f = JSON.parse(store.get(key) ?? "null");
      if (f && typeof f === "object") {
        const z = zoom();
        const vw = window.innerWidth / z;
        const vh = window.innerHeight / z;
        place({ left: Math.min(Math.max(f.left, 80 - f.width), vw - 80), top: Math.min(Math.max(f.top, 0), vh - 40), width: Math.min(f.width, vw), height: Math.min(f.height, vh) });
      }
    } catch { /* position mémorisée illisible : on garde celle de la feuille de style */ }
  }

  // Redimensionnement par la poignée.
  let rs = null;
  grip.addEventListener("pointerdown", (e) => {
    if (e.button > 0) return;
    const z = zoom();
    const r = root.getBoundingClientRect();
    rs = { id: e.pointerId, x: e.clientX, y: e.clientY, w: r.width / z, h: r.height / z, left: r.left / z, top: r.top / z };
    grip.setPointerCapture?.(e.pointerId);
    e.preventDefault();
    e.stopPropagation();
  });
  grip.addEventListener("pointermove", (e) => {
    if (!rs || e.pointerId !== rs.id) return;
    const z = zoom();
    const vw = window.innerWidth / z;
    const vh = window.innerHeight / z;
    place({ width: Math.min(rs.w + (e.clientX - rs.x) / z, vw - rs.left), height: Math.min(rs.h + (e.clientY - rs.y) / z, vh - rs.top) });
    onResize();
  });
  const endResize = (e) => {
    if (!rs || e.pointerId !== rs.id) return;
    rs = null;
    save();
    onResize();
  };
  grip.addEventListener("pointerup", endResize);
  grip.addEventListener("pointercancel", endResize);

  // Déplacement par le bandeau (pas depuis un bouton, un menu ou un champ).
  if (drag) {
    const head = root.querySelector("header");
    let d = null;
    head?.addEventListener("pointerdown", (e) => {
      if (e.button > 0 || e.target.closest("button, select, input, label, option, textarea")) return;
      const r = root.getBoundingClientRect();
      const z = zoom();
      d = { id: e.pointerId, x: e.clientX, y: e.clientY, left: r.left / z, top: r.top / z };
      head.setPointerCapture?.(e.pointerId);
      e.preventDefault();
    });
    head?.addEventListener("pointermove", (e) => {
      if (!d || e.pointerId !== d.id) return;
      const z = zoom();
      place(dragPos(d, e.clientX - d.x, e.clientY - d.y, z, { vw: window.innerWidth / z, vh: window.innerHeight / z, w: root.offsetWidth }));
    });
    const end = (e) => {
      if (!d || e.pointerId !== d.id) return;
      d = null;
      save();
    };
    head?.addEventListener("pointerup", end);
    head?.addEventListener("pointercancel", end);
  }
  return { grip, save };
}
