// Témoin d'entrée MIDI d'une piste : trois petites lettres (N notes, C contrôleurs, P molette et pressions) qui s'allument quand la
// piste reçoit un message en direct. L'hôte envoie l'ensemble des pistes actives (retenue de ~150 ms côté hôte).

export const MIDI_BITS = [
  { bit: 1, letter: "N", title: "notes" },
  { bit: 2, letter: "C", title: "contrôleurs" },
  { bit: 4, letter: "P", title: "molette et pressions" },
];

/** Élément du témoin (masqué tant que la piste n'a pas d'entrée MIDI). */
export function createWitness() {
  const el = document.createElement("span");
  el.className = "midiw";
  el.hidden = true;
  el.setAttribute("aria-label", "Entrée MIDI");
  el.title = "Entrée MIDI : s'allume quand la piste reçoit des notes (N), des contrôleurs (C) ou la molette (P)";
  for (const b of MIDI_BITS) {
    const i = document.createElement("i");
    i.textContent = b.letter;
    i.dataset.bit = String(b.bit);
    el.append(i);
  }
  return el;
}

/** Montre ou cache le témoin selon l'entrée de la piste, et allume les lettres de `bits`. */
export function paintWitness(el, node, bits) {
  if (!el) return;
  const midi = node?.input?.kind === "midi";
  el.hidden = !midi;
  if (!midi) return;
  for (const i of el.children) i.classList.toggle("on", (bits & Number(i.dataset.bit)) !== 0);
}
