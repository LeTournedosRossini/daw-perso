// Barre d'édition sous le transport : toujours la même ligne (pas de saut de mise en page), dont le texte dit ce qui est sélectionné et
// dont le groupe d'outils des clips audio (attaques, calage sur la grille, moteur d'étirement) n'apparaît que si la sélection contient
// de l'audio. Les boutons gardent leurs identifiants et leurs écouteurs d'origine : ce module ne fait que les montrer ou les cacher.

import { selectionContext, laneContextLabel } from "../edit-context.js";

export class EditBar {
  #state;
  #context;
  #audio;
  #lane;
  /** La voie d'accords (donne la sélection et exécute copier / couper / coller / dupliquer / supprimer). */
  chordLane = null;

  constructor(state, context = document.getElementById("edit-context"), audio = document.getElementById("edit-audio"), lane = document.getElementById("edit-lane")) {
    this.#state = state;
    this.#lane = lane;
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
    this.#updateLane(c.total);
  }

  /** Les éléments des voies d'accords (accords, gammes, tonalités, sections, syllabes) sélectionnés : boutons copier, couper, coller, dupliquer, supprimer. */
  #updateLane(clips) {
    const lane = this.#lane;
    const info = this.chordLane?.selectionInfo?.();
    if (!lane) return;
    const show = !!info && info.count > 0 && clips === 0;
    lane.hidden = !show;
    if (!show) return;
    const text = laneContextLabel(info.kind, info.count);
    this.#context.textContent = text;
    this.#context.dataset.kind = "lane";
    const paste = lane.querySelector("[data-lane='v']");
    if (paste) paste.disabled = !info.clip || info.clip.kind !== info.kind;
  }
}
