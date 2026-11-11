// Faux moteur : respecte le protocole (docs/protocole-ui.md) sans produire de son. Il permet de construire et de
// vérifier l'interface seule, et de tester la couche de messages sans carte son. Il applique la même validation
// stricte que le vrai moteur.

import { CATEGORIES } from "./section-model.js";
import { EngineClient } from "./engine-client.js";
import { validateCommand } from "./protocol.js";
import { mergeGeneral, parseSettings, stringifySettings } from "./align-model.js";
import { crossfadeBounds, crossfadeEdges } from "./crossfade.js";
import { waveformFor, columnPeaks } from "./mock-waveform.js";
import { MOCK_CHORD_CATALOG } from "./mock-chord-catalog.js";
import { resolveQuality } from "./chord-model.js";
import { fadd, fcmp } from "./notes-model.js";
import { ENGINES, plain, rewindow, validate as validateStretch } from "./stretch-model.js";

const SR = 48_000;
const BAR = SR * 2; // 4/4 à 120 noires par minute : une ronde = 2 s
const bars = (n) => n * BAR;

function demoProject() {
  const audio = (id, name, startBar, lenBars, wave, extra = {}) => ({ id, kind: "audio", name, start: bars(startBar), len: bars(lenBars), offset: 0, source_len: bars(lenBars + 4), gain: 1, fade_in: 0, fade_out: 0, fade_in_curve: { shape: "linear", bend: 0, smooth: 0 }, fade_out_curve: { shape: "linear", bend: 0, smooth: 0 }, wave, ...extra });
  const arpeggio = [];
  const chords = [[57, 60, 64], [57, 60, 64], [53, 57, 60], [55, 59, 62]];
  for (let step = 0; step < 64; step++) {
    const chord = chords[Math.floor(step / 8) % 4];
    arpeggio.push({ start: step * (BAR / 8), len: BAR / 8 - 600, key: chord[[0, 1, 2, 1][step % 4]] + (step % 8 >= 4 ? 12 : 0), velocity: 0.55 + 0.35 * ((step * 7) % 5) / 4 });
  }
  return {
    audio_inputs: 10,
    audio_outputs: 8, // la UMC de Jean : 8 sorties ouvertes aujourd'hui (4 paires)
    sample_rate: SR,
    tempo_map: [{ pos: [0, 1], num: 120, den: 1 }],
    meters: [{ bar: 0, num: 4, den: 4 }],
    latency_samples: 356,
    nodes: [
      { id: 0, name: "Sortie", kind: "master", color: "#9aa3b2", gain: 1, pan: 0, mute: false, solo: false, invert: false, arm: false, output: null, input: { kind: "audio", channels: [1] }, monitor: true, inserts: [], clips: [] },
      { id: 1, name: "Batterie", kind: "stereo", color: "#e5484d", gain: 0.8, pan: 0, mute: false, solo: false, invert: false, arm: false, output: 0, input: { kind: "audio", channels: [1] }, monitor: true, inserts: [], clips: [audio(10, "Beat A", 0, 8, "drums"), audio(11, "Beat B", 8, 8, "drums", { fade_out: 24_000 })] },
      { id: 2, name: "Basse", kind: "mono", color: "#f5a524", gain: 0.7, pan: -0.1, mute: false, solo: false, invert: false, arm: false, output: 0, input: { kind: "audio", channels: [1] }, monitor: true, inserts: [], clips: [audio(20, "Basse", 0, 16, "bass")] },
      { id: 3, name: "Claviers", kind: "stereo", color: "#4aa3ff", gain: 0.6, pan: 0.15, mute: false, solo: false, invert: false, arm: false, output: 0, input: { kind: "audio", channels: [1] }, monitor: true, inserts: [], clips: [{ id: 30, kind: "midi", name: "Arpège", start: bars(4), len: bars(8), offset: 0, source_len: 1_000_000_000_000, fade_in: 0, fade_out: 0, mat: arpeggio }] },
      { id: 4, name: "Guitare", kind: "stereo", color: "#30a46c", gain: 0.65, pan: -0.35, mute: false, solo: false, invert: false, arm: false, output: 0, input: { kind: "audio", channels: [1] }, monitor: true, inserts: [], clips: [audio(40, "Riff 1", 2, 4.5, "pad", { fade_in: 12_000, fade_out: 48_000, fade_out_curve: { shape: "equal_power", bend: 0, smooth: 0 } }), audio(41, "Riff 2", 6, 4, "pad", { fade_in: 48_000, fade_out: 12_000, fade_in_curve: { shape: "equal_power", bend: 0, smooth: 0 } })] },
      { id: 5, name: "Voix", kind: "mono", color: "#a78bfa", gain: 0.9, pan: 0.05, mute: false, solo: false, invert: false, arm: false, output: 0, input: { kind: "audio", channels: [1] }, monitor: true, inserts: [], clips: [audio(50, "Couplet", 8, 7, "voice", { fade_out: 36_000 })] },
    ],
  };
}

/** Plugins de démonstration : mêmes champs que l'inventaire du vrai moteur. `params` : [nom, module, min, max, défaut]. */
const MOCK_PLUGINS = [
  { clap_id: "mock.analogsynth", name: "AnalogSynth", vendor: "Le Tournedos Rossini", file: "AnalogSynth.clap", description: "Synthé analogique", features: ["instrument"], latency: 0, params: [["Volume", "Sortie", 0, 1, 0.7], ["Fréquence de coupure", "Filtre", 20, 20000, 4000], ["Résonance", "Filtre", 0, 1, 0.2], ["Attaque", "Enveloppe", 0, 5, 0.01], ["Relâchement", "Enveloppe", 0, 10, 0.4]] },
  { clap_id: "mock.neptune", name: "Neptune", vendor: "Le Tournedos Rossini", file: "Neptune.clap", description: "Sampleur multi-sorties", features: ["instrument"], latency: 0, aux: ["Sortie 2", "Sortie 3", "Sortie 4"], params: [["Volume", "Sortie", 0, 1, 0.7], ["Accord", "Sortie", -12, 12, 0]] },
  { clap_id: "mock.compresseur", name: "Compresseur", vendor: "Le Tournedos Rossini", file: "Compresseur.clap", description: "Compresseur", features: ["audio-effect"], latency: 0, sidechain: true, params: [["Seuil", "", -60, 0, -18], ["Ratio", "", 1, 20, 4], ["Attaque", "", 0.1, 100, 10], ["Relâchement", "", 10, 1000, 120], ["Gain de sortie", "", -12, 12, 0]] },
  { clap_id: "mock.egaliseur", name: "Egaliseur", vendor: "Le Tournedos Rossini", file: "Egaliseur.clap", description: "Égaliseur paramétrique", features: ["audio-effect"], latency: 0, params: Array.from({ length: 40 }, (_, i) => [`Bande ${Math.floor(i / 4) + 1} ${["fréquence", "gain", "largeur", "type"][i % 4]}`, `Bande ${Math.floor(i / 4) + 1}`, 0, 1, 0.5]) },
  { clap_id: "mock.digue", name: "Digue", vendor: "Le Tournedos Rossini", file: "Digue.clap", description: "Limiteur", features: ["audio-effect"], latency: 480, params: [["Plafond", "", -12, 0, -1], ["Anticipation", "", 0, 10, 5]] },
  { clap_id: "mock.tal-chorus", name: "TAL-Chorus-LX", vendor: "TAL Software", file: "TAL-Chorus-LX.clap", description: "Chorus", features: ["audio-effect"], latency: 0, gui: false, params: [["Dry/Wet", "", 0, 1, 0.5]] },
];
const TRACK_COLORS = ["#4aa3ff", "#e5484d", "#f5a524", "#30a46c", "#a78bfa", "#f76b15", "#12a594", "#d6409f", "#9aa3b2"];
const OUT_OF_RANGE = "position hors du clip, ou clip qui serait vide, ou données audio insuffisantes";

const pan = (law, p) => (law === "mono" ? [Math.cos(((p + 1) * Math.PI) / 4), Math.sin(((p + 1) * Math.PI) / 4)] : [Math.min(1, 1 - p), Math.min(1, 1 + p)]);

export class MockEngine extends EngineClient {
  constructor() {
    super();
    this.project = demoProject();
    this.pos = 0;
    this.playing = false;
    this.loop = null;
    this.punchOn = false; // punch allumé : record enregistre exactement dans la zone bleue (la boucle)
    this.waves = new Map();
    this.preview = null; // aperçu sonore d'un fondu en cours (voir `#startPreview`)
    this.timer = null;
    this.autoCrossfade = true;
    this.undoStack = []; // { label, before, after } : copies des clips avant et après chaque édition
    this.redoStack = [];
    // Fichiers : le faux moteur n'écrit rien sur le disque, il garde en mémoire ce qu'il « enregistre » (chemin → projet).
    this.disk = new Map();
    this.plugins = new Map(); // identifiant d'insert → { def, values } (gardé même quand le plugin est retiré : l'annulation le retrouve)
    this.nextInsert = 1;
    this.metronome = { enabled: false, only_recording: false, volume: 0.8, count_in_bars: 0, pre_roll: 0, post_roll: 0, pre_roll_bars: 0, pre_roll_unit: "seconds" };
    // Préférences : le faux moteur « tourne » avec les valeurs par défaut ; un redémarrage n'a pas de sens ici (voir `#prefsEvent`).
    this.prefs = { version: 1, driver: null, buffer: null, threads: 1, ahead_ms: 0, ahead_threads: 2, transport: { play_pre_roll: false, return_on_stop: true }, remote: { elevated_hours: 12 }, metronome: { ...this.metronome } };
    this.running = { effective: { driver: "UMC", buffer: 64, threads: 1, ahead_ms: 0, ahead_threads: 2, forced: [] }, sample_rate: 44100, inputs: 10, drivers: ["UMC ASIO Driver", "ASIO4ALL v2"], cores: 4, audio: true };
    this.recording = null; // enregistrement en cours : { start, nodes }
    this.openUis = new Set(); // identifiants des plugins dont l'interface native est « ouverte »
    this.catalogSent = false;
    this.file = { name: "Démonstration", path: null, dirty: false, generation: 0 };
  }

