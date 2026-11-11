// Assemblage : crée le moteur (réel ou faux), reçoit ses événements dans un état unique et redessine à chaque image.

import { startRangeFill } from "./range-fill.js";
import { AlignController } from "./ui/align-dialog.js";
import { createEngine } from "./engine-select.js";
import { TempoMap, MeterMap } from "./time.js";
import { PeaksCache } from "./ui/peaks-cache.js";
import { Headers } from "./ui/headers.js";
import { Transport } from "./ui/transport.js";
import { Timeline, snapSpp, ROW_MIN, ROW_MAX, ROW_DEFAULT } from "./ui/timeline.js";
import { allClipIds } from "./selection.js";
import { FadeEditor } from "./ui/fade-editor.js";
import { ExportDialog } from "./ui/export-dialog.js";
import { SettingsDialog } from "./ui/settings-dialog.js";
import { ImportController } from "./ui/import-dialog.js";
import { PluginsPanel } from "./ui/plugins-panel.js";
import { Markers } from "./ui/markers.js";
import { PianoRoll } from "./ui/piano-roll.js";
import { ScoreWindow } from "./ui/score-window.js";
import { ScriptsUI } from "./ui/scripts-ui.js";
import { Mixer } from "./ui/mixer.js";
import { AutomationEditor } from "./ui/automation.js";
import { ProjectFiles, makeDialogs } from "./project-files.js";
import { ChordLane } from "./ui/chord-lane.js";
import { initScale, setScale, stepScale, getScale, resetScale, isManualScale, refreshAutoScale } from "./ui-scale.js";

const theme = new URLSearchParams(location.search).get("theme");
if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;

initScale(); // taille de l'interface mémorisée, avant de mesurer quoi que ce soit
const { engine, label, start } = await createEngine();
if (new URLSearchParams(location.search).has("debug")) window.__daw = { engine, get scripts() { return scripts; }, get pianoRoll() { return pianoRoll; }, get timeline() { return timeline; }, get scoreWindow() { return scoreWindow; } }; // accès pour la vérification dans le navigateur
document.getElementById("status-engine").textContent = label;

const state = {
  project: null,
  tempo: null,
  meters_: null, // carte des chiffrages (le champ `meters` ci-dessous est celui des niveaux)
  pos: 0,
  playing: false,
  loop: null,
  punchOn: false, // punch allumé : R enregistre dans la zone bleue
  loopRecOn: false, // enregistrement en boucle : la zone bleue tourne pendant la prise
  meters: {},
  midiAct: {}, // témoin d'entrée MIDI : types de messages reçus par piste (bits 1 notes, 2 contrôleurs, 4 molette)
  view: { start: 0, spp: 256 },
  selection: null, // clip « principal » de la sélection (découper, fondu, piano roll)
  selected: new Set(), // tous les clips sélectionnés
  clipboardCount: 0,
  autoOpen: new Set(),
  takesOpen: new Set(), // pistes dont les voies de prises sont dépliées
  chordsOpen: false, // voie d'accords (accords, tonalités, gammes, motifs) affichée en tête de la timeline
  chordTrack: null, // dernière piste d'accords reçue : { revision, track }
  chordCatalog: null, // qualités, alias et gammes pour la saisie
  autoTarget: new Map(),
  autoMode: new Map(),
  history: { can_undo: false, can_redo: false, undo_label: null, redo_label: null },
};

// Fichiers du projet : abonné avant la première demande pour ne pas rater l'état initial (nom, modifications).
const fileHooks = {};
const files = new ProjectFiles(engine, state, makeDialogs(), { onLoaded: () => fileHooks.onLoaded?.(), onWarnings: (w) => fileHooks.onWarnings?.(w) });

// Premier projet : on s'abonne d'abord (start), puis on le demande, et on attend sa réception avant de construire quoi que ce soit.
await start();
const first = new Promise((resolve, reject) => {
  const off = engine.on("project", (e) => {
    off();
    clearTimeout(timer);
    resolve(e);
  });
  const timer = setTimeout(() => reject(new Error("le moteur n'a pas répondu à get_project")), 8000);
});
engine.send({ type: "get_project" });
applyProject(await first);

