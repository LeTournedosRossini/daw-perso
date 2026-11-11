// Barre de transport : boutons et affichage de la position (mesures et temps), tempo, mesure, latence.

import { formatBBT, formatTime } from "../time.js";
import { SNAP_MODES, getSnapMode, setSnapMode, onSnapChange } from "../snap.js";
import { openTempoForm, openMeterForm } from "./time-menu.js";
import { openMetronomeForm, preRollText, DEFAULT_METRONOME } from "./metronome-menu.js";

export class Transport {
  constructor(state, client) {
    this.state = state;
    this.client = client;
    this.$ = (id) => document.getElementById(id);
    this.$("btn-play").addEventListener("click", () => client.send({ type: "transport", action: "toggle" }));
    this.$("btn-stop").addEventListener("click", () => {
      client.send({ type: "transport", action: "stop" });
    });
    this.$("btn-rewind").addEventListener("click", () => client.send({ type: "seek", pos: 0 }));
    this.$("btn-loop").addEventListener("click", () => this.toggleLoop());
    this.$("btn-panic").addEventListener("click", () => client.send({ type: "panic" }));
    this.$("disp-tempo").addEventListener("click", (e) => openTempoForm(e.currentTarget, state, client));
    this.$("disp-meter").addEventListener("click", (e) => openMeterForm(e.currentTarget, state, client));
    this.$("btn-record").addEventListener("click", () => this.toggleRecord());
    this.$("btn-punch").addEventListener("click", () => this.togglePunch());
    this.$("btn-looprec").addEventListener("click", () => this.toggleLoopRecord());
    this.$("btn-metro").addEventListener("click", () => this.toggleMetronome());
    this.$("btn-metro-opts").addEventListener("click", (e) => openMetronomeForm(e.currentTarget, state, client));
    this.$("btn-undo").addEventListener("click", () => client.send({ type: "undo" }));
    this.$("btn-redo").addEventListener("click", () => client.send({ type: "redo" }));
    this.$("btn-split").addEventListener("click", () => this.splitSelected());
    this.$("btn-delete").addEventListener("click", () => this.deleteSelected());
    const snap = this.$("snap-grid");
    for (const [m, label] of SNAP_MODES) snap.append(new Option(m === "auto" ? "Aimant. : auto" : m === "off" ? "Aimant. : non" : label, m));
    snap.value = getSnapMode();
    snap.addEventListener("change", () => setSnapMode(snap.value));
    onSnapChange((m) => (snap.value = m));
    this.$("btn-xfade").addEventListener("click", () => client.send({ type: "set_auto_crossfade", enabled: !this.state.project.auto_crossfade }));
  }

  /** Allume ou éteint le punch : R n'enregistre alors que dans la zone bleue de la règle (la même que la boucle). */
  togglePunch() {
    this.client.send({ type: "set_punch", enabled: !this.state.punchOn });
  }

  /** Enregistrement en boucle : la zone bleue tourne pendant la prise, un passage = une prise du groupe (exclusif avec Punch). */
  toggleLoopRecord() {
    this.client.send({ type: "set_loop_record", enabled: !this.state.loopRecOn });
  }

  updateLoopRecord() {
    const on = !!this.state.loopRecOn;
    const btn = this.$("btn-looprec");
    btn.setAttribute("aria-pressed", String(on));
    btn.title = `Enregistrement en boucle ${on ? "allumé : R enregistre en tournant dans la zone bleue ; chaque passage donne une prise du groupe (▤ sous la piste), la dernière est active" : "éteint"}`;
  }

  updatePunch() {
    const on = !!this.state.punchOn;
    const btn = this.$("btn-punch");
    btn.setAttribute("aria-pressed", String(on));
    btn.title = `Punch ${on ? "allumé : R enregistre exactement dans la zone bleue (pose-la dans la règle), la boucle est suspendue pendant la prise" : "éteint : R enregistre depuis la tête de lecture jusqu'à l'arrêt"} — touche P`;
  }

  toggleMetronome() {
    const cur = { ...DEFAULT_METRONOME, ...this.state.metronome };
    this.client.send({ ...cur, type: "set_metronome", enabled: !cur.enabled });
  }

  updateMetronome() {
    const m = { ...DEFAULT_METRONOME, ...this.state.metronome };
    const btn = this.$("btn-metro");
    btn.setAttribute("aria-pressed", String(m.enabled));
    const detail = [m.only_recording ? "seulement à l'enregistrement" : null, m.count_in_bars ? `pré-compte ${m.count_in_bars} mes.` : null, preRollText(m) ? `pré-roll ${preRollText(m)}` : null, m.post_roll ? `post-roll ${m.post_roll} s` : null].filter(Boolean).join(", ");
    btn.title = `Métronome ${m.enabled ? "activé" : "désactivé"}${detail ? ` (${detail})` : ""}`;
  }

  /** Lance ou arrête l'enregistrement. Pour lancer : le projet doit avoir un dossier (on le demande au besoin). */
  async toggleRecord() {
    if (this.state.recording?.active) {
      this.client.send({ type: "stop_record" });
      return;
    }
    if (this.files && !(await this.files.ensureSaved())) return;
    this.client.send({ type: "record" });
  }

