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

export const LIMITS = { timeoutMs: 30_000, maxCommands: 5_000, callTimeoutMs: 10_000, maxLog: 500, handlerMs: 10_000, handlerCommands: 500, burst: 30, burstMs: 10_000, errorsInARow: 3 };

/** Événements auxquels un script peut s'abonner avec `daw.on` (la position, qui change 30 fois par seconde, n'en fait pas partie). */
export const EVENT_TYPES = ["play", "stop", "selection", "project", "recording-end"];

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
const handlers = new Map();
const EVENT_TYPES = ${JSON.stringify(EVENT_TYPES)};
const base = {
  state: () => call("state"),
  on: (type, fn) => {
    if (!EVENT_TYPES.includes(type)) throw new Error("daw.on : événement inconnu « " + type + " » (" + EVENT_TYPES.join(", ") + ")");
    if (typeof fn !== "function") throw new Error("daw.on : une fonction est attendue");
    if (!handlers.has(type)) handlers.set(type, new Set());
    handlers.get(type).add(fn);
    postMessage({ listen: type });
    return () => handlers.get(type)?.delete(fn);
  },
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
      post({ done: true, result: result === undefined ? null : result, listening: [...handlers].filter(([, set]) => set.size).map(([type]) => type) });
    } catch (err) {
      post({ done: true, error: String((err && err.message) || err) });
    }
    return;
  }
  if (m.event !== undefined) {
    let error = null;
    for (const fn of [...(handlers.get(m.event) ?? [])]) {
      try { await fn(m.payload); } catch (err) { error = String((err && err.message) || err); break; }
    }
    post({ handled: m.seq, error });
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
    lyrics: state.lyrics ?? [],
    structure: state.structure ?? [],
    chords: state.chordTrack?.track ?? null,
  });
}

export class ScriptRunner {
  #engine;
  #state;
  #makeWorker;
  #running = null; // { name, resident, stop(raison) }

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

  /** Un script est-il en cours (y compris un script qui écoute des événements) ? */
  get running() {
    return this.#running !== null;
  }

  /** Nom du script qui écoute des événements, sinon null. */
  get listening() {
    return this.#running?.resident ? this.#running.name : null;
  }

  /** Arrête le script en cours (ce qu'il avait fait est défait ; un script qui écoute s'arrête simplement). */
  abort() {
    this.#running?.stop("arrêté");
  }

