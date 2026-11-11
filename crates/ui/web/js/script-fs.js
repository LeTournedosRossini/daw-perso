// Fichiers des scripts (lot 4). Un script qui déclare `// @permissions files` (et que l'utilisateur a autorisé) peut lire et écrire du
// TEXTE : dans le dossier du projet, dans le dossier des scripts, et dans les fichiers ou dossiers que l'utilisateur choisit dans un
// dialogue natif pendant le lancement (le choix tient lieu de confirmation). Remplacer un fichier existant demande une confirmation, sauf
// s'il vient d'être choisi dans un dialogue « Enregistrer sous » (qui l'a déjà demandée). Les commandes Tauri revérifient les chemins réels.

export const MAX_TEXT = 1024 * 1024;

const winLike = (p) => /^[A-Za-z]:[\\/]/.test(p) || p.startsWith("\\\\");

/** Chemin normalisé pour comparer : séparateurs `/`, sans `.` ni `..` ni doublons, sans `/` final ; minuscules pour un chemin Windows. */
export function normalizePath(p) {
  const s = String(p).replace(/\\/g, "/");
  const lead = s.startsWith("//") ? "//" : s.startsWith("/") ? "/" : "";
  const out = [];
  for (const part of s.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") out.pop();
    else out.push(part);
  }
  const joined = lead + out.join("/");
  return winLike(String(p)) ? joined.toLowerCase() : joined;
}

/** `path` est-il `root` lui-même ou dedans ? (« /a/b » n'est pas dans « /a/bc ».) */
export function inRoots(path, roots) {
  const p = normalizePath(path);
  return roots.some((r) => {
    if (!r) return false;
    const n = normalizePath(r);
    return p === n || p.startsWith(n.endsWith("/") ? n : `${n}/`);
  });
}

/** Colle des morceaux de chemin avec `/` (Windows l'accepte). */
export function joinPath(...parts) {
  return parts.filter((x) => x !== "" && x != null).map((x, i) => (i === 0 ? String(x).replace(/[\\/]+$/, "") : String(x).replace(/^[\\/]+|[\\/]+$/g, ""))).join("/");
}

/** Empreinte (FNV-1a 32 bits) d'un texte : sert à retenir qu'un script précis a été autorisé (modifié, il est redemandé). */
export function hashText(text) {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, "0");
}

export const KNOWN_PERMISSIONS = ["files"];

/** Permissions déclarées inconnues (le script n'est alors pas lancé). */
export function unknownPermissions(list) {
  return list.filter((p) => !KNOWN_PERMISSIONS.includes(p));
}

/** Texte de la confirmation à la première exécution. */
export const consentText = (name) =>
  `Le script « ${name} » demande l'accès aux fichiers.\n\nIl pourra lire et écrire du texte dans le dossier du projet et dans les fichiers ou dossiers que tu choisis dans un dialogue. Il ne peut pas écrire de programme ni de script.\n\nN'autorise que des scripts que tu as lus. Autoriser ?`;

/**
 * Accès aux fichiers pour UN lancement de script. `backend` : { pickFile, pickSave, pickFolder, read, write, list } (Tauri ou mémoire).
 * `confirm(message)` : dialogue de confirmation (promesse de booléen).
 */
export class ScriptFs {
  #name;
  #backend;
  #confirm;
  #projectDir;
  #scriptsDir;
  #grants = []; // { path, kind: "file" | "save" | "folder" } choisis dans un dialogue pendant ce lancement

  constructor({ name, backend, confirm, projectDir = null, scriptsDir = null }) {
    this.#name = name;
    this.#backend = backend;
    this.#confirm = confirm;
    this.#projectDir = projectDir;
    this.#scriptsDir = scriptsDir;
  }

  /** Racines autorisées en ce moment (dossiers du projet et des scripts, choix de l'utilisateur). */
  roots() {
    return [this.#projectDir, this.#scriptsDir, ...this.#grants.map((g) => g.path)].filter(Boolean);
  }

  #check(path) {
    if (typeof path !== "string" || path.trim() === "") throw new Error("daw.fs : un chemin (texte) est attendu");
    if (!inRoots(path, this.roots())) {
      throw new Error("ce script n'a pas le droit d'accéder à ce chemin (choisis-le avec daw.fs.pickFile / pickSave / pickFolder, ou utilise le dossier du projet)");
    }
  }

