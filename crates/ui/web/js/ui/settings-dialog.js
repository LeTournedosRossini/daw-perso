// Réglages : pilote audio, tampon, calcul parallèle, calcul en avance, métronome. Les préférences sont enregistrées par le moteur
// dans un fichier de l'utilisateur (valable pour tous les projets). Le métronome s'applique tout de suite ; le pilote, le tampon et
// les fils de calcul ne s'appliquent qu'au prochain démarrage de l'application (le projet ouvert vit dans le moteur qui tourne) :
// la fenêtre le dit et propose de redémarrer. Les réglages imposés par une variable d'environnement sont montrés et verrouillés.

import { validatePrefs } from "../protocol.js";

const $ = (id) => document.getElementById(id);

export const BUFFERS = [16, 32, 64, 128, 256, 512];
export const AHEADS = [0, 50, 100];
export const DEFAULT_DRIVER = "UMC";
export const DEFAULT_BUFFER = 64;

/** Le pilote de la liste que désigne `wanted` (un morceau du nom suffit, sans tenir compte de la casse), ou `null`. */
export function matchDriver(wanted, drivers) {
  const w = String(wanted ?? "").trim().toLowerCase();
  if (!w) return null;
  return drivers.find((d) => d.toLowerCase() === w) ?? drivers.find((d) => d.toLowerCase().includes(w) || w.includes(d.toLowerCase())) ?? null;
}

/** Le tampon proposé : les tailles usuelles plus la valeur actuelle si elle est d'une autre taille. */
export function bufferChoices(current) {
  return [...new Set([...BUFFERS, current])].filter(Number.isFinite).sort((a, b) => a - b);
}

/** Latence d'un tampon, en millisecondes (une période), pour l'afficher à côté du choix. */
export function bufferMs(buffer, rate) {
  return rate > 0 ? (buffer / rate) * 1000 : null;
}

/**
 * Les cœurs que les réglages font travailler en même temps pendant la lecture. Tous ces fils font de l'attente active (ils
 * tournent à 100 % tant que la lecture dure) : chacun occupe donc un cœur entier.
 *  - 1 fil audio (celui du pilote, toujours là) ;
 *  - `threads − 1` fils de travail du calcul parallèle ;
 *  - `aheadThreads` fils de rendu, seulement si le calcul en avance est activé.
 */
export function coreBudget(threads, aheadMs, aheadThreads, cores) {
  const workers = Math.max(0, threads - 1);
  const render = aheadMs > 0 ? aheadThreads : 0;
  const total = 1 + workers + render;
  const level = !(cores > 0) ? "ok" : total > cores ? "over" : total === cores ? "full" : "ok";
  return { audio: 1, workers, render, total, cores, level };
}

/** Phrase du budget : « 1 fil audio + 2 fils de calcul + 2 fils de rendu = 5 cœurs occupés sur 4 ». */
export function budgetText(b) {
  const parts = ["1 fil audio"];
  if (b.workers) parts.push(`${b.workers} fil${b.workers > 1 ? "s" : ""} de travail du calcul`);
  if (b.render) parts.push(`${b.render} fil${b.render > 1 ? "s" : ""} de rendu`);
  return `${parts.join(" + ")} = ${b.total} cœur${b.total > 1 ? "s" : ""} occupé${b.total > 1 ? "s" : ""}${b.cores ? ` sur ${b.cores}` : ""}`;
}

/** Conseil selon le budget (`null` si tout va bien). Mesuré sur 4 cœurs : 4 fils sur 4 cœurs ont donné des à-coups de 10 ms. */
export function threadsWarning(threads, aheadMs, aheadThreads, cores) {
  const b = coreBudget(threads, aheadMs, aheadThreads, cores);
  if (b.level === "over") {
    const advice = aheadMs > 0 && threads > 1 ? " Avec le calcul en avance, laisse les fils de calcul à 1 : presque tout le travail est fait par les fils de rendu." : " Baisse le nombre de fils.";
    return `Il y a plus de fils (${b.total}) que de cœurs (${b.cores}) : ils se gêneront et des coupures sont probables.${advice}`;
  }
  if (b.level === "full") return `Aucun cœur n'est libre pour le système. Mesuré sur 4 cœurs, 4 fils actifs ont donné des à-coups de 10 ms ; laisse si possible un cœur libre.`;
  return null;
}

