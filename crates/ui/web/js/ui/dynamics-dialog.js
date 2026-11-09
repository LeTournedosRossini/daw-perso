// Dynamique d'entrée d'une piste : une porte et un compresseur sur le signal qui entre dans la carte son (avant l'écoute directe et,
// si on le demande, avant la prise). Réglages appliqués en direct (on les entend dans l'écoute directe pendant qu'on les tourne).

export const DEFAULT_DYNAMICS = { enabled: false, gate_db: -90, threshold_db: -18, ratio: 3, attack_ms: 10, release_ms: 120, makeup_db: 0, record: false };

/** Les curseurs : clé, libellé, min, max, pas, unité, aide. */
export const FIELDS = [
  { key: "gate_db", label: "Porte", min: -90, max: -20, step: 1, unit: "dB", off: -90, help: "Sous ce niveau le signal est étouffé (souffle, bruit de pièce). À fond à gauche : pas de porte." },
  { key: "threshold_db", label: "Seuil", min: -60, max: 0, step: 1, unit: "dB", help: "Au-dessus de ce niveau, le compresseur réduit le signal." },
  { key: "ratio", label: "Rapport", min: 1, max: 20, step: 0.5, unit: ":1", help: "1 : rien ; 3 : doux ; 8 et plus : presque un limiteur." },
  { key: "attack_ms", label: "Attaque", min: 0.1, max: 100, step: 0.1, unit: "ms", help: "Vitesse à laquelle la compression se met en route." },
  { key: "release_ms", label: "Relâchement", min: 10, max: 2000, step: 10, unit: "ms", help: "Vitesse à laquelle le signal retrouve son niveau." },
  { key: "makeup_db", label: "Compensation", min: 0, max: 24, step: 0.5, unit: "dB", help: "Gain ajouté après la compression." },
];

/** Texte d'une valeur : « −18 dB », « 3:1 », « 10 ms », « — » pour la porte éteinte. */
export function formatValue(f, v) {
  if (f.off !== undefined && v <= f.off) return "—";
  const n = Math.abs(v) >= 100 || Number.isInteger(v) ? String(Math.round(v * 10) / 10) : v.toFixed(1);
  return `${v < 0 ? "−" : ""}${n.replace(/^-/, "").replace(".", ",")} ${f.unit}`.replace(/^−0 /, "0 ");
}

/** Résumé pour l'info-bulle du menu. */
export function describeDynamics(d) {
  if (!d?.enabled) return "désactivée";
  const parts = [];
  if (d.gate_db > -90) parts.push(`porte ${formatValue(FIELDS[0], d.gate_db)}`);
  parts.push(`compresseur ${formatValue(FIELDS[1], d.threshold_db)} ${formatValue(FIELDS[2], d.ratio)}`);
  return parts.join(" · ") + (d.record ? " · enregistrée" : "");
}

const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

export function openDynamicsDialog({ client, node }) {
  let spec = { ...DEFAULT_DYNAMICS, ...(node.dynamics ?? {}) };
  const dlg = document.createElement("dialog");
  dlg.className = "group-dialog dyn-dialog";
  const form = h("form");
  form.method = "dialog";
  form.append(h("h2", "", `Dynamique d'entrée de « ${node.name} »`));
  form.append(h("p", "gd-empty", "Dompte l'entrée avant tout le reste : porte (souffle) puis compresseur. Tu l'entends tout de suite dans l'écoute directe. La prise reste brute, sauf si tu coches l'option."));
  let timer = null;
  const send = () => {
    clearTimeout(timer);
    timer = setTimeout(() => client.send({ type: "set_track_dynamics", node: node.id, dynamics: spec }), 60);
  };
  const on = h("label", "dyn-check");
  const onBox = h("input");
  onBox.type = "checkbox";
  onBox.checked = spec.enabled;
  onBox.addEventListener("change", () => {
    spec.enabled = onBox.checked;
    send();
  });
  on.append(onBox, h("span", "", " Activer la dynamique d'entrée"));
  form.append(on);
  const outs = new Map();
  const inputs = new Map();
  for (const f of FIELDS) {
    const row = h("label", "dyn-row");
    row.title = f.help;
    const name = h("span", "dyn-name", f.label);
    const r = h("input");
    r.type = "range";
    r.min = f.min;
    r.max = f.max;
    r.step = f.step;
    r.value = spec[f.key];
    r.setAttribute("aria-label", f.label);
    const out = h("output", "dyn-out");
    out.textContent = formatValue(f, spec[f.key]);
    r.addEventListener("input", () => {
      spec[f.key] = Number(r.value);
      out.textContent = formatValue(f, spec[f.key]);
      send();
    });
    r.addEventListener("dblclick", () => {
      r.value = String(DEFAULT_DYNAMICS[f.key]);
      r.dispatchEvent(new Event("input"));
    });
    outs.set(f.key, out);
    inputs.set(f.key, r);
    row.append(name, r, out);
    form.append(row);
  }
  const rec = h("label", "dyn-check");
  const recBox = h("input");
  recBox.type = "checkbox";
  recBox.checked = spec.record;
  recBox.addEventListener("change", () => {
    spec.record = recBox.checked;
    send();
  });
  rec.title = "Si coché, la prise contient le signal traité : irréversible. Sinon la prise reste brute et tu peux tout refaire.";
  rec.append(recBox, h("span", "", " Enregistrer le signal traité (sinon la prise reste brute)"));
  form.append(rec);
  const buttons = h("div", "gd-buttons");
  const reset = h("button", "", "Valeurs par défaut");
  reset.type = "button";
  reset.addEventListener("click", () => {
    spec = { ...DEFAULT_DYNAMICS, enabled: spec.enabled };
    for (const f of FIELDS) {
      inputs.get(f.key).value = String(spec[f.key]);
      outs.get(f.key).textContent = formatValue(f, spec[f.key]);
    }
    recBox.checked = false;
    send();
  });
  const close = h("button", "primary", "Fermer");
  close.type = "submit";
  buttons.append(reset, h("span", "grow"), close);
  form.append(buttons);
  form.addEventListener("submit", (e) => e.preventDefault());
  dlg.append(form);
  dlg.addEventListener("close", () => {
    clearTimeout(timer);
    client.send({ type: "set_track_dynamics", node: node.id, dynamics: spec });
    dlg.remove();
  });
  form.addEventListener("submit", () => dlg.close());
  dlg.addEventListener("keydown", (k) => k.stopPropagation());
  document.body.append(dlg);
  dlg.showModal();
  return dlg;
}
