// Courbe de réponse de la vélocité d'une piste MIDI : entre les notes (jouées en direct ou lues dans les clips) et les plugins de la
// piste. Une courbe de concave à convexe (avec adoucissement en S) et deux poignées sur les bords : la vélocité de sortie minimale
// (entrée 1) et maximale (entrée 127). Les poignées au même niveau donnent une vélocité fixe. L'enregistrement MIDI reste brut.
// Réglages appliqués en direct ; un point montre ce que devient chaque note jouée sur le clavier.

import { DEFAULT_CURVE, hitHandle, mapVelocity, normalizeCurve, yToVelocity } from "../velocity-model.js";
import { pixelRatio } from "../ui-scale.js";

const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

export function openVelocityDialog({ client, node }) {
  let spec = normalizeCurve({ ...DEFAULT_CURVE, ...(node.velocity_curve ?? {}) });
  let lastNote = null; // { input, output } de la dernière note jouée sur la piste
  const dlg = document.createElement("dialog");
  dlg.className = "group-dialog vel-dialog";
  const form = h("form");
  form.method = "dialog";
  form.append(h("h2", "", `Courbe de vélocité de « ${node.name} »`));
  form.append(h("p", "gd-empty", "Change la force des notes entre le clavier (ou les clips) et les plugins de la piste. Tire les deux poignées des bords pour régler la vélocité de sortie minimale et maximale. L'enregistrement reste brut."));
  let timer = null;
  const send = () => {
    clearTimeout(timer);
    timer = setTimeout(() => client.send({ type: "set_track_velocity_curve", node: node.id, curve: spec }), 40);
  };
  const on = h("label", "dyn-check");
  const onBox = h("input");
  onBox.type = "checkbox";
  onBox.checked = spec.enabled;
  onBox.addEventListener("change", () => {
    spec.enabled = onBox.checked;
    redraw();
    send();
  });
  on.append(onBox, h("span", "", " Activer la courbe de vélocité"));
  form.append(on);

  // Tracé : entrée 1 à 127 en abscisse, sortie 1 à 127 en ordonnée.
  const SIZE = 220;
  const canvas = h("canvas", "vel-plot");
  canvas.style.width = `${SIZE + 36}px`;
  canvas.style.height = `${SIZE + 36}px`;
  canvas.setAttribute("aria-label", "Courbe de réponse de la vélocité : tirer les poignées des bords");
  form.append(canvas);
  const PAD = 18;
  const redraw = () => {
    const dpr = pixelRatio();
    const W = SIZE + 2 * PAD;
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(W * dpr);
    const g = canvas.getContext("2d");
    g.setTransform(dpr, 0, 0, dpr, PAD * dpr, PAD * dpr);
    g.clearRect(-PAD, -PAD, W, W);
    g.fillStyle = css("--bg") || "#111";
    g.fillRect(0, 0, SIZE, SIZE);
    g.strokeStyle = css("--line") || "#444";
    g.lineWidth = 1;
    for (let i = 0; i <= 4; i++) {
      g.beginPath();
      g.moveTo((i * SIZE) / 4, 0);
      g.lineTo((i * SIZE) / 4, SIZE);
      g.moveTo(0, (i * SIZE) / 4);
      g.lineTo(SIZE, (i * SIZE) / 4);
      g.stroke();
    }
    g.setLineDash([4, 4]);
    g.beginPath();
    g.moveTo(0, SIZE);
    g.lineTo(SIZE, 0);
    g.stroke();
    g.setLineDash([]);
    const px = (v) => ((v - 1) / 126) * SIZE;
    const py = (v) => SIZE - ((v - 1) / 126) * SIZE;
    const accent = spec.enabled ? css("--accent") || "#4ea1ff" : css("--muted") || "#888";
    g.strokeStyle = accent;
    g.lineWidth = 2.5;
    g.beginPath();
    for (let v = 1; v <= 127; v++) {
      const o = spec.fixed > 0 ? spec.fixed : mapVelocity({ ...spec, fixed: 0 }, v);
      v === 1 ? g.moveTo(px(v), py(o)) : g.lineTo(px(v), py(o));
    }
    g.stroke();
    if (spec.fixed === 0) {
      g.fillStyle = accent;
      for (const [x, y] of [[0, spec.min], [SIZE, spec.max]]) {
        g.beginPath();
        g.arc(x, py(y), 8, 0, Math.PI * 2);
        g.fill();
        g.strokeStyle = css("--bg") || "#111";
        g.lineWidth = 2;
        g.stroke();
      }
    }
    if (lastNote) {
      g.fillStyle = "#e9d34a";
      g.beginPath();
      g.arc(px(lastNote.input), py(lastNote.output), 5, 0, Math.PI * 2);
      g.fill();
    }
    g.fillStyle = css("--muted") || "#888";
    g.font = "10px sans-serif";
    g.fillText("entrée →", SIZE - 46, SIZE + 12);
    g.save();
    g.rotate(-Math.PI / 2);
    g.fillText("sortie →", -SIZE + 4, -6);
    g.restore();
  };
  let dragging = null;
  const point = (e) => {
    const r = canvas.getBoundingClientRect();
    const k = (SIZE + 2 * PAD) / r.width;
    return { x: (e.clientX - r.left) * k - PAD, y: (e.clientY - r.top) * k - PAD };
  };
  canvas.addEventListener("pointerdown", (e) => {
    const p = point(e);
    const id = spec.fixed === 0 ? hitHandle(spec, SIZE, p.x, p.y, e.pointerType === "touch" ? 28 : 16) : null;
    if (!id) return;
    dragging = id;
    canvas.setPointerCapture(e.pointerId);
    e.preventDefault();
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    const v = yToVelocity(SIZE, point(e).y);
    spec[dragging] = e.altKey ? spec[dragging] : v; // Alt : on ne bouge pas (évite un geste involontaire)
    syncFields();
    redraw();
    send();
  });
  const stop = () => (dragging = null);
  canvas.addEventListener("pointerup", stop);
  canvas.addEventListener("pointercancel", stop);

  const rows = [];
  const addRange = (key, label, min, max, step, help, fmt) => {
    const row = h("label", "dyn-row");
    row.title = help;
    const r = h("input");
    r.type = "range";
    r.min = min;
    r.max = max;
    r.step = step;
    r.setAttribute("aria-label", label);
    const out = h("output", "dyn-out");
    r.addEventListener("input", () => {
      spec[key] = Number(r.value);
      out.textContent = fmt(spec[key]);
      redraw();
      send();
    });
    r.addEventListener("dblclick", () => {
      r.value = String(DEFAULT_CURVE[key]);
      r.dispatchEvent(new Event("input"));
    });
    row.append(h("span", "dyn-name", label), r, out);
    form.append(row);
    rows.push(() => {
      r.value = String(spec[key]);
      out.textContent = fmt(spec[key]);
    });
  };
  addRange("bend", "Courbure", -1000, 1000, 10, "Concave (négatif) : les notes douces montent. Convexe (positif) : il faut jouer fort. Double-clic : droite.", (v) => (v === 0 ? "droite" : v > 0 ? `convexe ${Math.round(v / 10)}` : `concave ${Math.round(-v / 10)}`));
  addRange("smooth", "Adoucissement S", 0, 1000, 10, "Mélange la courbe avec un S : plus doux aux extrémités.", (v) => `${Math.round(v / 10)} %`);
  addRange("min", "Sortie minimale", 1, 127, 1, "Vélocité de sortie pour une note jouée au plus doux (poignée du bas à gauche).", String);
  addRange("max", "Sortie maximale", 1, 127, 1, "Vélocité de sortie pour une note jouée au plus fort (poignée du haut à droite).", String);

  // Vélocité fixe : toutes les notes sortent à cette valeur.
  const fixedRow = h("label", "dyn-check");
  const fixedBox = h("input");
  fixedBox.type = "checkbox";
  const fixedNum = h("input");
  fixedNum.type = "number";
  fixedNum.min = "1";
  fixedNum.max = "127";
  fixedNum.setAttribute("aria-label", "Vélocité fixe");
  fixedNum.style.width = "4.5em";
  fixedRow.append(fixedBox, h("span", "", " Vélocité fixe : toutes les notes à "), fixedNum);
  fixedRow.title = "Toutes les notes sortent à cette vélocité, quelle que soit la force jouée (la courbe est ignorée).";
  const applyFixed = () => {
    const v = Math.max(1, Math.min(127, Math.round(Number(fixedNum.value) || 64)));
    spec.fixed = fixedBox.checked ? v : 0;
    fixedNum.value = String(v);
    fixedNum.disabled = !fixedBox.checked;
    redraw();
    send();
  };
  fixedBox.addEventListener("change", applyFixed);
  fixedNum.addEventListener("change", applyFixed);
  fixedNum.addEventListener("keydown", (k) => k.stopPropagation());
  form.append(fixedRow);

  const syncFields = () => {
    for (const f of rows) f();
    fixedBox.checked = spec.fixed > 0;
    fixedNum.value = String(spec.fixed > 0 ? spec.fixed : fixedNum.value || 64);
    fixedNum.disabled = spec.fixed === 0;
  };

  const buttons = h("div", "gd-buttons");
  const reset = h("button", "", "Valeurs par défaut");
  reset.type = "button";
  reset.addEventListener("click", () => {
    spec = { ...DEFAULT_CURVE, enabled: spec.enabled };
    syncFields();
    redraw();
    send();
  });
  const close = h("button", "primary", "Fermer");
  close.type = "submit";
  buttons.append(reset, h("span", "grow"), close);
  form.append(buttons);
  form.addEventListener("submit", (e) => e.preventDefault());
  dlg.append(form);

  // Chaque note jouée en direct sur la piste : le point jaune montre entrée → sortie.
  const off = client.on("note_velocity", (e) => {
    if (e.node !== node.id) return;
    lastNote = { input: e.input, output: spec.enabled ? mapVelocity(spec, e.input) : e.input };
    redraw();
  });
  dlg.addEventListener("close", () => {
    clearTimeout(timer);
    off?.();
    client.send({ type: "set_track_velocity_curve", node: node.id, curve: spec });
    dlg.remove();
  });
  form.addEventListener("submit", () => dlg.close());
  dlg.addEventListener("keydown", (k) => k.stopPropagation());
  document.body.append(dlg);
  dlg.showModal();
  syncFields();
  redraw();
  return dlg;
}