  #emitFile(warnings = []) {
    this.emit("file", { name: this.file.name, path: this.file.path, dirty: this.file.dirty, generation: this.file.generation, warnings });
  }

  #markDirty() {
    if (!this.file.dirty) {
      this.file.dirty = true;
      this.#emitFile();
    }
  }

  /** Remplace le projet (nouveau ou ouvert) : historique, lecture, boucle et aperçu repartent de zéro. */
  #replaceProject(nodes, autoCrossfade, path) {
    this.#endPreview(false, false);
    this.openUis.clear();
    this.project = { ...this.project, nodes, groups: [] };
    this.groupOffsets = new Map();
    this.autoCrossfade = autoCrossfade;
    this.undoStack = [];
    this.redoStack = [];
    this.waves.clear();
    this.pos = 0;
    this.playing = false;
    this.loop = null;
    this.punchOn = false;
    this.file = { name: path ? path.replace(/^.*[\\/]/, "").replace(/\.daw$/i, "") : "Sans titre", path, dirty: false, generation: this.file.generation + 1 };
    this.#emitFile();
    this.#emitProject();
    this.#emitHistory();
    this.#emitPosition();
    this.#emitPunch();
  }

  /** Piste d'accords vide (le vrai hôte en a une qui vient d'un projet ou d'un import). */
  get chordTrack() {
    return (this._chordTrack ??= { format: "flibuste-carte", version: 1, chords: [], scales: [], keys: [], patterns: [], measures: [], qualities: {} });
  }

  set chordTrack(t) {
    this._chordTrack = t;
  }

  #emitChords() {
    this.chordRevision = (this.chordRevision ?? 0) + 1;
    this.emit("chords", { revision: this.chordRevision, track: structuredClone(this.chordTrack), skipped: [], structure: structuredClone(this.structure ?? []), lyrics: structuredClone(this.lyrics ?? []) });
  }

  /** Mêmes refus que l'hôte : durée nulle, accords qui se chevauchent, qualité inconnue (jamais de perte silencieuse). */
  #checkChordTrack(track) {
    const t = structuredClone(track);
    for (const k of ["chords", "scales", "keys", "patterns", "measures"]) t[k] ??= [];
    for (const c of t.chords) {
      if (c.nc) continue;
      const q = resolveQuality(c.quality ?? "", MOCK_CHORD_CATALOG);
      if (q === null) throw new Error(`qualité « ${c.quality} » inconnue`);
      c.quality = q;
    }
    for (const list of [t.chords, t.scales, t.patterns]) for (const x of list) if (!(x.dur[0] > 0)) throw new Error("durée nulle");
    t.chords.sort((a, b) => fcmp(a.at, b.at));
    for (let i = 1; i < t.chords.length; i++) if (fcmp(fadd(t.chords[i - 1].at, t.chords[i - 1].dur), t.chords[i].at) > 0) throw new Error("des accords se chevauchent");
    t.scales.sort((a, b) => fcmp(a.at, b.at));
    return t;
  }

  /** Jeton de l'adresse d'un retour : tiré une fois puis gardé (comme dans le projet). */
  #token(n) {
    if (!n.monitor_token) n.monitor_token = Array.from({ length: 32 }, () => "0123456789abcdef"[Math.floor(Math.random() * 16)]).join("");
    return n.monitor_token;
  }

  /** Aperçu du serveur des retours : une fausse adresse et un faux code QR (le vrai serveur est dans l'hôte). */
  #emitControlPages() {
    this.emit("control_pages", { entries: [...(this.bank?.values() ?? [])], error: null });
  }

  #emitMonitor() {
    const returns = this.monitorOn ? this.project.nodes.filter((n) => n.kind === "bus" && n.is_return).map((n) => ({ node: n.id, name: n.name, role: n.monitor_role ?? 0, url: `http://192.168.0.10:8780/r/${this.#token(n)}`, qr: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 4" width="160" height="160"><rect width="4" height="4" fill="#fff"/><path d="M0 0h2v2H0zM2 2h2v2H2z"/></svg>' })) : [];
    this.emit("monitor", { active: !!this.monitorOn, port: this.monitorOn ? 8780 : 0, returns, error: null });
  }

  #emitPunch() {
    this.emit("loop_record", { enabled: !!this.loopRecOn });
    this.emit("punch", { enabled: this.punchOn });
  }

  /** Copie des pistes (structure, clips) pour l'historique. */
  #snapshot() {
    const s = structuredClone(this.project.nodes);
    s.__chords = structuredClone(this.chordTrack); // la piste d'accords suit l'historique, comme le reste
    s.__structure = structuredClone(this.structure ?? []);
    s.__lyrics = structuredClone(this.lyrics ?? []);
    s.__groups = structuredClone(this.project.groups ?? []);
    return s;
  }

  /** Remet la structure et les clips ; les réglages de mixage (hors historique) restent ceux d'aujourd'hui. */
  #restore(snap) {
    if (snap.__structure) this.structure = structuredClone(snap.__structure);
    if (snap.__lyrics) this.lyrics = structuredClone(snap.__lyrics);
    if (snap.__groups) this.project.groups = structuredClone(snap.__groups);
    if (snap.__chords) {
      this.chordTrack = structuredClone(snap.__chords);
      this.#emitChords();
    }
    const current = new Map(this.project.nodes.map((n) => [n.id, n]));
    this.project.nodes = snap.map((s) => {
      const cur = current.get(s.id);
      const copy = structuredClone(s);
      return cur ? { ...copy, gain: cur.gain, pan: cur.pan, mute: cur.mute, solo: cur.solo, arm: cur.arm } : copy;
    });
  }

  #newGroupId() {
    const all = [...(this.project.groups ?? []), ...this.project.nodes.flatMap((n) => n.return_groups?.groups ?? [])];
    this.nextGroupId = Math.max(this.nextGroupId ?? 0, ...all.map((g) => g.id)) + 1;
    return this.nextGroupId;
  }

  /** Les groupes visés par une commande : ceux du projet, ou les groupes propres du retour `ret` (qui doit en avoir). */
  #groupScope(ret) {
    if (ret === undefined) {
      const list = (this.project.groups ??= []);
      return {
        list,
        memberOf: new Map(this.project.nodes.filter((n) => n.group).map((n) => [n.id, n.group])),
        write: (pairs) => {
          const m = new Map(pairs);
          for (const n of this.project.nodes) n.group = m.get(n.id) ?? 0;
        },
        replaceList: (l) => (this.project.groups = l),
      };
    }
    const r = this.node(ret);
    if (!r?.return_groups) throw new Error("ce retour n'a pas de groupes propres : commence par les personnaliser");
    return {
      list: r.return_groups.groups,
      memberOf: new Map(r.return_groups.members.map(([n, g]) => [n, g])),
      write: (pairs) => (r.return_groups.members = [...new Map(pairs)].map(([n, g]) => [n, g])),
      replaceList: (l) => (r.return_groups.groups = l),
    };
  }

  /** Les groupes et l'appartenance qui comptent dans le mix de `to` (ses groupes propres, sinon ceux du projet). */
  #effective(to) {
    const r = to === undefined ? null : this.node(to);
    if (r?.return_groups) {
      const of = new Map(r.return_groups.members.map(([n, g]) => [n, g]));
      return { groups: r.return_groups.groups, groupOf: (id) => of.get(id) ?? 0 };
    }
    return { groups: this.project.groups ?? [], groupOf: (id) => this.node(id)?.group ?? 0 };
  }

  /** Remet à 0 les décalages de groupes vers `ret` (les niveaux des envois reviennent). */
  #clearOffsets(ret) {
    for (const [key, db] of [...(this.groupOffsets ?? new Map())]) {
      const [g, to] = key.split(":");
      if (to === String(ret) && db !== 0) this.send({ type: "set_group_offset", group: Number(g), db: 0, to: ret });
    }
    for (const key of [...(this.groupOffsets ?? new Map()).keys()]) if (key.endsWith(`:${ret}`)) this.groupOffsets.delete(key);
  }

  /** Valide une sortie sur la carte son pour `n` (un bus ou un retour) et rend la sortie physique, ou lève le refus. */
  #checkPhys(n, o) {
    const last = o.first + (o.stereo ? 2 : 1) - 1;
    if (n.kind !== "bus") throw new Error("seul un bus ou un retour peut sortir sur la carte son");
    if (o.first < 3) throw new Error("les sorties 1 et 2 sont celles de la sortie principale : on prend les suivantes");
    if (last > this.project.audio_outputs) throw new Error(`sortie ${last} inexistante : le pilote en a ouvert ${this.project.audio_outputs}`);
    const taken = this.project.nodes.find((x) => x.id !== n.id && x.phys && o.first < x.phys.first + (x.phys.stereo ? 2 : 1) && x.phys.first < o.first + (o.stereo ? 2 : 1));
    if (taken) throw new Error(`les sorties demandées sont déjà prises par « ${taken.name} »`);
    return { first: o.first, stereo: !!o.stereo, mono: !!o.mono || !o.stereo };
  }

  /** Les membres d'un groupe : des pistes ou des bus qui existent (pas la sortie principale, pas un retour). */
  #checkGroupNodes(ids, ret) {
    for (const id of ids) {
      const n = this.node(id);
      if (!n || n.kind === "master" || (n.kind === "bus" && n.is_return) || id === ret) throw new Error(`membre inexistant ou impossible : ${id}`);
    }
  }

  /** « Piste N » ou « Bus N » : le plus petit N libre. */
  #freeName(label) {
    for (let n = 1; ; n++) if (!this.project.nodes.some((x) => x.name === `${label} ${n}`)) return `${label} ${n}`;
  }

  /** Vrai si diriger `node` vers `target` ferait une boucle. */
  #loops(node, target) {
    for (let t = this.node(target); t; t = t.output === null ? null : this.node(t.output)) if (t.id === node.id) return true;
    return false;
  }

  #nextClipId() {
    return Math.max(...this.project.nodes.flatMap((n) => n.clips.map((c) => c.id))) + 1;
  }

  /** Pistes (nœuds) d'arrivée quand un groupe dont les clips sont sur `origins` descend de `delta` pistes (borné, ou refusé si `strict`). */
  #shiftedTracks(origins, delta, strict) {
    const tracks = this.project.nodes.filter((n) => n.kind === "mono" || n.kind === "stereo");
    const idx = origins.map((o) => tracks.indexOf(o));
    if (idx.some((i) => i < 0)) throw new Error("un clip n'est pas sur une piste");
    const lo = -Math.min(...idx);
    const hi = tracks.length - 1 - Math.max(...idx);
    if (strict && (delta < lo || delta > hi)) throw new Error("pas assez de pistes pour coller tous les clips à partir de cette piste");
    const d = Math.max(lo, Math.min(hi, delta));
    return idx.map((i) => tracks[i + d]);
  }

  /** Découpe en `msg.at` tous les clips désignés qui le contiennent (les autres sont ignorés). */
  #splitMany(msg) {
    const clips = msg.clips.map((id) => this.clip(id));
    if (clips.some((c) => !c)) {
      this.emit("error", { command: msg.type, message: `clip inconnu : ${msg.clips[clips.findIndex((c) => !c)]}` });
      return false;
    }
    const under = clips.filter((c) => c.start < msg.at && msg.at < c.start + c.len);
    if (!under.length) {
      this.emit("error", { command: msg.type, message: "aucun clip sélectionné ne contient cette position" });
      return false;
    }
    const made = [];
    const ok = this.#edit("Découper", msg, () => {
      for (const c of under) {
        const d = msg.at - c.start;
        const node = this.project.nodes.find((n) => n.clips.includes(c));
        const right = structuredClone({ ...c, id: this.#nextClipId(), start: msg.at, offset: c.offset + d, len: c.len - d, fade_in: 0 });
        this.#splitStretch(c, right, d);
        c.len = d;
        c.fade_out = 0;
        c.fade_in = Math.min(c.fade_in, c.len);
        right.fade_out = Math.min(right.fade_out, right.len);
        node.clips.splice(node.clips.indexOf(c) + 1, 0, right);
        made.push(c.id, right.id);
      }
    });
    if (ok) this.emit("selection", { clips: made });
    return ok;
  }

  /** Pose des copies de clips : le plus ancien démarre en `at`, les autres gardent leurs écarts. `forcedNode` : la plus haute des pistes
   *  copiées vient sur cette piste, les autres gardent leur écart ; `nodeDelta` : décalage en pistes (duplication). */
  #place(msg, label, snaps, at, forcedNode, nodeDelta) {
    if (!snaps.length) {
      this.emit("error", { command: msg.type, message: "le presse-papiers est vide" });
      return false;
    }
    const base = Math.min(...snaps.map((x) => x.clip.start));
    let targets;
    try {
      const origins = snaps.map((x) => this.node(x.node));
      if (forcedNode !== undefined && forcedNode !== null) {
        const tracks = this.project.nodes.filter((n) => n.kind === "mono" || n.kind === "stereo");
        const dest = tracks.findIndex((n) => n.id === forcedNode);
        if (dest < 0) throw new Error(`le nœud ${forcedNode} n'est pas une piste : un clip ne peut être posé que sur une piste`);
        const top = Math.min(...origins.map((o) => tracks.indexOf(o)));
        targets = this.#shiftedTracks(origins, dest - top, true);
      } else {
        targets = this.#shiftedTracks(origins, nodeDelta, false);
      }
    } catch (e) {
      this.emit("error", { command: msg.type, message: e.message });
      return false;
    }
    const created = [];
    const ok = this.#edit(label, msg, () => {
      const owners = new Set();
      snaps.forEach((x, k) => {
        const target = this.node(targets[k].id);
        const c = structuredClone(x.clip);
        c.id = this.#nextClipId();
        c.start = at + (x.clip.start - base);
        target.clips.push(c);
        created.push(c.id);
        owners.add(target);
      });
      for (const n of owners) this.#autoCrossfade(n);
    });
    if (ok) this.emit("selection", { clips: created });
    return ok;
  }

  #clipOrFail(id) {
    const c = this.clip(id);
    if (!c) throw new Error(`clip inconnu : ${id}`);
    return c;
  }

  /** Fondus enchaînés automatiques : les clips audio « a puis b » qui se chevauchent partagent la zone commune. */
  #autoCrossfade(node) {
    if (!this.autoCrossfade || !node) return;
    const clips = node.clips.filter((c) => c.kind === "audio").sort((a, b) => a.start - b.start || a.id - b.id);
    for (let i = 0; i + 1 < clips.length; i++) {
      const [a, b] = [clips[i], clips[i + 1]];
      const end = a.start + a.len;
      if (a.start < b.start && b.start < end && end <= b.start + b.len) {
        const overlap = end - b.start;
        if (a.fade_out !== overlap || b.fade_in !== overlap) {
          // La forme déjà choisie est conservée ; sans fondu préalable : puissance constante.
          if (a.fade_out === 0) a.fade_out_curve = { shape: "equal_power", bend: 0, smooth: 0 };
          if (b.fade_in === 0) b.fade_in_curve = { shape: "equal_power", bend: 0, smooth: 0 };
          a.fade_out = overlap;
          b.fade_in = overlap;
        }
      }
    }
  }

  /** Notes visibles d'un clip de notes : celles de la fenêtre, placées depuis son début, vélocité atténuée par les fondus. */
  #visibleNotes(c) {
    return c.mat
      .filter((n) => n.start >= c.offset && n.start < c.offset + c.len)
      .map((n) => {
        const rel = n.start - c.offset;
        let g = 1;
        if (c.fade_in > 0 && rel < c.fade_in) g *= rel / c.fade_in;
        if (c.fade_out > 0 && c.len - rel < c.fade_out) g *= (c.len - rel) / c.fade_out;
        return { start: rel, len: n.len, key: n.key, velocity: Math.max(1 / 127, n.velocity * g) };
      });
  }

  // ---- notes en temps musical (piano roll) : mêmes règles que l'hôte, tempo du premier palier seulement ------------------

  #spw() {
    const t = this.project.tempo_map[0];
    return (this.project.sample_rate * 240) / (t.num / t.den); // échantillons par ronde
  }
  #fracOf(samples) {
    const g = (a, b) => (b ? g(b, a % b) : Math.abs(a));
    const n = Math.round((samples / this.#spw()) * 3840);
    const k = g(n, 3840) || 1;
    return [n / k, 3840 / k];
  }
  #samplesOf(f) {
    return Math.round((f[0] / f[1]) * this.#spw());
  }
  #midiClip(id) {
    const c = this.clip(id);
    if (!c || c.kind !== "midi") throw new Error(`clip de notes inconnu : ${id}`);
    return c;
  }
  #notesEvent(c, selection = []) {
    c.mat.sort((a, b) => a.start - b.start || a.key - b.key);
    return {
      clip: c.id,
      start: this.#fracOf(c.start),
      len: this.#fracOf(c.len),
      offset: this.#fracOf(c.offset),
      notes: c.mat.map((n) => ({ start: this.#fracOf(n.start), len: this.#fracOf(n.len), key: n.key, velocity: n.velocity, channel: n.channel ?? 0, voice: n.voice ?? 1 })),
      selection: selection.map((n) => c.mat.indexOf(n)),
      events: (c.evts ?? []).map((e) => ({ start: this.#fracOf(e.start), channel: e.channel ?? 0, kind: e.kind, controller: e.controller ?? null, key: null, value: e.value })),
    };
  }
  /** Modification de notes : une action annulable, puis l'événement `notes` (avec les indices des notes touchées). */
  #notesEdit(msg, label, fn) {
    let result = [];
    let clip = null;
    const ok = this.#edit(label, msg, () => {
      clip = this.#midiClip(msg.clip);
      clip.mat.sort((a, b) => a.start - b.start || a.key - b.key);
      result = fn(clip) ?? [];
      const end = clip.offset + clip.len;
      for (const n of clip.mat) if (n.start < clip.offset || n.start >= end || n.len <= 0) throw new Error("note hors de la partie visible du clip");
    });
    if (ok) this.emit("notes", this.#notesEvent(clip, result));
    return ok;
  }

  /** Nœud émetteur et destination d'un envoi ; refuse la sortie principale comme émetteur et tout ce qui n'est pas bus / sortie. */
  #sendEnds(msg, checkTarget = true) {
    const n = this.node(msg.node);
    if (!n) throw new Error(`nœud inconnu : ${msg.node}`);
    if (n.kind === "master") throw new Error("la sortie principale n'a pas d'envoi");
    const to = this.node(msg.to);
    if (checkTarget && (!to || (to.kind !== "bus" && to.kind !== "master") || to.id === n.id)) throw new Error("un envoi va vers un bus ou la sortie principale (pas vers soi-même)");
    return [n, to];
  }

  /** Vrai si `from` mène à `target` en suivant sorties et envois (détection de boucle). */
  #reaches(from, target, seen = new Set()) {
    if (from === target) return true;
    if (seen.has(from)) return false;
    seen.add(from);
    const n = this.node(from);
    if (!n) return false;
    const next = [...(n.output === null || n.output === undefined ? [] : [n.output]), ...(n.sends ?? []).map((s) => s.to)];
    return next.some((t) => this.#reaches(t, target, seen));
  }

  /** Applique une édition de clips : tout ou rien, une entrée d'historique, puis `project` et `history`. */
  #edit(label, msg, fn) {
    const before = this.#snapshot();
    try {
      fn();
    } catch (e) {
      this.#restore(before);
      this.emit("error", { command: msg.type, message: e.message });
      return false;
    }
    this.undoStack.push({ label, before, after: this.#snapshot() });
    this.redoStack = [];
    this.#markDirty();
    this.#emitProject();
    this.#emitHistory();
    this.#refreshPreview();
    return true;
  }

  /** Zone du fondu écouté (échantillons absolus) et piste qui le porte, ou `null` s'il n'existe plus ou n'a pas de longueur. */
  #previewZone(t) {
    let start;
    let end;
    let owner;
    if (t.kind === "crossfade") {
      const a = this.clip(t.out);
      const b = this.clip(t.in);
      if (a?.kind !== "audio" || b?.kind !== "audio") return null;
      [start, end, owner] = [b.start, a.start + a.len, this.project.nodes.find((n) => n.clips.includes(a))];
    } else {
      const c = this.clip(t.clip);
      if (c?.kind !== "audio") return null;
      [start, end] = t.side === "in" ? [c.start, c.start + c.fade_in] : [c.start + c.len - c.fade_out, c.start + c.len];
      owner = this.project.nodes.find((n) => n.clips.includes(c));
    }
    return owner && start < end ? { start, end, owner: owner.id } : null;
  }

  /** (Ré)applique l'aperçu : solos et boucle sur la zone plus la marge. Renvoie la boucle, ou `null` si le fondu a disparu. */
  #applyPreview() {
    const p = this.preview;
    const z = this.#previewZone(p.target);
    if (!z) return null;
    for (const n of this.project.nodes) if (n.kind !== "master") n.solo = p.solo ? n.id === z.owner : p.savedSolos.get(n.id);
    const margin = Math.round((p.margin_ms * SR) / 1000);
    this.loop = { start: Math.max(0, z.start - margin), end: z.end + margin };
    return this.loop;
  }

  #startPreview(msg) {
    const target = msg.clip_out !== undefined ? { kind: "crossfade", out: msg.clip_out, in: msg.clip_in } : { kind: "single", clip: msg.clip, side: msg.side };
    const ids = target.kind === "crossfade" ? [target.out, target.in] : [target.clip];
    if (ids.some((id) => this.clip(id)?.kind !== "audio")) {
      this.emit("error", { command: "preview_fade", message: target.kind === "crossfade" ? "preview_fade : deux clips audio existants" : "preview_fade : clip audio inexistant" });
      return false;
    }
    const keep = this.preview;
    this.preview = {
      target,
      margin_ms: msg.margin_ms,
      solo: msg.solo,
      savedPos: keep?.savedPos ?? this.pos,
      savedPlaying: keep?.savedPlaying ?? this.playing,
      savedLoop: keep ? keep.savedLoop : this.loop,
      savedSolos: keep?.savedSolos ?? new Map(this.project.nodes.map((n) => [n.id, n.solo])),
    };
    const loop = this.#applyPreview();
    if (!loop) {
      this.#endPreview(true, true);
      this.emit("error", { command: "preview_fade", message: "pas de fondu à écouter (longueur nulle ou clips qui ne se chevauchent pas)" });
      return false;
    }
    this.pos = loop.start;
    this.playing = true;
    this.emit("preview", { active: true });
    this.#emitProject();
    this.#emitPosition();
    return true;
  }

  /** Termine l'aperçu s'il y en a un : rend les solos, puis selon les cas la boucle et la position avec l'état de lecture. */
  #endPreview(restoreLoop, restorePos) {
    const p = this.preview;
    if (!p) return false;
    this.preview = null;
    for (const n of this.project.nodes) if (n.kind !== "master") n.solo = p.savedSolos.get(n.id) ?? n.solo;
    if (restoreLoop) this.loop = p.savedLoop;
    if (restorePos) {
      this.pos = p.savedPos;
      this.playing = p.savedPlaying;
    }
    this.emit("preview", { active: false });
    this.#emitProject();
    this.#emitPosition();
    return true;
  }

  /** Après une édition : l'aperçu suit le fondu modifié ; s'il a disparu, il se termine. */
  #refreshPreview() {
    if (!this.preview) return;
    const loop = this.#applyPreview();
    if (!loop) return void this.#endPreview(true, true);
    if (this.pos < loop.start || this.pos >= loop.end) this.pos = loop.start;
    this.#emitProject();
    this.#emitPosition();
  }

  #emitHistory() {
    this.emit("history", {
      can_undo: this.undoStack.length > 0,
      can_redo: this.redoStack.length > 0,
      undo_label: this.undoStack.at(-1)?.label ?? null,
      redo_label: this.redoStack.at(-1)?.label ?? null,
    });
  }

  /** Démarre l'horloge de l'interface (30 Hz). Les tests appellent `advance()` à la main. */
  start() {
    if (this.timer !== null) return;
    let last = performance.now();
    this.timer = setInterval(() => {
      const now = performance.now();
      this.advance(now - last); // temps réellement écoulé : reste juste même si le navigateur ralentit l'onglet
      last = now;
    }, 1000 / 30);
  }

  stop() {
    clearInterval(this.timer);
    this.timer = null;
  }

  node(id) {
    return this.project.nodes.find((n) => n.id === id);
  }

  clip(id) {
    for (const n of this.project.nodes) {
      const c = n.clips.find((c) => c.id === id);
      if (c) return c;
    }
    return undefined;
  }

  #wave(clip) {
    if (!this.waves.has(clip.id)) this.waves.set(clip.id, waveformFor(clip.wave, clip.id));
    return this.waves.get(clip.id);
  }

  /** Les réponses sont synchrones ici ; le vrai moteur répond de façon asynchrone — l'interface ne doit pas en dépendre. */
  /** Refuse une commande sans sens pendant un enregistrement (comme le vrai moteur). */
  #refusedWhileRecording(msg) {
    if (!this.recording) return false;
    const refused = ["seek", "loop", "add_track", "delete_track", "new_project", "open_project", "undo", "redo", "set_track_input", "set_punch", "set_loop_record"].includes(msg.type) || (msg.type === "set_node" && ("arm" in msg || "monitor" in msg));
    if (refused) this.emit("error", { command: msg.type, message: "arrête d'abord l'enregistrement" });
    return refused;
  }

  /** Les prises deviennent des clips (une seule action annulable) puis l'enregistrement est déclaré terminé. */
  #finishRecording() {
    const rec = this.recording;
    this.recording = null;
    if (rec.loop) this.loop = rec.loop; // la zone est de nouveau bouclée
    this.emit("recording", { active: true, stopping: true, overruns: 0, warnings: [], start: this.recording?.start ?? rec.start, nodes: rec.nodes });
    const len = Math.min(this.pos, rec.end ?? Infinity) - rec.start;
    if (len > 0) {
      this.#edit("Enregistrer", { type: "record" }, () => {
        for (const id of rec.nodes) {
          const node = this.node(id);
          const n = 1 + node.clips.filter((c) => c.name.startsWith(`${node.name} prise `)).length;
          const name = `${node.name} prise ${n}`;
          const base = { id: this.#nextClipId(), name, start: rec.start, len, offset: 0, fade_in: 0, fade_out: 0 };
          if (node.input.kind === "midi") {
            const mat = Array.from({ length: 4 }, (_, k) => ({ start: Math.floor((len * k) / 4), len: Math.floor(len / 5), key: 60 + 2 * k, velocity: 0.7 }));
            node.clips.push({ ...base, kind: "midi", source_len: 1_000_000_000_000, mat });
          } else {
            const flat = { shape: "linear", bend: 0, smooth: 0 };
            node.clips.push({ ...base, kind: "audio", source_len: len, gain: 1, fade_in_curve: flat, fade_out_curve: flat, wave: "voice" });
          }
        }
      });
    }
    this.emit("recording", { active: false, stopping: false, overruns: 0, warnings: [] });
  }

  send(msg) {
    const err = validateCommand(msg);
    if (err) {
      this.emit("error", { command: msg?.type ?? "?", message: err });
      return false;
    }
    if (this.#refusedWhileRecording(msg)) return false;
    switch (msg.type) {
      case "set_tempo": {
        const g = (a, b) => (b ? g(b, a % b) : a);
        const m = this.project.meters[0];
        const pos = (bar) => {
          const n = bar * m.num;
          const d = m.den;
          const k = g(n, d) || 1;
          return [n / k, d / k];
        };
        const num = Math.round(msg.bpm * 1000);
        const k = g(num, 1000);
        const entry = { pos: msg.at_bar === 1 ? [0, 1] : pos(msg.at_bar - 1), num: num / k, den: 1000 / k };
        const map = this.project.tempo_map.filter((t) => !(t.pos[0] * entry.pos[1] === entry.pos[0] * t.pos[1]));
        map.push(entry);
        map.sort((a, b) => a.pos[0] * b.pos[1] - b.pos[0] * a.pos[1]);
        this.project.tempo_map = map;
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "remove_tempo": {
        const same = (t) => t.pos[0] === msg.pos[0] && t.pos[1] === msg.pos[1];
        if (msg.pos[0] === 0 || !this.project.tempo_map.some(same)) {
          this.emit("error", { command: "remove_tempo", message: "aucun changement de tempo à cette position" });
          return false;
        }
        this.project.tempo_map = this.project.tempo_map.filter((t) => !same(t));
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "remove_meter": {
        const bar = msg.at_bar - 1;
        if (!this.project.meters.some((x) => x.bar === bar)) {
          this.emit("error", { command: "remove_meter", message: "aucun changement de chiffrage à cette mesure" });
          return false;
        }
        this.project.meters = this.project.meters.filter((x) => x.bar !== bar);
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "set_meter": {
        const bar = msg.at_bar - 1;
        const list = this.project.meters.filter((x) => x.bar !== bar);
        list.push({ bar, num: msg.num, den: msg.den });
        list.sort((a, b) => a.bar - b.bar);
        this.project.meters = list;
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "set_track_input": {
        const n = this.node(msg.node);
        if (!n || n.kind === "master" || n.kind === "bus") {
          this.emit("error", { command: "set_track_input", message: `${msg.node} n'est pas une piste` });
          return false;
        }
        const bad = msg.input.kind === "audio" ? msg.input.channels.find((c) => c > this.project.audio_inputs) : undefined;
        if (bad !== undefined) {
          this.emit("error", { command: "set_track_input", message: `entrée ${bad} inexistante : le pilote en a ouvert ${this.project.audio_inputs}` });
          return false;
        }
        n.input = { kind: msg.input.kind, channels: msg.input.kind === "midi" ? [] : [...msg.input.channels] };
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "set_track_dynamics": {
        const n = this.node(msg.node);
        if (!n || n.kind === "master" || n.kind === "bus") {
          this.emit("error", { command: msg.type, message: `${msg.node} n'est pas une piste` });
          return false;
        }
        n.dynamics = { record: false, ...msg.dynamics };
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "set_track_velocity_curve": {
        const n = this.node(msg.node);
        if (!n || n.kind === "master" || n.kind === "bus") {
          this.emit("error", { command: msg.type, message: `${msg.node} n'est pas une piste` });
          return false;
        }
        n.velocity_curve = { bend: 0, smooth: 0, fixed: 0, ...msg.curve };
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "set_track_midi_filter": {
        const n = this.node(msg.node);
        if (!n || n.kind === "master" || n.kind === "bus") {
          this.emit("error", { command: msg.type, message: `${msg.node} n'est pas une piste` });
          return false;
        }
        n.midi_filter = msg.rules.map((r) => ({ device: r.device ?? "", channels: [...(r.channels ?? [])] }));
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "get_notes": {
        try {
          this.emit("notes", this.#notesEvent(this.#midiClip(msg.clip)));
        } catch (e) {
          this.emit("error", { command: "get_notes", message: e.message });
          return false;
        }
        break;
      }
      case "set_events":
        return this.#notesEdit(msg, msg.label ?? "Modifier une courbe de contrôle", (c) => {
          const end = c.offset + c.len;
          const added = msg.events.map((e) => ({ start: this.#samplesOf(e.start), channel: e.channel ?? 0, kind: msg.kind, controller: msg.kind === "cc" ? msg.controller : undefined, value: e.value }));
          if (added.some((e) => e.start < c.offset || e.start >= end)) throw new Error("point hors de la partie visible du clip");
          const lane = (e) => e.kind === msg.kind && (msg.kind !== "cc" || e.controller === msg.controller);
          c.evts = [...(c.evts ?? []).filter((e) => !(lane(e) && e.start >= c.offset && e.start < end)), ...added].sort((a, b) => a.start - b.start);
          return [];
        });
      case "add_notes":
        return this.#notesEdit(msg, "Ajouter des notes", (c) => {
          const added = msg.notes.map((n) => ({ start: this.#samplesOf(n.start), len: this.#samplesOf(n.len), key: n.key, velocity: n.velocity, voice: n.voice ?? 1 }));
          c.mat.push(...added);
          return added;
        });
      case "edit_notes":
        return this.#notesEdit(msg, msg.label ?? "Modifier des notes", (c) => {
          const seen = new Set();
          return msg.edits.map((e) => {
            const n = c.mat[e.index];
            if (!n || seen.has(e.index)) throw new Error(`note inconnue : ${e.index}`);
            seen.add(e.index);
            if (e.start) n.start = this.#samplesOf(e.start);
            if (e.len) n.len = this.#samplesOf(e.len);
            if (e.key !== undefined) n.key = e.key;
            if (e.velocity !== undefined) n.velocity = e.velocity;
            if (e.channel !== undefined) n.channel = e.channel;
            if (e.voice !== undefined) n.voice = e.voice;
            return n;
          });
        });
      case "delete_notes":
        return this.#notesEdit(msg, "Supprimer des notes", (c) => {
          const idx = [...new Set(msg.indices)].sort((a, b) => b - a);
          if (idx.some((i) => !c.mat[i])) throw new Error("note inconnue");
          for (const i of idx) c.mat.splice(i, 1);
        });
      case "quantize_notes":
        return this.#notesEdit(msg, "Quantifier", (c) => {
          const step = (msg.grid[0] / msg.grid[1]) * this.#spw();
          for (const n of c.mat) {
            const snapped = Math.round((c.start - c.offset + n.start) / step) * step - c.start + c.offset;
            if (snapped >= c.offset && snapped < c.offset + c.len) n.start = Math.round(snapped);
          }
        });
      case "preview_note":
        break;
      case "set_metronome": {
        const { type, ...settings } = msg;
        this.metronome = settings;
        this.prefs.metronome = { ...settings };
        this.emit("metronome", { ...this.metronome });
        break;
      }
      case "get_prefs":
        this.emit("prefs", this.#prefsEvent());
        break;
      case "set_prefs": {
        this.prefs = { ...msg.prefs };
        this.metronome = { ...msg.prefs.metronome };
        this.emit("prefs", this.#prefsEvent());
        this.emit("metronome", { ...this.metronome });
        break;
      }
      case "record": {
        const fail = (message) => (this.emit("error", { command: "record", message }), false);
        if (this.recording) return fail("un enregistrement est déjà en cours");
        if (!this.file.path) return fail("pas encore de dossier pour les prises : utilise « Enregistrer sous » d'abord");
        if (this.punchOn && !this.loop) return fail("punch allumé : pose d'abord une zone (bande bleue de la règle) ou éteins le punch");
        if (this.loop && !this.punchOn && !this.loopRecOn) return fail("désactive la boucle avant d'enregistrer (ou allume Punch pour enregistrer dans la zone)");
        const armed = this.project.nodes.filter((n) => n.arm && n.kind !== "master" && n.kind !== "bus");
        if (!armed.length) return fail("aucune piste armée : arme une piste (●) pour enregistrer");
        this.#endPreview(true, false);
        const z = this.punchOn ? this.loop : null;
        if (z && this.playing && this.pos >= z.end) return fail("la tête de lecture est après la fin de la zone de punch : reviens avant ou déplace la zone");
        // Avec une zone : la prise commence et finit exactement à ses bords ; à l'arrêt la lecture part du début de la zone.
        const from = z ? (this.playing && this.pos >= z.start ? this.pos : z.start) : this.pos;
        if (z && !this.playing) this.pos = z.start;
        this.recording = { start: from, end: z ? z.end : null, nodes: armed.map((n) => n.id), loop: z ? this.loop : null };
        if (z) this.loop = null; // la zone n'est plus bouclée pendant la prise
        this.playing = true;
        this.emit("recording", { active: true, stopping: false, overruns: 0, warnings: [], start: this.recording?.start ?? 0, nodes: this.recording?.nodes ?? [] });
        this.#emitPosition();
        break;
      }
      case "stop_record":
        if (this.recording) {
          this.playing = false;
          this.#finishRecording();
          this.#emitPosition();
        } else {
          this.emit("recording", { active: false, stopping: false, overruns: 0, warnings: [] });
        }
        break;
      case "set_plugin_view":
        break; // la zone de l'interface native n'existe que dans l'application
      case "open_plugin_ui":
      case "close_plugin_ui": {
        const ins = this.project.nodes.flatMap((n) => n.inserts).find((i) => i.id === msg.insert);
        const open = msg.type === "open_plugin_ui";
        if (!ins) {
          if (!open) break;
          this.emit("error", { command: msg.type, message: `plugin inconnu : ${msg.insert}` });
          return false;
        }
        if (open && !ins.gui) {
          this.emit("error", { command: msg.type, message: `« ${ins.name} » n'a pas d'interface pour cette plateforme` });
          return false;
        }
        if (open === this.openUis.has(ins.id)) {
          if (open) this.emit("plugin_ui", { insert: ins.id, open: true });
          break;
        }
        this.openUis[open ? "add" : "delete"](ins.id);
        this.emit("plugin_ui", { insert: ins.id, open });
        this.#emitProject();
        break;
      }
      case "probe_audio":
        this.emit("audio_info", { files: msg.files.map((f) => this.#fakeProbe(f)) });
        break;
      case "import_audio":
        return this.#startImport(msg);
      case "cancel_import":
        if (this.importJob) this.importJob.cancelled = true;
        break;
      case "align_clips":
      case "align_redo":
        return this.#startAlign(msg);
      case "cancel_align":
        if (this.alignJob) this.alignJob.cancelled = true;
        break;
      case "set_stretch":
        return this.#startStretch(msg);
      case "detect_onsets": {
        const c = this.clip(msg.clip);
        if (!c || c.kind !== "audio") return (this.emit("error", { command: msg.type, message: `clip audio inconnu : ${msg.clip}` }), false);
        // Aperçu : une « attaque » toutes les demi-secondes (le vrai hôte analyse l'audio).
        const step = Math.round(this.project.sample_rate * (msg.sensitivity > 0.7 ? 0.35 : 0.5));
        const positions = [];
        for (let p = step; p < c.len - 1000; p += step) positions.push(p);
        this.emit("onsets", { clip: c.id, positions });
        break;
      }
      case "cancel_stretch":
        if (this.stretchJob) this.stretchJob.cancelled = true;
        break;
      case "listen_align": {
        const g = this.clip(msg.guide);
        if (!g) return (this.emit("error", { command: msg.type, message: `guide inconnu : ${msg.guide}` }), false);
        if (!this.node(msg.node)) return (this.emit("error", { command: msg.type, message: `piste inconnue : ${msg.node}` }), false);
        if (msg.what.endsWith("aligned") && !this.alignRecords?.has(msg.aligned)) return (this.emit("error", { command: msg.type, message: "pas de prise calée à écouter" }), false);
        this.listening ??= { savedPos: this.pos, savedPlaying: this.playing, savedLoop: this.loop };
        const gl = [g, ...(msg.guide_more ?? []).map((id) => this.clip(id)).filter(Boolean)];
        const ws = Math.min(...gl.map((c) => c.start));
        const we = Math.max(...gl.map((c) => c.start + c.len));
        this.loop = { start: ws, end: we };
        if (!this.playing || this.pos < ws || this.pos >= we) this.pos = ws;
        this.playing = true;
        this.emit("listen", { active: true, what: msg.what });
        this.#emitPosition();
        break;
      }
      case "stop_listen":
        if (this.listening) {
          const l = this.listening;
          this.listening = null;
          this.loop = l.savedLoop;
          this.pos = l.savedPos;
          this.playing = l.savedPlaying;
          this.emit("listen", { active: false, what: "" });
          this.#emitPosition();
        }
        break;
      case "get_align":
        return this.#alignDetail(msg);
      case "merge_align_settings":
        try {
          JSON.parse(msg.dst);
          JSON.parse(msg.src);
        } catch {
          this.emit("error", { command: msg.type, message: "réglages : JSON invalide" });
          return false;
        }
        this.emit("align_settings", { settings: stringifySettings(mergeGeneral(parseSettings(msg.dst), parseSettings(msg.src))) });
        break;
      case "export":
        return this.#startExport(msg);
      case "cancel_export":
        if (this.exportJob) this.exportJob.cancelled = true;
        break;
      case "set_return_output": {
        const n = this.node(msg.node);
        if (!n) {
          this.emit("error", { command: msg.type, message: `nœud inconnu : ${msg.node}` });
          return false;
        }
        return this.#edit(`${msg.output ? "Sortie" : "Retirer la sortie"} d'un ${n.is_return ? "retour" : "bus"}`, msg, () => {
          if (!msg.output) {
            if (n.phys && n.is_return) this.#clearOffsets(n.id); // un retour qui perd sa sortie rend les décalages de niveau de ses groupes
            return void delete n.phys;
          }
          n.phys = this.#checkPhys(n, msg.output);
        });
      }
      case "get_chords":
        this.#emitChords();
        break;
      case "get_chord_catalog":
        this.emit("chord_catalog", MOCK_CHORD_CATALOG);
        break;
      case "set_chords":
        return this.#edit(msg.label ?? "Modifier les accords", msg, () => {
          const t = this.#checkChordTrack(msg.track);
          this.chordTrack = t;
        }) && (this.#emitChords(), true);
      case "start_monitor": {
        this.monitorOn = true;
        this.#emitMonitor();
        return true;
      }
      case "renew_monitor_address": {
        const n = this.node(msg.node);
        if (!n?.is_return) {
          this.emit("error", { command: msg.type, message: "seul un retour de monitoring a une adresse" });
          return false;
        }
        n.monitor_token = "";
        this.#markDirty();
        this.#emitMonitor();
        return true;
      }
      case "set_monitor_role": {
        const n = this.node(msg.node);
        if (!n?.is_return) {
          this.emit("error", { command: msg.type, message: "seul un retour de monitoring a un rôle de télécommande" });
          return false;
        }
        n.monitor_role = msg.role;
        this.#markDirty();
        this.#emitMonitor();
        return true;
      }
      case "get_monitor_log":
        this.emit("monitor_log", { entries: [] }); // le faux moteur n'a pas de serveur : journal vide
        return true;
      case "stop_monitor":
        this.monitorOn = false;
        this.#emitMonitor();
        return true;
      case "set_structure":
        return this.#edit(msg.label ?? "Modifier la structure", msg, () => {
          const list = structuredClone(msg.sections).sort((a, b) => fcmp(a.at, b.at));
          for (const s of list) {
            if (!String(s.name ?? "").trim() || s.name.length > 60) throw new Error("nom de section invalide (1 à 60 caractères)");
            if (s.category != null && !CATEGORIES.includes(s.category)) throw new Error(`catégorie « ${s.category} » inconnue`);
            if (s.color != null && !/^#[0-9a-fA-F]{6}$/.test(s.color)) throw new Error(`couleur « ${s.color} » invalide (attendu #rrggbb)`);
            if (!(s.dur[0] > 0) || !(s.at[1] > 0) || !(s.dur[1] > 0)) throw new Error("une section a une durée nulle");
          }
          for (let i = 1; i < list.length; i++) if (fcmp(fadd(list[i - 1].at, list[i - 1].dur), list[i].at) > 0) throw new Error("deux sections se chevauchent");
          this.structure = list;
        }) && (this.#emitChords(), true);
      case "set_lyrics":
        return this.#edit(msg.label ?? "Modifier les paroles", msg, () => {
          const list = structuredClone(msg.syllables).map((s) => ({ at: s.at, text: s.text, hyphen: !!s.hyphen, verse: s.verse ?? 1, ...(s.end ? { end: s.end } : {}) })).sort((a, b) => fcmp(a.at, b.at) || a.verse - b.verse);
          for (const [i, s] of list.entries()) {
            if (!(s.at[1] > 0)) throw new Error("set_lyrics : fraction à dénominateur nul");
            if (!s.text || s.text.length > 40 || /\s/.test(s.text)) throw new Error(`syllabe invalide « ${s.text} » (1 à 40 caractères, sans espace)`);
            if (s.verse < 1 || s.verse > 8) throw new Error(`couplet ${s.verse} invalide (1 à 8)`);
            if (s.end && fcmp(s.end, s.at) <= 0) throw new Error("la fin d'un trait de prolongation doit suivre le début de la syllabe");
            const n = list[i + 1];
            if (n && n.verse === s.verse && fcmp(n.at, s.at) === 0) throw new Error("deux syllabes au même instant dans le même couplet");
          }
          this.lyrics = list;
        }) && (this.#emitChords(), true);
      case "import_chords":
      case "export_chords":
        this.emit("error", { command: msg.type, message: "l'aperçu sans l'application ne lit ni n'écrit de fichier de carte" });
        return false;
      case "get_project":
        this.#emitControlPages();
        this.emit("metronome", { ...this.metronome });
        this.#emitChords();
        this.emit("chord_catalog", MOCK_CHORD_CATALOG);
        this.#emitPunch();
        this.emit("prefs", this.#prefsEvent());
        this.#emitProject();
        this.#emitHistory();
        this.#emitFile();
        break;
      case "set_auto_crossfade":
        this.autoCrossfade = msg.enabled;
        this.#markDirty();
        this.#emitProject();
        break;
      case "list_plugins": {
        const listing = MOCK_PLUGINS.map(({ params, latency, ...l }) => l);
        if (this.catalogSent) {
          this.emit("plugins", { plugins: listing, scanning: false });
        } else {
          this.catalogSent = true;
          this.emit("plugins", { plugins: [], scanning: true });
          setTimeout(() => this.emit("plugins", { plugins: listing, scanning: false }), 150);
        }
        break;
      }
      case "add_plugin":
        return this.#edit("Ajouter un plugin", msg, () => {
          const node = this.node(msg.node);
          if (!node) throw new Error(`nœud inconnu : ${msg.node}`);
          const def = MOCK_PLUGINS.find((p) => p.clap_id === msg.clap_id);
          if (!def) throw new Error(`plugin introuvable : ${msg.clap_id}`);
          const id = this.nextInsert++;
          this.plugins.set(id, { def, values: def.params.map((p) => p[4]) });
          node.inserts.push({ id, name: def.name, vendor: def.vendor, bypass: false, has_sidechain: !!def.sidechain, sidechain: null, aux_ports: (def.aux ?? []).map((name) => ({ name, to: null })), latency: def.latency, gui: def.gui !== false, ui_open: false });
        });
      case "remove_plugin":
        return this.#edit("Retirer un plugin", msg, () => {
          const node = this.project.nodes.find((n) => n.inserts.some((i) => i.id === msg.insert));
          if (!node) throw new Error(`plugin inconnu : ${msg.insert}`);
          node.inserts = node.inserts.filter((i) => i.id !== msg.insert);
        });
      case "move_plugin":
        return this.#edit("Déplacer un plugin", msg, () => {
          const node = this.project.nodes.find((n) => n.inserts.some((i) => i.id === msg.insert));
          if (!node) throw new Error(`plugin inconnu : ${msg.insert}`);
          const ins = node.inserts.find((i) => i.id === msg.insert);
          const rest = node.inserts.filter((i) => i !== ins);
          let at = rest.length;
          if (msg.before !== undefined) {
            at = rest.findIndex((i) => i.id === msg.before);
            if (at < 0) throw new Error(`plugin inconnu : ${msg.before}`);
          }
          rest.splice(at, 0, ins);
          node.inserts = rest;
        });
      case "set_plugin_bypass": {
        const ins = this.project.nodes.flatMap((n) => n.inserts).find((i) => i.id === msg.insert);
        if (!ins) {
          this.emit("error", { command: "set_plugin_bypass", message: `plugin inconnu : ${msg.insert}` });
          return false;
        }
        ins.bypass = msg.bypass;
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "set_plugin_aux": {
        const owner = this.project.nodes.find((n) => n.inserts.some((i) => i.id === msg.insert));
        const ins = owner?.inserts.find((i) => i.id === msg.insert);
        const fail = (message) => (this.emit("error", { command: "set_plugin_aux", message }), false);
        if (!ins) return fail(`plugin inconnu : ${msg.insert}`);
        if (msg.port >= ins.aux_ports.length) return fail(`ce plugin n'a pas de port de sortie auxiliaire n° ${msg.port}`);
        if (msg.to !== null) {
          const dest = this.node(msg.to);
          if (!dest) return fail(`nœud inconnu : ${msg.to}`);
          if (dest.id === owner.id || !(dest.kind === "bus" || dest.kind === "master")) return fail("sortie auxiliaire refusée : il faut un bus ou la sortie principale");
          if (dest.kind === "bus" && this.#reaches(dest.id, owner.id)) return fail("sortie auxiliaire refusée : boucle");
        }
        ins.aux_ports[msg.port].to = msg.to;
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "set_insert_sidechain": {
        const owner = this.project.nodes.find((n) => n.inserts.some((i) => i.id === msg.insert));
        const ins = owner?.inserts.find((i) => i.id === msg.insert);
        const src = msg.source === null ? null : this.node(msg.source);
        const fail = (message) => (this.emit("error", { command: "set_insert_sidechain", message }), false);
        if (!ins) return fail(`plugin inconnu : ${msg.insert}`);
        if (msg.source !== null) {
          if (!src) return fail(`nœud inconnu : ${msg.source}`);
          if (src.kind === "master" || src.id === owner.id) return fail("chaîne latérale refusée : la sortie principale ou le nœud lui-même");
          if (this.#reaches(owner.id, src.id)) return fail("chaîne latérale refusée : boucle");
        }
        ins.sidechain = msg.source;
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "get_control_pages":
        this.#emitControlPages();
        break;
      case "set_control_page": {
        this.bank ??= new Map();
        if (msg.params.length) this.bank.set(msg.plugin_id, { plugin_id: msg.plugin_id, name: msg.name, params: msg.params.map((p) => ({ id: p.id, name: p.name })) });
        else this.bank.delete(msg.plugin_id);
        this.#emitControlPages();
        break;
      }
      case "get_insert_page": {
        const p = this.plugins.get(msg.insert);
        if (!p) {
          this.emit("error", { command: "get_insert_page", message: `plugin inconnu : ${msg.insert}` });
          return false;
        }
        const plugin_id = p.def.clap_id;
        const entry = this.bank?.get(plugin_id);
        const exposed = p.def.params.map(([name, module, min, max, def], id) => ({ id, name, module, min, max, default: def, value: p.values[id], text: p.values[id].toFixed(2) }));
        const params = [];
        const missing = [];
        for (const w of entry?.params ?? []) {
          const f = exposed.find((x) => x.id === w.id && x.name.toLowerCase() === w.name.toLowerCase()) ?? exposed.find((x) => x.name.toLowerCase() === w.name.toLowerCase()) ?? exposed.find((x) => x.id === w.id);
          if (f && !params.includes(f)) params.push(f);
          else missing.push(w.name);
        }
        this.emit("insert_page", { insert: msg.insert, plugin_id, configured: !!entry, params, missing });
        break;
      }
      case "get_plugin_params": {
        const p = this.plugins.get(msg.insert);
        if (!p || !this.project.nodes.some((n) => n.inserts.some((i) => i.id === msg.insert))) {
          this.emit("error", { command: "get_plugin_params", message: `plugin inconnu : ${msg.insert}` });
          return false;
        }
        this.emit("plugin_params", { insert: msg.insert, params: p.def.params.map(([name, module, min, max, def], id) => ({ id, name, module, min, max, default: def, value: p.values[id], text: p.values[id].toFixed(2) })) });
        break;
      }
      case "set_plugin_param": {
        const p = this.plugins.get(msg.insert);
        if (!p || !p.def.params[msg.param]) {
          this.emit("error", { command: "set_plugin_param", message: `plugin inconnu : ${msg.insert}` });
          return false;
        }
        const [, , min, max] = p.def.params[msg.param];
        p.values[msg.param] = Math.min(max, Math.max(min, msg.value));
        this.#markDirty();
        this.emit("plugin_param", { insert: msg.insert, param: msg.param, value: p.values[msg.param], text: p.values[msg.param].toFixed(2) });
        break;
      }
      case "add_track": {
        this.listenNode = null; // les numéros de nœuds changent : l'écoute est défaite, comme dans l'hôte
        this.#endPreview(true, false);
        return this.#edit(msg.kind === "return" ? "Ajouter un retour" : msg.kind === "bus" ? "Ajouter un bus" : "Ajouter une piste", msg, () => {
          const id = Math.max(...this.project.nodes.map((n) => n.id)) + 1;
          const name = msg.name?.trim() || this.#freeName(msg.kind === "bus" ? "Bus" : msg.kind === "return" ? "Retour" : "Piste");
          const color = TRACK_COLORS[(this.project.nodes.length - 1) % TRACK_COLORS.length];
          const isReturn = msg.kind === "return";
          const node = { id, name, kind: isReturn ? "bus" : msg.kind, is_return: isReturn, color, gain: 1, pan: 0, mute: false, solo: false, invert: false, arm: false, output: 0, input: { kind: "audio", channels: [1] }, monitor: true, inserts: [], clips: [] };
          if (msg.output) node.phys = this.#checkPhys(node, msg.output); // refus avant d'ajouter quoi que ce soit
          this.project.nodes.push(node);
        });
      }
      case "delete_track": {
        this.listenNode = null;
        this.#endPreview(true, false);
        return this.#edit("Supprimer une piste", msg, () => {
          const n = this.node(msg.node);
          if (!n) throw new Error(`nœud inconnu : ${msg.node}`);
          this.project.nodes = this.project.nodes.filter((x) => x !== n);
          for (const o of this.project.nodes) {
            if (o.output === n.id) o.output = 0;
            if (o.sends) o.sends = o.sends.filter((s) => s.to !== n.id);
            for (const i of o.inserts ?? []) {
              if (i.sidechain === n.id) i.sidechain = null; // sa chaîne latérale disparaît avec la source
              for (const a of i.aux_ports ?? []) if (a.to === n.id) a.to = null;
            }
            if (o.automation) o.automation = o.automation.filter((l) => !(l.target.kind === "send" && l.target.to === n.id));
          }
        });
      }
      case "rename_track":
        return this.#edit("Renommer une piste", msg, () => {
          const n = this.node(msg.node);
          if (!n) throw new Error(`nœud inconnu : ${msg.node}`);
          n.name = msg.name.trim();
        });
      case "set_track_color":
        return this.#edit("Couleur d'une piste", msg, () => {
          const n = this.node(msg.node);
          if (!n) throw new Error(`nœud inconnu : ${msg.node}`);
          n.color = msg.color.toLowerCase();
        });
      case "set_write_mode": {
        const arms = (this.project.write_arms ??= []);
        const same = (a) => a.node === msg.node && JSON.stringify(a.target) === JSON.stringify(msg.target);
        this.project.write_arms = arms.filter((a) => !same(a));
        if (msg.mode !== "none") {
          this.project.write_arms.push({ node: msg.node, target: msg.target, mode: msg.mode });
          const n = this.node(msg.node);
          const lane = n?.automation?.find((l) => JSON.stringify(l.target) === JSON.stringify(msg.target));
          if (lane) lane.mode = "read";
        }
        this.#emitProject();
        break;
      }
      case "gesture":
        break;
      case "get_plugin_param_text":
        this.emit("plugin_param_text", { insert: msg.insert, param: msg.param, value: msg.value, text: String(Math.round(msg.value * 100) / 100) });
        break;
      case "set_lane": {
        const n = this.node(msg.node);
        if (!n) {
          this.emit("error", { command: "set_lane", message: `nœud inconnu : ${msg.node}` });
          return false;
        }
        const t = msg.target;
        const same = (l) => l.target.kind === t.kind && l.target.to === t.to && l.target.insert === t.insert && l.target.param === t.param;
        const exists = t.kind === "send" ? (n.sends ?? []).some((s) => s.to === t.to) : t.kind === "insert" ? n.inserts.some((i) => i.id === t.insert) : true;
        if (!exists) {
          this.emit("error", { command: "set_lane", message: "cette cible n'existe pas sur ce nœud" });
          return false;
        }
        return this.#edit(msg.points.length ? (msg.label ?? "Modifier l'automation") : "Supprimer l'automation", msg, () => {
          n.automation ??= [];
          const old = n.automation.find(same);
          n.automation = n.automation.filter((l) => !same(l));
          if (msg.points.length) n.automation.push({ target: { ...t }, mode: msg.mode ?? old?.mode ?? "trim", points: msg.points.map((p) => ({ ...p })) });
        });
      }
      case "set_lane_mode": {
        const n = this.node(msg.node);
        const t = msg.target;
        const lane = n?.automation?.find((l) => l.target.kind === t.kind && l.target.to === t.to && l.target.insert === t.insert && l.target.param === t.param);
        if (!lane) {
          this.emit("error", { command: "set_lane_mode", message: "pas de courbe pour cette cible" });
          return false;
        }
        return this.#edit("Mode d'automation", msg, () => {
          lane.mode = msg.mode;
        });
      }
      case "add_send":
        return this.#edit("Ajouter un envoi", msg, () => {
          const [n, to] = this.#sendEnds(msg);
          n.sends ??= [];
          if (n.sends.some((s) => s.to === to.id)) throw new Error("déjà un envoi vers cette destination");
          if (this.#reaches(to.id, n.id)) throw new Error("cet envoi fermerait une boucle");
          n.sends.push({ to: to.id, gain: 1, pre: false });
        });
      case "remove_send":
        return this.#edit("Supprimer un envoi", msg, () => {
          const [n] = this.#sendEnds(msg, false);
          const before = (n.sends ??= []).length;
          n.sends = n.sends.filter((s) => s.to !== msg.to);
          if (n.sends.length === before) throw new Error(`pas d'envoi vers ${msg.to}`);
        });
      case "set_send": {
        const [n] = this.#sendEnds(msg, false);
        const s = (n.sends ??= []).find((x) => x.to === msg.to);
        if (!s) {
          this.emit("error", { command: "set_send", message: `pas d'envoi de ${msg.node} vers ${msg.to}` });
          return false;
        }
        if (msg.pre !== undefined) {
          const ok = this.#edit(msg.pre ? "Envoi avant le fader" : "Envoi après le fader", msg, () => (s.pre = msg.pre));
          if (!ok) return false;
        }
        if (msg.mute !== undefined || msg.solo !== undefined) {
          if (msg.mute !== undefined) s.mute = msg.mute;
          if (msg.solo !== undefined) s.solo = msg.solo;
          this.#markDirty();
          if (msg.pre === undefined && msg.gain === undefined) this.#emitProject();
        }
        if (msg.gain !== undefined) {
          s.gain = msg.gain;
          this.#markDirty();
          if (msg.pre === undefined) this.#emitProject();
        }
        break;
      }
      case "create_group":
        return this.#edit("Créer un groupe", msg, () => {
          const { list, memberOf, write } = this.#groupScope(msg.ret);
          this.#checkGroupNodes(msg.nodes, msg.ret);
          const id = this.#newGroupId();
          list.push({ id, name: msg.name.trim(), color: msg.color ?? "" });
          write(msg.nodes.map((n) => [n, id]).concat([...memberOf].filter(([n]) => !msg.nodes.includes(n))));
        });
      case "edit_group":
        return this.#edit("Modifier un groupe", msg, () => {
          const { list, memberOf, write } = this.#groupScope(msg.ret);
          const g = list.find((x) => x.id === msg.group);
          if (!g) throw new Error(`groupe inconnu : ${msg.group}`);
          if (msg.nodes) this.#checkGroupNodes(msg.nodes, msg.ret);
          if (msg.name !== undefined) g.name = msg.name.trim();
          if (msg.color !== undefined && msg.color !== null) g.color = msg.color;
          if (msg.nodes) write([...memberOf].filter(([n, gr]) => gr !== g.id && !msg.nodes.includes(n)).concat(msg.nodes.map((n) => [n, g.id])));
        });
      case "delete_group":
        return this.#edit("Supprimer un groupe", msg, () => {
          const { list, memberOf, write, replaceList } = this.#groupScope(msg.ret);
          if (!list.some((g) => g.id === msg.group)) throw new Error(`groupe inconnu : ${msg.group}`);
          replaceList(list.filter((g) => g.id !== msg.group));
          write([...memberOf].filter(([, gr]) => gr !== msg.group));
        });
      case "customize_return_groups":
        return this.#edit("Groupes propres au retour", msg, () => {
          const ret = this.node(msg.ret);
          if (!ret || !ret.is_return) throw new Error("seul un retour de monitoring peut avoir ses propres groupes");
          if (ret.return_groups) throw new Error("ce retour a déjà ses propres groupes");
          this.#clearOffsets(msg.ret);
          const groups = [];
          const members = [];
          for (const g of this.project.groups ?? []) {
            const id = this.#newGroupId();
            groups.push({ ...g, id });
            for (const n of this.project.nodes) if (n.group === g.id && !n.is_return) members.push([n.id, id]);
          }
          ret.return_groups = { groups, members };
        });
      case "copy_return_groups":
        return this.#edit("Copier les groupes d'un retour", msg, () => {
          const ret = this.node(msg.ret);
          const from = this.node(msg.from);
          if (!ret?.is_return || !from?.is_return) throw new Error("les deux nœuds doivent être des retours de monitoring");
          if (ret === from) throw new Error("choisis un autre retour que celui-ci");
          this.#clearOffsets(msg.ret);
          const src = this.#effective(msg.from);
          const groups = [];
          const members = [];
          for (const g of src.groups) {
            const id = this.#newGroupId();
            groups.push({ id, name: g.name, color: g.color });
            for (const n of this.project.nodes) if (n.id !== msg.ret && !n.is_return && n.kind !== "master" && src.groupOf(n.id) === g.id) members.push([n.id, id]);
          }
          ret.return_groups = { groups, members };
        });
      case "reset_return_groups":
        return this.#edit("Revenir aux groupes du projet", msg, () => {
          const ret = this.node(msg.ret);
          if (!ret?.return_groups) throw new Error("ce retour suit déjà les groupes du projet");
          this.#clearOffsets(msg.ret);
          delete ret.return_groups;
        });
      case "set_group_offset": {
        if (!this.#effective(msg.to).groups.some((g) => g.id === msg.group)) {
          this.emit("error", { command: "set_group_offset", message: `groupe inconnu : ${msg.group}` });
          return false;
        }
        const offsets = (this.groupOffsets ??= new Map());
        const key = `${msg.group}:${msg.to ?? "-"}`;
        const db = Math.round(msg.db * 100) / 100;
        const delta = db - (offsets.get(key) ?? 0);
        if (Math.abs(delta) < 1e-9) return true;
        offsets.set(key, db);
        return this.send({ type: "set_group", group: msg.group, delta_db: delta, ...(msg.to === undefined ? {} : { to: msg.to }) });
      }
      case "set_group": {
        if (!this.#effective(msg.to).groups.some((g) => g.id === msg.group)) {
          this.emit("error", { command: "set_group", message: `groupe inconnu : ${msg.group}` });
          return false;
        }
        const k = msg.delta_db === undefined ? null : 10 ** (msg.delta_db / 20);
        const eff = this.#effective(msg.to);
        for (const n of this.project.nodes.filter((x) => x.kind !== "master" && eff.groupOf(x.id) === msg.group)) {
          if (msg.to === undefined) {
            if (msg.mute !== undefined) n.mute = msg.mute;
            if (msg.solo !== undefined) n.solo = msg.solo;
            if (k !== null) n.gain = Math.min(2, Math.max(0, n.gain * k));
          } else {
            let s = (n.sends ??= []).find((x) => x.to === msg.to);
            if (!s && (msg.mute === true || msg.solo === true)) n.sends.push((s = { to: msg.to, gain: 0, pre: false, mute: false, solo: false }));
            if (!s) continue;
            if (msg.mute !== undefined) s.mute = msg.mute;
            if (msg.solo !== undefined) s.solo = msg.solo;
            if (k !== null) s.gain = Math.min(2, Math.max(0, s.gain * k));
          }
        }
        this.#markDirty();
        this.#emitProject();
        break;
      }
      case "set_track_output":
        return this.#edit("Sortie d'une piste", msg, () => {
          const n = this.node(msg.node);
          const target = this.node(msg.output);
          if (!n) throw new Error(`nœud inconnu : ${msg.node}`);
          if (!target) throw new Error(`nœud inconnu : ${msg.output}`);
          if (target.kind !== "bus" && target.kind !== "master") throw new Error("sortie refusée : une piste n'est pas une destination");
          if (this.#loops(n, target.id)) throw new Error("sortie refusée : boucle de routage");
          n.output = target.id;
        });
      case "move_track":
        return this.#edit("Déplacer une piste", msg, () => {
          const n = this.node(msg.node);
          if (!n) throw new Error(`nœud inconnu : ${msg.node}`);
          const rest = this.project.nodes.filter((x) => x !== n);
          let at = rest.length;
          if (msg.before !== undefined) {
            at = rest.findIndex((x) => x.id === msg.before);
            if (at < 1) throw new Error(`nœud inconnu : ${msg.before}`);
          }
          rest.splice(at, 0, n);
          this.project.nodes = rest;
        });
      case "new_project": {
        this.listenNode = null;
        this.alignRecords?.clear();
        if (this.exportJob) return (this.emit("error", { command: "new_project", message: "un export est en cours : attends sa fin ou annule-le" }), false);
        if (this.importJob) return (this.emit("error", { command: "new_project", message: "un import est en cours : attends sa fin ou annule-le" }), false);
        if (this.convertJob) return (this.emit("error", { command: "new_project", message: "une conversion de projet est en cours : attends sa fin ou annule-la" }), false);
        const base = this.project.nodes[0];
        this.#replaceProject([{ ...structuredClone(base), clips: [] }, { id: 1, name: "Piste 1", kind: "stereo", color: TRACK_COLORS[0], gain: 1, pan: 0, mute: false, solo: false, invert: false, arm: false, output: 0, input: { kind: "audio", channels: [1] }, monitor: true, inserts: [], clips: [] }], true, null);
        break;
      }
      case "save_project": {
        const path = msg.path !== undefined ? (/\.daw$/i.test(msg.path) ? msg.path : `${msg.path}.daw`) : this.file.path;
        if (!path) {
          this.emit("error", { command: "save_project", message: "pas encore de dossier : utiliser « Enregistrer sous »" });
          return false;
        }
        this.#endPreview(true, false);
        this.disk.set(path, structuredClone({ nodes: this.project.nodes, autoCrossfade: this.autoCrossfade, rate: SR, punchOn: this.punchOn }));
        this.file = { ...this.file, name: path.replace(/^.*[\\/]/, "").replace(/\.daw$/i, ""), path, dirty: false };
        this.#emitFile();
        break;
      }
      case "open_project": {
        const saved = this.disk.get(msg.path);
        if (!saved) {
          this.emit("error", { command: "open_project", message: `lecture de ${msg.path} : dossier introuvable (le faux moteur ne relit que ce qu'il a enregistré lui-même)` });
          return false;
        }
        if (saved.rate && saved.rate !== SR && !msg.recover) {
          this.emit("rate_mismatch", { path: msg.path, project_rate: saved.rate, engine_rate: SR });
          break;
        }
        this.#replaceProject(structuredClone(saved.nodes), saved.autoCrossfade, msg.path);
        this.punchOn = !!saved.punchOn;
        this.#emitPunch();
        break;
      }
      case "convert_project": {
        const fail = (message) => (this.emit("error", { command: "convert_project", message }), false);
        if (this.convertJob) return fail("une conversion est déjà en cours");
        if (this.recording || this.importJob) return fail("arrête d'abord l'enregistrement ou l'import");
        const saved = this.disk.get(msg.path);
        if (!saved) return fail(`lecture de ${msg.path} : dossier introuvable`);
        if (!saved.rate || saved.rate === SR) return fail(`ce projet est déjà à ${SR} Hz : l'ouvrir suffit`);
        this.convertJob = { path: msg.path, saved, elapsed: 0, cancelled: false };
        this.#emitConvert(true);
        return true;
      }
      case "cancel_convert":
        if (this.convertJob) this.convertJob.cancelled = true;
        break;
      case "move_clip":
        return this.#edit("Déplacer un clip", msg, () => {
          const c = this.#clipOrFail(msg.clip);
          const from = this.project.nodes.find((n) => n.clips.includes(c));
          let to = from;
          if (msg.node !== undefined) {
            to = this.node(msg.node);
            if (!to) throw new Error(`nœud inconnu : ${msg.node}`);
            if (to.kind !== "mono" && to.kind !== "stereo") throw new Error(`le nœud ${msg.node} n'est pas une piste : un clip ne peut être posé que sur une piste`);
          }
          c.start = msg.start;
          if (to !== from) {
            from.clips.splice(from.clips.indexOf(c), 1);
            to.clips.push(c);
          }
          this.#autoCrossfade(to);
        });
      case "trim_clip":
        return this.#edit("Rogner un clip", msg, () => {
          const c = this.#clipOrFail(msg.clip);
          const dataStart = c.start - c.offset;
          const oldStart = c.start;
          const oldLen = c.len;
          let { start } = c;
          let end = c.start + c.len;
          if (msg.start !== undefined) {
            if (msg.start >= end || msg.start < dataStart) throw new Error(OUT_OF_RANGE);
            c.offset += msg.start - start;
            start = msg.start;
          }
          if (msg.end !== undefined) {
            if (msg.end <= start || msg.end > dataStart + c.source_len) throw new Error(OUT_OF_RANGE);
            end = msg.end;
          }
          c.start = start;
          c.len = end - start;
          c.fade_in = Math.min(c.fade_in, c.len);
          c.fade_out = Math.min(c.fade_out, c.len);
          // Un clip étiré rogné garde ses marqueurs (ceux qui restent dans la fenêtre) ; une extension au-delà de ce qu'il couvrait n'est pas simulée.
          if (c.stretch && start >= oldStart && start - oldStart + c.len <= oldLen) c.stretch = rewindow(c.stretch, oldLen, start - oldStart, start - oldStart + c.len) ?? undefined;
          this.#autoCrossfade(this.project.nodes.find((n) => n.clips.includes(c)));
        });
      case "split_clip":
        return this.#edit("Découper", msg, () => {
          const c = this.#clipOrFail(msg.clip);
          if (msg.at <= c.start || msg.at >= c.start + c.len) throw new Error(OUT_OF_RANGE);
          const d = msg.at - c.start;
          const node = this.project.nodes.find((n) => n.clips.includes(c));
          const right = structuredClone({ ...c, id: this.#nextClipId(), start: msg.at, offset: c.offset + d, len: c.len - d, fade_in: 0 });
          this.#splitStretch(c, right, d);
          c.len = d;
          c.fade_out = 0;
          c.fade_in = Math.min(c.fade_in, c.len);
          right.fade_out = Math.min(right.fade_out, right.len);
          node.clips.splice(node.clips.indexOf(c) + 1, 0, right);
        });
      case "move_clips":
        return this.#edit("Déplacer des clips", msg, () => {
          const clips = msg.clips.map((id) => this.#clipOrFail(id));
          const delta = Math.max(msg.delta, -Math.min(...clips.map((c) => c.start)));
          const origins = clips.map((c) => this.project.nodes.find((n) => n.clips.includes(c)));
          const dests = this.#shiftedTracks(origins, msg.node_delta ?? 0, false);
          const owners = new Set();
          clips.forEach((c, k) => {
            c.start += delta;
            if (dests[k] !== origins[k]) {
              origins[k].clips.splice(origins[k].clips.indexOf(c), 1);
              dests[k].clips.push(c);
            }
            owners.add(dests[k]);
          });
          for (const n of owners) this.#autoCrossfade(n);
        });
      case "delete_clips":
        return this.#edit("Supprimer des clips", msg, () => {
          const clips = msg.clips.map((id) => this.#clipOrFail(id));
          for (const c of clips) {
            const node = this.project.nodes.find((n) => n.clips.includes(c));
            node.clips.splice(node.clips.indexOf(c), 1);
          }
        });
      case "copy_clips": {
        const found = msg.clips.map((id) => this.clip(id));
        if (found.some((c) => !c)) {
          this.emit("error", { command: msg.type, message: `clip inconnu : ${msg.clips[found.findIndex((c) => !c)]}` });
          return false;
        }
        this.clipboard = found.map((c) => ({ node: this.project.nodes.find((n) => n.clips.includes(c)).id, clip: structuredClone(c) }));
        this.emit("clipboard", { count: this.clipboard.length });
        return true;
      }
      case "split_clips":
        return this.#splitMany(msg);
      case "paste_clips":
        return this.#place(msg, "Coller des clips", this.clipboard ?? [], msg.at, msg.node, 0);
      case "duplicate_clips": {
        const items = msg.clips.map((id) => this.clip(id));
        if (items.some((c) => !c)) {
          this.emit("error", { command: msg.type, message: `clip inconnu : ${msg.clips[items.findIndex((c) => !c)]}` });
          return false;
        }
        const snaps = items.map((c) => ({ node: this.project.nodes.find((n) => n.clips.includes(c)).id, clip: structuredClone(c) }));
        const base = Math.min(...snaps.map((x) => x.clip.start));
        const end = Math.max(...snaps.map((x) => x.clip.start + x.clip.len));
        const at = msg.delta === undefined || msg.delta === null ? end : Math.max(0, base + msg.delta);
        return this.#place(msg, "Dupliquer des clips", snaps, at, undefined, msg.node_delta ?? 0);
      }
      case "set_active_take":
        return this.#edit("Changer de prise", msg, () => {
          const c = this.#clipOrFail(msg.clip);
          if (!c.take_group) throw new Error("ce clip n'est dans aucun groupe de prises");
          const node = this.project.nodes.find((n) => n.clips.includes(c));
          for (const o of node.clips) {
            if (o !== c && o.take_group === c.take_group && o.start < c.start + c.len && c.start < o.start + o.len) o.muted = true;
          }
          c.muted = false;
        });
      case "activate_take_range":
        return this.#edit("Composer une prise", msg, () => {
          const c = this.#clipOrFail(msg.clip);
          if (c.kind !== "audio") throw new Error("composer par plage : seulement pour l'audio (un conteneur de notes s'active en entier)");
          if (!c.take_group) throw new Error("ce clip n'est dans aucun groupe de prises");
          const node = this.project.nodes.find((n) => n.clips.includes(c));
          const s = Math.max(msg.start, c.start);
          const e = Math.min(msg.end, c.start + c.len);
          if (s >= e) throw new Error("plage hors de la prise");
          // Coupe `clip` en `at` : la partie gauche garde l'identifiant, la droite est ajoutée et rendue.
          const cut = (clip, at) => {
            const right = { ...clip, id: this.#nextClipId(), start: at, len: clip.start + clip.len - at, offset: clip.offset + (at - clip.start) };
            clip.len = at - clip.start;
            node.clips.push(right);
            return right;
          };
          let mid = c;
          if (c.start < s) mid = cut(c, s);
          if (mid.start + mid.len > e) cut(mid, e);
          const [ms, me] = [mid.start, mid.start + mid.len];
          // Les autres prises actives du groupe qui la recouvrent : seule la partie recouverte devient muette.
          for (const o of [...node.clips]) {
            if (o === mid || o.take_group !== mid.take_group || o.muted || !(o.start < me && ms < o.start + o.len)) continue;
            let inner = o;
            if (o.start < ms) inner = cut(o, ms);
            if (inner.start + inner.len > me) cut(inner, me);
            inner.muted = true;
          }
          mid.muted = false;
        });
      case "set_clip_muted":
        return this.#edit(msg.muted ? "Couper un clip" : "Rétablir un clip", msg, () => {
          const c = this.#clipOrFail(msg.clip);
          c.muted = !!msg.muted;
        });
      case "delete_clip":
        return this.#edit("Supprimer un clip", msg, () => {
          const c = this.#clipOrFail(msg.clip);
          const node = this.project.nodes.find((n) => n.clips.includes(c));
          node.clips.splice(node.clips.indexOf(c), 1);
        });
      case "set_clip_fades":
        return this.#edit("Modifier un clip", msg, () => {
          const c = this.#clipOrFail(msg.clip);
          if ((msg.curve_in !== undefined || msg.curve_out !== undefined) && c.kind !== "audio") throw new Error("les fondus d'un clip de notes sont linéaires : pas de courbe à choisir");
          if (msg.fade_in !== undefined) c.fade_in = Math.min(msg.fade_in, c.len);
          if (msg.fade_out !== undefined) c.fade_out = Math.min(msg.fade_out, c.len);
          if (msg.curve_in !== undefined) c.fade_in_curve = { bend: 0, smooth: 0, ...msg.curve_in };
          if (msg.curve_out !== undefined) c.fade_out_curve = { bend: 0, smooth: 0, ...msg.curve_out };
        });
      case "set_crossfade":
        return this.#edit("Courbe du fondu enchaîné", msg, () => {
          const a = this.clip(msg.clip_out);
          const b = this.clip(msg.clip_in);
          if (a?.kind !== "audio" || b?.kind !== "audio") throw new Error("set_crossfade : deux clips audio existants");
          if (msg.curve_out !== undefined) a.fade_out_curve = { bend: 0, smooth: 0, ...msg.curve_out };
          if (msg.curve_in !== undefined) b.fade_in_curve = { bend: 0, smooth: 0, ...msg.curve_in };
        });
      case "set_crossfade_length":
        return this.#edit("Longueur du fondu enchaîné", msg, () => {
          const a = this.clip(msg.clip_out);
          const b = this.clip(msg.clip_in);
          if (a?.kind !== "audio" || b?.kind !== "audio") throw new Error("set_crossfade_length : deux clips audio existants");
          const bounds = crossfadeBounds(a, b, msg.anchor);
          if (!bounds) throw new Error("les clips ne se chevauchent pas");
          if (msg.length > bounds.max) throw new Error(OUT_OF_RANGE);
          const { bStart, aEnd } = crossfadeEdges(a, b, msg.length, msg.anchor);
          const bEnd = b.start + b.len;
          a.len = aEnd - a.start;
          b.offset += bStart - b.start;
          b.start = bStart;
          b.len = bEnd - bStart;
          a.fade_out = msg.length;
          b.fade_in = msg.length;
        });
      case "undo":
      case "redo": {
        const from = msg.type === "undo" ? this.undoStack : this.redoStack;
        const to = msg.type === "undo" ? this.redoStack : this.undoStack;
        const step = from.pop();
        if (!step) {
          this.emit("error", { command: msg.type, message: msg.type === "undo" ? "rien à annuler" : "rien à rétablir" });
          return false;
        }
        this.#restore(msg.type === "undo" ? step.before : step.after);
        to.push(step);
        this.#markDirty();
        this.#emitProject();
        this.#emitHistory();
        this.#refreshPreview();
        break;
      }
      case "preview_fade":
        return this.#startPreview(msg);
      case "panic":
        this.emit("panic", {});
        this.#emitPosition();
        break;
      case "stop_preview":
        if (this.preview) this.#endPreview(true, true);
        else this.emit("preview", { active: false });
        break;
      case "transport":
        this.#endPreview(true, false);
        if (this.recording && (msg.action === "stop" || (msg.action === "toggle" && this.playing))) {
          this.playing = false;
          this.#finishRecording();
          this.#emitPosition();
          break;
        }
        {
          const was = this.playing;
          this.playing = msg.action === "toggle" ? !this.playing : msg.action === "play";
          if (this.playing && !was) this.playOrigin = this.pos;
          else if (was && !this.playing && this.prefs.transport?.return_on_stop && this.playOrigin != null) this.pos = this.playOrigin;
        }
        this.#emitPosition();
        break;
      case "seek":
        this.#endPreview(true, false);
        this.pos = msg.pos;
        if (this.playing) this.playOrigin = msg.pos;
        this.#emitPosition();
        break;
      case "set_loop_record":
        this.loopRecOn = msg.enabled;
        if (msg.enabled) this.punchOn = false;
        this.#emitPunch();
        break;
      case "set_listen": {
        const n = msg.node === null ? null : this.node(msg.node);
        if (msg.node !== null && n?.kind !== "bus") return (this.emit("error", { command: "set_listen", message: "seul un bus ou un retour peut être écouté" }), false);
        this.listenNode = msg.node;
        this.#emitProject();
        break;
      }
      case "set_punch":
        this.punchOn = msg.enabled;
        if (msg.enabled) this.loopRecOn = false;
        this.#markDirty();
        this.#emitPunch();
        break;
      case "loop":
        this.#endPreview(false, false);
        this.loop = msg.start === null ? null : { start: msg.start, end: msg.end };
        this.#emitPosition();
        break;
      case "set_node": {
        const n = this.node(msg.node);
        if (!n) {
          this.emit("error", { command: "set_node", message: `nœud inconnu : ${msg.node}` });
          return false;
        }
        for (const k of ["gain", "pan", "mute", "solo", "arm", "monitor", "invert"]) if (k in msg) n[k] = msg[k];
        if (["gain", "pan", "mute", "solo", "monitor", "invert"].some((k) => k in msg)) this.#markDirty();
        this.#emitProject();
        break;
      }
      case "get_peaks": {
        const c = this.clip(msg.clip);
        if (!c || c.kind !== "audio") {
          this.emit("error", { command: "get_peaks", message: `clip audio inconnu : ${msg.clip}` });
          return false;
        }
        const from = msg.from - c.start + c.offset;
        const to = msg.to - c.start + c.offset;
        const columns = columnPeaks(this.#wave(c), SR, from, to, msg.columns);
        // Comme le vrai moteur : les crêtes couvrent les données du fichier (même cachées par un rognage), silence au-delà.
        const span = to - from;
        columns.forEach((col, i) => {
          const mid = from + (span * (i + 0.5)) / msg.columns;
          if (mid < 0 || mid >= c.source_len) col[0] = col[1] = 0;
        });
        this.emit("peaks", { clip: msg.clip, from: msg.from, to: msg.to, columns });
        break;
      }
    }
    return true;
  }

  /** Fait avancer le temps de `ms` millisecondes (lecture, boucle) et publie position et niveaux. */
  #emitConvert(active, extra = {}) {
    const j = this.convertJob;
    const stem = j.path.replace(/^.*[\\/]/, "").replace(/\.daw$/i, "");
    this.emit("convert", { active, progress: Math.min(0.9, (0.9 * j.elapsed) / 1200), done: 0, total: 1, current: `${stem} (${SR} Hz)`, path: null, warnings: [], cancelled: false, error: null, ...extra });
  }

  /** Conversion simulée : 1,2 s fictive, annulable ; la copie « Nom (48000 Hz).daw » est ouverte à la fin, l'original est intact. */
  #advanceConvert(ms) {
    const j = this.convertJob;
    if (!j) return;
    if (j.cancelled) {
      this.#emitConvert(false, { cancelled: true, progress: 0 });
      this.convertJob = null;
      return;
    }
    j.elapsed += ms;
    if (j.elapsed < 1200) return this.#emitConvert(true);
    this.convertJob = null;
    const parent = j.path.replace(/[\\/][^\\/]*$/, "");
    const stem = j.path.replace(/^.*[\\/]/, "").replace(/\.daw$/i, "");
    let path = `${parent}/${stem} (${SR} Hz).daw`;
    for (let n = 2; this.disk.has(path); n++) path = `${parent}/${stem} (${SR} Hz) (${n}).daw`;
    this.disk.set(path, structuredClone({ ...j.saved, rate: SR }));
    this.emit("convert", { active: false, progress: 1, done: 1, total: 1, current: `${stem} (${SR} Hz)`, path, warnings: [], cancelled: false, error: null });
    this.#replaceProject(structuredClone(j.saved.nodes), j.saved.autoCrossfade, path);
  }

  /** En-tête simulé d'un fichier d'après son nom : « 44 » dans le nom → 44,1 kHz, « 5.1 » ou « six » → 6 canaux, « mono » → 1 canal. */
  #fakeProbe(path) {
    const file = path.replace(/^.*[\\/]/, "");
    const name = file.replace(/\.[^.]*$/, "");
    if (!/\.(wav|flac|mp3)$/i.test(file) || /faux/i.test(name)) return { path, name, format: null, sample_rate: 0, channels: 0, frames: null, needs_conversion: false, error: "format non reconnu (WAV, FLAC et MP3 sont gérés)" };
    const rate = /44/.test(name) ? 44_100 : SR;
    const channels = /5\.1|six|6ch/i.test(name) ? 6 : /mono/i.test(name) ? 1 : 2;
    return { path, name, format: file.split(".").pop().toLowerCase(), sample_rate: rate, channels, frames: rate * 4, needs_conversion: rate !== SR, error: null };
  }

  /** Import simulé : mêmes refus que l'hôte, 0,4 s fictive par fichier, annulable ; les clips sont posés d'un seul coup. */
  #startImport(msg) {
    const fail = (message) => (this.emit("error", { command: "import_audio", message }), false);
    if (this.importJob) return fail("un import est déjà en cours");
    if (this.recording) return fail("arrête d'abord l'enregistrement");
    const target = msg.node != null ? this.node(msg.node) : null;
    if (msg.node != null && !(target && (target.kind === "mono" || target.kind === "stereo"))) return fail(`nœud ${msg.node} : ce n'est pas une piste`);
    const infos = msg.files.map((f) => this.#fakeProbe(f));
    const plans = infos.map((i) => (msg.reference && !i.error && i.channels <= 2 && !i.needs_conversion ? "reference" : "copy"));
    if (plans.includes("copy") && !this.file.path) return fail("le projet n'a pas encore de dossier : enregistre-le d'abord, ou choisis « référencer en place »");
    const warnings = infos.flatMap((i, k) => (msg.reference && plans[k] === "copy" && !i.error ? [`« ${i.name} » : copié dans le projet au lieu d'être référencé`] : []));
    this.#endPreview(true, false);
    this.importJob = { msg, infos, plans, warnings, elapsed: 0, cancelled: false, pos: msg.pos ?? this.pos };
    this.#emitImport(true);
    return true;
  }

  #emitImport(active, extra = {}) {
    const j = this.importJob;
    const total = j.infos.length;
    const index = Math.min(Math.floor(j.elapsed / 400), total - 1);
    this.emit("import", { active, progress: Math.min(1, j.elapsed / (400 * total)), done: index, total, current: j.infos[index].name, clips: [], warnings: j.warnings, cancelled: false, error: null, ...extra });
  }

  #advanceImport(ms) {
    const j = this.importJob;
    if (!j) return;
    if (j.cancelled) {
      this.#emitImport(false, { cancelled: true, progress: 0, done: 0 });
      this.importJob = null;
      return;
    }
    j.elapsed += ms;
    if (j.elapsed < 400 * j.infos.length) return this.#emitImport(true);
    this.importJob = null;
    const flat = { shape: "linear", bend: 0, smooth: 0 };
    const good = j.infos.filter((i) => !i.error);
    const warnings = [...j.warnings, ...j.infos.filter((i) => i.error).map((i) => `« ${i.name} » : ${i.error}`)];
    const items = good.flatMap((i) => {
      if (i.channels <= 2) return [{ name: i.name, channels: i.channels, info: i }];
      if (j.msg.multichannel === "split") return Array.from({ length: i.channels }, (_, c) => ({ name: `${i.name} ${c + 1}`, channels: 1, info: i }));
      if (j.msg.multichannel === "stereo") return [{ name: i.name, channels: 2, info: i }];
      warnings.push(`« ${i.name} » : ${i.channels} canaux — choisis « une piste par canal » ou « mixage stéréo »`);
      return [];
    });
    if (!items.length) return this.emit("import", { active: false, progress: 0, done: 0, total: j.infos.length, current: "", clips: [], warnings, cancelled: false, error: warnings.at(-1) ?? "aucun fichier importable" });
    const clips = [];
    this.#edit("Importer de l'audio", { type: "import_audio" }, () => {
      items.forEach((it, k) => {
        let node = k === 0 && j.msg.node != null ? this.node(j.msg.node) : null;
        if (!node) {
          const id = Math.max(...this.project.nodes.map((n) => n.id)) + 1;
          node = { id, name: it.name, kind: it.channels === 2 ? "stereo" : "mono", color: TRACK_COLORS[(this.project.nodes.length - 1) % TRACK_COLORS.length], gain: 1, pan: 0, mute: false, solo: false, invert: false, arm: false, output: 0, input: { kind: "audio", channels: [1] }, monitor: true, inserts: [], clips: [] };
          this.project.nodes.push(node);
        }
        const len = Math.round((it.info.frames / it.info.sample_rate) * SR);
        node.clips.push({ id: this.#nextClipId(), kind: "audio", name: it.name, start: j.pos, len, offset: 0, source_len: len, gain: 1, fade_in: 0, fade_out: 0, fade_in_curve: flat, fade_out_curve: flat, wave: "pad", missing: false });
        clips.push({ name: it.name, track: node.name, converted: it.info.needs_conversion, referenced: j.msg.reference && it.channels <= 2 && !it.info.needs_conversion });
      });
    });
    this.emit("import", { active: false, progress: 1, done: j.infos.length, total: j.infos.length, current: "", clips, warnings, cancelled: false, error: null });
  }

  /** Calage simulé : mêmes règles de refus que l'hôte, 0,5 s fictive par double, annulable ; la prise calée est une prise de plus dans le groupe. */
  #startAlign(msg) {
    const fail = (message) => (this.emit("error", { command: msg.type, message }), false);
    if (this.alignJob) return fail("un calage est déjà en cours");
    if (this.recording) return fail("arrête d'abord l'enregistrement");
    this.alignRecords ??= new Map();
    const settingsOk = (t) => {
      try {
        JSON.parse(t);
        return true;
      } catch {
        return false;
      }
    };
    if (msg.settings != null && !settingsOk(msg.settings)) return fail("réglages : JSON invalide");
    const items = [];
    if (msg.type === "align_clips") {
      const guide = this.clip(msg.guide);
      if (!guide) return fail(`guide inconnu : ${msg.guide}`);
      const gnode = this.project.nodes.find((n) => n.clips.includes(guide));
      const gclips = [guide];
      for (const id of msg.guide_more ?? []) {
        const c = this.clip(id);
        if (!c) return fail(`guide inconnu : ${id}`);
        if (!gnode.clips.includes(c)) return fail("les clips du guide doivent être sur la même piste");
        gclips.push(c);
      }
      const gs = Math.min(...gclips.map((c) => c.start));
      const ge = Math.max(...gclips.map((c) => c.start + c.len));
      const gname = gclips.length > 1 ? `${guide.name} + ${gclips.length - 1}` : guide.name;
      const byNode = new Map();
      for (const id of msg.doubles) {
        const c = this.clip(id);
        if (!c) return fail(`double inconnu : ${id}`);
        const node = this.project.nodes.find((n) => n.clips.includes(c));
        if (node === gnode) return fail(`« ${c.name} » est sur la même piste que le guide « ${gname} » : la prise calée le recouvrirait. Mets le guide sur une autre piste.`);
        if (!byNode.has(node)) byNode.set(node, []);
        byNode.get(node).push(c);
      }
      for (const group of byNode.values()) {
        const c = group[0];
        const name = group.length > 1 ? `${c.name} + ${group.length - 1}` : c.name;
        if (!group.some((k) => k.start < ge && gs < k.start + k.len)) return fail(`« ${name} » ne recouvre pas la fenêtre du guide « ${gname} »`);
        items.push({ clip: c, name, guideName: gname, window: { start: gs, len: ge - gs }, settings: msg.settings ?? stringifySettings(parseSettings(undefined)) });
      }
    } else {
      for (const id of msg.clips) {
        const c = this.clip(id);
        const rec = this.alignRecords.get(id);
        if (!c || !rec) return fail(`le clip ${id} n'est pas une prise calée`);
        const merged = msg.settings == null ? rec.settings : msg.general_only ? stringifySettings(mergeGeneral(parseSettings(rec.settings), parseSettings(msg.settings))) : msg.settings;
        items.push({ clip: c, name: rec.name, guideName: rec.guideName, window: rec.window, settings: merged });
      }
    }
    this.alignJob = { msg, items, elapsed: 0, cancelled: false, label: msg.type === "align_clips" ? "Caler des prises" : "Refaire le calage" };
    this.#emitAlign(true);
    return true;
  }

  /** Chaque moitié d'un clip étiré découpé garde ses marqueurs (`d` : position de la coupe dans le clip, avant que `c.len` ne change). */
  #splitStretch(c, right, d) {
    if (!c.stretch) return;
    const len = c.len;
    right.stretch = rewindow(c.stretch, len, d, len) ?? undefined;
    c.stretch = rewindow(c.stretch, len, 0, d) ?? undefined;
  }

  /** Étirement simulé : mêmes refus que l'hôte, 0,4 s fictive, annulable ; une seule action annulable à la fin ; sans marqueur, retour au clip ordinaire. */
  #startStretch(msg) {
    const fail = (message) => (this.emit("error", { command: msg.type, message }), false);
    if (this.recording) return fail("arrête d'abord l'enregistrement");
    if (this.stretchJob) return fail("un étirement est déjà en cours");
    if (this.alignJob) return fail("un calage est en cours : attends sa fin ou annule-le");
    const c = this.clip(msg.clip);
    if (!c || c.kind !== "audio") return fail(`clip audio inconnu : ${msg.clip}`);
    const info = c.stretch ?? plain(c.len);
    const engine = msg.engine ?? info.engine;
    if (!ENGINES.includes(engine)) return fail(`moteur inconnu : « ${engine} »`);
    const error = validateStretch(msg.markers, c.len, info.src_len, this.project.sample_rate);
    if (error) return fail(error);
    if (!msg.markers.length && info.src_len === c.len) {
      if (!c.stretch) return fail("ce clip n'a pas de marqueur : rien à étirer");
      return this.#edit("Retirer l'étirement", msg, () => {
        delete c.stretch;
        c.data_id = (c.data_id ?? 0) + 1;
      });
    }
    this.stretchJob = { msg, clip: c, engine, src_len: info.src_len, elapsed: 0, cancelled: false };
    this.emit("stretch", { active: true, progress: 0, clip: c.id, name: c.name, cancelled: false, error: null, fell_back: false });
    return true;
  }

  #advanceStretch(ms) {
    const j = this.stretchJob;
    if (!j) return;
    if (j.cancelled) {
      this.stretchJob = null;
      this.emit("stretch", { active: false, progress: 1, clip: j.clip.id, name: j.clip.name, cancelled: true, error: null, fell_back: false });
      return;
    }
    j.elapsed += ms;
    if (j.elapsed < 400) return this.emit("stretch", { active: true, progress: j.elapsed / 400, clip: j.clip.id, name: j.clip.name, cancelled: false, error: null, fell_back: false });
    this.stretchJob = null;
    this.#edit("Étirer l'audio", j.msg, () => {
      j.clip.stretch = { engine: j.engine, src_len: j.src_len, markers: structuredClone(j.msg.markers), smooth: j.msg.markers.map((_, i) => !!j.msg.smooth?.[i]) };
      j.clip.data_id = (j.clip.data_id ?? 0) + 1; // le fichier du clip a changé (comme le vrai hôte)
    });
    this.emit("stretch", { active: false, progress: 1, clip: j.clip.id, name: j.clip.name, cancelled: false, error: null, fell_back: false });
  }

  #emitAlign(active, extra = {}) {
    const j = this.alignJob;
    const total = j.items.length;
    const index = Math.min(Math.floor(j.elapsed / 500), total - 1);
    this.emit("align", { active, progress: Math.min(1, j.elapsed / (500 * total)), done: index, total, current: j.items[index].name, clips: [], warnings: [], cancelled: false, error: null, ...extra });
  }

  #advanceAlign(ms) {
    const j = this.alignJob;
    if (!j) return;
    if (j.cancelled) {
      this.#emitAlign(false, { cancelled: true, progress: 0, done: 0 });
      this.alignJob = null;
      return;
    }
    j.elapsed += ms;
    if (j.elapsed < 500 * j.items.length) return this.#emitAlign(true);
    this.alignJob = null;
    const clips = [];
    const flat = { shape: "linear", bend: 0, smooth: 0 };
    this.#edit(j.label, j.msg, () => {
      for (const it of j.items) {
        const node = this.project.nodes.find((n) => n.clips.includes(it.clip));
        const group = it.clip.take_group || this.#newTakeGroup();
        for (const o of node.clips) if (o.kind === "audio" && !o.muted && o.start < it.window.start + it.window.len && it.window.start < o.start + o.len) Object.assign(o, { muted: true, take_group: group });
        const name = `${it.name} · calé sur ${it.guideName}`;
        const id = this.#nextClipId();
        node.clips.push({ id, kind: "audio", name, start: it.window.start, len: it.window.len, offset: 0, source_len: it.window.len, gain: it.clip.gain ?? 1, fade_in: 0, fade_out: 0, fade_in_curve: flat, fade_out_curve: flat, wave: it.clip.wave ?? "pad", missing: false, muted: false, take_group: group, aligned: true });
        this.alignRecords.set(id, { name: it.name, guideName: it.guideName, window: it.window, settings: it.settings });
        clips.push({ clip: id, name, track: node.name, fell_back: false });
      }
    });
    this.emit("align", { active: false, progress: 1, done: j.items.length, total: j.items.length, current: "", clips, warnings: [], cancelled: false, error: null });
  }

  #newTakeGroup() {
    const all = this.project.nodes.flatMap((n) => n.clips.map((c) => c.take_group ?? 0));
    return Math.max(0, ...all) + 1;
  }

  #alignDetail(msg) {
    const rec = this.alignRecords?.get(msg.clip);
    if (!rec) return (this.emit("error", { command: "get_align", message: `le clip ${msg.clip} n'est pas une prise calée` }), false);
    const secs = rec.window.len / SR;
    const n = Math.round(secs / 0.005);
    const path = Array.from({ length: n }, (_, i) => i * 0.005 + 0.04 * Math.sin((i * 0.005) / 1.7));
    const shift = path.map((p, i) => p - i * 0.005);
    const settings = msg.settings ?? rec.settings;
    this.emit("align_detail", { clip: msg.clip, name: rec.name, guide: rec.guideName, window_start: rec.window.start, window_len: rec.window.len, hop: 0.005, path, shift, consonants: [[0.4, 0.46], [secs * 0.4, secs * 0.4 + 0.07]], suggest: [[secs * 0.2, 0], [secs * 0.25, 1], [secs * 0.35, 1], [secs * 0.4, 0]], settings });
    return true;
  }

  /** Export simulé : mêmes règles de refus que l'hôte réel, durée fictive de 0,6 s par fichier, annulable. */
  #startExport(msg) {
    const fail = (message) => (this.emit("error", { command: "export", message }), false);
    if (this.exportJob) return fail("un export est déjà en cours");
    if (this.recording) return fail("arrête d'abord l'enregistrement");
    const dir = msg.dir ?? (this.file.path ? `${this.file.path}/export` : null);
    if (!dir) return fail("le projet n'a pas encore de dossier : choisis un dossier d'export, ou enregistre d'abord le projet");
    let range = { start: 0, end: Math.max(0, ...this.project.nodes.flatMap((n) => n.clips.map((c) => c.start + c.len))) };
    if (msg.scope === "loop") {
      if (!this.loop) return fail("aucune boucle : pose une boucle (Maj + glisser sur la règle) ou choisis une autre zone");
      range = { ...this.loop };
    } else if (msg.scope === "range") range = { start: msg.start, end: msg.end };
    if (range.end <= range.start) return fail(msg.scope === "project" ? "le projet est vide : rien à exporter" : "zone d'export vide");
    const base = (msg.name ?? this.file.name).replace(/[\\/:*?"<>|]/g, "-");
    const ext = msg.format.startsWith("flac") ? "flac" : "wav";
    let names = [base];
    if (msg.mode === "stems") {
      const pool = this.project.nodes.filter((n) => n.kind !== "master");
      const chosen = msg.nodes ? msg.nodes.map((id) => pool.find((n) => n.id === id) ?? null) : pool;
      if (chosen.some((n) => n === null)) return fail("nœud inconnu ou sans stem");
      names = chosen.map((n) => `${base} - ${n.name}`);
    }
    this.#endPreview(true, false);
    this.exportJob = { dir, names, ext, frames: range.end - range.start + Math.round((msg.tail_secs ?? 5) * SR * 0.5), elapsed: 0, files: [], cancelled: false, msg };
    this.#emitExport(true);
    return true;
  }

  #emitExport(active, extra = {}) {
    const j = this.exportJob;
    const total = j.names.length;
    const i = Math.min(Math.floor(j.elapsed / 600), total - 1);
    const progress = Math.min(1, j.elapsed / (600 * total));
    this.emit("export", { active, progress: active ? progress : extra.error || extra.cancelled ? j.files.length / total : 1, done: j.files.length, total, current: j.names[i], files: j.files, warnings: [], cancelled: false, error: null, dir: j.dir, ...extra });
  }

  #advanceExport(ms) {
    const j = this.exportJob;
    if (!j) return;
    if (j.cancelled) {
      this.#emitExport(false, { cancelled: true });
      this.exportJob = null;
      return;
    }
    j.elapsed += ms;
    while (j.files.length < j.names.length && j.elapsed >= 600 * (j.files.length + 1)) {
      const stem = j.names[j.files.length];
      const taken = this.exportedPaths ?? (this.exportedPaths = new Set());
      let path = `${j.dir}/${stem}.${j.ext}`;
      for (let n = 2; taken.has(path); n++) path = `${j.dir}/${stem} (${n}).${j.ext}`;
      taken.add(path);
      j.files.push({ path, frames: j.frames, peak_db: -6.2, clipped: 0, applied_gain_db: j.msg.normalize_db != null ? 5.1 : 0 });
    }
    if (j.files.length === j.names.length) {
      this.#emitExport(false);
      this.exportJob = null;
    } else this.#emitExport(true);
  }

  advance(ms) {
    this.#advanceExport(ms);
    this.#advanceImport(ms);
    this.#advanceAlign(ms);
    this.#advanceStretch(ms);
    this.#advanceConvert(ms);
    if (this.playing) {
      this.pos += Math.round((ms * SR) / 1000);
      if (this.loop && this.pos >= this.loop.end) this.pos = this.loop.start + ((this.pos - this.loop.start) % (this.loop.end - this.loop.start));
      // Punch-out : la prise se ferme toute seule à la fin de la zone, la lecture continue.
      if (this.recording?.end != null && this.pos >= this.recording.end) this.#finishRecording();
    }
    this.#emitPosition();
    this.#emitMeters();
  }

  /** Événement `prefs` : le « redémarrage nécessaire » compare les réglages audio enregistrés à ceux qui tournent. */
  #prefsEvent() {
    const p = this.prefs;
    const e = this.running.effective;
    const want = [p.driver ?? "UMC", p.buffer ?? 64, p.threads, p.ahead_ms, p.ahead_threads];
    const have = [e.driver, e.buffer, e.threads, e.ahead_ms, e.ahead_threads];
    return { prefs: { ...p, metronome: { ...p.metronome } }, running: this.running, restart_needed: want.some((v, i) => v !== have[i]), path: "(mémoire)" };
  }

  #emitProject() {
    // Une interface ouverte dont le plugin a disparu est fermée ; celle d'un plugin revenu (annulation) ne se rouvre pas toute seule.
    const alive = new Set(this.project.nodes.flatMap((n) => n.inserts.map((i) => i.id)));
    for (const id of [...this.openUis]) if (!alive.has(id)) this.openUis.delete(id);
    for (const n of this.project.nodes) for (const i of n.inserts) i.ui_open = this.openUis.has(i.id);
    const { nodes, ...rest } = this.project;
    this.emit("project", {
      ...rest,
      midi_devices: rest.midi_devices ?? ["Clavier MIDI (aperçu)", "Pad MIDI (aperçu)"],
      groups: (rest.groups ?? []).map((g) => ({ ...structuredClone(g), offsets: [...(this.groupOffsets ?? new Map())].filter(([k, db]) => k.startsWith(`${g.id}:`) && db !== 0).map(([k, db]) => ({ to: k.split(":")[1] === "-" ? null : Number(k.split(":")[1]), db })) })),
      auto_crossfade: this.autoCrossfade,
      listen: this.listenNode ?? null,
      nodes: nodes.map(({ ...n }) => ({
        ...n,
        phys: n.phys ? { ...n.phys } : null,
        is_return: !!n.is_return,
        group: n.group ?? 0,
        midi_filter: structuredClone(n.midi_filter ?? []),
        velocity_curve: { enabled: false, bend: 0, smooth: 0, min: 1, max: 127, fixed: 0, ...(n.velocity_curve ?? {}) },
        dynamics: { enabled: false, gate_db: -90, threshold_db: -18, ratio: 3, attack_ms: 10, release_ms: 120, makeup_db: 0, record: false, ...(n.dynamics ?? {}) },
        return_groups: n.return_groups ? structuredClone(n.return_groups) : null,
        sends: (n.sends ?? []).map((s) => ({ mute: false, solo: false, ...s })),
        automation: structuredClone((n.automation ?? []).filter((l) => (l.target.kind === "send" ? (n.sends ?? []).some((s) => s.to === l.target.to) : l.target.kind === "insert" ? n.inserts.some((i) => i.id === l.target.insert) : true))),
        clips: n.clips.map(({ wave, mat, ...c }) => (c.kind === "midi" ? { ...c, notes: this.#visibleNotes({ ...c, mat }) } : c)),
      })),
    });
  }

  #emitPosition() {
    this.emit("position", { pos: this.pos, playing: this.playing, loop: this.loop });
  }

  #emitMeters() {
    const levels = {};
    const anySolo = this.project.nodes.some((n) => n.kind !== "master" && n.solo);
    let mixL = 0;
    let mixR = 0;
    for (const n of this.project.nodes) {
      if (n.kind === "master") continue;
      let level = 0;
      if (this.playing) {
        const t = this.pos / SR;
        for (const c of n.clips) {
          if (this.pos < c.start || this.pos >= c.start + c.len) continue;
          if (c.kind === "audio") {
            const w = this.#wave(c);
            for (let k = 0; k < 6; k++) level = Math.max(level, Math.abs(w(t - (c.start - c.offset) / SR + k / 4000)));
          } else if (this.#visibleNotes(c).some((x) => this.pos >= c.start + x.start && this.pos < c.start + x.start + x.len)) {
            level = 0.5;
          }
        }
      }
      const silent = n.mute || (anySolo && !n.solo);
      const [gl, gr] = pan(n.kind === "mono" ? "mono" : "balance", n.pan);
      const l = silent ? 0 : level * n.gain * gl;
      const r = silent ? 0 : level * n.gain * gr;
      levels[n.id] = [l, r];
      mixL = Math.max(mixL, l);
      mixR = Math.max(mixR, r);
    }
    const master = this.node(0);
    levels[0] = [mixL * master.gain, mixR * master.gain];
    this.emit("meters", { levels });
    // Témoin d'entrée MIDI factice : une piste MIDI armée « reçoit des notes » pendant la lecture (l'aperçu n'a pas de clavier).
    const active = {};
    if (this.playing) for (const n of this.project.nodes) if (n.arm && n.input?.kind === "midi") active[n.id] = 1;
    const key = JSON.stringify(active);
    if (key !== this.midiActKey) {
      this.midiActKey = key;
      this.emit("midi_activity", { active });
    }
  }
}
