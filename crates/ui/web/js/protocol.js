// Validation des commandes du protocole (docs/protocole-ui.md). Le moteur réel est tout aussi strict :
// une commande invalide est refusée avec un message, jamais ignorée en silence.

import { ANCHORS } from "./crossfade.js";
import { CATEGORIES } from "./section-model.js";

/** Nom de piste : de 1 à 60 caractères. */
function checkName(name, command) {
  const t = typeof name === "string" ? name.trim() : "";
  return t.length >= 1 && [...t].length <= 60 ? null : `${command} : nom de 1 à 60 caractères`;
}

export const MAX_PREVIEW_MARGIN_MS = 10_000;
export const EXPORT_FORMATS = ["wav16", "wav24", "wav32f", "flac16", "flac24"];
const SHAPES = ["linear", "equal_power"];
/** Courbe de fondu `{ shape, bend, smooth }` (voir docs/protocole-ui.md) : retourne un message d'erreur ou `null`. */
function checkCurve(c) {
  if (!c || typeof c !== "object" || !SHAPES.includes(c.shape)) return "forme de fondu inconnue";
  const bend = c.bend ?? 0;
  const smooth = c.smooth ?? 0;
  if (!Number.isInteger(bend) || bend < -1000 || bend > 1000 || !Number.isInteger(smooth) || smooth < 0 || smooth > 1000) {
    return "courbe de fondu hors plage : bend de −1000 à 1000, smooth de 0 à 1000";
  }
  return null;
}

const isInt = (v) => Number.isInteger(v) && v >= 0;
const validIds = (a) => Array.isArray(a) && a.length >= 1 && a.length <= 10000 && a.every(isInt);
const isNum = (v) => typeof v === "number" && Number.isFinite(v);

/** Retourne `null` si la commande est valide, sinon un message d'erreur. */
const validFrac = (f) => Array.isArray(f) && f.length === 2 && Number.isInteger(f[0]) && Number.isInteger(f[1]) && f[0] >= 0 && f[1] > 0;
const validVoice = (v) => v === undefined || v === null || (Number.isInteger(v) && v >= 1 && v <= 4);
const validNote = (n) => n && validFrac(n.start) && validFrac(n.len) && n.len[0] > 0 && Number.isInteger(n.key) && n.key >= 0 && n.key < 128 && n.velocity > 0 && n.velocity <= 1 && validVoice(n.voice);

