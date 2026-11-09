// Filtre d'entrée MIDI d'une piste : une liste de règles « appareil + canal ». Un message passe s'il correspond à au moins une règle ;
// sans règle, tout passe. Exemple : « clavier A, canal 1 » et « pad B, tous les canaux » sur la même piste.

const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

/** Texte résumé d'un filtre, pour les info-bulles : « tout passe » ou « Clavier A (1) ; Pad B ». */
export function describeFilter(rules) {
  if (!rules?.length) return "tout passe";
  return rules.map((r) => `${r.device || "tout appareil"}${r.channels?.length ? ` (canal ${r.channels.join(", ")})` : ""}`).join(" ; ");
}

/** Règles à envoyer : un appareil vide = tous ; `channel` 0 = tous les canaux. Les lignes identiques sont fusionnées. */
export function rulesFromRows(rows) {
  const seen = new Set();
  const out = [];
  for (const { device, channel } of rows) {
    const key = `${device}|${channel}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push({ device, channels: channel ? [channel] : [] });
  }
  return out;
}

/** Lignes de la fenêtre d'après les règles reçues : une ligne par canal (une règle à plusieurs canaux donne plusieurs lignes). */
export function rowsFromRules(rules) {
  const rows = [];
  for (const r of rules ?? []) {
    if (!r.channels?.length) rows.push({ device: r.device ?? "", channel: 0 });
    else for (const c of r.channels) rows.push({ device: r.device ?? "", channel: c });
  }
  return rows;
}

export function openMidiFilterDialog({ client, project, node }) {
  const devices = project.midi_devices ?? [];
  let rows = rowsFromRules(node.midi_filter);
  const dlg = document.createElement("dialog");
  dlg.className = "group-dialog";
  const form = h("form");
  form.method = "dialog";
  const title = h("h2", "", `Filtre d'entrée MIDI de « ${node.name} »`);
  const note = h("p", "gd-empty", "Sans filtre, tous les appareils et tous les canaux arrivent sur cette piste. Avec des filtres, seuls les messages qui correspondent à l'un d'eux passent.");
  const list = h("div", "mf-rows");
  const render = () => {
    list.replaceChildren();
    if (!rows.length) list.append(h("p", "gd-empty", "Aucun filtre : tout passe."));
    rows.forEach((row, i) => {
      const line = h("div", "mf-row");
      const dev = h("select");
      dev.setAttribute("aria-label", "Appareil");
      const options = [["", "Tous les appareils"], ...devices.map((d) => [d, d])];
      if (row.device && !devices.includes(row.device)) options.push([row.device, `${row.device} (absent)`]);
      for (const [value, label] of options) {
        const o = h("option", "", label);
        o.value = value;
        dev.append(o);
      }
      dev.value = row.device;
      dev.addEventListener("change", () => (row.device = dev.value));
      const ch = h("select");
      ch.setAttribute("aria-label", "Canal");
      for (const c of [0, ...Array.from({ length: 16 }, (_, k) => k + 1)]) {
        const o = h("option", "", c === 0 ? "Tous les canaux" : `Canal ${c}`);
        o.value = String(c);
        ch.append(o);
      }
      ch.value = String(row.channel);
      ch.addEventListener("change", () => (row.channel = Number(ch.value)));
      const del = h("button", "", "✕");
      del.type = "button";
      del.title = "Retirer ce filtre";
      del.setAttribute("aria-label", "Retirer ce filtre");
      del.addEventListener("click", () => {
        rows.splice(i, 1);
        render();
      });
      line.append(dev, ch, del);
      list.append(line);
    });
  };
  render();
  const add = h("button", "", "＋ Ajouter un filtre");
  add.type = "button";
  add.addEventListener("click", () => {
    rows.push({ device: "", channel: 0 });
    render();
  });
  const buttons = h("div", "gd-buttons");
  const cancel = h("button", "", "Annuler");
  cancel.type = "button";
  cancel.addEventListener("click", () => dlg.close());
  const ok = h("button", "primary", "Enregistrer");
  ok.type = "submit";
  buttons.append(add, h("span", "grow"), cancel, ok);
  form.append(title, note, list, buttons);
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    client.send({ type: "set_track_midi_filter", node: node.id, rules: rulesFromRows(rows) });
    dlg.close();
  });
  dlg.append(form);
  dlg.addEventListener("close", () => dlg.remove());
  dlg.addEventListener("keydown", (k) => k.stopPropagation());
  document.body.append(dlg);
  dlg.showModal();
  return dlg;
}
