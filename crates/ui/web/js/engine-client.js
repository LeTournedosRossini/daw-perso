// Interface commune à toutes les réalisations du moteur vu de l'interface (voir docs/protocole-ui.md).
// L'interface n'importe jamais autre chose que cette classe : MockEngine, Tauri ou WebAssembly en héritent.

export class EngineClient {
  #listeners = new Map();

  /** Envoie une commande au moteur (objet avec un champ `type`). */
  send(_message) {
    throw new Error("send() doit être fourni par la réalisation du moteur");
  }

  /** Abonne `fn` aux événements de ce `type`. Retourne une fonction de désabonnement. */
  on(type, fn) {
    if (!this.#listeners.has(type)) this.#listeners.set(type, new Set());
    this.#listeners.get(type).add(fn);
    return () => this.#listeners.get(type)?.delete(fn);
  }

  /** Utilisé par les réalisations pour publier un événement. */
  emit(type, payload = {}) {
    for (const fn of this.#listeners.get(type) ?? []) fn({ type, ...payload });
  }
}
