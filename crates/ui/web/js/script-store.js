// Où vivent les scripts enregistrés : dans l'application, le dossier `…/DAW/scripts` (commandes Tauri `scripts_list` et `script_save`) ;
// dans un navigateur, le stockage local (avec les exemples). Même interface pour la page : `list()`, `save(file, source, overwrite)`.

import { EXAMPLE_SCRIPTS } from "./scripts-examples.js";

const KEY = "daw.scripts";
const safeName = (n) => /^[\p{L}\p{N} _.-]{1,76}\.js$/u.test(n) && !n.startsWith(".") && !n.includes("..");

export function toFileName(name) {
  const base = String(name).trim().replace(/\.js$/i, "").replace(/[^\p{L}\p{N} _.-]/gu, "-").replace(/^[.\s-]+/, "").slice(0, 76) || "script";
  return `${base}.js`;
}

class TauriStore {
  #tauri;
  dir = "";
  constructor(tauri) {
    this.#tauri = tauri;
  }
  async list() {
    const r = await this.#tauri.core.invoke("scripts_list");
    this.dir = r.dir;
    if (!r.existed) {
      for (const ex of EXAMPLE_SCRIPTS) await this.save(ex.file, ex.source, false).catch(() => {});
      return (await this.#tauri.core.invoke("scripts_list")).scripts;
    }
    return r.scripts;
  }
  save(file, source, overwrite) {
    if (!safeName(file)) return Promise.reject(new Error("nom de script invalide"));
    return this.#tauri.core.invoke("script_save", { file, source, overwrite });
  }
}

class LocalStore {
  dir = "";
  #read() {
    try {
      const v = JSON.parse(localStorage.getItem(KEY) ?? "null");
      if (Array.isArray(v)) return v;
    } catch { /* stockage indisponible ou abîmé : on repart des exemples */ }
    return EXAMPLE_SCRIPTS.map((s) => ({ ...s }));
  }
  #write(list) {
    try { localStorage.setItem(KEY, JSON.stringify(list)); } catch { /* non mémorisé */ }
  }
  async list() {
    return this.#read();
  }
  async save(file, source, overwrite) {
    if (!safeName(file)) throw new Error("nom de script invalide");
    const list = this.#read();
    const i = list.findIndex((s) => s.file === file);
    if (i >= 0 && !overwrite) throw new Error("ce script existe déjà");
    if (i >= 0) list[i] = { file, source };
    else list.push({ file, source });
    this.#write(list);
  }
}

export function createScriptStore(tauri = globalThis.__TAURI__) {
  return tauri?.core?.invoke ? new TauriStore(tauri) : new LocalStore();
}