/** Préférences : mêmes bornes que le crate Rust (`prefs.rs`). `null` si valides, sinon un message. */
/** Champs d'un groupe de pistes (mêmes règles que l'hôte) : nom de 1 à 40 caractères, couleur #rrggbb, membres sans doublon. */
function groupFieldsError(name, color, nodes, required) {
  if (required || name !== undefined) {
    if (typeof name !== "string" || !name.trim() || [...name].length > 40) return "groupe : nom de 1 à 40 caractères";
  }
  if (color !== undefined && color !== null && !(color === "" || /^#[0-9a-fA-F]{6}$/.test(color))) return "groupe : couleur au format #rrggbb";
  if (required || nodes !== undefined) {
    if (!Array.isArray(nodes) || nodes.length > 512 || nodes.some((n) => !Number.isInteger(n) || n <= 0) || new Set(nodes).size !== nodes.length) return "groupe : membres sans doublon, sans la sortie principale";
  }
  return null;
}

/** `ret` facultatif d'une commande de groupe : le retour dont ce sont les groupes propres. */
const retError = (m) => (m.ret === undefined || (Number.isInteger(m.ret) && m.ret > 0) ? null : `${m.type} : ret`);

export function validatePrefs(p) {
  if (!p || typeof p !== "object") return "préférences absentes";
  if (p.driver != null && (typeof p.driver !== "string" || !p.driver.trim() || p.driver.length > 200)) return "pilote : nom vide ou trop long";
  if (p.buffer != null && !(Number.isInteger(p.buffer) && p.buffer >= 8 && p.buffer <= 2048)) return "tampon : de 8 à 2048 échantillons";
  if (!(Number.isInteger(p.threads) && p.threads >= 1 && p.threads <= 16)) return "fils de calcul : de 1 à 16";
  if (!(Number.isInteger(p.ahead_ms) && (p.ahead_ms === 0 || (p.ahead_ms >= 10 && p.ahead_ms <= 500)))) return "avance : 0 (désactivée) ou de 10 à 500 ms";
  if (!(Number.isInteger(p.ahead_threads) && p.ahead_threads >= 1 && p.ahead_threads <= 8)) return "fils de rendu : de 1 à 8";
  const as = p.auto_stop;
  if (as !== undefined && (!as || typeof as.enabled !== "boolean" || !(Number.isInteger(as.after_end_secs) && as.after_end_secs >= 10 && as.after_end_secs <= 3600) || !(Number.isInteger(as.recording_secs) && as.recording_secs >= 10 && as.recording_secs <= 3600))) return "arrêt automatique : de 10 à 3 600 secondes";
  const rm = p.remote;
  if (rm !== undefined && (!rm || !(Number.isInteger(rm.elevated_hours) && rm.elevated_hours >= 0 && rm.elevated_hours <= 168))) return "droits de la télécommande : de 0 (illimité) à 168 heures";
  const tp = p.transport;
  if (tp !== undefined && (!tp || typeof tp.play_pre_roll !== "boolean" || typeof tp.return_on_stop !== "boolean")) return "transport : deux cases à cocher";
  const su = p.startup;
  if (su !== undefined && (!su || typeof su.reopen_last !== "boolean")) return "démarrage : une case à cocher";
  const m = p.metronome;
  if (!m || !(m.volume >= 0 && m.volume <= 1.5) || !(Number.isInteger(m.count_in_bars) && m.count_in_bars >= 0 && m.count_in_bars <= 2) || !(m.pre_roll >= 0 && m.pre_roll <= 10) || !(m.post_roll >= 0 && m.post_roll <= 10) || !(m.pre_roll_bars == null || (Number.isInteger(m.pre_roll_bars) && m.pre_roll_bars >= 0 && m.pre_roll_bars <= 8)) || !(m.pre_roll_unit == null || ["seconds", "bars"].includes(m.pre_roll_unit))) {
    return "métronome : volume 0 à 1,5, pré-compte 0 à 2 mesures, pré-roll et post-roll 0 à 10 s";
  }
  return null;
}

export function validateCommand(m) {
  if (!m || typeof m !== "object" || typeof m.type !== "string") return "commande sans type";
  switch (m.type) {
    case "set_return_output": {
      if (!isInt(m.node)) return "set_return_output : node invalide";
      if (m.output === null) return null;
      const o = m.output;
      if (!o || !Number.isInteger(o.first) || o.first < 3 || o.first > 32 || typeof o.stereo !== "boolean" || typeof o.mono !== "boolean") return "set_return_output : output { first (3 à 32), stereo, mono } ou null";
      return null;
    }
    case "get_chords":
    case "get_chord_catalog":
    case "get_recent":
    case "clear_recent":
    case "gather_files":
      return null;
    case "set_chords":
      return m.track && typeof m.track === "object" && !Array.isArray(m.track) ? null : "set_chords : track (objet flibuste-carte) requis";
    case "start_monitor":
      return m.port === undefined || (Number.isInteger(m.port) && m.port >= 0 && m.port <= 65535) ? null : "start_monitor : port entre 0 et 65535";
    case "get_control_pages":
      return null;
    case "set_control_page":
      if (typeof m.plugin_id !== "string" || !m.plugin_id.trim() || m.plugin_id.length > 200 || typeof m.name !== "string" || m.name.length > 120) return "set_control_page : plugin_id et name";
      if (!Array.isArray(m.params) || m.params.length > 16 || m.params.some((p) => !p || !Number.isInteger(p.id) || p.id < 0 || typeof p.name !== "string" || p.name.length > 120)) return "set_control_page : params (16 au plus, chacun { id, name })";
      return new Set(m.params.map((p) => p.id)).size === m.params.length ? null : "set_control_page : paramètre en double";
    case "get_insert_page":
      return isInt(m.insert) ? null : "get_insert_page : insert invalide";
    case "stop_monitor":
    case "get_monitor_log":
      return null;
    case "set_track_midi_filter":
      if (!Number.isInteger(m.node) || m.node <= 0 || !Array.isArray(m.rules) || m.rules.length > 16) return "set_track_midi_filter : node et rules (16 au plus)";
      return m.rules.every((r) => r && (r.device === undefined || (typeof r.device === "string" && [...r.device].length <= 120)) && (r.channels === undefined || (Array.isArray(r.channels) && r.channels.every((c) => Number.isInteger(c) && c >= 1 && c <= 16)))) ? null : "set_track_midi_filter : appareil de 120 caractères au plus, canaux de 1 à 16";
    case "set_track_dynamics": {
      const d = m.dynamics;
      const ok = (v, lo, hi) => typeof v === "number" && Number.isFinite(v) && v >= lo && v <= hi;
      if (!Number.isInteger(m.node) || m.node <= 0 || !d || typeof d.enabled !== "boolean") return "set_track_dynamics : node et dynamics { enabled, ... }";
      return ok(d.gate_db, -90, -20) && ok(d.threshold_db, -60, 0) && ok(d.ratio, 1, 20) && ok(d.attack_ms, 0.1, 100) && ok(d.release_ms, 10, 2000) && ok(d.makeup_db, 0, 24) ? null : "set_track_dynamics : porte −90 à −20 dB, seuil −60 à 0 dB, rapport 1 à 20, attaque 0,1 à 100 ms, relâchement 10 à 2 000 ms, compensation 0 à 24 dB";
    }
    case "align_clips":
      if (!Number.isInteger(m.guide) || !Array.isArray(m.doubles) || m.doubles.length < 1 || m.doubles.length > 32 || !m.doubles.every(Number.isInteger) || m.doubles.includes(m.guide)) return "align_clips : un guide et de 1 à 32 doubles (clips), différents du guide";
      if (m.guide_more !== undefined && (!Array.isArray(m.guide_more) || m.guide_more.length > 32 || !m.guide_more.every(Number.isInteger) || m.guide_more.some((g) => m.doubles.includes(g)))) return "align_clips : guide_more = clips de la piste du guide, pas parmi les doubles";
      return m.settings === undefined || m.settings === null || typeof m.settings === "string" ? null : "align_clips : settings = texte JSON";
    case "set_stretch": {
      if (!Number.isInteger(m.clip) || m.clip < 0) return "set_stretch : clip";
      if (!Array.isArray(m.markers) || m.markers.length > 256) return "set_stretch : markers (256 au plus)";
      let prev = [0, 0];
      for (const p of m.markers) {
        if (!Array.isArray(p) || p.length !== 2 || !Number.isInteger(p[0]) || !Number.isInteger(p[1])) return "set_stretch : chaque marqueur est [position dans le clip, position dans la source] en échantillons entiers";
        if (p[0] <= prev[0] || p[1] <= prev[1]) return "set_stretch : marqueurs strictement croissants (position dans le clip et dans la source, au-delà de 0)";
        prev = p;
      }
      if (m.smooth !== undefined && (!Array.isArray(m.smooth) || m.smooth.length !== m.markers.length || !m.smooth.every((b) => typeof b === "boolean"))) return "set_stretch : smooth = un booléen par marqueur";
      if (m.engine !== undefined && !["signalsmith", "rubberband_r2", "rubberband_r3"].includes(m.engine)) return "set_stretch : engine = signalsmith, rubberband_r2 ou rubberband_r3";
      return null;
    }
    case "detect_onsets":
      if (!Number.isInteger(m.clip) || m.clip < 0) return "detect_onsets : clip";
      return m.sensitivity === undefined || (typeof m.sensitivity === "number" && m.sensitivity >= 0 && m.sensitivity <= 1) ? null : "detect_onsets : sensitivity entre 0 et 1";
    case "cancel_stretch":
      return null;
    case "align_redo": {
      if (!Array.isArray(m.clips) || m.clips.length < 1 || m.clips.length > 32 || !m.clips.every(Number.isInteger)) return "align_redo : de 1 à 32 clips";
      if (m.settings !== undefined && m.settings !== null && typeof m.settings !== "string") return "align_redo : settings = texte JSON";
      if (m.consonants !== undefined && m.consonants !== null) {
        const ok = Array.isArray(m.consonants) && m.clips.length === 1 && m.consonants.every((p) => Array.isArray(p) && p.length === 2 && Number.isFinite(p[0]) && Number.isFinite(p[1]) && p[0] >= 0 && p[1] > p[0]);
        if (!ok) return "align_redo : consonnes pour un seul clip, paires (début, fin) en secondes, fin > début";
      }
      return null;
    }
    case "cancel_align":
    case "stop_listen":
      return null;
    case "listen_align": {
      if (!Number.isInteger(m.guide) || !Number.isInteger(m.node) || m.node <= 0) return "listen_align : guide (clip) et node (piste du double)";
      if (m.guide_more !== undefined && (!Array.isArray(m.guide_more) || !m.guide_more.every(Number.isInteger))) return "listen_align : guide_more = liste de clips";
      if (!["guide", "original", "aligned", "guide_original", "guide_aligned"].includes(m.what)) return "listen_align : what = guide, original, aligned, guide_original ou guide_aligned";
      if (String(m.what).endsWith("aligned") && !Number.isInteger(m.aligned)) return "listen_align : aligned (la prise calée) est requis pour écouter le double recalé";
      return null;
    }
    case "get_align":
      if (!Number.isInteger(m.clip)) return "get_align : clip";
      return m.threshold_ms === undefined || m.threshold_ms === null || (Number.isFinite(m.threshold_ms) && m.threshold_ms > 0 && m.threshold_ms <= 5000) ? null : "get_align : seuil de 0 à 5 000 ms";
    case "merge_align_settings":
      return typeof m.dst === "string" && typeof m.src === "string" ? null : "merge_align_settings : dst et src (textes JSON)";
    case "set_track_velocity_curve": {
      const c = m.curve;
      const int = (v, lo, hi) => Number.isInteger(v) && v >= lo && v <= hi;
      if (!Number.isInteger(m.node) || m.node <= 0 || !c || typeof c.enabled !== "boolean") return "set_track_velocity_curve : node et curve { enabled, ... }";
      return int(c.min, 1, 127) && int(c.max, 1, 127) && (c.bend === undefined || int(c.bend, -1000, 1000)) && (c.smooth === undefined || int(c.smooth, 0, 1000)) && (c.fixed === undefined || int(c.fixed, 0, 127)) ? null : "set_track_velocity_curve : courbure −1000 à 1000, adoucissement 0 à 1000, vélocités minimale et maximale de 1 à 127, vélocité fixe 0 à 127";
    }
    case "set_plugin_aux":
      return Number.isInteger(m.insert) && m.insert >= 0 && Number.isInteger(m.port) && m.port >= 0 && m.port < 31 && (m.to === null || (Number.isInteger(m.to) && m.to >= 0)) ? null : "set_plugin_aux : insert, port (0 à 30) et to (un nœud, ou null)";
    case "set_insert_sidechain":
      return Number.isInteger(m.insert) && m.insert >= 0 && (m.source === null || (Number.isInteger(m.source) && m.source > 0)) ? null : "set_insert_sidechain : insert et source (un nœud, ou null)";
    case "renew_monitor_address":
      return Number.isInteger(m.node) && m.node > 0 ? null : "renew_monitor_address : node (un retour)";
    case "set_monitor_role":
      return Number.isInteger(m.node) && m.node > 0 && (Number.isInteger(m.role) && m.role >= 0 && m.role <= 7) ? null : "set_monitor_role : node (un retour) et role de 0 à 7 (1 = télécommande, 2 = régie, 4 = réglages fins, additionnés)";
    case "set_structure":
      if (!(Array.isArray(m.sections) && m.sections.every((s) => s && Array.isArray(s.at) && Array.isArray(s.dur) && typeof s.name === "string"))) return "set_structure : sections (liste de { at, dur, name }) requises";
      return m.sections.every((s) => s.category === undefined || s.category === null || CATEGORIES.includes(s.category)) ? null : "set_structure : catégorie inconnue";
    case "set_lyrics":
      if (!(Array.isArray(m.syllables) && m.syllables.every((s) => s && Array.isArray(s.at) && s.at.length === 2 && typeof s.text === "string"))) return "set_lyrics : syllables (liste de { at, text }) requises";
      if (!m.syllables.every((s) => s.verse === undefined || (Number.isInteger(s.verse) && s.verse >= 1 && s.verse <= 8))) return "set_lyrics : couplet de 1 à 8";
      return m.syllables.every((s) => s.end === undefined || (Array.isArray(s.end) && s.end.length === 2 && s.end[1] > 0 && s.end[0] * s.at[1] > s.at[0] * s.end[1])) ? null : "set_lyrics : la fin d'un trait de prolongation doit suivre le début";
    case "import_chords":
    case "export_chords":
      return typeof m.path === "string" && m.path.trim() ? null : `${m.type} : path requis`;
    case "set_loop_record":
      return typeof m.enabled === "boolean" ? null : "set_loop_record : enabled est un booléen";
    case "set_plugin_area":
      return Number.isInteger(m.insert) && m.insert >= 0 && Number.isInteger(m.x) && Number.isInteger(m.y) && m.x >= 0 && m.y >= 0 && m.x <= 20000 && m.y <= 20000 ? null : "set_plugin_area : insert, x et y (pixels, entiers positifs)";
    case "switch_plugin_ui":
      return Number.isInteger(m.from) && m.from >= 0 && Number.isInteger(m.to) && m.to >= 0 ? null : "switch_plugin_ui : from et to (numéros de plugins)";
    case "set_plugin_view":
      return Number.isInteger(m.insert) && m.insert >= 0 && (m.view === "native" || m.view === "web") ? null : "set_plugin_view : insert (numéro) et view (native ou web)";
    case "set_listen":
      return m.node === null || (Number.isInteger(m.node) && m.node > 0) ? null : "set_listen : node est le numéro d'un bus ou d'un retour, ou null";
    case "set_punch":
      return typeof m.enabled === "boolean" ? null : "set_punch : enabled est un booléen";
    case "get_project":
    case "get_prefs":
      return null;
    case "set_prefs": {
      const err = validatePrefs(m.prefs);
      return err ? `set_prefs : ${err}` : null;
    }
    case "transport":
      return ["play", "stop", "toggle"].includes(m.action) ? null : `action de transport inconnue : ${m.action}`;
    case "seek":
      return isInt(m.pos) ? null : "seek : pos doit être un entier ≥ 0";
    case "loop":
      if (m.start === null && m.end === null) return null;
      return isInt(m.start) && isInt(m.end) && m.start < m.end ? null : "loop : start < end, entiers ≥ 0 (ou null pour supprimer)";
    case "set_node": {
      if (!isInt(m.node)) return "set_node : node invalide";
      const known = ["type", "node", "gain", "pan", "mute", "solo", "arm", "monitor", "invert"];
      for (const k of Object.keys(m)) if (!known.includes(k)) return `set_node : champ inconnu ${k}`;
      if (!("gain" in m || "pan" in m || "mute" in m || "solo" in m || "arm" in m || "monitor" in m || "invert" in m)) return "set_node : rien à régler";
      if ("gain" in m && !(isNum(m.gain) && m.gain >= 0)) return "set_node : gain doit être un nombre ≥ 0";
      if ("pan" in m && !(isNum(m.pan) && m.pan >= -1 && m.pan <= 1)) return "set_node : pan doit être entre -1 et 1";
      for (const b of ["mute", "solo", "arm", "monitor", "invert"]) if (b in m && typeof m[b] !== "boolean") return `set_node : ${b} doit être un booléen`;
      return null;
    }
    case "move_clip":
      if (!isInt(m.clip) || !isInt(m.start)) return "move_clip : clip et start (entiers ≥ 0)";
      return "node" in m && m.node !== undefined && !isInt(m.node) ? "move_clip : node invalide" : null;
    case "set_auto_crossfade":
      return typeof m.enabled === "boolean" ? null : "set_auto_crossfade : enabled doit être un booléen";
    case "trim_clip": {
      if (!isInt(m.clip)) return "trim_clip : clip invalide";
      const hasS = "start" in m && m.start !== undefined;
      const hasE = "end" in m && m.end !== undefined;
      if (!hasS && !hasE) return "trim_clip : start et/ou end";
      if ((hasS && !isInt(m.start)) || (hasE && !isInt(m.end))) return "trim_clip : start et end doivent être des entiers ≥ 0";
      return hasS && hasE && m.start >= m.end ? "trim_clip : start < end" : null;
    }
    case "split_clip":
      return isInt(m.clip) && isInt(m.at) ? null : "split_clip : clip et at (entiers ≥ 0)";
    case "set_active_take":
      return isInt(m.clip) ? null : "set_active_take : clip invalide";
    case "activate_take_range":
      return isInt(m.clip) && isInt(m.start) && isInt(m.end) && m.start < m.end ? null : "activate_take_range : clip, start < end requis";
    case "set_clip_muted":
      return isInt(m.clip) && typeof m.muted === "boolean" ? null : "set_clip_muted : clip et muted requis";
    case "delete_clip":
      return isInt(m.clip) ? null : "delete_clip : clip invalide";
    case "move_clips":
      if (!validIds(m.clips)) return "move_clips : clips (1 à 10 000 entiers)";
      if (!Number.isInteger(m.delta)) return "move_clips : delta doit être un entier";
      return "node_delta" in m && m.node_delta !== undefined && m.node_delta !== null && !Number.isInteger(m.node_delta) ? "move_clips : node_delta doit être un entier" : null;
    case "split_clips":
      if (!validIds(m.clips)) return "split_clips : clips (1 à 10 000 entiers)";
      return isInt(m.at) ? null : "split_clips : at (entier ≥ 0)";
    case "delete_clips":
    case "copy_clips":
      return validIds(m.clips) ? null : `${m.type} : clips (1 à 10 000 entiers)`;
    case "paste_clips":
      if (!isInt(m.at)) return "paste_clips : at (entier ≥ 0)";
      return "node" in m && m.node !== undefined && m.node !== null && !isInt(m.node) ? "paste_clips : node invalide" : null;
    case "duplicate_clips":
      if (!validIds(m.clips)) return "duplicate_clips : clips (1 à 10 000 entiers)";
      if ("delta" in m && m.delta !== undefined && m.delta !== null && !Number.isInteger(m.delta)) return "duplicate_clips : delta doit être un entier";
      return "node_delta" in m && m.node_delta !== undefined && m.node_delta !== null && !Number.isInteger(m.node_delta) ? "duplicate_clips : node_delta doit être un entier" : null;
    case "set_clip_fades": {
      if (!isInt(m.clip)) return "set_clip_fades : clip invalide";
      const has = (k) => k in m && m[k] !== undefined;
      if (!has("fade_in") && !has("fade_out") && !has("curve_in") && !has("curve_out")) return "set_clip_fades : fade_in, fade_out, curve_in et/ou curve_out";
      if ((has("fade_in") && !isInt(m.fade_in)) || (has("fade_out") && !isInt(m.fade_out))) return "set_clip_fades : fondus en échantillons entiers ≥ 0";
      return (has("curve_in") && checkCurve(m.curve_in)) || (has("curve_out") && checkCurve(m.curve_out)) || null;
    }
    case "set_crossfade": {
      if (!isInt(m.clip_out) || !isInt(m.clip_in)) return "set_crossfade : clip_out et clip_in invalides";
      if (m.clip_out === m.clip_in) return "set_crossfade : deux clips différents";
      const hasO = "curve_out" in m && m.curve_out !== undefined;
      const hasI = "curve_in" in m && m.curve_in !== undefined;
      if (!hasO && !hasI) return "set_crossfade : curve_out et/ou curve_in";
      return (hasO && checkCurve(m.curve_out)) || (hasI && checkCurve(m.curve_in)) || null;
    }
    case "preview_fade": {
      if (!Number.isInteger(m.margin_ms) || m.margin_ms < 0 || m.margin_ms > MAX_PREVIEW_MARGIN_MS) return `preview_fade : margin_ms entre 0 et ${MAX_PREVIEW_MARGIN_MS}`;
      if (typeof m.solo !== "boolean") return "preview_fade : solo doit être un booléen";
      const pair = isInt(m.clip_out) && isInt(m.clip_in) && m.clip === undefined && m.side === undefined;
      const single = m.clip_out === undefined && m.clip_in === undefined && isInt(m.clip) && (m.side === "in" || m.side === "out");
      if (pair && m.clip_out === m.clip_in) return "preview_fade : deux clips différents";
      return pair || single ? null : "preview_fade : clip_out + clip_in (fondu enchaîné) ou clip + side (« in » ou « out »)";
    }
    case "stop_preview":
    case "panic":
    case "new_project":
    case "cancel_export":
    case "cancel_import":
    case "cancel_convert":
      return null;
    case "open_plugin_ui":
    case "close_plugin_ui":
      return isInt(m.insert) ? null : `${m.type} : insert (identifiant du plugin)`;
    case "convert_project":
      return typeof m.path === "string" && m.path.trim() ? null : "convert_project : path vide";
    case "probe_audio":
    case "import_audio": {
      const ok = Array.isArray(m.files) && m.files.length >= 1 && m.files.length <= 64 && m.files.every((f) => typeof f === "string" && f.trim());
      if (!ok) return "fichiers : de 1 à 64 chemins non vides";
      if (m.type === "probe_audio") return null;
      if (m.node != null && !(isInt(m.node) && m.node !== 0)) return "import_audio : la sortie principale ne reçoit pas de clip";
      if (m.pos != null && !isInt(m.pos)) return "import_audio : pos en échantillons (entier ≥ 0)";
      if (m.multichannel != null && !["split", "stereo"].includes(m.multichannel)) return "import_audio : multichannel = split ou stereo";
      return null;
    }
    case "export": {
      if (!["project", "loop", "range"].includes(m.scope)) return "export : scope = project, loop ou range";
      if (m.scope === "range" && !(isInt(m.start) && isInt(m.end) && m.start < m.end)) return "export : scope « range » demande start < end (échantillons)";
      if (!["mix", "stems"].includes(m.mode)) return "export : mode = mix ou stems";
      if (!EXPORT_FORMATS.includes(m.format)) return `export : format = ${EXPORT_FORMATS.join(", ")}`;
      if (m.tail_secs !== undefined && !(isNum(m.tail_secs) && m.tail_secs >= 0 && m.tail_secs <= 60)) return "export : tail_secs entre 0 et 60";
      if (m.normalize_db != null && !(isNum(m.normalize_db) && m.normalize_db >= -30 && m.normalize_db <= 0)) return "export : normalize_db entre −30 et 0 dBFS";
      if (m.mode === "stems" && m.normalize_db != null) return "export : la normalisation ne s'applique qu'au mixage (les stems gardent leur équilibre)";
      if (m.nodes != null && !(Array.isArray(m.nodes) && m.nodes.length && m.nodes.every((n) => isInt(n) && n !== 0))) return "export : nodes = au moins une piste ou un bus (pas la sortie principale)";
      for (const k of ["dir", "name"]) if (m[k] != null && !(typeof m[k] === "string" && m[k].trim())) return `export : ${k} ne peut pas être vide`;
      return null;
    }
    case "set_tempo":
      if (!(typeof m.bpm === "number" && m.bpm >= 20 && m.bpm <= 400)) return "set_tempo : bpm entre 20 et 400";
      return Number.isInteger(m.at_bar) && m.at_bar >= 1 ? null : "set_tempo : at_bar ≥ 1 (1 = le début du projet)";
    case "set_meter":
      if (!(Number.isInteger(m.num) && m.num >= 1 && m.num <= 32 && [1, 2, 4, 8, 16, 32].includes(m.den))) return "set_meter : num de 1 à 32, den parmi 1, 2, 4, 8, 16, 32";
      return Number.isInteger(m.at_bar) && m.at_bar >= 1 ? null : "set_meter : at_bar ≥ 1 (1 = le début du projet)";
    case "set_lane": {
      const t = m.target;
      if (!t || !["gain", "pan", "send", "insert"].includes(t.kind)) return "set_lane : target { kind: gain | pan | send | insert }";
      if (!Number.isInteger(m.node) || !Array.isArray(m.points)) return "set_lane : node et points";
      if (m.mode !== undefined && !["read", "trim", "off"].includes(m.mode)) return "set_lane : mode read, trim ou off";
      const [lo, hi] = t.kind === "pan" ? [-1, 1] : t.kind === "insert" ? [-Infinity, Infinity] : [0, 4];
      let last = -1;
      for (const p of m.points) {
        const pos = Array.isArray(p.pos) && Number.isInteger(p.pos[0]) && Number.isInteger(p.pos[1]) && p.pos[1] > 0 ? p.pos[0] / p.pos[1] : NaN;
        if (!(pos > last) || !Number.isFinite(p.value) || p.value < lo || p.value > hi || !["linear", "step"].includes(p.curve)) return "set_lane : points à positions strictement croissantes, valeurs dans la plage de la cible, courbe linear ou step";
        last = pos;
      }
      return null;
    }
    case "set_lane_mode":
      return m.target && ["read", "trim", "off"].includes(m.mode) && Number.isInteger(m.node) ? null : "set_lane_mode : node, target et mode (read, trim, off)";
    case "add_send":
    case "remove_send":
      if (!Number.isInteger(m.node) || !Number.isInteger(m.to) || m.node <= 0 || m.to < 0 || m.node === m.to) return `${m.type} : node (une piste ou un bus) et to (destination différente)`;
      return null;
    case "set_send":
      if (!Number.isInteger(m.node) || !Number.isInteger(m.to) || m.node <= 0 || m.to < 0 || m.node === m.to) return "set_send : node et to";
      if (m.gain === undefined && m.pre === undefined && m.mute === undefined && m.solo === undefined) return "set_send : gain, pre, mute et/ou solo";
      if (m.gain !== undefined && !(m.gain >= 0 && m.gain <= 2)) return "set_send : gain entre 0 et 2";
      for (const k of ["pre", "mute", "solo"]) if (m[k] !== undefined && typeof m[k] !== "boolean") return `set_send : ${k} est un booléen`;
      return null;
    case "create_group":
      return groupFieldsError(m.name, m.color, m.nodes, true) ?? retError(m);
    case "edit_group":
      if (!Number.isInteger(m.group) || m.group <= 0) return "edit_group : groupe inconnu";
      return groupFieldsError(m.name, m.color, m.nodes, false) ?? retError(m);
    case "delete_group":
      return Number.isInteger(m.group) && m.group > 0 ? retError(m) : "delete_group : groupe inconnu";
    case "copy_return_groups":
      if (!Number.isInteger(m.from) || m.from <= 0) return "copy_return_groups : from (le retour dont on copie les groupes)";
      return Number.isInteger(m.ret) && m.ret > 0 ? null : "copy_return_groups : ret (un retour de monitoring)";
    case "customize_return_groups":
    case "reset_return_groups":
      return Number.isInteger(m.ret) && m.ret > 0 ? null : `${m.type} : ret (un retour de monitoring)`;
    case "set_group_offset":
      if (!Number.isInteger(m.group) || m.group <= 0) return "set_group_offset : groupe inconnu";
      if (!(Number.isFinite(m.db) && m.db >= -24 && m.db <= 24)) return "set_group_offset : db entre -24 et 24";
      return m.to === undefined || (Number.isInteger(m.to) && m.to >= 0) ? null : "set_group_offset : to";
    case "set_group":
      if (!Number.isInteger(m.group) || m.group <= 0) return "set_group : groupe inconnu";
      if (m.mute === undefined && m.solo === undefined && m.delta_db === undefined) return "set_group : mute, solo et/ou delta_db";
      if (m.delta_db !== undefined && !(Number.isFinite(m.delta_db) && m.delta_db >= -96 && m.delta_db <= 96)) return "set_group : delta_db entre -96 et 96";
      if (m.to !== undefined && !(Number.isInteger(m.to) && m.to >= 0)) return "set_group : to";
      return null;
    case "set_write_mode":
      if (!Number.isInteger(m.node) || !m.target || !["gain", "pan", "send", "insert"].includes(m.target.kind)) return "set_write_mode : node et target";
      return ["none", "touch", "latch", "write"].includes(m.mode) ? null : "set_write_mode : none, touch, latch ou write";
    case "gesture":
      return Number.isInteger(m.node) && m.target && ["gain", "pan", "send", "insert"].includes(m.target.kind) && typeof m.active === "boolean" ? null : "gesture : node, target, active";
    case "get_plugin_param_text":
      return Number.isInteger(m.insert) && Number.isInteger(m.param) && Number.isFinite(m.value) ? null : "get_plugin_param_text : insert, param, value";
    case "get_notes":
      return Number.isInteger(m.clip) ? null : "get_notes : clip manquant";
    case "set_events": {
      const range = { cc: [0, 127], pressure: [0, 127], bend: [-8192, 8191] }[m.kind];
      if (!range) return "set_events : kind = cc, bend ou pressure";
      if ((m.kind === "cc") !== (m.controller !== undefined && m.controller !== null) || (m.kind === "cc" && !(Number.isInteger(m.controller) && m.controller >= 0 && m.controller <= 127))) return "set_events : controller (0 à 127) requis pour cc, et seulement pour cc";
      if (!Array.isArray(m.events) || m.events.length > 4096) return "set_events : au plus 4096 points";
      return m.events.every((e) => e && validFrac(e.start) && Number.isInteger(e.value) && e.value >= range[0] && e.value <= range[1] && (e.channel === undefined || (Number.isInteger(e.channel) && e.channel >= 0 && e.channel < 16))) ? null : "set_events : points invalides (fraction, valeur dans la plage, canal < 16)";
    }
    case "add_notes":
      return Array.isArray(m.notes) && m.notes.length && m.notes.every(validNote) ? null : "add_notes : au moins une note valide";
    case "edit_notes":
      return Array.isArray(m.edits) && m.edits.length && m.edits.every((e) => Number.isInteger(e.index) && e.index >= 0 && (e.start === undefined || validFrac(e.start)) && (e.len === undefined || (validFrac(e.len) && e.len[0] > 0)) && (e.key === undefined || (Number.isInteger(e.key) && e.key >= 0 && e.key < 128)) && (e.channel === undefined || (Number.isInteger(e.channel) && e.channel >= 0 && e.channel < 16)) && (e.velocity === undefined || (e.velocity > 0 && e.velocity <= 1)) && validVoice(e.voice)) ? null : "edit_notes : modifications invalides";
    case "delete_notes":
      return Array.isArray(m.indices) && m.indices.length && m.indices.every((i) => Number.isInteger(i) && i >= 0) ? null : "delete_notes : aucune note";
    case "quantize_notes":
      return validFrac(m.grid) && m.grid[0] > 0 ? null : "quantize_notes : grille invalide";
    case "preview_note":
      return Number.isInteger(m.node) && Number.isInteger(m.key) && m.key >= 0 && m.key < 128 && m.velocity > 0 && m.velocity <= 1 && typeof m.on === "boolean" ? null : "preview_note : node, key (< 128), velocity (]0, 1]), on";
    case "discard_autosave":
      return typeof m.path === "string" && m.path.trim() ? null : "discard_autosave : path vide";
    case "remove_tempo":
      return Array.isArray(m.pos) && m.pos.length === 2 && Number.isInteger(m.pos[0]) && Number.isInteger(m.pos[1]) && m.pos[1] > 0 ? null : "remove_tempo : pos = [n, d]";
    case "remove_meter":
      return Number.isInteger(m.at_bar) && m.at_bar >= 2 ? null : "remove_meter : at_bar ≥ 2 (le chiffrage initial ne se supprime pas)";
    case "set_metronome":
      if (typeof m.enabled !== "boolean" || typeof m.only_recording !== "boolean") return "set_metronome : enabled et only_recording sont des booléens";
      if (!(m.volume >= 0 && m.volume <= 1.5) || !(Number.isInteger(m.count_in_bars) && m.count_in_bars >= 0 && m.count_in_bars <= 2) || !(m.pre_roll >= 0 && m.pre_roll <= 10) || !(m.post_roll >= 0 && m.post_roll <= 10) || !(m.pre_roll_bars == null || (Number.isInteger(m.pre_roll_bars) && m.pre_roll_bars >= 0 && m.pre_roll_bars <= 8)) || !(m.pre_roll_unit == null || ["seconds", "bars"].includes(m.pre_roll_unit))) {
        return "set_metronome : volume 0 à 1,5, pré-compte 0 à 2 mesures, pré-roll 0 à 10 s ou 0 à 8 mesures, post-roll 0 à 10 s";
      }
      return null;
    case "list_plugins":
    case "record":
    case "stop_record":
      return null;
    case "set_track_input": {
      if (!isInt(m.node) || m.node === 0) return "set_track_input : node (une piste)";
      const inp = m.input;
      if (!inp || typeof inp !== "object") return "set_track_input : input manquant";
      if (inp.kind === "midi") return null;
      if (inp.kind !== "audio") return `set_track_input : kind « ${inp.kind} » inconnu (audio ou midi)`;
      const ch = inp.channels;
      return Array.isArray(ch) && ch.length >= 1 && ch.length <= 2 && ch.every((c) => Number.isInteger(c) && c >= 1 && c <= 64) ? null : "set_track_input : audio demande 1 ou 2 canaux, numérotés de 1 à 64";
    }
    case "add_plugin":
      if (!isInt(m.node)) return "add_plugin : node (piste ou bus)";
      if (typeof m.clap_id !== "string" || m.clap_id.trim() === "") return "add_plugin : clap_id (identifiant du plugin)";
      return m.file === undefined || typeof m.file === "string" ? null : "add_plugin : file doit être un texte";
    case "remove_plugin":
    case "get_plugin_params":
      return isInt(m.insert) ? null : `${m.type} : insert (identifiant du plugin)`;
    case "move_plugin":
      if (!isInt(m.insert)) return "move_plugin : insert (identifiant du plugin)";
      return m.before === undefined || isInt(m.before) ? null : "move_plugin : before doit être un entier";
    case "set_plugin_bypass":
      if (!isInt(m.insert)) return "set_plugin_bypass : insert (identifiant du plugin)";
      return typeof m.bypass === "boolean" ? null : "set_plugin_bypass : bypass doit être un booléen";
    case "set_plugin_param":
      if (!isInt(m.insert) || !isInt(m.param)) return "set_plugin_param : insert et param (entiers)";
      return typeof m.value === "number" && Number.isFinite(m.value) ? null : "set_plugin_param : value doit être un nombre fini";
    case "add_track":
      if (!["mono", "stereo", "bus", "return"].includes(m.kind)) return `add_track : kind « ${m.kind} » inconnu (mono, stereo, bus ou return)`;
      if (m.output !== undefined && m.output !== null) {
        if (m.kind !== "bus" && m.kind !== "return") return "add_track : une sortie physique ne se donne qu'à un bus ou à un retour";
        if (!Number.isInteger(m.output.first) || m.output.first < 3) return "add_track : output.first (à partir de 3)";
      }
      return m.name === undefined ? null : checkName(m.name, "add_track");
    case "delete_track":
    case "move_track":
      if (!isInt(m.node) || m.node === 0) return `${m.type} : node (piste ou bus, pas la sortie principale)`;
      if (m.type === "move_track" && m.before !== undefined && !isInt(m.before)) return "move_track : before doit être un entier";
      return null;
    case "rename_track":
      if (!isInt(m.node) || m.node === 0) return "rename_track : node (piste ou bus, pas la sortie principale)";
      return checkName(m.name, "rename_track");
    case "set_track_color":
      if (!isInt(m.node) || m.node === 0) return "set_track_color : node (piste ou bus, pas la sortie principale)";
      return typeof m.color === "string" && /^#[0-9a-fA-F]{6}$/.test(m.color) ? null : "set_track_color : color doit être #rrggbb";
    case "set_track_output":
      if (!isInt(m.node) || m.node === 0) return "set_track_output : node (piste ou bus, pas la sortie principale)";
      return isInt(m.output) ? null : "set_track_output : output (0 = sortie principale, ou un bus)";
    case "open_project":
      return typeof m.path === "string" && m.path.trim() !== "" ? null : "open_project : path (dossier du projet)";
    case "save_project":
      return m.path === undefined || (typeof m.path === "string" && m.path.trim() !== "") ? null : "save_project : path doit être un texte non vide";
    case "set_crossfade_length":
      if (!isInt(m.clip_out) || !isInt(m.clip_in)) return "set_crossfade_length : clip_out et clip_in invalides";
      if (m.clip_out === m.clip_in) return "set_crossfade_length : deux clips différents";
      if (!isInt(m.length) || m.length < 1) return "set_crossfade_length : length ≥ 1 échantillon";
      if (!ANCHORS.includes(m.anchor)) return `set_crossfade_length : anchor inconnu « ${m.anchor} » (center, start ou end)`;
      return null;
    case "begin_group":
      return typeof m.label === "string" && m.label.trim() !== "" && [...m.label].length <= 80 ? null : "begin_group : libellé de 1 à 80 caractères";
    case "end_group":
    case "abort_group":
    case "undo":
    case "redo":
      return null;
    case "get_peaks":
      if (!isInt(m.clip) || !isInt(m.from) || !isInt(m.to) || m.from >= m.to) return "get_peaks : clip, from < to";
      if (!Number.isInteger(m.columns) || m.columns < 1 || m.columns > 8192) return "get_peaks : columns entre 1 et 8192";
      return null;
    default:
      return `commande inconnue : ${m.type}`;
  }
}

/** Gain linéaire ↔ décibels (affichage des faders). Sous -60 dB l'affichage dit « -∞ ». */
export const gainToDb = (g) => (g <= 0 ? -Infinity : 20 * Math.log10(g));
export const dbToGain = (db) => (db <= -60 ? 0 : 10 ** (db / 20));
