// Exécuteur de scripts : un script tourne dans un Web Worker (pas de DOM, tué au bout d'un délai) et ne parle au DAW que par
// l'objet `daw` : `await daw.state()`, `await daw.send(commande)`, `daw.log(...)`, `await daw.sleep(ms)`. Chaque commande part par le
// protocole ordinaire ; tout ce qu'un script fait forme UNE action annulable (begin_group / end_group, ou abort_group en cas d'échec).
// Voir docs/protocole-ui.md et CLAUDE.md (cadrage des scripts).

import { scriptLib } from "./script-api.js";

/** Commandes qu'un script ne peut pas envoyer : cycle de vie du projet, historique (géré par l'exécuteur) et tout ce qui ouvre le réseau. */
export const FORBIDDEN_COMMANDS = new Set([
  "begin_group", "end_group", "abort_group", "undo", "redo",
  "new_project", "open_project", "save_project", "convert_project", "cancel_convert",
  "start_monitor", "stop_monitor", "set_monitor_role", "renew_monitor_address", "get_monitor_log",
  "set_prefs",
]);

export const LIMITS = { timeoutMs: 30_000, maxCommands: 5_000, callTimeoutMs: 10_000, maxLog: 500 };

/**
 * Lit l'en-tête d'un script : les lignes de commentaires `// @clé valeur` du début (`name`, `description`, `shortcut`, `permissions`).
 * Une ligne qui n'est ni vide ni un commentaire termine l'en-tête.
 */
export function parseHeader(source) {
  const meta = { name: "", description: "", shortcut: "", permissions: [] };
  for (const raw of String(source).split("\n")) {
    const line = raw.trim();
    if (line === "") continue;
    const m = /^\/\/\s*@(\w+)\s*(.*)$/.exec(line);
    if (!m) {
      if (line.startsWith("//")) continue;
      break;
    }
    const [, key, value] = m;
    if (key === "permissions") meta.permissions = value.split(/[\s,]+/).filter(Boolean);
    else if (key in meta) meta[key] = value.trim();
  }
  return meta;
}

/**
 * Raccourci d'un script (« Ctrl+Alt+1 ») : forme normalisée, ou null s'il n'est pas permis. On exige Ctrl+Alt (ou Alt+Maj) avec une
 * lettre ou un chiffre de 1 à 9 : les raccourcis courants du DAW (Ctrl+S, Espace, Ctrl+Alt+0 / + / −…) restent intacts.
 */
export function parseShortcut(text) {
  if (!text) return null;
  const parts = String(text).split("+").map((p) => p.trim().toLowerCase()).filter(Boolean);
  const key = parts.pop();
  if (!key || key.length !== 1 || !/[a-z1-9]/.test(key)) return null;
  const mods = new Set(parts.map((p) => (p === "control" ? "ctrl" : p)));
  const has = (m) => mods.has(m);
  const ok = parts.every((p) => ["ctrl", "control", "alt", "shift"].includes(p)) && mods.size === parts.length;
  if (!ok || !((has("ctrl") && has("alt")) || (has("alt") && has("shift")))) return null;
  return [has("ctrl") ? "Ctrl" : "", has("alt") ? "Alt" : "", has("shift") ? "Maj" : "", key.toUpperCase()].filter(Boolean).join("+");
}

/** L'événement clavier correspond-il à ce raccourci normalisé ? */
export function shortcutMatches(normalized, e) {
  if (!normalized) return false;
  const parts = normalized.split("+");
  const key = parts.pop();
  const k = (e.code?.startsWith("Key") ? e.code.slice(3) : e.code?.startsWith("Digit") ? e.code.slice(5) : e.key ?? "").toUpperCase();
  return k === key && parts.includes("Ctrl") === !!(e.ctrlKey || e.metaKey) && parts.includes("Alt") === !!e.altKey && parts.includes("Maj") === !!e.shiftKey;
}