  /** Voyant et bouton d'enregistrement d'après l'état reçu du moteur. */
  updateRecording() {
    const r = this.state.recording ?? { active: false, stopping: false, overruns: 0 };
    const btn = this.$("btn-record");
    btn.setAttribute("aria-pressed", String(r.active));
    btn.title = r.active ? "Arrêter l'enregistrement (R, ou Espace)" : "Enregistrer les pistes armées (R)";
    const tag = this.$("rec-status");
    tag.hidden = !r.active;
    tag.classList.toggle("warn", r.overruns > 0);
    tag.textContent = r.stopping ? "Écriture des prises…" : r.overruns > 0 ? `● REC — ${r.overruns} bloc(s) perdu(s)` : "● REC";
  }

  /** Découpe le clip sélectionné à la position de la tête de lecture. */
  splitSelected() {
    const ids = [...(this.state.selected ?? [])];
    const at = Math.round(this.state.pos);
    if (ids.length > 1) this.client.send({ type: "split_clips", clips: ids, at }); // tous ceux que la tête de lecture traverse
    else if (this.state.selection !== null) this.client.send({ type: "split_clip", clip: this.state.selection, at: this.state.pos });
  }

  deleteSelected() {
    const ids = [...(this.state.selected ?? [])];
    if (ids.length > 1) this.client.send({ type: "delete_clips", clips: ids });
    else if (this.state.selection !== null) this.client.send({ type: "delete_clip", clip: this.state.selection });
  }

  /** Copie (et coupe, si `cut`) les clips sélectionnés. */
  copySelected(cut = false) {
    const ids = [...(this.state.selected ?? [])];
    if (!ids.length) return;
    this.client.send({ type: "copy_clips", clips: ids });
    if (cut) this.client.send({ type: "delete_clips", clips: ids });
  }

  /** Colle à la tête de lecture ; chaque clip retrouve sa piste d'origine. */
  pasteClips() {
    // Une piste désignée (clic dans son vide) reçoit le collage ; sinon chaque clip retrouve sa piste d'origine.
    const node = this.state.activeTrack;
    this.client.send(node != null ? { type: "paste_clips", at: Math.round(this.state.pos), node } : { type: "paste_clips", at: Math.round(this.state.pos) });
  }

  duplicateSelected() {
    const ids = [...(this.state.selected ?? [])];
    if (ids.length) this.client.send({ type: "duplicate_clips", clips: ids });
  }

  /** Met à jour les boutons d'édition (annuler / rétablir avec le nom de l'action, découper / supprimer selon la sélection). */
  updateEditing() {
    const h = this.state.history;
    const set = (id, enabled, title) => {
      const b = this.$(id);
      b.disabled = !enabled;
      b.title = title;
    };
    set("btn-undo", h.can_undo, h.can_undo ? `Annuler : ${h.undo_label} (Ctrl+Z)` : "Annuler (Ctrl+Z)");
    set("btn-redo", h.can_redo, h.can_redo ? `Rétablir : ${h.redo_label} (Ctrl+Maj+Z)` : "Rétablir (Ctrl+Maj+Z)");
    const x = this.$("btn-xfade");
    x.setAttribute("aria-pressed", String(!!this.state.project.auto_crossfade));
    x.title = this.state.project.auto_crossfade ? "Fondus enchaînés automatiques : activés (cliquer pour désactiver)" : "Fondus enchaînés automatiques : désactivés (cliquer pour activer)";
    const selected = this.state.selection !== null;
    set("btn-split", selected, "Découper les clips sélectionnés à la tête de lecture (S)");
    set("btn-delete", selected, "Supprimer le clip sélectionné (Suppr). Sélection multiple : Ctrl/Maj + clic ou rectangle ; Ctrl+C / X / V copie, coupe, colle (clic dans le vide d'une piste : la désigne comme destination) ; Ctrl+D ou Alt + glisser duplique ; S découpe tous les clips sélectionnés.");
  }

  toggleLoop() {
    const { project, loop } = this.state;
    if (loop) {
      this.client.send({ type: "loop", start: null, end: null });
    } else {
      // Boucle par défaut : la mesure 1 à la mesure 5 (4 mesures).
      const bar = (b) => Math.round(this.state.tempo.samplesAt(this.state.meters_.barStart(b)));
      this.client.send({ type: "loop", start: bar(0), end: bar(4) });
    }
    void project;
  }

  update() {
    const s = this.state;
    const playing = s.playing;
    this.$("btn-play").setAttribute("aria-pressed", String(playing));
    this.$("btn-play").setAttribute("aria-label", playing ? "Pause" : "Lecture");
    this.$("play-icon").setAttribute("d", playing ? "M6 5h4v14H6zM14 5h4v14h-4z" : "M7 4v16l13-8z");
    this.$("btn-loop").setAttribute("aria-pressed", String(!!s.loop));
    this.$("disp-bbt").textContent = formatBBT(s.pos, s.tempo, s.meters_);
    this.$("disp-time").textContent = formatTime(s.pos, s.project.sample_rate);
    this.$("disp-tempo").textContent = String(Math.round(s.tempo.bpmAt(s.pos) * 100) / 100);
    const bar = s.meters_.locate(Math.max(0, s.tempo.wholeAt(s.pos))).bar;
    const m = s.meters_.meterAtBar(bar);
    this.$("disp-meter").textContent = `${m.num}/${m.den}`;
    const ms = (s.project.latency_samples / s.project.sample_rate) * 1000;
    this.$("disp-latency").textContent = `${ms.toFixed(1)} ms`;
  }
}