  /** Une opération demandée par le script : `{ op, ... }`. */
  async handle(m) {
    switch (m.op) {
      case "projectDir":
        return this.#projectDir;
      case "scriptsDir":
        return this.#scriptsDir;
      case "pickFile":
      case "pickSave":
      case "pickFolder": {
        const path = await this.#backend[m.op]({ title: typeof m.title === "string" ? m.title : undefined, extensions: Array.isArray(m.extensions) ? m.extensions.filter((e) => typeof e === "string") : undefined, defaultName: typeof m.defaultName === "string" ? m.defaultName : undefined });
        if (!path) return null; // dialogue annulé
        this.#grants.push({ path, kind: m.op === "pickFile" ? "file" : m.op === "pickSave" ? "save" : "folder" });
        return path;
      }
      case "readText":
        this.#check(m.path);
        return this.#backend.read(m.path, this.roots());
      case "list":
        this.#check(m.path);
        return this.#backend.list(m.path, this.roots());
      case "writeText": {
        this.#check(m.path);
        if (typeof m.text !== "string") throw new Error("daw.fs.writeText : un texte est attendu");
        if (m.text.length > MAX_TEXT) throw new Error("texte trop long (1 Mo au plus)");
        const chosen = this.#grants.some((g) => g.kind === "save" && normalizePath(g.path) === normalizePath(m.path));
        if (m.overwrite && !chosen && !(await this.#confirm(`Le script « ${this.#name} » veut remplacer le fichier :\n${m.path}\n\nAutoriser ?`))) {
          throw new Error("remplacement refusé par l'utilisateur");
        }
        await this.#backend.write(m.path, m.text, this.roots(), !!m.overwrite || chosen);
        return null;
      }
      default:
        throw new Error(`daw.fs : opération inconnue « ${m.op} »`);
    }
  }
}

// ---- réalisations -------------------------------------------------------------------------------------------------------------

/** Dans l'application : dialogues natifs et commandes Tauri `script_fs_*` (qui revérifient les chemins réels). */
export class TauriFsBackend {
  #tauri;
  constructor(tauri) {
    this.#tauri = tauri;
  }
  #filters(extensions) {
    return extensions?.length ? [{ name: "Fichiers", extensions }] : undefined;
  }
  async pickFile({ title, extensions }) {
    const r = await this.#tauri.dialog.open({ multiple: false, directory: false, title: title ?? "Choisir un fichier pour le script", filters: this.#filters(extensions) });
    return Array.isArray(r) ? r[0] ?? null : r ?? null;
  }
  async pickSave({ title, extensions, defaultName }) {
    return (await this.#tauri.dialog.save({ title: title ?? "Enregistrer pour le script", defaultPath: defaultName, filters: this.#filters(extensions) })) ?? null;
  }
  async pickFolder({ title }) {
    const r = await this.#tauri.dialog.open({ multiple: false, directory: true, title: title ?? "Choisir un dossier pour le script" });
    return Array.isArray(r) ? r[0] ?? null : r ?? null;
  }
  read(path, roots) {
    return this.#tauri.core.invoke("script_fs_read", { path, roots });
  }
  write(path, text, roots, overwrite) {
    return this.#tauri.core.invoke("script_fs_write", { path, text, roots, overwrite });
  }
  list(path, roots) {
    return this.#tauri.core.invoke("script_fs_list", { dir: path, roots });
  }
}

/**
 * Fichiers « en mémoire » : double de test et repli d'un navigateur ordinaire (pas de vrai disque). Il imite les règles des commandes
 * Tauri : racines, écrasement refusé sans `overwrite`, extensions interdites, taille. `queue` : chemins que les dialogues rendront.
 */
export class MemoryFsBackend {
  files = new Map();
  queue = [];
  denied = ["exe", "dll", "bat", "cmd", "com", "msi", "scr", "ps1", "vbs", "lnk", "sh", "app", "so", "dylib", "clap", "js"];
  #next() {
    return this.queue.length ? this.queue.shift() : null;
  }
  async pickFile() {
    return this.#next();
  }
  async pickSave() {
    return this.#next();
  }
  async pickFolder() {
    return this.#next();
  }
  #key(p) {
    return normalizePath(p);
  }
  async read(path, roots) {
    if (!inRoots(path, roots)) throw new Error("ce script n'a pas le droit d'accéder à ce chemin");
    const t = this.files.get(this.#key(path));
    if (t === undefined) throw new Error("chemin introuvable");
    if (t.length > MAX_TEXT) throw new Error("fichier trop gros (1 Mo au plus)");
    return t;
  }
  async write(path, text, roots, overwrite) {
    if (!inRoots(path, roots)) throw new Error("ce script n'a pas le droit d'accéder à ce chemin");
    const ext = String(path).split(".").pop().toLowerCase();
    if (this.denied.includes(ext)) throw new Error(`un script ne peut pas écrire de fichier « .${ext} »`);
    if (this.files.has(this.#key(path)) && !overwrite) throw new Error("ce fichier existe déjà");
    this.files.set(this.#key(path), text);
  }
  async list(path, roots) {
    if (!inRoots(path, roots)) throw new Error("ce script n'a pas le droit d'accéder à ce chemin");
    const base = `${this.#key(path)}/`;
    const names = new Map();
    for (const [k, v] of this.files) {
      if (!k.startsWith(base)) continue;
      const rest = k.slice(base.length);
      const [first, ...more] = rest.split("/");
      if (more.length) names.set(first, { name: first, is_dir: true, size: 0 });
      else names.set(first, { name: first, is_dir: false, size: v.length });
    }
    return [...names.values()].sort((a, b) => (a.name < b.name ? -1 : 1));
  }
}

export function createFsBackend(tauri = globalThis.__TAURI__) {
  return tauri?.core?.invoke && tauri?.dialog ? new TauriFsBackend(tauri) : new MemoryFsBackend();
}