/** Code exécuté dans le Worker (en texte : il est fabriqué avec un Blob). Le réseau et les Workers imbriqués y sont retirés (au mieux). */
export const WORKER_SOURCE = `
"use strict";
for (const k of ["fetch", "XMLHttpRequest", "WebSocket", "EventSource", "importScripts", "Worker", "SharedWorker", "indexedDB", "caches", "BroadcastChannel", "WebTransport", "RTCPeerConnection"]) {
  try { Object.defineProperty(self, k, { value: undefined, configurable: false, writable: false }); } catch (_) {}
}
try { Object.defineProperty(self.navigator, "sendBeacon", { value: undefined }); } catch (_) {}
const pending = new Map();
let seq = 0;
const call = (op, arg) => new Promise((resolve, reject) => {
  const id = ++seq;
  pending.set(id, { resolve, reject });
  postMessage({ id, op, arg });
});
const base = {
  state: () => call("state"),
  send: (command) => call("send", command),
  log: (...args) => postMessage({ log: args.map((a) => typeof a === "string" ? a : JSON.stringify(a)).join(" ") }),
  sleep: (ms) => new Promise((r) => setTimeout(r, Math.max(0, Math.min(5000, +ms || 0)))),
};
const lib = (${scriptLib.toString()})(base, call);
const daw = Object.freeze({ ...base, ...lib });
const post = postMessage;
onmessage = async (e) => {
  const m = e.data;
  if (m.source !== undefined) {
    try {
      const fn = new Function("daw", "return (async () => {\\n" + m.source + "\\n})();");
      const result = await fn(daw);
      post({ done: true, result: result === undefined ? null : result });
    } catch (err) {
      post({ done: true, error: String((err && err.message) || err) });
    }
    return;
  }
  const p = pending.get(m.id);
  if (!p) return;
  pending.delete(m.id);
  if (m.error) p.reject(new Error(m.error)); else p.resolve(m.result);
};
`;

const clone = (v) => (v === undefined ? null : JSON.parse(JSON.stringify(v)));

/** Instantané en lecture seule de ce qu'un script peut lire. */
export function snapshotOf(state) {
  return clone({
    project: state.project,
    position: state.pos,
    playing: state.playing,
    loop: state.loop,
    selection: [...(state.selected ?? [])],
    history: state.history ?? null,
  });
}

export class ScriptRunner {
  #engine;
  #state;
  #makeWorker;
  #running = null;

  /**
   * @param engine   client du moteur (`send`, `on`)
   * @param state    état de la page (`project`, `pos`, `selected`…)
   * @param makeWorker  fabrique un Worker à partir de WORKER_SOURCE (injectable : les tests n'ont pas de vrai Worker)
   */
  constructor(engine, state, makeWorker = defaultWorker) {
    this.#engine = engine;
    this.#state = state;
    this.#makeWorker = makeWorker;
  }

  get running() {
    return this.#running !== null;
  }

  /** Arrête le script en cours (ce qu'il avait fait est défait). */
  abort() {
    this.#running?.stop("arrêté");
  }