  /**
   * Lance `source`. Résultat : `{ ok, log, result, error, commands, listening }`. Une erreur, un arrêt ou un dépassement de délai défont
   * tout ce que le script avait fait ; sinon tout forme une seule action annulable « Script : nom ». Un script qui s'abonne à des
   * événements (`daw.on`) reste ensuite actif : chaque réaction forme une action annulable « Script : nom (événement) » ; `onEnd(raison)`
   * est appelé quand il s'arrête.
   */
  async run(source, { name = "script", timeoutMs = LIMITS.timeoutMs, maxCommands = LIMITS.maxCommands, onLog = () => {}, onEnd = () => {}, handlerMs = LIMITS.handlerMs, fs = null } = {}) {
    if (this.#running) return { ok: false, log: [], error: `un script est déjà en cours${this.listening ? ` (« ${this.listening} » écoute des événements : arrête-le d'abord)` : ""}`, commands: 0 };
    const log = [];
    const ctx = { commands: 0, max: maxCommands, resident: null, setupDone: false };
    const worker = this.#makeWorker(WORKER_SOURCE);
    let finish;
    const finished = new Promise((r) => (finish = r));
    let timer = null;
    const fail = (reason) => (ctx.resident ? this.#endResident(ctx.resident, reason) : finish({ ok: false, error: reason }));
    this.#running = { name, resident: false, stop: fail };
    const begun = await this.#request({ type: "begin_group", label: `Script : ${name}` });
    if (begun.error) {
      this.#running = null;
      worker.terminate?.();
      return { ok: false, log, error: begun.error, commands: 0 };
    }
    const startTimer = () => {
      clearTimeout(timer);
      timer = setTimeout(() => fail(`délai dépassé (${Math.round(timeoutMs / 1000)} s)`), timeoutMs);
    };
    startTimer();
    ctx.pause = () => clearTimeout(timer); // un dialogue attend l'utilisateur : le délai ne court pas pendant ce temps
    ctx.resume = () => !ctx.resident && !ctx.setupDone && startTimer();
    const push = (line) => {
      if (log.length < LIMITS.maxLog) log.push(line);
      onLog(line);
    };
    const types = new Set();
    worker.onmessage = async (e) => {
      const m = e.data;
      if (m.log !== undefined) return push(m.log);
      if (m.listen !== undefined) return types.add(m.listen);
      if (m.handled !== undefined) return ctx.resident?.handled?.(m);
      if (m.done) return finish(m.error ? { ok: false, error: m.error } : { ok: true, result: m.result });
      const reply = (r) => worker.postMessage({ id: m.id, ...r });
      if (m.op === "state") return reply({ result: snapshotOf(this.#state) });
      // Une action n'est permise que dans le premier passage ou pendant une réaction : sinon (minuteur d'un script qui écoute) elle
      // échapperait au groupe d'annulation et aux bornes.
      if (ctx.setupDone && !ctx.resident?.busy) return reply({ error: "hors d'une réaction : daw.send, daw.notes et daw.fs ne sont permis que dans le premier passage du script ou dans une réaction à un événement" });
      if (m.op === "send") {
        const cmd = m.arg;
        if (!cmd || typeof cmd !== "object" || typeof cmd.type !== "string") return reply({ error: "daw.send : une commande est un objet avec un champ type" });
        if (FORBIDDEN_COMMANDS.has(cmd.type)) return reply({ error: `commande interdite aux scripts : ${cmd.type}` });
        if (++ctx.commands > ctx.max) return fail(`trop de commandes (${ctx.max})`);
        const r = await this.#request(cmd);
        return reply(r.error ? { error: r.error } : { result: snapshotOf(this.#state) });
      }
      if (m.op === "notes") {
        const clip = m.arg;
        if (!Number.isInteger(clip) || clip < 0) return reply({ error: "daw.notes : l'identifiant d'un clip de notes est attendu" });
        if (++ctx.commands > ctx.max) return fail(`trop de commandes (${ctx.max})`);
        const r = await this.#request({ type: "get_notes", clip }, { type: "notes", match: (ev) => ev.clip === clip });
        if (r.error) return reply({ error: r.error });
        return reply(r.captured ? { result: clone(r.captured) } : { error: "pas un clip de notes" });
      }
      if (m.op === "fs") {
        if (!fs) return reply({ error: "ce script n'a pas déclaré « // @permissions files » (ou l'accès aux fichiers n'a pas été autorisé)" });
        if (++ctx.commands > ctx.max) return fail(`trop de commandes (${ctx.max})`);
        ctx.pause();
        try {
          reply({ result: await fs.handle(m.arg) });
        } catch (err) {
          reply({ error: String(err?.message ?? err) });
        } finally {
          ctx.resume();
        }
        return;
      }
      reply({ error: `opération inconnue : ${m.op}` });
    };
    worker.onerror = (e) => fail(String(e?.message ?? e));
    worker.postMessage({ source });
    const outcome = await finished;
    ctx.setupDone = true;
    clearTimeout(timer);
    const keep = outcome.ok && types.size > 0 && !ctx.resident;
    if (!keep) worker.terminate?.();
    const closing = await this.#request({ type: outcome.ok ? "end_group" : "abort_group" });
    if (!keep) {
      this.#running = null;
      if (closing.error && outcome.ok) return { ok: false, log, error: closing.error, commands: ctx.commands };
      return { ...outcome, log, commands: ctx.commands };
    }
    if (closing.error) {
      worker.terminate?.();
      this.#running = null;
      return { ok: false, log, error: closing.error, commands: ctx.commands };
    }
    this.#goResident({ worker, name, types, ctx, push, onEnd, handlerMs });
    return { ...outcome, log, commands: ctx.commands, listening: [...types] };
  }

  // ---- script qui écoute des événements --------------------------------------------------------------------------------------

  #goResident({ worker, name, types, ctx, push, onEnd, handlerMs }) {
    const res = { name, worker, types, ctx, push, onEnd, handlerMs, busy: false, dead: false, offs: [], runs: [], errors: 0, handled: null };
    ctx.resident = res;
    this.#running = { name, resident: true, stop: (reason) => this.#endResident(res, reason) };
    const emit = (type, payload) => {
      if (res.dead || res.busy || !res.types.has(type)) return; // pendant une réaction, les événements (dont ceux que le script provoque lui-même) sont ignorés
      this.#handle(res, type, payload);
    };
    let playing = !!this.#state.playing;
    res.offs.push(
      this.#engine.on("position", (e) => {
        if (e.playing !== playing) {
          playing = e.playing;
          emit(playing ? "play" : "stop", { position: e.pos });
        }
      }),
    );
    let recording = false;
    res.offs.push(
      this.#engine.on("recording", (e) => {
        const was = recording;
        recording = !!e.active;
        if (was && !e.active) emit("recording-end", {});
      }),
    );
    res.offs.push(this.#engine.on("project", () => emit("project", {})));
    let sel = [...(this.#state.selected ?? [])].join(",");
    const poll = setInterval(() => {
      const now = [...(this.#state.selected ?? [])].join(",");
      if (now === sel) return;
      sel = now; // mis à jour même pendant une réaction : un changement provoqué par le script n'est pas relayé
      emit("selection", { clips: now === "" ? [] : now.split(",").map(Number) });
    }, 150);
    res.offs.push(() => clearInterval(poll));
  }

  async #handle(res, type, payload) {
    res.busy = true;
    const now = Date.now();
    res.runs = res.runs.filter((t) => now - t < LIMITS.burstMs);
    res.runs.push(now);
    if (res.runs.length > LIMITS.burst) {
      res.busy = false;
      return this.#endResident(res, `trop d'événements (${LIMITS.burst} en ${LIMITS.burstMs / 1000} s)`);
    }
    const begun = await this.#request({ type: "begin_group", label: `Script : ${res.name} (${type})` });
    if (begun.error) {
      res.busy = false;
      return this.#endResident(res, begun.error);
    }
    res.ctx.commands = 0;
    res.ctx.max = LIMITS.handlerCommands;
    const seq = (res.seq = (res.seq ?? 0) + 1);
    const outcome = await new Promise((resolve) => {
      let t = null;
      const arm = () => {
        clearTimeout(t);
        t = setTimeout(() => resolve({ timeout: true }), res.handlerMs);
      };
      arm();
      res.ctx.pause = () => clearTimeout(t); // pendant un dialogue de fichier, le délai de la réaction ne court pas
      res.ctx.resume = arm;
      res.handled = (m) => {
        if (m.handled !== seq) return;
        clearTimeout(t);
        resolve({ error: m.error });
      };
      res.worker.postMessage({ event: type, payload, seq });
    });
    res.handled = null;
    if (res.dead) return; // arrêté pendant la réaction : #endResident a déjà défait le groupe
    const ok = !outcome.timeout && !outcome.error;
    await this.#request({ type: ok ? "end_group" : "abort_group" });
    res.busy = false;
    if (outcome.timeout) return this.#endResident(res, `réaction à « ${type} » trop longue (${Math.round(res.handlerMs / 1000)} s)`);
    if (outcome.error) {
      res.push(`✗ réaction à « ${type} » : ${outcome.error} — défaite`);
      if (++res.errors >= LIMITS.errorsInARow) this.#endResident(res, `${LIMITS.errorsInARow} erreurs de suite`);
    } else {
      res.errors = 0;
    }
  }

  async #endResident(res, reason) {
    if (res.dead) return;
    res.dead = true;
    for (const off of res.offs) off();
    res.worker.terminate?.();
    if (res.busy) await this.#request({ type: "abort_group" }); // une réaction interrompue ne laisse rien
    if (this.#running?.resident) this.#running = null;
    res.onEnd(reason);
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