const peaks = new PeaksCache(engine, () => (timeline.dirty = true));
const headers = new Headers(document.getElementById("header-list"), state, engine);
const transport = new Transport(state, engine);
transport.files = files;
const timeline = new Timeline({
  tracks: document.getElementById("tracks"),
  ruler: document.getElementById("ruler"),
  scroller: document.getElementById("scroller"),
  state,
  client: engine,
  peaks,
});
// Voies d'automation : l'éditeur dessine les lignes dépliées dans la timeline et pilote leurs en-têtes.
const automation = new AutomationEditor(state, engine, timeline);
timeline.automation = automation;
headers.automation = automation;
automation.onHeaders = () => {
  headers.render();
  timeline.dirty = true;
};

// Voie d'accords : deux lignes en tête de la timeline, bouton « Accords » dans la barre.
timeline.onNotice = (m) => flash(m);
const chordLane = new ChordLane(state, engine, timeline, makeDialogs(), (m) => flash(m));
timeline.chordLane = chordLane;
headers.chordLane = chordLane;
chordLane.onHeaders = () => {
  headers.render();
  timeline.dirty = true;
  document.getElementById("btn-chords")?.setAttribute("aria-pressed", String(state.chordsOpen));
};
document.getElementById("btn-chords")?.addEventListener("click", () => chordLane.toggle());
document.getElementById("btn-score")?.addEventListener("click", () => {
  const has = state.project?.nodes?.some((n) => n.clips.some((c) => c.kind === "midi" && (c.id === state.selection || state.selected?.has?.(c.id))));
  if (has) timeline.openScoreForSelection();
  else flash("Sélectionne d'abord un clip de notes (un clic dessus), puis ♪ Partition");
});
// Le projet a déjà été demandé avant que la voie existe : on demande ce qui la concerne.
engine.send({ type: "get_chords" });
engine.send({ type: "get_chord_catalog" });

function applyProject(p) {
  state.project = p;
  state.tempo = new TempoMap(p.tempo_map, p.sample_rate);
  state.meters_ = new MeterMap(p.meters);
}

let uiDirty = true;
engine.on("project", (p) => {
  applyProject(p);
  headers.render();
  timeline.resize();
  if (state.fitNext) {
    state.fitNext = false;
    timeline.fit();
  }
  timeline.settle();
  transport.updateEditing();
  fadeEditor.refresh();
  pluginsPanel.refresh();
  mixer.render();
  uiDirty = true;
});
engine.on("selection", (e) => timeline.selectClips(e.clips));
engine.on("monitor", (e) => (state.monitor = e));
engine.on("clipboard", (e) => (state.clipboardCount = e.count));
engine.on("history", (h) => {
  state.history = h;
  transport.updateEditing();
});
engine.on("position", (e) => {
  // Le moteur publie la position ~30 fois par seconde même à l'arrêt : on ne redessine que si quelque chose a changé
  // (sinon la timeline entière serait repeinte en permanence pour rien).
  const loopChanged = JSON.stringify(e.loop) !== JSON.stringify(state.loop);
  if (e.pos === state.pos && e.playing === state.playing && !loopChanged) return;
  state.pos = e.pos;
  state.playing = e.playing;
  state.loop = e.loop;
  headers.follow();
  mixer.follow();
  pluginsPanel.follow();
  timeline.follow();
  timeline.dirty = true;
  pianoRoll.dirty = true;
  uiDirty = true;
});
engine.on("meters", (e) => (state.meters = e.levels));
engine.on("midi_activity", (e) => {
  state.midiAct = e.active ?? {};
  headers.setMidiActivity();
  mixer.setMidiActivity();
});
engine.on("loop_record", (e) => {
  state.loopRecOn = !!e.enabled;
  transport.updateLoopRecord();
});
engine.on("punch", (e) => {
  state.punchOn = !!e.enabled;
  transport.updatePunch();
});
engine.on("writing", (e) => {
  document.getElementById("write-status").hidden = !e.active;
});
engine.on("metronome", (e) => {
  state.metronome = e;
  transport.updateMetronome();
});
engine.on("recording", (e) => {
  state.recording = e;
  transport.updateRecording();
  headers.render();
  if (e.warnings?.length) fileHooks.onWarnings?.(e.warnings);
});
let errorTimer = null;
/** Message de quelques secondes dans la barre d'état (refus du moteur, remarque de la page). */
function flash(text) {
  const status = document.getElementById("status-engine");
  status.textContent = `${label} — ${text}`;
  clearTimeout(errorTimer);
  errorTimer = setTimeout(() => (status.textContent = label), 6000);
}
engine.on("error", (e) => {
  console.warn(`Le moteur a refusé « ${e.command} » : ${e.message}`);
  flash(`refus (${e.command}) : ${e.message}`);
  timeline.settle();
});

