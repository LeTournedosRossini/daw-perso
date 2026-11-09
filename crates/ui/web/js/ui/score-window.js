// Fenêtre « Partition » : une autre vue d'édition d'un clip de notes (comme le piano roll), dessinée par le module de Scorsaire
// (`js/vendor/scorsaire-score-view`, copie livrée le 8 novembre 2026, ne pas modifier). Le DAW reste la vérité : le module ne garde aucune
// donnée, il dit ce que l'utilisateur veut faire (`onEdit`) ; `score-adapter.js` le traduit en commandes annulables, le DAW renvoie le clip
// (événement `notes`) et la vue se redessine. Réglages de vue (portée, armure, grille, taille) : de l'affichage seulement, jamais dans le MIDI.

import { createScoreView } from "../vendor/scorsaire-score-view/scorsaire-score-view.js";
import { fifthsAt, keyChangesWithin, keyAt, markerAt, KEY_CHOICES, choiceOf, snapWhole } from "../chord-model.js";
import { fadd, fsub, fcmp as fcmpAbs } from "../notes-model.js";
import { scoreClipInput, scoreTiming, editToCommands, suggestStaff, defaultPresentation, playheadFraction, startInBarOf } from "../score-adapter.js";
import { GRIDS, gridById, noteId } from "../notes-model.js";
import { splitBlocks, blockData, sectionOf, orderBlocks, toClipIndices, toBlockIndices } from "../section-blocks.js";
import { applyLyricTap, lyricsForClip, lyricsToText, parseLyricsText, noteOnsets, noteSpans, placeOnNotes, clearVerseRange, verseCount, notesFromFirstSelected } from "../lyrics-model.js";
import { glyphSvg, nextCycle, durationLabel, DIRECT_DURATIONS, CYCLE_PAIR, CYCLE_DELAY_MS } from "../note-glyphs.js";
import { TouchPinch } from "../touch-pinch.js";
import { clampDockRows, clampWindowPos, dragPos, DOCK_ROWS_DEFAULT } from "../window-pos.js";

const SCALES = [0.5, 0.65, 0.8, 1, 1.25, 1.5, 2, 2.5, 3];
const STAFFS = [["treble", "Clé de sol"], ["bass", "Clé de fa"], ["grand", "Deux portées (piano)"]];

const MODES = ["float", "full", "dock"];
const uiZoom = () => parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--ui-zoom")) || 1;

const store = {
  get: (k) => { try { return localStorage.getItem(k); } catch { return null; } },
  set: (k, v) => { try { localStorage.setItem(k, v); } catch { /* stockage indisponible : le réglage ne sera pas mémorisé */ } },
};

