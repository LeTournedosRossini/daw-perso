// Propriétés des notes sélectionnées dans le piano roll : touche, canal MIDI (1 à 16), vélocité (1 à 127), et pour une seule note
// son début et sa durée en noires. Le canal et la vélocité se changent ici ; chaque changement est une commande (un pas d'annulation).

import { describeSelection } from "../notes-model.js";

const fmt = (v) => String(Math.round(v * 1000) / 1000).replace(".", ",");

/** Barre de propriétés (un élément à placer dans l'en-tête de la fenêtre ou de l'ancrage). `onChange({ channel? , velocity? })`. */
export class NoteProps {
  constructor(onChange) {
    this.onChange = onChange;
    this.el = document.createElement("div");
    this.el.className = "note-props";
    this.el.hidden = true;
    this.el.innerHTML = `
      <span class="np-what"></span>
      <label>Canal <select class="np-ch" aria-label="Canal MIDI des notes sélectionnées"></select></label>
      <label>Vélocité <input class="np-vel" type="number" min="1" max="127" step="1" aria-label="Vélocité des notes sélectionnées (1 à 127)"></label>
      <span class="np-pos"></span>`;
    this.ch = this.el.querySelector(".np-ch");
    this.vel = this.el.querySelector(".np-vel");
    this.ch.append(new Option("—", ""));
    for (let c = 1; c <= 16; c++) this.ch.append(new Option(String(c), String(c)));
    this.ch.addEventListener("change", () => {
      if (this.ch.value !== "") this.onChange({ channel: Number(this.ch.value) - 1 });
    });
    const commitVel = () => {
      const v = Math.round(Number(this.vel.value));
      if (this.vel.value !== "" && v >= 1 && v <= 127) this.onChange({ velocity: v / 127 });
    };
    this.vel.addEventListener("change", commitVel);
    this.vel.addEventListener("keydown", (e) => {
      e.stopPropagation(); // pas de raccourcis du piano roll pendant la saisie
      if (e.key === "Enter") {
        commitVel();
        this.vel.blur();
      }
    });
  }

  /** Met à jour l'affichage d'après les notes sélectionnées (rien : masqué). Ne touche pas un champ en cours de saisie. */
  update(notes) {
    const d = describeSelection(notes);
    this.el.hidden = !d;
    if (!d) return;
    this.el.querySelector(".np-what").textContent = d.count === 1 ? (d.keyName ?? "") : `${d.count} notes`;
    if (document.activeElement !== this.ch) this.ch.value = d.channel === null ? "" : String(d.channel);
    if (document.activeElement !== this.vel) this.vel.value = d.velocity === null ? "" : String(d.velocity);
    this.vel.placeholder = d.velocity === null ? "—" : "";
    this.el.querySelector(".np-pos").textContent = d.startBeats === null ? "" : `début ${fmt(d.startBeats)} · durée ${fmt(d.lenBeats)} noires`;
  }
}
