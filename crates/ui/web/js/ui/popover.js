// Petit panneau flottant ouvert par un bouton (taille de l'interface) : s'ouvre sous le bouton, se ferme par Échap ou un appui ailleurs,
// mais PAS au clic sur un de ses boutons (on appuie souvent plusieurs fois sur A+). Les boutons du panneau gardent leurs identifiants et
// leurs écouteurs : ce module ne gère que l'ouverture et la position.

export class Popover {
  #button;
  #panel;

  /** `closeOnItem` : sélecteur des éléments dont un appui referme le panneau (aucun par défaut : on appuie souvent plusieurs fois). */
  constructor(button, panel, { closeOnItem = null } = {}) {
    this.#button = button;
    this.#panel = panel;
    if (!button || !panel) return;
    button.addEventListener("click", (e) => {
      e.stopPropagation();
      this.isOpen ? this.close() : this.open();
    });
    document.addEventListener(
      "pointerdown",
      (e) => {
        if (this.isOpen && !panel.contains(e.target) && !button.contains(e.target)) this.close();
      },
      true,
    );
    if (closeOnItem) panel.addEventListener("click", (e) => e.target.closest?.(closeOnItem) && this.close());
    panel.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.close(true);
      }
    });
    window.addEventListener("resize", () => this.isOpen && this.#place());
    window.addEventListener("ui-scale-change", () => this.isOpen && requestAnimationFrame(() => this.#place())); // le panneau suit la nouvelle taille
  }

  get isOpen() {
    return !!this.#panel && !this.#panel.hidden;
  }

  open() {
    if (!this.#panel || this.isOpen) return;
    this.#panel.hidden = false;
    this.#button.setAttribute("aria-expanded", "true");
    this.#place();
  }

  close(refocus = false) {
    if (!this.isOpen) return;
    this.#panel.hidden = true;
    this.#button.setAttribute("aria-expanded", "false");
    if (refocus) this.#button.focus({ preventScroll: true });
  }

  /** Sous le bouton, aligné à droite si le panneau dépasserait (le zoom d'interface est une propriété CSS `zoom`). */
  #place() {
    const z = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--ui-zoom")) || 1;
    const r = this.#button.getBoundingClientRect();
    const panel = this.#panel;
    panel.style.top = `${r.bottom / z + 3}px`;
    panel.style.left = `${r.left / z}px`;
    const right = panel.getBoundingClientRect().right / z;
    const limit = window.innerWidth / z - 6;
    if (right > limit) panel.style.left = `${Math.max(4, r.left / z - (right - limit))}px`;
  }
}

/** Recopie le texte d'un élément dans un autre à chaque changement (le bouton « Aa » montre la taille courante). */
export function mirrorText(source, target, format = (t) => t) {
  if (!source || !target) return;
  const copy = () => (target.textContent = format(source.textContent.trim()));
  new MutationObserver(copy).observe(source, { childList: true, characterData: true, subtree: true });
  copy();
}