  /**
   * Lance `source`. Résultat : `{ ok, log, result, error, commands }`. Une erreur, un arrêt ou un dépassement de délai défont tout
   * ce que le script avait fait ; sinon tout forme une seule action annulable « Script : nom ».
   */
  async run(source, { name = "script", timeoutMs = LIMITS.timeoutMs, maxCommands = LIMITS.maxCommands, onLog = () => {} } = {}) {
    if (this.#running) return { ok: false, log: [], error: "un script est déjà en cours", commands: 0 };
    const log = [];
    let commands = 0;
    const worker = this.#makeWorker(WORKER_SOURCE);
    let finish;
    const finished = new Promise((r) => (finish = r));
    let timer = null;
    const stop = (reason) => finish({ ok: false, error: reason });
    this.#running = { stop };
    const begun = await this.#request({ type: "begin_group", label: `Script : ${name}` });
    if (begun.error) {
      this.#running = null;
      worker.terminate?.();
      return { ok: false, log, error: begun.error, commands };
    }
    timer = setTimeout(() => stop(`délai dépassé (${Math.round(timeoutMs / 1000)} s)`), timeoutMs);
    const push = (line) => {
      if (log.length < LIMITS.maxLog) {
        log.push(line);
        onLog(line);
      }
    };
    worker.onmessage = async (e) => {
      const m = e.data;
      if (m.log !== undefined) return push(m.log);
      if (m.done) return finish(m.error ? { ok: false, error: m.error } : { ok: true, result: m.result });
      const reply = (r) => worker.postMessage({ id: m.id, ...r });
      if (m.op === "state") return reply({ result: snapshotOf(this.#state) });
      if (m.op === "send") {
        const cmd = m.arg;
        if (!cmd || typeof cmd !== "object" || typeof cmd.type !== "string") return reply({ error: "daw.send : une commande est un objet avec un champ type" });
        if (FORBIDDEN_COMMANDS.has(cmd.type)) return reply({ error: `commande interdite aux scripts : ${cmd.type}` });
        if (++commands > maxCommands) return stop(`trop de commandes (${maxCommands})`);
        const r = await this.#request(cmd);
        return reply(r.error ? { error: r.error } : { result: snapshotOf(this.#state) });
      }
      if (m.op === "notes") {
        const clip = m.arg;
        if (!Number.isInteger(clip) || clip < 0) return reply({ error: "daw.notes : l'identifiant d'un clip de notes est attendu" });
        if (++commands > maxCommands) return stop(`trop de commandes (${maxCommands})`);
        const r = await this.#request({ type: "get_notes", clip }, { type: "notes", match: (e) => e.clip === clip });
        if (r.error) return reply({ error: r.error });
        return reply(r.captured ? { result: clone(r.captured) } : { error: "pas un clip de notes" });
      }
      reply({ error: `opération inconnue : ${m.op}` });
    };
    worker.onerror = (e) => finish({ ok: false, error: String(e?.message ?? e) });
    worker.postMessage({ source });
    const outcome = await finished;
    clearTimeout(timer);
    worker.terminate?.();
    const closing = await this.#request({ type: outcome.ok ? "end_group" : "abort_group" });
    this.#running = null;
    if (closing.error && outcome.ok) return { ok: false, log, error: closing.error, commands };
    return { ...outcome, log, commands };
  }

  /**
   * Envoie une commande et attend sa fin : les événements arrivent dans l'ordre des commandes, donc l'événement `prefs` d'une commande
   * sentinelle (`get_prefs`, sans effet) marque le moment où tout ce que la commande a produit a été reçu. Rend `{ error }` si le
   * moteur a refusé la commande. `capture` : `{ type, match }` garde le dernier événement de ce type qui convient (`captured`).
   */
  #request(cmd, capture = null) {
    return new Promise((resolve) => {
      let error = null;
      let captured = null;
      const offs = [];
      const done = (r) => {
        for (const off of offs) off();
        clearTimeout(timer);
        resolve(r);
      };
      const timer = setTimeout(() => done({ error: `pas de réponse du moteur (${cmd.type})` }), LIMITS.callTimeoutMs);
      offs.push(this.#engine.on("error", (e) => { if (e.command === cmd.type && error === null) error = e.message; }));
      if (capture) offs.push(this.#engine.on(capture.type, (e) => { if (capture.match(e)) captured = e; }));
      offs.push(this.#engine.on("prefs", () => done({ error, captured })));
      if (this.#engine.send(cmd) === false) return done({ error: error ?? `commande refusée : ${cmd.type}` });
      this.#engine.send({ type: "get_prefs" });
    });
  }
}

function defaultWorker(source) {
  const url = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
  const w = new Worker(url);
  const terminate = w.terminate.bind(w);
  w.terminate = () => {
    terminate();
    URL.revokeObjectURL(url);
  };
  return w;
}