headers.render();
const markers = new Markers(document.getElementById("markers"), state, engine, timeline);
timeline.onDraw = () => markers.refresh();
const fadeEditor = new FadeEditor(state, engine);
const exportDialog = new ExportDialog(state, engine, files, makeDialogs());
const settingsDialog = new SettingsDialog(state, engine, files, makeDialogs());
const scripts = new ScriptsUI(engine, state, (m) => flash(m));
const aligner = new AlignController(state, engine, (lines) => fileHooks.onWarnings?.(lines), peaks);
const importer = new ImportController(state, engine, files, makeDialogs(), timeline, (lines) => fileHooks.onWarnings?.(lines));
const mixer = new Mixer(state, engine);
mixer.onPlugins = (id) => pluginsPanel.open(id);
const pianoRoll = new PianoRoll(state, engine);
timeline.onPianoRoll = (clipId) => pianoRoll.open(clipId);
const scoreWindow = new ScoreWindow(state, engine);
scoreWindow.onOpenRoll = (clipId) => pianoRoll.open(clipId);
scoreWindow.chordLane = chordLane;
scoreWindow.onDock = () => pianoRoll.docked && pianoRoll.undock();
pianoRoll.onDock = () => scoreWindow.mode === "dock" && scoreWindow.setMode("float");
document.getElementById("pr-score").addEventListener("click", () => pianoRoll.clipId != null && scoreWindow.open(pianoRoll.clipId));
timeline.onScore = (clipId) => scoreWindow.open(clipId);
headers.pianoRoll = pianoRoll;
// Hauteur des pistes mémorisée (zoom vertical de la timeline au pincement).
try {
  const saved = parseInt(localStorage.getItem("daw.rowH") ?? "", 10);
  if (saved >= ROW_MIN && saved <= ROW_MAX) document.documentElement.style.setProperty("--row-h", `${saved}px`);
} catch {
  /* stockage indisponible : hauteur par défaut */
}
timeline.onLayout = () => pianoRoll.dockHost.layout();
headers.rowControl = { get: () => timeline.rowH, set: (px, light) => timeline.setRowHeight(px, light), reset: () => timeline.setRowHeight(ROW_DEFAULT, true) };
aligner.onTakes = (ids) => headers.openTakes(ids);
scoreWindow.dockHost = pianoRoll.dockHost = {
  timeline,
  zoom: (direction, anchorX) => timeline.zoom(direction, anchorX),
  layout: () => {
    headers.render();
    timeline.resize();
    timeline.dirty = true;
  },
};
const pluginsPanel = new PluginsPanel(state, engine);
headers.onPlugins = (id) => pluginsPanel.open(id);
// Un autre projet vient d'être chargé : les crêtes, la sélection et la fenêtre de fondu appartiennent à l'ancien.
fileHooks.onLoaded = () => {
  peaks.clear();
  state.selection = null;
  state.selected = new Set();
  state.activeTrack = null;
  state.clipboardCount = 0;
  if (fadeEditor.dialog.open) fadeEditor.dialog.close();
  pluginsPanel.close();
  timeline.autoFit = true;
  state.fitNext = true;
};
fileHooks.onWarnings = (list) => {
  const status = document.getElementById("status-engine");
  status.textContent = `${label} — ${list.join(" · ")}`;
  clearTimeout(errorTimer);
  errorTimer = setTimeout(() => (status.textContent = label), 12000);
};
timeline.onFadeEditor = (target) => fadeEditor.open(target);
timeline.onSelection = () => transport.updateEditing();
timeline.refreshColors();
timeline.resize();
timeline.fit();
transport.updateEditing();
state.view.spp = snapSpp(state.view.spp);

const scaleLabel = document.getElementById("ui-scale-value");
const scaleAuto = document.getElementById("btn-ui-auto");
const refreshScale = () => {
  scaleLabel.textContent = `${getScale()} %`;
  scaleAuto.hidden = !isManualScale();
  timeline.resize();
  timeline.dirty = true;
  headers.peakHold.clear();
  headers.cleared?.clear();
  fadeEditor.refresh();
  uiDirty = true;
};
window.addEventListener("ui-scale-change", refreshScale);
document.getElementById("btn-ui-smaller").addEventListener("click", () => stepScale(-1));
document.getElementById("btn-ui-bigger").addEventListener("click", () => stepScale(1));
scaleAuto.addEventListener("click", () => resetScale());
scaleLabel.textContent = `${getScale()} %`;
scaleAuto.hidden = !isManualScale();
document.getElementById("btn-zoom-in").addEventListener("click", () => timeline.zoom(1));
document.getElementById("btn-zoom-out").addEventListener("click", () => timeline.zoom(-1));
document.getElementById("btn-zoom-fit").addEventListener("click", () => {
  timeline.autoFit = true;
  timeline.fit();
});
window.addEventListener("resize", () => {
  refreshAutoScale();
  timeline.resize();
  fadeEditor.relayout();
});
new ResizeObserver(() => timeline.resize()).observe(document.getElementById("tracks-wrap"));
matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => setTimeout(() => timeline.refreshColors(), 0));

