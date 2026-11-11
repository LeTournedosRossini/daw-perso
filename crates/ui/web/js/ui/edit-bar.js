// Barre d'édition sous le transport : toujours la même ligne (pas de saut de mise en page), dont le texte dit ce qui est sélectionné et
// dont le groupe d'outils des clips audio (attaques, calage sur la grille, moteur d'étirement) n'apparaît que si la sélection contient
// de l'audio. Les boutons gardent leurs identifiants et leurs écouteurs d'origine : ce module ne fait que les montrer ou les cacher.

import { selectionContext } from "../edit-context.js";

export class EditBar {
  #state;
  #context;
  #audio;

  constructor(state, context = document.getElementById("edit-context"), audio = document.getElementById("edit-audio")) {
    this.#state = state;
    this.#context = context;
    this.#audio = audio;
  }

  /** À appeler quand la sélection ou le projet changent. */
  update() {
    if (!this.#context || !this.#audio) return;
    const c = selectionContext(this.#state.project, this.#state.selected);
    this.#context.textContent = c.label;
    this.#context.dataset.kind = c.audio && c.notes ? "mixed" : c.audio ? "audio" : c.notes ? "notes" : "none";
    this.#audio.hidden = !c.showAudioTools;
  }
}
