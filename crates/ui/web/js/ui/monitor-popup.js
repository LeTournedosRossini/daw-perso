// Adresse et code QR d'un retour de monitoring : le musicien scanne le code avec sa tablette et règle son propre mix.
// Le serveur web local n'existe que si on le démarre ici ; il s'arrête avec le bouton « Arrêter le serveur » (ou en quittant l'application).

import { closeMenu, showMenu } from "./track-menu.js";

/** Ouvre la fenêtre du retour `node` sous `anchor` ; démarre le serveur s'il ne tourne pas. */
export function openMonitorPopup(anchor, state, client, node) {
  const menu = document.createElement("div");
  menu.className = "track-menu monitor-pop";
  menu.setAttribute("role", "dialog");
  menu.setAttribute("aria-label", `Retour ${node.name}`);
  // Journal des commandes reçues par le serveur (les 12 dernières de ce retour, rafraîchi à l'ouverture et sur demande).
  let entries = [];
  const journalBox = () => {
    const box = document.createElement("details");
    box.className = "mp-log";
    const sum = document.createElement("summary");
    sum.textContent = "Journal des commandes reçues";
    const list = document.createElement("div");
    list.className = "mp-loglist";
    const mine = entries.filter((e) => e.ret === node.id || e.ret === null).slice(-12).reverse();
    list.replaceChildren(...(mine.length ? mine : [Object.assign(document.createElement("div"), { textContent: "Rien reçu pour l'instant." })]).map((e) => {
      const row = document.createElement("div");
      row.className = "mp-logrow" + (e.status === 200 ? "" : " bad");
      const t = e.t_ms ? new Date(e.t_ms).toLocaleTimeString() : "";
      row.textContent = e.route ? `${t} · ${e.ip} · ${e.route} · ${e.status} · ${e.what}` : e.textContent;
      return row;
    }));
    const refresh = document.createElement("button");
    refresh.textContent = "Actualiser";
    refresh.addEventListener("click", () => client.send({ type: "get_monitor_log" }));
    box.append(sum, list, refresh);
    box.open = open;
    box.addEventListener("toggle", () => { open = box.open; if (box.open) client.send({ type: "get_monitor_log" }); });
    return box;
  };
  let open = false;
  const render = () => {
    const m = state.monitor;
    const ret = m?.active ? m.returns.find((r) => r.node === node.id) : null;
    menu.replaceChildren();
    const title = document.createElement("div");
    title.className = "mp-title";
    title.textContent = `Retour « ${node.name} »`;
    menu.append(title);
    if (m?.error) {
      const e = document.createElement("div");
      e.className = "mp-err";
      e.textContent = m.error;
      menu.append(e);
    } else if (!ret) {
      const w = document.createElement("div");
      w.className = "mp-note";
      w.textContent = m?.active ? "Ce retour n'est pas encore prêt…" : "Démarrage du serveur…";
      menu.append(w);
    } else {
      const qr = document.createElement("div");
      qr.className = "mp-qr";
      qr.innerHTML = ret.qr; // SVG produit par l'hôte (code QR) : contenu de confiance
      qr.setAttribute("role", "img");
      qr.setAttribute("aria-label", "Code QR de l'adresse du retour");
      const url = document.createElement("code");
      url.className = "mp-url";
      url.textContent = ret.url;
      const note = document.createElement("div");
      note.className = "mp-note";
      note.textContent = "Scanne le code avec la tablette (même réseau que cet ordinateur). L'adresse est fixe : enregistrée avec le projet, elle ne change pas quand on relance l'application. Elle est secrète : elle donne accès au réglage de ce retour.";
      const copy = document.createElement("button");
      copy.textContent = "Copier l'adresse";
      copy.addEventListener("click", () => navigator.clipboard?.writeText(ret.url).then(() => (copy.textContent = "Copiée ✓"), () => (copy.textContent = "Copie impossible")));
      const renew = document.createElement("button");
      renew.textContent = "Nouvelle adresse";
      renew.title = "Change l'adresse de ce retour : l'ancienne cesse de fonctionner (à refaire scanner)";
      renew.addEventListener("click", () => {
        if (window.confirm(`Donner une nouvelle adresse au retour « ${node.name} » ? L'ancienne cessera de fonctionner : la tablette devra scanner de nouveau.`)) client.send({ type: "renew_monitor_address", node: node.id });
      });
      const role = document.createElement("label");
      role.className = "mp-role";
      role.append("Cette adresse permet ");
      const sel = document.createElement("select");
      sel.setAttribute("aria-label", "Ce que permet cette adresse");
      sel.append(
        new Option("de régler ce retour seulement", "0"),
        new Option("ce retour + la télécommande (lecture, enregistrement, sections, pré-roll)", "1"),
        new Option("ce retour + la régie (niveau, pan, muet, solo, armement de toutes les pistes)", "2"),
        new Option("ce retour + la télécommande + la régie", "3"),
      );
      sel.value = String(ret.role ?? 0);
      sel.addEventListener("change", () => client.send({ type: "set_monitor_role", node: node.id, role: Number(sel.value) }));
      role.append(sel);
      const warn = document.createElement("div");
      warn.className = "mp-note";
      const remote = (ret.role & 1) !== 0;
      const desk = (ret.role & 2) !== 0;
      warn.textContent = `${remote ? "Avec la télécommande, quiconque a cette adresse peut lancer la lecture et l'enregistrement (appui long). " : ""}${desk ? "Avec la régie, quiconque a cette adresse peut régler et armer toutes les pistes. " : ""}« Nouvelle adresse » révoque ces droits.${remote ? " Active l'arrêt automatique dans les Réglages si le réseau est incertain." : ""}`;
      warn.hidden = ret.role === 0;
      const expired = document.createElement("div");
      expired.className = "mp-err";
      expired.hidden = !ret.expired;
      expired.textContent = "Les droits de cette adresse ont expiré : elle ne règle plus que son retour. Choisis de nouveau le droit ci-dessus (ou « Renouveler ») pour les redonner.";
      const renewRights = document.createElement("button");
      renewRights.textContent = "Renouveler les droits";
      renewRights.hidden = !ret.expired;
      renewRights.addEventListener("click", () => client.send({ type: "set_monitor_role", node: node.id, role: ret.role ?? 0 }));
      menu.append(qr, url, note, role, warn, expired, renewRights, copy, renew, journalBox());
    }
    if (m?.active) {
      const stop = document.createElement("button");
      stop.textContent = "Arrêter le serveur";
      stop.addEventListener("click", () => {
        client.send({ type: "stop_monitor" });
        closeMenu();
      });
      menu.append(stop);
    }
  };
  const off = client.on("monitor", () => (menu.isConnected ? render() : off?.()));
  const offLog = client.on("monitor_log", (e) => { entries = e.entries ?? []; if (menu.isConnected) render(); else offLog?.(); });
  render();
  showMenu(menu, anchor);
  if (!state.monitor?.active) client.send({ type: "start_monitor" });
}