export class ScoreWindow {
  constructor(state, client) {
    this.state = state;
    this.client = client;
    this.ids = []; // clips affichés (ordre d'affichage)
    this.cur = null; // clip courant : celui dont la barre montre portée et armure
    this.items = new Map(); // clip → { data (dernier événement `notes`), sel, staff }
    this.blocksOn = store.get("daw-score-blocks") !== "0"; // un morceau par section de la forme (affichage seulement)
    this.blockMap = new Map(); // identifiant donné au module → { clip, indexMap } (pour ramener ses gestes au vrai clip)
    this.blockKeys = new Map(); // clip → identifiants des morceaux envoyés (pour savoir quand tout redessiner)
    this.shown = false;
    this.view = null;
    this.lastHead = null;
    this.held = null;
    this.mode = MODES.includes(store.get("daw-score-mode")) ? store.get("daw-score-mode") : "float"; // fenêtre flottante, plein écran, ou ancrée sous la piste
    this.dockHost = null; // { timeline, layout() } donnés par main.js
    this.onDock = null; // appelé avant de s'ancrer (le piano roll ancré passe alors en fenêtre : il n'y a qu'un seul ancrage)
    this.rows = clampDockRows(Number(store.get("daw-sc-rows")) || DOCK_ROWS_DEFAULT);
    this.pos = null; // position de la fenêtre flottante { left, top }
    try { this.pos = JSON.parse(store.get("daw-score-pos") ?? "null"); } catch { this.pos = null; }
    this.root = document.createElement("section");
    this.root.className = "score-window";
    this.root.hidden = true;
    this.root.setAttribute("aria-label", "Partition");
    this.root.innerHTML = `
      <header>
        <h2 class="sc-title">Partition</h2>
        <label class="sc-clip" hidden>Clip <select class="sc-clipsel"></select></label>
        <label>Portée <select class="sc-staff"></select></label>
        <label class="sc-keylabel"><span class="sc-keybar">Armure</span> <select class="sc-key" title="Tonalité à partir de la mesure où se trouve la tête de lecture (touche K) : la même que la voie « Tonalités » de la timeline"></select></label><button class="sc-keyx" title="Retirer la tonalité posée à cette mesure" aria-label="Retirer la tonalité posée à cette mesure" hidden>✕</button>
        <span class="sc-group sc-dur" role="group" aria-label="Durée des notes : grille et durée des notes sélectionnées"></span><label class="sc-other" title="Autres grilles (triolets, libre)"><select class="sc-grid"></select></label>
        <span class="sc-group sc-voices" role="group" aria-label="Voix d'entrée" hidden>Voix <button data-v="1">1</button><button data-v="2">2</button><button data-v="3">3</button><button data-v="4">4</button></span>
        <span class="sc-group sc-tovoice" role="group" aria-label="Mettre la sélection dans une voix" hidden>Sélection → <button data-t="1">1</button><button data-t="2">2</button><button data-t="3">3</button><button data-t="4">4</button></span>
        <span class="sc-group" role="group" aria-label="Taille de la portée"><button class="sc-smaller" title="Portée plus petite" aria-label="Portée plus petite">A−</button><span class="sc-size"></span><button class="sc-bigger" title="Portée plus grande" aria-label="Portée plus grande">A＋</button></span>
        <button class="sc-blocks" aria-pressed="true" title="Un titre par section de la forme : les pistes d'une même section se suivent" hidden>Par sections</button>
        <button class="sc-lyr" title="Écrire les paroles du clip : une syllabe par note">Paroles…</button>
        <button class="sc-lyrline" aria-pressed="false" title="Montre la ligne de paroles sous la portée : toucher une place (ou une syllabe) pour la saisir au clavier">Saisie ♪</button>
        <label class="sc-seek"><input type="checkbox" class="sc-seekbox"> Placer la tête de lecture</label>
        <button class="sc-roll" title="Ouvrir le clip courant dans le piano roll">Piano roll</button>
        <button class="sc-dockbtn" title="Ancrer la partition sous la piste"></button>
        <button class="sc-fullbtn" title="Plein écran" aria-label="Plein écran"></button>
        <button class="close" aria-label="Fermer la partition">✕</button>
      </header>
      <div class="sc-body"><div class="sc-host"></div></div>
      <div class="sc-msg" hidden></div>
      <footer>Toucher : ajouter une note · glisser : déplacer · ↑ ↓ : hauteur (Maj : octave) · ← → : temps · Alt + ← → : durée · Suppr · deux doigts : défiler et zoomer</footer>`;
    document.body.append(this.root);
    this.title = this.root.querySelector(".sc-title");
    this.dockBtn = this.root.querySelector(".sc-dockbtn");
    this.fullBtn = this.root.querySelector(".sc-fullbtn");
    this.#buildDock();
    this.#bindDrag();
    this.dockBtn.addEventListener("click", () => this.setMode(this.mode === "dock" ? "float" : "dock"));
    this.fullBtn.addEventListener("click", () => this.setMode(this.mode === "full" ? "float" : "full"));
    this.title.addEventListener("dblclick", () => this.setMode(this.mode === "full" ? "float" : "full"));
    window.addEventListener("resize", () => this.mode === "float" && this.#applyPos());
    this.msgEl = this.root.querySelector(".sc-msg");
    this.voicesEl = this.root.querySelector(".sc-voices");
    this.toVoiceEl = this.root.querySelector(".sc-tovoice");
    this.entryVoice = Math.min(4, Math.max(1, Number(store.get("daw-score-voice")) || 1));
    this.voicesEl.addEventListener("click", (e) => {
      const v = Number(e.target.dataset?.v);
      if (!v) return;
      this.entryVoice = v;
      store.set("daw-score-voice", String(v));
      this.view?.setEntryVoice?.(v);
      this.#voiceButtons();
    });
    this.toVoiceEl.addEventListener("click", (e) => {
      const v = Number(e.target.dataset?.t);
      const it = this.items.get(this.cur);
      if (!v || !it?.data || !it.sel.length) return;
      for (const c of editToCommands({ type: "voice", clip: this.cur, indices: it.sel, voice: v }, it.data, {})) this.client.send(c);
    });
    this.clipLabel = this.root.querySelector(".sc-clip");
    this.clipSel = this.root.querySelector(".sc-clipsel");
    this.staffSel = this.root.querySelector(".sc-staff");
    this.keySel = this.root.querySelector(".sc-key");
    this.gridSel = this.root.querySelector(".sc-grid");
    this.body = this.root.querySelector(".sc-body");
    this.host = this.root.querySelector(".sc-host");
    this.sizeEl = this.root.querySelector(".sc-size");
    for (const [v, t] of STAFFS) this.staffSel.append(new Option(t, v));
    for (const c of KEY_CHOICES) this.keySel.append(new Option(c.label, c.id));
    this.keyBarEl = this.root.querySelector(".sc-keybar");
    this.keyX = this.root.querySelector(".sc-keyx");
    this.lastKeyBar = null;
    this.gridSel.append(new Option("Autre grille…", ""));
    for (const g of GRIDS.filter((x) => !DIRECT_DURATIONS.includes(x.id) && !CYCLE_PAIR.includes(x.id))) this.gridSel.append(new Option(`${g.id === "free" ? "Libre" : g.id} — ${g.label}`, g.id));
    this.scale = Number(store.get("daw-score-scale")) || 1;
    this.grid = gridById(store.get("daw-pr-grid") ?? "1/16"); // la même grille que le piano roll
    this.#buildDur();
    this.#syncDur();
    this.clipSel.addEventListener("change", () => this.#setCurrent(Number(this.clipSel.value)));
    this.staffSel.addEventListener("change", () => this.#present({ staff: this.staffSel.value }));
    // L'armure est celle de la piste des tonalités : la changer ici pose (ou remplace) la tonalité **au début de la mesure de la tête de lecture**,
    // exactement comme la touche K et la voie « Tonalités » de la timeline.
    this.keySel.addEventListener("change", () => {
      const c = KEY_CHOICES.find((x) => x.id === this.keySel.value);
      const kb = this.#keyBar();
      if (c && kb) this.chordLane?.setKeyAt(kb.at, c);
    });
    this.keyX.addEventListener("click", () => {
      const kb = this.#keyBar();
      if (kb) this.chordLane?.setKeyAt(kb.at, null);
    });
    this.gridSel.addEventListener("change", () => this.gridSel.value && this.#chooseGrid(this.gridSel.value, false));
    this.root.querySelector(".sc-smaller").addEventListener("click", () => this.setScale(this.#step(-1)));
    this.root.querySelector(".sc-bigger").addEventListener("click", () => this.setScale(this.#step(1)));
    this.root.querySelector(".sc-seekbox").addEventListener("change", (e) => this.view?.setTool(e.target.checked ? "seek" : "edit"));
    this.blocksBtn = this.root.querySelector(".sc-blocks");
    this.blocksBtn.addEventListener("click", () => {
      this.blocksOn = !this.blocksOn;
      store.set("daw-score-blocks", this.blocksOn ? "1" : "0");
      this.#syncBlocksBtn();
      this.#sections();
      if (this.isOpen() && this.shown) this.#refreshAll();
    });
    this.root.querySelector(".sc-lyr").addEventListener("click", () => this.#lyricsPopup());
    // Ligne de paroles toujours réservée (pour pouvoir taper la première syllabe au clic) ; sinon elle n'apparaît que sous un clip qui a des paroles.
    this.lyrLine = store.get("daw-score-lyrline") === "1";
    this.lyrLineBtn = this.root.querySelector(".sc-lyrline");
    this.lyrLineBtn.setAttribute("aria-pressed", String(this.lyrLine));
    this.lyrLineBtn.addEventListener("click", () => {
      this.lyrLine = !this.lyrLine;
      store.set("daw-score-lyrline", this.lyrLine ? "1" : "0");
      this.lyrLineBtn.setAttribute("aria-pressed", String(this.lyrLine));
      if (this.isOpen() && this.shown) this.#refreshAll();
    });
    this.root.querySelector(".sc-roll").addEventListener("click", () => this.cur != null && this.onOpenRoll?.(this.cur));
    this.root.querySelector(".close").addEventListener("click", () => this.close());
    // Les touches de la partition (flèches, Suppr, Alt + flèches…) ne doivent pas déclencher les raccourcis du DAW ; Ctrl+Z, Ctrl+S et la barre d'espace passent.
    this.root.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && !e.target.closest?.(".sc-host")) return this.mode === "full" ? this.setMode("float") : this.close();
      if ((e.key === "k" || e.key === "K") && !e.ctrlKey && !e.metaKey && !e.altKey && !/^(INPUT|SELECT|TEXTAREA)$/.test(e.target?.tagName ?? "")) {
        e.preventDefault();
        e.stopPropagation();
        return this.focusKey();
      }
      if (e.ctrlKey || e.metaKey || e.key === " ") return;
      e.stopPropagation();
    });
    // Au doigt : un doigt édite, deux doigts défilent et changent la taille de la portée (comme le piano roll).
    this.pinch = new TouchPinch(this.body, {
      onMove: ({ factor, dx, dy }) => {
        this.body.scrollLeft -= dx / (this.state.uiZoom ?? 1);
        this.body.scrollTop -= dy / (this.state.uiZoom ?? 1);
        if (factor !== 1) this.#pinchScale(factor);
      },
    });
    client.on("notes", (e) => this.onNotes(e));
    client.on("project", () => this.onProject());
    client.on("chords", () => (this.#sections(), this.#onKeysChanged()));
    this.#showSize();
  }

  isOpen() {
    return !this.root.hidden && this.ids.length > 0;
  }

  #clip(id) {
    for (const n of this.state.project?.nodes ?? []) for (const c of n.clips) if (c.id === id && c.kind === "midi") return { clip: c, node: n };
    return null;
  }

  /** Ordre d'affichage : les pistes dans l'ordre du projet, puis les clips d'une piste dans le temps. */
  #sorted(ids) {
    const nodes = this.state.project?.nodes ?? [];
    const key = (id) => {
      const c = this.#clip(id);
      return [c ? nodes.indexOf(c.node) : 1e9, c?.clip.start?.[0] / (c?.clip.start?.[1] || 1) || 0];
    };
    return [...ids].sort((x, y) => key(x)[0] - key(y)[0] || key(x)[1] - key(y)[1]);
  }

  /** Ouvre un clip, ou plusieurs (une portée par clip, empilées) ; le premier de la liste est le clip courant (ses réglages sont ceux de la barre). */
  open(clipIds) {
    const asked = (Array.isArray(clipIds) ? clipIds : [clipIds]).filter((id) => this.#clip(id));
    if (!asked.length) return;
    this.#release();
    this.ids = this.#sorted(asked);
    this.cur = asked[0];
    this.items = new Map(this.ids.map((id) => [id, { data: null, sel: [], staff: null }]));
    this.shown = false; // les portées sont posées d'un coup quand toutes les notes sont arrivées (pour garder l'ordre)
    this.root.hidden = false;
    this.#applyMode();
    if (!this.view) {
      this.view = createScoreView(this.host, { onEdit: (c) => this.#onEdit(c), onPlayheadSeek: (f) => this.#seek(f), onError: (e) => this.#onModuleError(e), locale: "fr" });
      this.view.setStaffScale(this.scale);
    }
    this.#applyEditGrid();
    this.view.setTool(this.root.querySelector(".sc-seekbox").checked ? "seek" : "edit");
    this.view.setEntryVoice?.(this.entryVoice);
    this.#voiceButtons();
    this.view.setClips([]);
    this.lastHead = null;
    this.#timing();
    this.#sections();
    this.#fillClipSelect();
    for (const id of this.ids) this.client.send({ type: "get_notes", clip: id });
    this.host.focus?.();
  }

  close() {
    this.#release();
    this.root.hidden = true;
    this.#undockState();
    this.ids = [];
    this.cur = null;
    this.items = new Map();
    this.view?.setClips([]);
  }

  // ---- durée : boutons de glyphes ---------------------------------------------------------------------------------------------

  /** Quatre boutons directs (blanche, noire, croche, double croche) et un bouton à deux états (ronde / triple croche). */
  #buildDur() {
    const box = this.root.querySelector(".sc-dur");
    this.durBtns = new Map();
    const make = (id) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "sc-dbtn";
      b.title = `${durationLabel(id)} : grille et durée des notes sélectionnées`;
      b.setAttribute("aria-label", durationLabel(id));
      b.innerHTML = glyphSvg(id);
      return b;
    };
    for (const id of DIRECT_DURATIONS) {
      const b = make(id);
      b.addEventListener("click", () => {
        this.#cancelCycle();
        this.#chooseGrid(id, true);
      });
      this.durBtns.set(id, b);
      box.append(b);
    }
    // Ronde / triple croche : chaque clic change d'état, mais le choix n'est pris en compte que CYCLE_DELAY_MS après le dernier clic
    // (ronde puis tout de suite triple croche : seule la triple croche est appliquée).
    const cyc = document.createElement("button");
    cyc.type = "button";
    cyc.className = "sc-dbtn sc-cycle";
    cyc.title = "Ronde / triple croche : un clic change d'état, le choix est pris en compte un instant après le dernier clic";
    cyc.addEventListener("click", () => {
      this.cyclePending = nextCycle(this.cyclePending, this.grid.id);
      clearTimeout(this.cycleTimer);
      this.cycleTimer = setTimeout(() => {
        const id = this.cyclePending;
        this.cyclePending = null;
        this.cycleTimer = null;
        if (id) this.#chooseGrid(id, true);
      }, CYCLE_DELAY_MS);
      this.#syncDur();
    });
    this.cycleBtn = cyc;
    box.append(cyc);
  }

  #cancelCycle() {
    clearTimeout(this.cycleTimer);
    this.cycleTimer = null;
    this.cyclePending = null;
  }

  #syncDur() {
    for (const [id, b] of this.durBtns) b.setAttribute("aria-pressed", String(this.grid.id === id));
    const shown = this.cyclePending ?? (CYCLE_PAIR.includes(this.grid.id) ? this.grid.id : CYCLE_PAIR[0]);
    this.cycleBtn.innerHTML = glyphSvg(shown);
    this.cycleBtn.setAttribute("aria-label", `${durationLabel(CYCLE_PAIR[0])} / ${durationLabel(CYCLE_PAIR[1])} : ${durationLabel(shown)}`);
    this.cycleBtn.classList.toggle("pending", !!this.cyclePending);
    this.cycleBtn.setAttribute("aria-pressed", String(!this.cyclePending && this.grid.id === shown));
    const other = !DIRECT_DURATIONS.includes(this.grid.id) && !CYCLE_PAIR.includes(this.grid.id);
    this.gridSel.value = other ? this.grid.id : "";
  }

  /** La grille choisie règle l'aimantation des gestes (la grille d'écriture, elle, est calculée d'après les notes : `displayGrid`). */
  #applyEditGrid() {
    try {
      this.view?.setEditGrid?.(this.grid.f);
    } catch {
      /* grille refusée par le module : l'aimantation reste celle de l'écriture */
    }
  }

  /**
   * Paroles du clip courant : un texte (« ma-man est la plus bel-le ») posé sur les notes, une syllabe par instant de note (un accord compte pour
   * un seul), dans le couplet choisi, à partir de la première note — ou de la première note sélectionnée s'il y en a. Il remplace les paroles de
   * ce couplet sur la partie du clip concernée. Une seule action annulable (la voie « Paroles » de la timeline les montre aussi).
   */
  #lyricsPopup() {
    this.root.querySelector(".sc-lyrpop")?.remove();
    const it = this.items.get(this.cur);
    if (!it?.data) return;
    const d = it.data;
    const list = this.state.lyrics ?? [];
    const pop = document.createElement("div");
    pop.className = "chord-pop sc-lyrpop";
    const mkLabel = (t, el) => {
      const l = document.createElement("label");
      l.append(t, el);
      return l;
    };
    const text = document.createElement("textarea");
    text.rows = 3;
    text.placeholder = "ma-man est la plus bel-le";
    text.setAttribute("aria-label", "Texte des paroles");
    const verse = document.createElement("select");
    for (let v = 1; v <= verseCount(list); v++) verse.append(new Option(`Couplet ${v}`, String(v)));
    verse.value = String(this.state.lyricVerse ?? 1);
    // Le texte déjà posé sur ce clip dans ce couplet : on le modifie plutôt que de tout réécrire.
    const from = d.start;
    const to = fadd(d.start, d.len);
    const inClip = (v) => list.filter((s) => (s.verse ?? 1) === v).filter((s) => fcmpAbs(s.at, from) >= 0 && fcmpAbs(s.at, to) < 0);
    const fill = () => (text.value = lyricsToText(inClip(Number(verse.value)), Number(verse.value), noteOnsets(d)));
    verse.addEventListener("change", fill);
    fill();
    const hint = document.createElement("div");
    hint.className = "cd-hint";
    hint.textContent = `${it.sel.length ? "Une syllabe par note, à partir de la première note sélectionnée." : "Une syllabe par note, à partir de la première note du clip."} « _ » prolonge la syllabe sur la note suivante.`;
    const info = document.createElement("div");
    info.className = "cd-hint";
    const row = document.createElement("div");
    row.className = "cd-foot";
    const mk = (label, fn) => {
      const b = document.createElement("button");
      b.textContent = label;
      b.addEventListener("click", fn);
      return b;
    };
    const ok = mk("Placer sur les notes", () => {
      const v = Number(verse.value);
      const items = parseLyricsText(text.value);
      const spans = noteSpans(d, it.sel.length ? notesFromFirstSelected(d, it.sel) : null);
      if (!items.length) {
        // Un texte vide efface les paroles de ce couplet sur ce clip.
        if (!inClip(v).length) return (info.textContent = "Écris au moins une syllabe.");
        this.client.send({ type: "set_lyrics", syllables: clearVerseRange(list, v, from, to), label: "Effacer des paroles" });
        return pop.remove();
      }
      if (!spans.length) return (info.textContent = "Ce clip n'a pas de note à laquelle accrocher les syllabes.");
      const { placed, extra } = placeOnNotes(items, spans, v);
      // Remplace ce couplet sur la plage couverte (de la première note posée à la fin du clip si la sélection part du milieu).
      const base = clearVerseRange(list, v, spans[0].at, to);
      this.state.lyricVerse = v;
      this.client.send({ type: "set_lyrics", syllables: [...base, ...placed].sort((a, b) => fcmpAbs(a.at, b.at) || (a.verse ?? 1) - (b.verse ?? 1)), label: "Écrire des paroles" });
      if (extra > 0) return (info.textContent = `${extra} syllabe${extra > 1 ? "s" : ""} en trop (plus que de notes, ou « _ » sans syllabe devant) : ${extra > 1 ? "elles n'ont" : "elle n'a"} pas été placée${extra > 1 ? "s" : ""}.`);
      pop.remove();
    });
    row.append(mk("Fermer", () => pop.remove()), ok);
    pop.append(mkLabel("Texte", text), mkLabel("Couplet", verse), hint, info, row);
    pop.addEventListener("keydown", (k) => {
      k.stopPropagation();
      if (k.key === "Escape") pop.remove();
    });
    this.root.append(pop);
    text.focus();
  }

  /**
   * Sélectionner des notes de même durée met les boutons de durée sur cette durée (repère visuel, et la grille d'édition suit) ;
   * on peut ensuite en cliquer un autre pour changer la durée des notes. Durées mélangées ou sans bouton : rien ne bouge.
   */
  #followSelection() {
    const it = this.items.get(this.cur);
    if (!it?.data || !it.sel.length || this.cyclePending) return;
    const lens = it.sel.map((i) => it.data.notes[i]?.len);
    if (lens.some((l) => !l) || lens.some((l) => l[0] * lens[0][1] !== lens[0][0] * l[1])) return;
    const g = GRIDS.find((x) => (DIRECT_DURATIONS.includes(x.id) || CYCLE_PAIR.includes(x.id)) && x.f[0] * lens[0][1] === lens[0][0] * x.f[1]);
    if (!g || g.id === this.grid.id) return;
    this.grid = g; // pas mémorisée : c'est un repère, pas un choix de l'utilisateur
    this.#syncDur();
    this.#applyEditGrid();
    this.view?.setNewNoteLength(this.grid.f);
  }

  /** Change la grille (aimantation et durée des nouvelles notes) ; `apply` : donne aussi cette durée aux notes sélectionnées. */
  #chooseGrid(id, apply) {
    this.grid = gridById(id);
    store.set("daw-pr-grid", this.grid.id); // la même grille que le piano roll
    this.#syncDur();
    this.#applyEditGrid();
    this.view?.setNewNoteLength(this.grid.f);
    if (!apply || !this.isOpen()) return;
    for (const [clip, it] of this.items) {
      if (!it.data || !it.sel.length) continue;
      this.client.send({ type: "edit_notes", clip, edits: it.sel.map((index) => ({ index, len: this.grid.f })), label: "Changer la durée" });
    }
  }

  // ---- présentation : flottante (déplaçable), plein écran, ancrée sous la piste ----------------------------------------------

  setMode(mode) {
    if (!MODES.includes(mode) || mode === this.mode) return;
    this.mode = mode;
    store.set("daw-score-mode", mode);
    if (this.isOpen()) this.#applyMode();
    else this.#labels();
  }

  #labels() {
    this.dockBtn.textContent = this.mode === "dock" ? "⇱ Fenêtre" : "⇲ Sous la piste";
    this.dockBtn.title = this.mode === "dock" ? "Détacher en fenêtre flottante" : "Ancrer la partition sous la piste";
    this.fullBtn.textContent = this.mode === "full" ? "⤡" : "⛶";
    const t = this.mode === "full" ? "Quitter le plein écran" : "Plein écran";
    this.fullBtn.title = t;
    this.fullBtn.setAttribute("aria-label", t);
    this.dockHead.querySelector(".sh-full").textContent = "⛶";
  }