/** Préférences d'après les champs de la fenêtre (le métronome est repris de l'état courant : il se règle ailleurs, tout de suite). */
export function prefsFromForm(f, metronome, base = {}) {
  return {
    ...base,
    version: 1,
    ...(f.system === "jack" || f.system === "linux" || f.system === "coreaudio" ? {} : { driver: f.driver || null }),
    ...(f.system === "coreaudio" || f.system === "linux" || f.system === "windows" ? { device: f.device || null } : {}),
    ...(f.system === "linux" || f.system === "windows" ? { audio_system: f.audio_system || null, capture_device: f.capture_device || null } : {}),
    ...(f.system === "jack" || f.system === "linux" ? { out_ports: (f.out_ports ?? []).filter(Boolean), in_ports: (f.in_ports ?? []).filter(Boolean) } : {}),
    buffer: f.buffer ?? null,
    threads: f.threads,
    ahead_ms: f.ahead_ms,
    ahead_threads: f.ahead_threads,
    ...(f.auto_stop ? { auto_stop: f.auto_stop } : {}),
    ...(f.transport ? { transport: f.transport } : {}),
    ...(f.remote ? { remote: f.remote } : {}),
    metronome: { enabled: !!metronome.enabled, only_recording: !!metronome.only_recording, volume: metronome.volume ?? 0.8, count_in_bars: metronome.count_in_bars ?? 0, pre_roll: metronome.pre_roll ?? 0, post_roll: metronome.post_roll ?? 0, pre_roll_bars: metronome.pre_roll_bars ?? 0, pre_roll_unit: metronome.pre_roll_unit ?? "seconds" },
  };
}

/** Phrase sur ce qui tourne maintenant. */
export function runningText(r) {
  if (!r) return "Le moteur audio n'a pas encore répondu.";
  const e = r.effective;
  if (!r.audio) return "Aucun pilote audio n'est ouvert : le moteur tourne sans son.";
  const ms = bufferMs(e.buffer, r.sample_rate);
  const what = r.system === "wasapi" ? `WASAPI partagé (${e.device || "appareil par défaut"})` : r.system === "alsa" ? `ALSA (${e.device || "default"})` : r.system === "jack" ? "JACK (ports : " + ((e.out_ports ?? []).length ? e.out_ports.join(", ") : "sorties physiques par défaut") + ")" : r.system === "coreaudio" ? `CoreAudio (${e.device || "périphérique par défaut du système"})` : e.driver;
  return `En cours : ${what}, ${r.sample_rate} Hz, tampon ${e.buffer}${ms ? ` (${ms.toFixed(2).replace(".", ",")} ms)` : ""}, ${r.inputs} entrée(s), ${e.threads} fil(s) de calcul${e.ahead_ms ? `, avance ${e.ahead_ms} ms` : ""}.`;
}

