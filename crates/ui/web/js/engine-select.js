// Choix de la réalisation du moteur : dans la fenêtre Tauri (`window.__TAURI__`, activé par `withGlobalTauri`), le vrai
// moteur ; dans un navigateur ordinaire, le faux moteur de démonstration. Plus tard : une réalisation WebAssembly
// (moteur dans un Worker) pour la PWA.

import { MockEngine } from "./mock-engine.js";
import { TauriEngine } from "./tauri-engine.js";

export async function createEngine() {
  const tauri = globalThis.__TAURI__;
  if (tauri?.core?.invoke && tauri?.event?.listen) {
    const engine = new TauriEngine(tauri);
    return { engine, label: "Moteur réel (Tauri)", start: () => engine.start() };
  }
  const engine = new MockEngine();
  return { engine, label: "Faux moteur (démonstration, aucun son)", start: async () => engine.start() };
}