  /** Met la fenêtre dans le mode voulu : déplace l'élément, règle les lignes de la timeline, restitue position et taille. */
  #applyMode() {
    if (this.mode === "dock") {
      const c = this.#clip(this.ids[0]);
      if (!c) this.mode = "float";
      else {
        this.onDock?.();
        this.#rememberSize();
        this.root.classList.remove("is-full");
        this.root.classList.add("is-docked");
        this.root.style.left = this.root.style.top = this.root.style.width = this.root.style.height = "";
        this.dockBody.append(this.root);
        this.dockBody.hidden = false;
        this.state.rollDockNode = c.node.id;
        this.state.dockRows = this.rows;
        this.state.dockHeadEl = this.dockHead;
        document.documentElement.style.setProperty("--sc-rows", String(this.rows));
        this.dockHead.querySelector(".sh-title").textContent = this.title.textContent;
        this.#labels();
        this.#relayout();
        return;
      }
    }
    this.#undockState();
    if (this.root.parentElement !== document.body) document.body.append(this.root);
    this.root.classList.remove("is-docked");
    this.root.classList.toggle("is-full", this.mode === "full");
    if (this.mode === "full") {
      this.#rememberSize();
      this.root.style.left = this.root.style.top = this.root.style.width = this.root.style.height = "";
    } else {
      if (this.floatSize) [this.root.style.width, this.root.style.height] = this.floatSize;
      this.#applyPos();
    }
    this.#labels();
    this.#relayout();
  }

  /** La taille donnée à la main (poignée de la fenêtre) est gardée tant qu'on est en plein écran ou ancré. */
  #rememberSize() {
    if (this.root.classList.contains("is-full") || this.root.classList.contains("is-docked")) return;
    this.floatSize = [this.root.style.width, this.root.style.height];
  }

  /** Rend les lignes de la timeline si c'est la partition qui les tenait. */
  #undockState() {
    this.dockBody.hidden = true;
    if (this.state.dockHeadEl !== this.dockHead) return;
    this.state.rollDockNode = null;
    this.state.dockHeadEl = null;
    this.state.dockRows = null;
    this.#relayout();
  }

  #relayout() {
    this.dockHost?.layout();
  }

  setRows(n) {
    const rows = clampDockRows(n);
    if (rows === this.rows) return;
    this.rows = rows;
    store.set("daw-sc-rows", String(rows));
    if (this.mode !== "dock") return;
    this.state.dockRows = rows;
    document.documentElement.style.setProperty("--sc-rows", String(rows));
    this.#relayout();
  }

  #applyPos() {
    if (this.mode !== "float") return;
    if (!(window.innerWidth > 0 && window.innerHeight > 0)) return; // page sans taille (onglet caché) : on ne sait pas où ramener la fenêtre
    const z = uiZoom();
    const p = clampWindowPos(this.pos ?? { left: 40, top: 100 }, { vw: window.innerWidth / z, vh: window.innerHeight / z, w: this.root.offsetWidth || 900 });
    this.pos = p;
    this.root.style.left = `${p.left}px`;
    this.root.style.top = `${p.top}px`;
  }

  /** Déplacement par le titre (souris et doigt) : seulement en mode flottant, et pas depuis les boutons ni les menus. */
  #bindDrag() {
    const head = this.root.querySelector("header");
    let d = null;
    head.addEventListener("pointerdown", (e) => {
      if (this.mode !== "float" || e.button > 0 || e.target.closest("button, select, input, label, option")) return;
      const cs = getComputedStyle(this.root);
      d = { id: e.pointerId, x: e.clientX, y: e.clientY, left: parseFloat(cs.left) || 0, top: parseFloat(cs.top) || 0 };
      head.setPointerCapture?.(e.pointerId);
      e.preventDefault();
    });
    head.addEventListener("pointermove", (e) => {
      if (!d || e.pointerId !== d.id) return;
      const z = uiZoom();
      this.pos = dragPos(d, e.clientX - d.x, e.clientY - d.y, z, { vw: window.innerWidth / z, vh: window.innerHeight / z, w: this.root.offsetWidth });
      this.#applyPos();
    });
    const end = (e) => {
      if (!d || e.pointerId !== d.id) return;
      d = null;
      store.set("daw-score-pos", JSON.stringify(this.pos));
    };
    head.addEventListener("pointerup", end);
    head.addEventListener("pointercancel", end);
  }

  /** Éléments de la présentation ancrée : l'en-tête (dans la colonne des pistes) et le corps (sur la largeur de la timeline). */
  #buildDock() {
    const head = document.createElement("div");
    head.className = "score-head";
    head.innerHTML = `
      <div class="sh-ctl">
        <div class="sh-title"></div>
        <div class="sh-row"><span title="Hauteur de la partition, en lignes de la timeline">Haut.</span><button class="sh-out" aria-label="Moins haut" title="Moins haut">−</button><button class="sh-in" aria-label="Plus haut" title="Plus haut">＋</button></div>
        <div class="sh-row"><button class="sh-float" title="Détacher en fenêtre flottante">⇱ Fenêtre</button><button class="sh-full" title="Plein écran" aria-label="Plein écran">⛶</button><button class="sh-close" aria-label="Fermer la partition">✕</button></div>
      </div>`;
    this.dockHead = head;
    head.querySelector(".sh-in").addEventListener("click", () => this.setRows(this.rows + 1));
    head.querySelector(".sh-out").addEventListener("click", () => this.setRows(this.rows - 1));
    head.querySelector(".sh-float").addEventListener("click", () => this.setMode("float"));
    head.querySelector(".sh-full").addEventListener("click", () => this.setMode("full"));
    head.querySelector(".sh-close").addEventListener("click", () => this.close());
    const body = document.createElement("div");
    body.className = "score-dock";
    body.hidden = true;
    this.dockBody = body;
    document.getElementById("tracks-wrap").append(body);
    // Poignée en bas : on tire pour changer la hauteur (par lignes entières de la timeline).
    const grip = document.createElement("div");
    grip.className = "roll-grip";
    grip.title = "Tirer pour changer la hauteur de la partition";
    body.append(grip);
    let drag = null;
    grip.addEventListener("pointerdown", (e) => {
      grip.setPointerCapture?.(e.pointerId);
      drag = { y: e.clientY, rows: this.rows };
    });
    grip.addEventListener("pointermove", (e) => {
      if (!drag) return;
      const rowH = (this.dockHost?.timeline.rowH ?? 88) * uiZoom();
      this.setRows(drag.rows + (e.clientY - drag.y) / rowH);
    });
    const end = () => (drag = null);
    grip.addEventListener("pointerup", end);
    grip.addEventListener("pointercancel", end);
  }

  /** Place le corps ancré sur ses lignes de la timeline (appelé à chaque image tant que la partition est ancrée). */
  layoutDock() {
    const tl = this.dockHost?.timeline;
    if (this.mode !== "dock" || !this.isOpen() || !tl) return;
    const i = tl.nodes().findIndex((r) => r.kind === "roll");
    if (i < 0) return;
    this.dockBody.style.top = `${i * tl.rowH}px`;
    this.dockBody.style.height = `${this.rows * tl.rowH - 1}px`;
  }

  /** Le clip dont la barre montre les réglages : le dernier touché. */
  #setCurrent(id) {
    if (id === this.cur || !this.items.has(id)) return;
    this.cur = id;
    this.#syncControls();
  }

  #syncControls() {
    const it = this.items.get(this.cur);
    if (!it) return;
    this.#voiceButtons();
    this.staffSel.value = it.staff ?? "treble";
    this.#syncKey();
    this.clipSel.value = String(this.cur);
    this.#title();
  }

  #fillClipSelect() {
    this.clipSel.replaceChildren(...this.ids.map((id) => {
      const c = this.#clip(id);
      return new Option(c ? `${c.clip.name} (${c.node.name})` : String(id), String(id));
    }));
    this.clipLabel.hidden = this.ids.length < 2;
    this.clipSel.value = String(this.cur);
  }

  #title() {
    const c = this.#clip(this.cur);
    this.title.textContent = this.ids.length > 1 ? `Partition — ${this.ids.length} clips` : c ? `Partition — ${c.clip.name} (${c.node.name})` : "Partition";
    this.dockHead.querySelector(".sh-title").textContent = this.title.textContent;
  }

  onProject() {
    if (!this.isOpen()) return;
    const gone = this.ids.filter((id) => !this.#clip(id)); // supprimé, ou édition annulée
    if (gone.length) {
      this.ids = this.ids.filter((id) => !gone.includes(id));
      for (const id of gone) this.items.delete(id);
      if (!this.ids.length) return this.close();
      if (gone.includes(this.cur)) this.cur = this.ids[0];
      this.view.setClips([]);
      if (this.shown) for (const id of this.ids) this.#safeUpdate(id);
      this.#fillClipSelect();
      this.#syncControls();
    }
    this.#title();
    this.#timing();
    for (const id of this.ids) this.client.send({ type: "get_notes", clip: id });
  }

  onNotes(e) {
    const it = this.items?.get(e.clip);
    if (!this.isOpen() || !it) return;
    // La sélection suit les notes elles-mêmes (début, touche, canal) et non leurs indices, qui changent à chaque édition.
    const keep = new Set(it.sel.map((i) => it.data?.notes[i]).filter(Boolean).map(noteId));
    it.data = e;
    if (!it.staff) it.staff = suggestStaff(e.notes);
    if (e.selection?.length) it.sel = [...e.selection];
    else it.sel = e.notes.flatMap((n, i) => (keep.has(noteId(n)) ? [i] : []));
    if (e.clip === this.cur) this.#syncControls();
    if (e.clip === this.cur) this.#followSelection();
    if (!this.shown) {
      if (!this.ids.every((id) => this.items.get(id)?.data)) return;
      this.shown = true;
      this.#refreshAll();
    } else {
      this.#safeUpdate(e.clip);
      this.#safeSelect(e.clip);
    }
    this.#showMsgs();
    this.#voiceButtons();
    this.view.setNewNoteLength(this.grid.f);
  }

  /** Armure du début de la partie visible d'un clip : celle de la piste des tonalités. */
  #fifthsOf(id, it = this.items.get(id), d = it?.data) {
    return fifthsAt(this.state.chordTrack?.track, d ? d.start[0] / d.start[1] : 0);
  }

  /** La mesure dont l'armure est montrée et modifiée : celle de la tête de lecture si elle est dans le clip courant, sinon la première du clip. */
  #keyBar() {
    const d = this.items.get(this.cur)?.data;
    const m = this.state.meters_;
    const tempo = this.state.tempo;
    if (!d || !m || !tempo) return null;
    const from = d.start[0] / d.start[1];
    const to = from + d.len[0] / d.len[1];
    let whole = tempo.wholeAt(Math.max(0, this.state.pos));
    if (!(whole >= from - 1e-9 && whole < to)) whole = from;
    const bar = m.locate(Math.max(0, whole)).bar;
    return { bar, whole: m.barStart(bar), at: snapWhole(m.barStart(bar), null) };
  }

  /** Le choix d'armure montre la tonalité en vigueur à la mesure de la tête de lecture ; ✕ si une tonalité est posée exactement là. */
  #syncKey() {
    const kb = this.#keyBar();
    if (!kb) return;
    const track = this.state.chordTrack?.track;
    const cur = choiceOf(keyAt(track, kb.whole)) ?? KEY_CHOICES.find((c) => c.id === "C");
    this.keySel.value = cur.id;
    this.keyBarEl.textContent = `Armure · mesure ${kb.bar + 1}`;
    this.keyX.hidden = !markerAt(track, kb.whole);
    this.lastKeyBar = kb.bar;
  }

  /** Touche K : ouvre le choix d'armure (la tonalité se pose à la mesure de la tête de lecture). */
  focusKey() {
    this.keySel.focus();
    try {
      this.keySel.showPicker?.();
    } catch {
      /* pas de sélecteur natif disponible : le menu reste utilisable au clavier */
    }
  }

  /** Les changements de tonalité de la piste à l'intérieur du clip, en positions « depuis le début de la matière » (comme les notes). */
  #keyChangesOf(it, d = it.data) {
    const track = this.state.chordTrack?.track;
    if (!d || !track?.keys?.length) return [];
    const from = d.start[0] / d.start[1];
    const inside = new Set(keyChangesWithin(track, from, from + d.len[0] / d.len[1]).map((k) => `${k.whole}`));
    return track.keys
      .filter((k) => inside.has(`${k.at[0] / k.at[1]}`) && Number.isInteger(k.fifths))
      .map((k) => ({ start: fadd(fsub(k.at, d.start), d.offset), fifths: k.fifths }));
  }

  /** La piste des tonalités a changé : les clips se redessinent avec la bonne armure. */
  #onKeysChanged() {
    if (!this.isOpen() || !this.shown) return;
    for (const id of this.ids) this.#safeUpdate(id);
    this.#showMsgs();
    this.#syncKey();
  }

  /** Les boutons de voix n'apparaissent que si le module sait écrire plusieurs voix (`setEntryVoice`). */
  #voiceButtons() {
    const ok = typeof this.view?.setEntryVoice === "function";
    this.voicesEl.hidden = !ok;
    this.toVoiceEl.hidden = !ok;
    if (!ok) return;
    for (const b of this.voicesEl.querySelectorAll("button")) b.setAttribute("aria-pressed", String(Number(b.dataset.v) === this.entryVoice));
    const none = !this.items.get(this.cur)?.sel?.length;
    for (const b of this.toVoiceEl.querySelectorAll("button")) b.disabled = none;
  }

  #msgs = new Map(); // morceau (identifiant du module) → pourquoi le module n'a pas pu l'écrire

  /**
   * Les morceaux d'un clip tels que le module les reçoit : un seul (le clip entier, même identifiant) sans sections ; sinon un par section de la forme
   * que la partie visible traverse, chacun avec son titre (`clip.section`). Chaque entrée garde `indexMap` pour ramener ses gestes au vrai clip.
   */
  #blocksOf(id) {
    const it = this.items.get(id);
    const d = it?.data;
    if (!d) return [];
    const nodeIndex = (this.state.project?.nodes ?? []).indexOf(this.#clip(id)?.node);
    const structure = this.state.structure ?? [];
    const parts = this.blocksOn && structure.length ? splitBlocks(d, structure) : [];
    if (parts.length < 2 && !(parts.length === 1 && parts[0].sec >= 0)) return [{ mid: id, data: d, indexMap: null, section: null, secAt: null, nodeIndex, from: d.start }];
    return parts.map((b, k) => {
      const { data, indexMap } = blockData(d, b);
      const sec = b.sec >= 0 ? structure[b.sec] : null;
      return { mid: `${id}:${k}`, data, indexMap, section: sec ? sectionOf(sec) : null, secAt: sec ? sec.at : null, nodeIndex, from: b.from };
    });
  }

  /** Tous les morceaux de tous les clips, dans l'ordre d'affichage (section après section, pistes dans l'ordre du projet). */
  #allBlocks() {
    return orderBlocks(this.ids.flatMap((id) => this.#blocksOf(id).map((b) => ({ ...b, clip: id }))));
  }

  /** Remet tout au module d'un coup (premier affichage, changement de forme ou de mode) ; un clip qu'il refuse ne cache pas les autres. */
  #refreshAll() {
    const blocks = this.#allBlocks();
    this.blockMap = new Map(blocks.map((b) => [b.mid, { clip: b.clip, indexMap: b.indexMap }]));
    const keys = new Map();
    for (const b of blocks) keys.set(b.clip, `${keys.get(b.clip) ?? ""}|${b.mid}`);
    this.blockKeys = keys;
    this.#msgs.clear();
    try {
      this.view.setClips(blocks.map((b) => this.#inputOf(b)));
    } catch {
      this.view.setClips([]);
      for (const b of blocks) this.#updateBlock(b);
    }
    for (const id of this.ids) this.#safeSelect(id);
    this.#showMsgs();
  }

  #updateBlock(b) {
    try {
      this.view.updateClip(this.#inputOf(b));
      this.#msgs.delete(b.mid);
    } catch (e) {
      this.#msgs.set(b.mid, String(e?.message ?? e));
    }
  }

  #safeUpdate(id) {
    if (!this.items.get(id)?.data) return;
    const blocks = this.#blocksOf(id).map((b) => ({ ...b, clip: id }));
    const key = blocks.map((b) => `|${b.mid}`).join("");
    if (key !== (this.blockKeys.get(id) ?? "")) return this.#refreshAll(); // le découpage a changé : l'ordre et les titres aussi
    for (const b of blocks) this.#updateBlock(b);
  }

  /** Le module refuse un clip (il écrit lui-même la raison à sa place) : on la garde aussi sous la partition. */
  #onModuleError({ clip, message }) {
    this.#msgs.set(clip, String(message ?? ""));
    this.#showMsgs();
  }

  /** Les morceaux du module → indices de leur clip. */
  #realOf(mid) {
    return this.blockMap.get(mid)?.clip ?? mid;
  }

  #safeSelect(id) {
    const sel = this.items.get(id)?.sel ?? [];
    for (const b of this.#blocksOf(id)) {
      try {
        this.view.setSelection(b.mid, b.indexMap ? toBlockIndices(sel, b.indexMap) : sel);
      } catch {
        /* morceau non dessiné : rien à sélectionner */
      }
    }
  }

  #showMsgs() {
    const lines = [...this.#msgs].map(([mid, m]) => `${this.#clip(this.#realOf(mid))?.clip.name ?? mid} : ${m.split("\n").slice(-1)[0].trim()}`);
    this.msgEl.textContent = lines.length ? `La partition ne sait pas encore écrire : ${lines.join(" · ")}` : "";
    this.msgEl.hidden = !lines.length;
  }

  #input(id) {
    const [b] = this.#blocksOf(id);
    return b ? this.#inputOf({ ...b, clip: id }) : null;
  }

  /** L'entrée de `setClips` / `updateClip` pour un morceau (ou pour un clip entier). */
  #inputOf(b) {
    const it = this.items.get(b.clip);
    const c = this.#clip(b.clip);
    const sel = b.indexMap ? toBlockIndices(it.sel, b.indexMap) : it.sel;
    const inp = scoreClipInput({ ...b.data, clip: b.mid, selection: sel }, { name: c?.clip.name ?? "", node: c?.node.id ?? null, lyrics: lyricsForClip(this.state.lyrics ?? [], b.data), startInBar: startInBarOf(b.data.start, this.state.project?.meters), presentation: { ...defaultPresentation(), staff: it.staff ?? "treble", keyFifths: this.#fifthsOf(b.clip, it, b.data), keyChanges: this.#keyChangesOf(it, b.data), ...(this.lyrLine ? { lyricsLine: true } : {}) } });
    // Avec un seul clip affiché, son nom sous chaque titre de section n'apprend rien : on le cache (avec plusieurs pistes il dit laquelle c'est).
    const named = this.ids.length > 1 || !b.section ? inp : { ...inp, hideName: true };
    return b.section ? { ...named, section: b.section } : named;
  }

  /** Change un réglage de vue (affichage seulement) : portée et armure pour le clip courant, grille pour tous ; et redessine. */
  #present(patch) {
    const it = this.items.get(this.cur);
    if (it) {
      if (patch.staff) it.staff = patch.staff;
    }
    if (!this.isOpen() || !this.shown) return;
    const redraw = patch.grid ? this.ids : [this.cur];
    for (const id of redraw) {
      this.#safeUpdate(id);
      this.#safeSelect(id);
    }
    this.#showMsgs();
    this.view.setNewNoteLength(this.grid.f);
  }

  #timing() {
    this.view?.setTiming(scoreTiming(this.state.project));
  }

  /** Les sections de la forme, nom au-dessus de la mesure de début (positions du projet). */
  #sections() {
    if (!this.view) return;
    this.#syncBlocksBtn();
    if (this.blocksOn && (this.state.structure ?? []).length) return this.view.setSections([]); // les titres viennent des morceaux (`clip.section`)
    this.view.setSections((this.state.structure ?? []).map((s) => ({ start: s.at, len: s.dur, name: s.name, ...(s.category ? { category: s.category } : {}), ...(s.color ? { color: s.color } : {}) })));
  }

  #syncBlocksBtn() {
    const any = (this.state.structure ?? []).length > 0;
    this.blocksBtn.hidden = !any;
    this.blocksBtn.setAttribute("aria-pressed", String(this.blocksOn));
  }

  #onEdit(raw) {
    // Un geste sur un morceau : on le ramène au vrai clip (même identifiant, indices de notes du clip entier).
    const map = this.blockMap.get(raw?.clip);
    const cmd = map ? { ...raw, clip: map.clip, ...(raw.indices ? { indices: toClipIndices(raw.indices, map.indexMap ?? raw.indices.map((i) => i)) } : {}), ...(Number.isInteger(raw.index) ? { index: map.indexMap ? map.indexMap[raw.index] : raw.index } : {}) } : raw;
    const it = this.items?.get(cmd?.clip);
    if (!this.isOpen() || !it?.data) return;
    this.#setCurrent(cmd.clip);
    if (cmd.type === "select") {
      it.sel = [...(cmd.indices ?? [])];
      this.#safeSelect(cmd.clip);
      this.#voiceButtons();
      this.#followSelection();
      return;
    }
    if (cmd.type === "preview") return this.#preview(cmd, it);
    if (cmd.type === "lyricTap") return this.#lyricTap(cmd, it);
    if (cmd.type === "delete") it.sel = [];
    const node = this.#clip(cmd.clip)?.node.id;
    for (const c of editToCommands(cmd, it.data, { node })) {
      if (c.type === "edit_notes" && cmd.type === "move") it.sel = c.edits.map((x) => x.index);
      this.client.send(c);
    }
    // Une durée posée à la souris ou au clavier devient la durée des prochaines notes ajoutées (comme dans le piano roll).
    if (cmd.type === "resize" && Array.isArray(cmd.len)) this.view.setNewNoteLength(cmd.len);
  }

  /**
   * Un tap sur la ligne de paroles : le module ne montre aucun champ, c'est le DAW qui ouvre le sien à `rect` (pixels de la fenêtre). Entrée valide
   * (texte vide : supprime la syllabe), Échap ou un clic ailleurs abandonne. Une commande `set_lyrics` annulable.
   */
  #lyricTap(cmd, it) {
    this.root.querySelector(".sc-lyrinput")?.remove();
    const zoom = Number(getComputedStyle(document.documentElement).getPropertyValue("--ui-zoom")) || 1;
    const r = cmd.rect ?? { left: 0, top: 0, width: 80, height: 24 };
    const input = document.createElement("input");
    input.type = "text";
    input.className = "sc-lyrinput";
    input.setAttribute("aria-label", "Syllabe");
    input.placeholder = "ma-";
    input.value = cmd.text ? `${cmd.text}${cmd.hyphen ? "-" : ""}` : "";
    const verse = cmd.verse ?? 1;
    Object.assign(input.style, { position: "fixed", left: `${r.left / zoom}px`, top: `${r.top / zoom}px`, width: `${Math.max(90, r.width) / zoom}px`, height: `${Math.max(28, r.height) / zoom}px`, zIndex: "10000" });
    let done = false;
    const close = () => {
      done = true;
      input.remove();
      this.view?.focus?.();
    };
    const commit = () => {
      if (done) return;
      const list = this.state.lyrics ?? [];
      const res = applyLyricTap(list, it.data, cmd, input.value);
      this.state.lyricVerse = verse;
      if (res) this.client.send({ type: "set_lyrics", syllables: res.syllables, label: res.label });
      close();
    };
    input.addEventListener("keydown", (k) => {
      k.stopPropagation();
      if (k.key === "Enter") commit();
      else if (k.key === "Escape") close();
    });
    input.addEventListener("blur", () => !done && commit());
    document.body.append(input);
    input.focus();
    input.select();
  }

  #preview(cmd, it) {
    const node = this.#clip(cmd.clip)?.node.id;
    if (node == null) return;
    const [out] = editToCommands(cmd, it.data, { node });
    if (!out) return;
    if (!cmd.on) return this.#release();
    this.#release();
    this.held = { node, key: cmd.key };
    this.client.send(out);
    this.heldTimer = setTimeout(() => this.#release(), 1500);
  }

  #release() {
    clearTimeout(this.heldTimer);
    if (this.held) this.client.send({ type: "preview_note", node: this.held.node, key: this.held.key, velocity: 0.8, on: false });
    this.held = null;
  }

  #seek([n, d]) {
    if (!this.isOpen() || !(d > 0)) return;
    this.client.send({ type: "seek", pos: Math.round(this.state.tempo.samplesAt(n / d)) });
  }

  // ---- taille de la portée ------------------------------------------------------------------------------------------------

  #step(dir) {
    const i = SCALES.findIndex((s) => s >= this.scale - 1e-6);
    return SCALES[Math.min(SCALES.length - 1, Math.max(0, (i < 0 ? SCALES.length - 1 : i) + dir))];
  }

  setScale(s) {
    this.scale = Math.min(3, Math.max(0.5, s));
    store.set("daw-score-scale", String(this.scale));
    this.view?.setStaffScale(this.scale);
    this.#showSize();
  }

  #pinchScale(factor) {
    this.scale = Math.min(3, Math.max(0.5, this.scale * factor));
    this.view?.setStaffScale(this.scale);
    this.#showSize();
  }

  #showSize() {
    this.sizeEl.textContent = `${Math.round(this.scale * 100)} %`;
  }

  /** Appelée à chaque image : la tête de lecture suit la position du moteur (le module ne redessine que si elle a changé). */
  draw() {
    this.layoutDock();
    if (!this.isOpen() || !this.view || !this.state.tempo) return;
    const kb = this.#keyBar();
    if (kb && kb.bar !== this.lastKeyBar) this.#syncKey(); // l'armure montrée suit la mesure de la tête de lecture
    const f = playheadFraction(this.state.tempo, this.state.pos);
    const key = `${f[0]}`;
    if (key === this.lastHead) return;
    this.lastHead = key;
    this.view.setPlayhead(f);
  }
}