export class SettingsDialog {
  constructor(state, client, files, dialogs) {
    this.state = state;
    this.client = client;
    this.files = files;
    this.dialogs = dialogs;
    this.last = null; // dernier événement `prefs`
    this.el = $("settings-dialog");
    if (!this.el) return;
    client.on("prefs", (e) => {
      this.last = e;
      state.prefs = e;
      if (this.el.open) this.#fill();
    });
    client.on("error", (e) => {
      if (e.command === "set_prefs") this.#message(e.message, true);
    });
    $("btn-settings")?.addEventListener("click", () => this.open());
    $("st-system").addEventListener("change", () => this.#audioLayout());
    $("st-save").addEventListener("click", () => this.#save());
    $("st-restart").addEventListener("click", () => this.#restart());
    for (const id of ["st-threads", "st-ahead", "st-ahead-threads", "st-driver", "st-buffer", "st-out-l", "st-out-r", "st-in-l", "st-in-r", "st-system", "st-alsa-out", "st-alsa-in"]) $(id).addEventListener("change", () => this.#refreshNotes());
    // Changer d'unité montre la valeur déjà réglée pour cette unité avant d'envoyer.
    $("st-m-pre-unit").addEventListener("change", () => this.#fillPre(this.state.metronome ?? this.#prefs().metronome));
    for (const id of ["st-m-only", "st-m-count", "st-m-pre", "st-m-pre-unit", "st-m-post"]) $(id).addEventListener("change", () => this.#sendMetronome());
    $("st-m-vol").addEventListener("change", () => this.#sendMetronome());
  }

  open() {
    if (!this.el) return;
    this.client.send({ type: "get_prefs" });
    this.#fill();
    this.#message("");
    if (!this.el.open) this.el.showModal();
  }

  #prefs() {
    return this.last?.prefs ?? { version: 1, driver: null, buffer: null, threads: 1, ahead_ms: 0, ahead_threads: 2, metronome: { ...(this.state.metronome ?? {}) } };
  }

  #fill() {
    const p = this.#prefs();
    const r = this.last?.running ?? null;
    const forced = new Set(r?.effective.forced ?? []);
    $("st-running").textContent = runningText(r);

    const system = r?.system ?? "asio";
    // Deux systèmes au choix sous Linux (JACK, ALSA) et sous Windows (ASIO, WASAPI) ; ailleurs un seul.
    const systems = r?.systems ?? [];
    const family = systems.includes("wasapi") ? "windows" : systems.includes("alsa") ? "linux" : "";
    const multi = !!family;
    $("st-f-system").hidden = !multi;
    $("st-driver-label").textContent = system === "coreaudio" ? "Périphérique" : "Pilote";
    if (multi) {
      const sys = $("st-system");
      sys.replaceChildren(...(family === "windows"
        ? [["", "Automatique (ASIO, sinon WASAPI)"], ["asio", "ASIO"], ["wasapi", "WASAPI partagé"]]
        : [["", "Automatique (JACK, sinon ALSA)"], ["jack", "JACK"], ["alsa", "ALSA"]]).map(([v, t]) => Object.assign(document.createElement("option"), { value: v, textContent: t })));
      sys.value = p.audio_system ?? "";
      if (family === "linux") this.#fillJack(p, r);
      else this.#fillAsio(p, r, forced);
      this.#fillAlsa(p, r, family);
      this.#audioLayout();
    } else if (system === "coreaudio") {
      $("st-f-driver").hidden = false;
      $("st-jack").hidden = true;
      $("st-alsa").hidden = true;
      this.#fillDevices(p, r);
    } else {
      // Pilote ASIO seul (aucune autre possibilité sur cette plateforme).
      $("st-f-driver").hidden = false;
      $("st-jack").hidden = true;
      $("st-alsa").hidden = true;
      this.#fillAsio(p, r, forced);
    }

    const current = p.buffer ?? DEFAULT_BUFFER;
    const buf = $("st-buffer");
    buf.replaceChildren(...bufferChoices(current).map((b) => Object.assign(document.createElement("option"), { value: String(b), textContent: this.#bufferLabel(b, r) })));
    buf.value = String(current);
    buf.disabled = forced.has("buffer");
    $("st-buffer-note").textContent = forced.has("buffer") ? "Imposé par DAW_BUFFER." : "";

    const cores = r?.cores ?? 4;
    const th = $("st-threads");
    th.replaceChildren(...Array.from({ length: Math.min(cores, 16) }, (_, i) => Object.assign(document.createElement("option"), { value: String(i + 1), textContent: i === 0 ? "1 (un seul cœur)" : String(i + 1) })));
    th.value = String(Math.min(p.threads, Math.min(cores, 16)));
    th.disabled = forced.has("threads");
    $("st-ahead").value = String(p.ahead_ms);
    $("st-ahead").disabled = forced.has("ahead_ms");
    $("st-ahead-threads").value = String(p.ahead_threads);
    $("st-ahead-threads").disabled = forced.has("ahead_threads");
    const as = p.auto_stop ?? { enabled: false, after_end_secs: 120, recording_secs: 300 };
    const tp = p.transport ?? { play_pre_roll: false, return_on_stop: true };
    $("st-t-return").checked = !!tp.return_on_stop;
    $("st-su-reopen").checked = !!(p.startup ?? {}).reopen_last;
    $("st-t-preplay").checked = !!tp.play_pre_roll;
    $("st-rm-hours").value = String((p.remote ?? { elevated_hours: 12 }).elevated_hours);
    $("st-as-on").checked = !!as.enabled;
    $("st-as-end").value = String(as.after_end_secs);
    $("st-as-rec").value = String(as.recording_secs);
    $("st-forced").textContent = forced.size ? `Réglage(s) imposé(s) par une variable d'environnement : ${[...forced].join(", ")}. Ils l'emportent sur ce fichier.` : "";
    $("st-forced").hidden = !forced.size;

    const m = this.state.metronome ?? p.metronome;
    $("st-m-only").checked = !!m.only_recording;
    $("st-m-vol").value = String(Math.round((m.volume ?? 0.8) * 100));
    $("st-m-count").value = String(m.count_in_bars ?? 0);
    $("st-m-pre-unit").value = m.pre_roll_unit ?? "seconds";
    this.#fillPre(m);
    $("st-m-post").value = String(m.post_roll ?? 0);

    $("st-path").textContent = this.last?.path ? `Fichier : ${this.last.path}` : "";
    this.#refreshNotes();
  }

  /** macOS : périphériques CoreAudio (la sortie par défaut du système si rien n'est choisi). */
  #fillDevices(p, r) {
    const devices = r?.devices ?? [];
    const asked = p.device ?? "";
    const wanted = asked ? matchDriver(asked, devices) ?? asked : "";
    const sel = $("st-driver");
    const opts = [Object.assign(document.createElement("option"), { value: "", textContent: "Sortie par défaut du système" })];
    for (const n of [...new Set([...devices, ...(wanted ? [wanted] : [])])]) opts.push(Object.assign(document.createElement("option"), { value: n, textContent: devices.includes(n) ? n : `${n} (introuvable)` }));
    sel.replaceChildren(...opts);
    sel.value = wanted;
    sel.disabled = false;
    $("st-driver-note").textContent = devices.length ? "Le périphérique choisi sert à la lecture et à l'enregistrement (toutes ses entrées). Pris en compte au prochain démarrage." : "Aucun périphérique de sortie trouvé.";
  }

  /** Pilote ASIO : ceux qui sont installés, plus celui qui est enregistré s'il n'y est pas (autre machine). */
  #fillAsio(p, r, forced) {
    const drivers = r?.drivers ?? [];
    const asked = p.driver ?? r?.effective.driver ?? DEFAULT_DRIVER;
    const wanted = matchDriver(asked, drivers) ?? asked;
    const names = [...new Set([...drivers, wanted])];
    const sel = $("st-driver");
    sel.replaceChildren(...names.map((n) => Object.assign(document.createElement("option"), { value: n, textContent: drivers.includes(n) ? n : `${n} (non installé)` })));
    sel.value = wanted;
    sel.disabled = forced.has("driver") || drivers.length === 0;
    $("st-driver-note").textContent = drivers.length === 0 ? "Pas de pilote audio branché sur cette plateforme." : forced.has("driver") ? "Imposé par DAW_ASIO_DRIVER." : "";
  }

  /** N'affiche que ce qui concerne le système choisi (automatique : tout). */
  #audioLayout() {
    const c = $("st-system").value;
    const windows = (this.last?.running?.systems ?? []).includes("wasapi");
    if (windows) {
      $("st-f-driver").hidden = c === "wasapi";
      $("st-driver-note").hidden = c === "wasapi";
      $("st-jack").hidden = true;
      $("st-alsa").hidden = c === "asio";
    } else {
      $("st-f-driver").hidden = true;
      $("st-driver-note").hidden = true;
      $("st-jack").hidden = c === "alsa";
      $("st-alsa").hidden = c === "jack";
    }
  }

  /** Linux (ALSA) : périphériques de lecture et de capture. */
  #fillAlsa(p, r, family = "linux") {
    const wasapi = family === "windows";
    const all = (wasapi ? r?.wasapi_devices : r?.alsa_devices) ?? [];
    const spans = $("st-alsa").querySelectorAll(".st-field > span");
    if (spans[0]) spans[0].textContent = wasapi ? "WASAPI : lecture" : "ALSA : lecture";
    if (spans[1]) spans[1].textContent = wasapi ? "WASAPI : capture" : "ALSA : capture";
    const label = (d) => (d.description ? `${d.name} — ${d.description}` : d.name);
    const fill = (id, list, saved, none) => {
      const sel = $(id);
      const opts = [Object.assign(document.createElement("option"), { value: "", textContent: none })];
      for (const d of list.filter((x) => x.name !== "default")) opts.push(Object.assign(document.createElement("option"), { value: d.name, textContent: label(d) }));
      if (saved && saved !== "default" && !list.some((d) => d.name === saved)) opts.push(Object.assign(document.createElement("option"), { value: saved, textContent: `${saved} (introuvable)` }));
      sel.replaceChildren(...opts);
      sel.value = saved && saved !== "default" ? saved : "";
    };
    fill("st-alsa-out", all.filter((d) => d.playback), p.device, wasapi ? "Par défaut du système" : "Par défaut du système (default)");
    fill("st-alsa-in", all.filter((d) => d.capture), p.capture_device, wasapi ? "Par défaut du système" : "Le même que la lecture");
    $("st-alsa-note").textContent = !all.length
      ? (wasapi ? "Aucun appareil audio actif trouvé." : "Aucun périphérique ALSA trouvé.")
      : wasapi
        ? "WASAPI partagé : le son passe par le mélangeur de Windows (fréquence et format du système, latence de quelques ms à 10 ms). Pour la basse latence, utilise ASIO. Le tampon est la période demandée. Pris en compte au prochain démarrage."
        : "« default » et « pipewire » passent par PipeWire : ça marche sans pw-jack. Une carte « hw: » est ouverte directement : aucun autre programme (PipeWire compris) ne doit la tenir. Le tampon est la taille de la période. Pris en compte au prochain démarrage.";
  }

  /** Linux (JACK) : ports de sortie et d'entrée du serveur, les ports physiques d'abord. */
  #fillJack(p, r) {
    const ports = r?.ports ?? null;
    const lists = { "st-out-l": ["outputs", (p.out_ports ?? [])[0]], "st-out-r": ["outputs", (p.out_ports ?? [])[1]], "st-in-l": ["inputs", (p.in_ports ?? [])[0]], "st-in-r": ["inputs", (p.in_ports ?? [])[1]] };
    for (const [id, [kind, saved]] of Object.entries(lists)) {
      const all = ports?.[kind] ?? [];
      const sel = $(id);
      const opts = [Object.assign(document.createElement("option"), { value: "", textContent: "Par défaut (ports physiques)" })];
      for (const n of [...new Set([...all, ...(saved ? [saved] : [])])]) opts.push(Object.assign(document.createElement("option"), { value: n, textContent: all.includes(n) ? n : `${n} (introuvable)` }));
      sel.replaceChildren(...opts);
      sel.value = saved ?? "";
      sel.disabled = !ports;
    }
    $("st-jack-note").textContent = ports ? "Ports du serveur JACK (PipeWire : lancer l'application avec pw-jack). Un port introuvable est remplacé par les ports physiques par défaut. La liste est lue au démarrage ; pris en compte au prochain démarrage." : "Le serveur JACK n'est pas joignable : lance l'application avec pw-jack (voir tools/lancer-linux.sh).";
  }

  #bufferLabel(b, r) {
    const ms = bufferMs(b, r?.sample_rate ?? 44100);
    return `${b} échantillons${ms ? ` — ${ms.toFixed(2).replace(".", ",")} ms` : ""}`;
  }

  #form() {
    const r = this.last?.running;
    const sys = r?.systems ?? [];
    const system = sys.includes("wasapi") ? "windows" : sys.includes("alsa") ? "linux" : r?.system ?? "asio";
    return {
      system,
      driver: $("st-driver").value,
      audio_system: $("st-system").value,
      device: system === "coreaudio" ? $("st-driver").value : system === "linux" || system === "windows" ? $("st-alsa-out").value : undefined,
      capture_device: $("st-alsa-in").value,
      out_ports: [$("st-out-l").value, $("st-out-r").value],
      in_ports: [$("st-in-l").value, $("st-in-r").value],
      buffer: Number($("st-buffer").value),
      threads: Number($("st-threads").value),
      ahead_ms: Number($("st-ahead").value),
      ahead_threads: Number($("st-ahead-threads").value),
      remote: { elevated_hours: Math.max(0, Math.min(168, Math.round(Number($("st-rm-hours").value) || 0))) },
      startup: { reopen_last: $("st-su-reopen").checked },
      transport: { play_pre_roll: $("st-t-preplay").checked, return_on_stop: $("st-t-return").checked },
      auto_stop: { enabled: $("st-as-on").checked, after_end_secs: Number($("st-as-end").value), recording_secs: Number($("st-as-rec").value) },
    };
  }

  #refreshNotes() {
    const f = this.#form();
    const r = this.last?.running;
    $("st-ahead-threads").closest(".st-field").hidden = f.ahead_ms === 0;
    $("st-ahead-note").hidden = f.ahead_ms === 0;
    const b = coreBudget(f.threads, f.ahead_ms, f.ahead_threads, r?.cores);
    $("st-budget").textContent = `Cœurs occupés pendant la lecture : ${budgetText(b)}.`;
    const w = threadsWarning(f.threads, f.ahead_ms, f.ahead_threads, r?.cores);
    $("st-threads-note").textContent = w ?? "";
    $("st-threads-note").classList.toggle("warn", b.level === "over");
    const restart = !!this.last?.restart_needed;
    $("st-restart").hidden = !restart || !this.files?.tauri;
    $("st-restart-note").hidden = !restart;
  }

  /** Le champ du pré-roll montre la valeur de l'unité choisie, avec ses bornes (0 à 10 s ou 0 à 8 mesures). */
  #fillPre(m) {
    const bars = $("st-m-pre-unit").value === "bars";
    const pre = $("st-m-pre");
    pre.min = "0";
    pre.max = bars ? "8" : "10";
    pre.step = bars ? "1" : "0.5";
    pre.value = String(bars ? (m.pre_roll_bars ?? 0) : (m.pre_roll ?? 0));
  }

