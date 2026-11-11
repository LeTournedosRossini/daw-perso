// Menu « Fichier ▾ » du bandeau : regroupe Nouveau, Ouvrir, Enregistrer sous, Importer, Exporter, Caler et Réglages. Les boutons gardent
// leurs identifiants et leurs écouteurs d'origine (ils sont seulement rangés dans le panneau) ; ce module ne fait qu'ouvrir et fermer
// le panneau, le placer sous le bouton et gérer le clavier (flèches, Échap) et le toucher.

export class FileMenu {
  #button;
  #panel;

  constructor(button = document.getElementById("btn-file"), panel = document.getElementById("file-menu")) {
    this.#button = button;
    this.#panel = panel;
    if (!button || !panel) return;
    button.addEventListener("click", (e) => {
      e.stopPropagation();
      this.toggle();
    });
    // Un appui ailleurs ferme le menu (en phase de capture : certains éléments arrêtent la propagation de leurs clics).
    document.addEventListener(
      "pointerdown",
      (e) => {
        if (!panel.hidden && !panel.contains(e.target) && !button.contains(e.target)) this.close();
      },
      true,
    );
    // Choisir un élément ferme le menu (l'action elle-même est celle du bouton, liée ailleurs).
    panel.addEventListener("click", (e) => {
      if (e.target.closest?.(".fm-item")) this.close();
    });
    panel.addEventListener("keydown", (e) => this.#key(e));
    button.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown" && panel.hidden) {
        e.preventDefault();
        this.open();
      }
    });
    window.addEventListener("resize", () => !panel.hidden && this.#place());
  }

  get isOpen() {
    return !!this.#panel && !this.#panel.hidden;
  }

  toggle() {
    if (this.isOpen) this.close();
    else this.open();
  }

  open() {
    if (!this.#panel || this.isOpen) return;
    this.#panel.hidden = false;
    this.#button.setAttribute("aria-expanded", "true");
    this.#place();
    this.#items()[0]?.focus({ preventScroll: true });
  }

  close(refocus = false) {
    if (!this.#panel || !this.isOpen) return;
    this.#panel.hidden = true;
    this.#button.setAttribute("aria-expanded", "false");
    if (refocus) this.#button.focus({ preventScroll: true });
  }

  #items() {
    return [...this.#panel.querySelectorAll(".fm-item:not(:disabled)")];
  }

  /** Sous le bouton, dans l'écran (le zoom d'interface est une propriété CSS `zoom` : les coordonnées s'y ramènent). */
  #place() {
    const z = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--ui-zoom")) || 1;
    const r = this.#button.getBoundingClientRect();
    const panel = this.#panel;
    panel.style.left = `${Math.max(4, r.left / z)}px`;
    panel.style.top = `${r.bottom / z + 3}px`;
    const over = panel.getBoundingClientRect().right / z - window.innerWidth / z + 8;
    if (over > 0) panel.style.left = `${Math.max(4, r.left / z - over)}px`;
  }

  #key(e) {
    const items = this.#items();
    const i = items.indexOf(document.activeElement);
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      this.close(true);
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = e.key === "ArrowDown" ? (i + 1) % items.length : (i - 1 + items.length) % items.length;
      items[next]?.focus({ preventScroll: true });
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      items[e.key === "Home" ? 0 : items.length - 1]?.focus({ preventScroll: true });
    } else if (e.key === "Tab") {
      this.close();
    }
  }
}
