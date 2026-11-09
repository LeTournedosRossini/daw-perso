// Calage de prises : le bouton « Caler… » de la barre ouvre la grande fenêtre (`align-window.js`) ; ce contrôleur relaie l'avancement du
// calage (indicateur de la barre, messages d'état, erreurs) et fait de la fenêtre un singleton. Les calculs sont dans `align-model.js`.

import { alignProgressText } from "../align-model.js";
import { AlignWindow } from "./align-window.js";

const ERRORS = ["align_clips", "align_redo", "get_align", "merge_align_settings", "listen_align"];

export class AlignController {
  constructor(state, client, say, peaks) {
    this.state = state;
    this.client = client;
    this.say = say;
    this.peaks = peaks;
    this.running = false;
    this.window = null;
    this.status = document.getElementById("align-status");
    document.getElementById("btn-align")?.addEventListener("click", () => this.open());
    this.status?.addEventListener("click", () => this.client.send({ type: "cancel_align" }));
    client.on("align", (e) => this.#onAlign(e));
    client.on("error", (e) => {
      if (ERRORS.includes(e.command)) {
        this.say([e.message]);
        this.window?.fail(e.message);
      }
    });
  }

  /** Ouvre la fenêtre de calage : le clip sélectionné est le guide, les clips audio sélectionnés sur d'autres pistes les doubles. */
  open() {
    if (this.window?.dlg.open) return;
    this.window ??= new AlignWindow({ state: this.state, client: this.client, peaks: this.peaks, controller: this });
    this.window.open();
  }

  windowClosed() {
    /* la fenêtre se garde pour la prochaine ouverture */
  }

  #onAlign(e) {
    this.running = e.active;
    if (this.status) {
      this.status.hidden = !e.active;
      if (e.active) this.status.textContent = alignProgressText(e);
    }
    this.window?.onAlign(e);
    if (!e.active && e.clips?.length) {
      // Les voies de prises des pistes calées s'ouvrent : l'original apparaît sous la prise calée, un clic le réactive.
      const ids = [...new Set(e.clips.map((c) => this.state.project?.nodes?.find((n) => n.name === c.track)?.id).filter((id) => id != null))];
      this.onTakes?.(ids);
    }
    if (!e.active) this.say([alignProgressText(e), ...(e.warnings ?? [])]);
  }
}