  #sendMetronome() {
    const cur = this.state.metronome ?? this.#prefs().metronome;
    const bars = $("st-m-pre-unit").value === "bars";
    const v = Number($("st-m-pre").value) || 0;
    const next = { ...cur, pre_roll_unit: bars ? "bars" : "seconds" };
    // Le nombre affiché appartient à l'unité choisie ; l'autre valeur est conservée telle quelle.
    if (bars) next.pre_roll_bars = Math.max(0, Math.min(8, Math.round(v)));
    else next.pre_roll = Math.max(0, Math.min(10, v));
    this.client.send({
      ...next,
      type: "set_metronome",
      only_recording: $("st-m-only").checked,
      volume: Number($("st-m-vol").value) / 100,
      count_in_bars: Number($("st-m-count").value),
      post_roll: Number($("st-m-post").value),
    });
  }

  #save() {
    const prefs = prefsFromForm(this.#form(), this.state.metronome ?? this.#prefs().metronome, this.#prefs());
    const err = validatePrefs(prefs);
    if (err) return this.#message(err, true);
    this.client.send({ type: "set_prefs", prefs });
    this.#message("Enregistré.");
  }

  async #restart() {
    const t = this.files?.tauri;
    if (!t?.core?.invoke) return;
    if (this.state.file?.dirty && !(await this.dialogs.confirm("Des modifications ne sont pas enregistrées. Redémarrer quand même ?", { ok: "Redémarrer" }))) return;
    t.core.invoke("restart_app").catch((e) => this.#message(`redémarrage impossible : ${e}`, true));
  }

  #message(text, error = false) {
    const el = $("st-message");
    if (!el) return;
    el.textContent = text;
    el.hidden = !text;
    el.classList.toggle("error", error);
  }
}
