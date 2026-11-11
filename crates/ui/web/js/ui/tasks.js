// Pastille « tâches en cours » : remplace cinq boutons de statut du bandeau (import, export, calage, étirement, conversion). Les boutons
// d'origine, avec leurs identifiants, leurs textes, leurs écouteurs et leur attribut `hidden`, sont rangés dans le panneau ; chaque module
// continue de les montrer et de les cacher comme avant. Ce module observe seulement ce qui est visible et le résume sur la pastille.
// (L'écoute de contrôle et le script qui écoute restent dans le bandeau : on ne doit pas les oublier.)

import { tasksSummary } from "../tasks-model.js";
import { Popover } from "./popover.js";

export class TasksIndicator {
  #pill;
  #panel;
  #popover;

  constructor(pill = document.getElementById("tasks-pill"), panel = document.getElementById("tasks-panel")) {
    this.#pill = pill;
    this.#panel = panel;
    if (!pill || !panel) return;
    this.#popover = new Popover(pill, panel);
    const obs = new MutationObserver(() => this.refresh());
    for (const task of panel.querySelectorAll(".task")) {
      obs.observe(task, { attributes: true, attributeFilter: ["hidden"], childList: true, characterData: true, subtree: true });
    }
    this.refresh();
  }

  /** Les tâches visibles, dans l'ordre du panneau. */
  #visible() {
    return [...this.#panel.querySelectorAll(".task")].filter((t) => !t.hidden);
  }

  refresh() {
    const sum = tasksSummary(this.#visible().map((t) => t.textContent));
    this.#pill.hidden = !sum;
    if (!sum) {
      this.#popover.close();
      this.#pill.textContent = "";
      return;
    }
    this.#pill.textContent = sum.label;
    this.#pill.title = sum.title;
    this.#pill.dataset.count = String(sum.count);
  }
}