window.addEventListener("keydown", (e) => {
  if (e.target instanceof Element && e.target.closest(".chord-dlg")) return; // la fenêtre de saisie d'accord gère ses touches
  if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
    if (e.key !== " " || e.target.type !== "range") return;
  }
  if (scripts.handleKey(e)) return; // raccourci d'un script (Ctrl+Alt+lettre ou chiffre 1-9, Alt+Maj+…)
  if ((e.ctrlKey || e.metaKey) && e.altKey) {
    // Taille de l'interface : Ctrl+Alt+ + / − / 0 (sans conflit avec le zoom du navigateur).
    if (e.key === "+" || e.key === "=") stepScale(1);
    else if (e.key === "-" || e.key === "_") stepScale(-1);
    else if (e.key === "0") resetScale();
    else return;
    e.preventDefault();
    return;
  }
  if ((e.ctrlKey || e.metaKey) && !e.altKey) {
    const k = e.key.toLowerCase();
    const fk = e.key.toLowerCase();
    if (e.key === "," && !e.shiftKey) {
      e.preventDefault();
      settingsDialog.open();
      return;
    }
    if (fk === "e" && !e.shiftKey) {
      e.preventDefault();
      exportDialog.open();
      return;
    }
    if (fk === "s" || fk === "o" || fk === "n") {
      e.preventDefault();
      if (fk === "s") (e.shiftKey ? files.saveAs() : files.save());
      else if (fk === "o") files.open();
      else files.newProject();
      return;
    }
    if (e.key === ".") {
      e.preventDefault();
      engine.send({ type: "panic" });
      return;
    }
    if (k === "c" || k === "x") {
      e.preventDefault();
      transport.copySelected(k === "x");
      return;
    }
    if (k === "v") {
      e.preventDefault();
      transport.pasteClips();
      return;
    }
    if (k === "d") {
      e.preventDefault();
      transport.duplicateSelected();
      return;
    }
    if (k === "a") {
      e.preventDefault();
      timeline.selectClips(allClipIds(timeline.nodes()));
      return;
    }
    if (k === "z" || k === "y") {
      e.preventDefault();
      engine.send({ type: k === "y" || e.shiftKey ? "redo" : "undo" });
    }
    return;
  }
  if (e.altKey) return;
  switch (e.key) {
    case "s":
    case "S":
      transport.splitSelected();
      break;
    case "r":
    case "R":
      transport.toggleRecord();
      break;
    case "m":
    case "M":
      mixer.toggle();
      break;
    case "Enter":
      timeline.openPianoRollForSelection();
      break;
    case "k":
    case "K":
      if (scoreWindow.isOpen()) scoreWindow.focusKey();
      else chordLane.placeKeyAtPlayhead();
      break;
    case "n":
    case "N":
      timeline.openScoreForSelection();
      break;
    case "e":
    case "E":
      timeline.openFadeEditorForSelection();
      break;
    case "Delete":
    case "Backspace":
      e.preventDefault();
      if (!automation.deleteSelected()) transport.deleteSelected();
      break;
    case " ":
      e.preventDefault();
      engine.send({ type: "transport", action: "toggle" });
      break;
    case "Home":
      engine.send({ type: "seek", pos: 0 });
      break;
    case "l":
    case "L":
      transport.toggleLoop();
      break;
    case "p":
    case "P":
      transport.togglePunch();
      break;
    case "+":
    case "=":
      timeline.zoom(1);
      break;
    case "-":
      timeline.zoom(-1);
      break;
  }
});

startRangeFill();
let last = performance.now();
function frame(now) {
  const dt = Math.min(100, now - last);
  last = now;
  headers.drawMeters(dt);
  mixer.drawMeters(dt);
  if (uiDirty) {
    transport.update();
    uiDirty = false;
  }
  timeline.draw();
  pianoRoll.draw();
  scoreWindow.draw();
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

// Pour les essais à la main dans la console du navigateur.
window.daw = { engine, state, timeline, pianoRoll, mixer, automation };
