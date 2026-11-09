// Adresse et code QR d'un retour de monitoring : le musicien scanne le code avec sa tablette et règle son propre mix.
// Le serveur web local n'existe que si on le démarre ici ; il s'arrête avec le bouton « Arrêter le serveur » (ou en quittant l'application).

import { closeMenu, showMenu } from "./track-menu.js";

/** Ouvre la fenêtre du retour `node` sous `anchor` ; démarre le serveur s'il ne tourne pas. */
export function openMonitorPopup(anchor, state, client, node) {
  const menu = document.createElement("div");
  menu.className = "track-menu monitor-pop";
  menu.setAttribute("role", "dialog");
  menu.setAttribute("aria-label", `Retour ${node.name}`);
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
      menu.append(qr, url, note, copy, renew);
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
  render();
  showMenu(menu, anchor);
  if (!state.monitor?.active) client.send({ type: "start_monitor" });
}
