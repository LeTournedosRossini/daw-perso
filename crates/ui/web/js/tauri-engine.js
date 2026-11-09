// Réalisation du moteur pour la fenêtre Tauri : les commandes partent par `invoke` vers le processus Rust, les événements
// du moteur reviennent par `listen`. Même protocole que le faux moteur (docs/protocole-ui.md).

import { EngineClient } from "./engine-client.js";
import { validateCommand } from "./protocol.js";

export class TauriEngine extends EngineClient {
  #tauri;

  constructor(tauri) {
    super();
    this.#tauri = tauri;
  }

  /** À appeler (et attendre) avant la première commande : sans abonnement, la réponse à `get_project` serait perdue. */
  async start() {
    await this.#tauri.event.listen("engine-event", (e) => {
      const p = e.payload;
      if (p && typeof p.type === "string") this.emit(p.type, p);
    });
  }

  send(msg) {
    const err = validateCommand(msg);
    if (err) {
      this.emit("error", { command: msg?.type ?? "?", message: err });
      return false;
    }
    // Sans attente : la réponse arrive par un événement. Un échec de transmission est signalé comme une erreur du moteur.
    this.#tauri.core.invoke("send_command", { msg }).catch((e) => this.emit("error", { command: msg.type, message: `transmission impossible : ${e}` }));
    return true;
  }
}
