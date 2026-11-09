// Réserve de paroles : les syllabes d'un couplet en pastilles. On en glisse une sur la partition pour la poser sur une note (le module de
// Scorsaire dit quelle colonne est sous le doigt : `view.positionAt`, et montre où elle tombera : `view.setDropMarker`), ou bien on touche une
// pastille puis la ligne de paroles sous la note voulue (`lyricTap`, au doigt). Les syllabes posées se voient tout de suite sous les notes
// (brouillon donné au module) ; rien n'est écrit dans le projet avant « Appliquer » (une seule action annulable), « Annuler » laisse tout intact.

import { fadd, fsub } from "../notes-model.js";
import { chipsFromLyrics, chipsFromText, chipsToSyllables, clearVerseRange, lyricsToText, nearestOnset, noteOnsets, noteSpans, placeChip, unplaceChip, verseCount } from "../lyrics-model.js";
import { fcmp } from "../notes-model.js";

const DRAG_PX = 6;

export class LyricReserve {
  /**
   * `host` : { clip(): { id, d } | null (clip courant et ses données), lyrics(): syllabes du projet, view(): module de partition,
   * resolve(res): { clip, at } | null (le résultat de `positionAt` ramené au vrai clip, position absolue), setDraft(list | null),
   * commit(syllabes), verse(): couplet proposé, zoom(): zoom d'interface }.
   */
  constructor(panel, host) {
    this.panel = panel;
    this.host = host;
    this.active = false;
    this.chips = [];
    this.sel = null;
    this.verse = 1;
    this.clipId = null;
    this.drag = null;
    panel.innerHTML = `
      <div class="lr-row">
        <strong>Réserve de paroles</strong>
        <label>Couplet <select class="lr-verse" aria-label="Couplet"></select></label>
        <span class="lr-btns"><button class="lr-split" title="Découpe le texte en pastilles (celles déjà posées sont reprises à zéro). Un « _ » est une pastille mélisme.">Découper</button><button class="lr-melisma" title="Ajoute une pastille mélisme « _ » (après la pastille choisie, sinon à la fin) : posée sur une note, elle prolonge la syllabe qui la précède jusqu'à la fin de cette note">＋ Mélisme</button><button class="lr-clear" title="Remet toutes les pastilles dans la réserve">Tout retirer</button><button class="lr-remove" title="Remet la pastille choisie dans la réserve">Retirer</button><button class="lr-apply">Appliquer</button><button class="lr-cancel">Annuler</button></span>
      </div>
      <textarea class="lr-text" rows="2" placeholder="ma-man est la plus bel-le" aria-label="Texte du couplet"></textarea>
      <div class="lr-chips" role="listbox" aria-label="Syllabes"></div>
      <div class="lr-hint"></div>`;
    this.verseSel = panel.querySelector(".lr-verse");
    this.text = panel.querySelector(".lr-text");
    this.chipsEl = panel.querySelector(".lr-chips");
    this.hint = panel.querySelector(".lr-hint");
    panel.querySelector(".lr-split").addEventListener("click", () => this.#split());
    panel.querySelector(".lr-melisma").addEventListener("click", () => this.#addMelisma());
    panel.querySelector(".lr-clear").addEventListener("click", () => this.#setChips(this.chips.map((_, i) => i).reduce((acc, i) => unplaceChip(acc, i), this.chips), null));
    panel.querySelector(".lr-remove").addEventListener("click", () => this.sel != null && this.#setChips(unplaceChip(this.chips, this.sel), this.sel));
    panel.querySelector(".lr-apply").addEventListener("click", () => this.apply());
    panel.querySelector(".lr-cancel").addEventListener("click", () => this.close());
    this.verseSel.addEventListener("change", () => this.#loadVerse(Number(this.verseSel.value)));
    this.chipsEl.addEventListener("pointerdown", (e) => this.#down(e));
    this.chipsEl.addEventListener("pointermove", (e) => this.#move(e));
    this.chipsEl.addEventListener("pointerup", (e) => this.#up(e, false));
    this.chipsEl.addEventListener("pointercancel", (e) => this.#up(e, true));
    panel.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Escape") this.close();
    });
  }

  open() {
    const c = this.host.clip();
    if (!c) return;
    this.active = true;
    this.clipId = c.id;
    this.panel.hidden = false;
    const n = Math.max(2, verseCount(this.host.lyrics()) + 1);
    this.verseSel.replaceChildren(...Array.from({ length: Math.min(8, n) }, (_, i) => new Option(`Couplet ${i + 1}`, String(i + 1))));
    this.#loadVerse(Math.min(8, this.host.verse()));
  }

  close() {
    this.#endDrag();
    this.active = false;
    this.panel.hidden = true;
    this.host.setDraft(null);
  }

  /** Le clip courant a changé : la réserve repart du texte posé sur le nouveau clip. */
  refresh() {
    if (!this.active) return;
    const c = this.host.clip();
    if (!c) return this.close();
    if (c.id !== this.clipId) {
      this.clipId = c.id;
      this.#loadVerse(this.verse);
    }
  }

  #loadVerse(v) {
    const c = this.host.clip();
    if (!c) return;
    this.verse = v;
    this.verseSel.value = String(v);
    const from = c.d.start;
    const to = fadd(c.d.start, c.d.len);
    const chips = chipsFromLyrics(this.host.lyrics(), v, from, to, noteOnsets(c.d));
    this.text.value = lyricsToText(this.host.lyrics().filter((s) => fcmp(s.at, from) >= 0 && fcmp(s.at, to) < 0), v, noteOnsets(c.d));
    this.#setChips(chips, null, false);
  }

  #split() {
    const chips = chipsFromText(this.text.value);
    if (!chips.length) return this.#say("Écris au moins une syllabe.");
    this.#setChips(chips, 0);
  }

  /** Une pastille mélisme de plus, juste après la pastille choisie (sinon à la fin) ; elle est choisie pour être posée tout de suite. */
  #addMelisma() {
    const at = this.sel != null ? this.sel + 1 : this.chips.length;
    const chips = [...this.chips.slice(0, at), { text: "_", extend: true, hyphen: false, at: null }, ...this.chips.slice(at)];
    this.#setChips(chips, at);
  }

  #setChips(chips, sel, draft = true) {
    this.chips = chips;
    this.sel = sel != null && chips[sel] ? sel : null;
    this.#render();
    if (draft) this.#pushDraft();
  }

  /** Le brouillon que le module dessine : le projet, avec le couplet remplacé sur la plage du clip par les pastilles posées. */
  #pushDraft() {
    const c = this.host.clip();
    if (!c) return;
    const base = clearVerseRange(this.host.lyrics(), this.verse, c.d.start, fadd(c.d.start, c.d.len));
    this.host.setDraft([...base, ...chipsToSyllables(this.chips, this.verse, noteSpans(c.d))].sort((a, b) => fcmp(a.at, b.at) || (a.verse ?? 1) - (b.verse ?? 1)));
  }

  apply() {
    const c = this.host.clip();
    if (!c) return;
    const base = clearVerseRange(this.host.lyrics(), this.verse, c.d.start, fadd(c.d.start, c.d.len));
    const placed = chipsToSyllables(this.chips, this.verse, noteSpans(c.d));
    if (!placed.length && base.length === this.host.lyrics().length) return this.close();
    this.host.commit([...base, ...placed].sort((a, b) => fcmp(a.at, b.at) || (a.verse ?? 1) - (b.verse ?? 1)), this.verse);
    this.close();
  }

  #render() {
    this.chipsEl.replaceChildren(
      ...this.chips.map((c, i) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = `lr-chip${c.at ? " placed" : ""}${i === this.sel ? " sel" : ""}${c.extend ? " ext" : ""}`;
        if (c.extend) b.title = "Mélisme : prolonge la syllabe précédente jusqu'à la fin de la note où on la pose";
        b.dataset.k = String(i);
        b.setAttribute("role", "option");
        b.setAttribute("aria-selected", String(i === this.sel));
        b.textContent = c.extend ? "_ mélisme" : `${c.text}${c.hyphen ? "-" : ""}`;
        return b;
      }),
    );
    const waiting = this.chips.filter((c) => !c.at).length;
    this.panel.querySelector(".lr-remove").disabled = this.sel == null || !this.chips[this.sel]?.at;
    this.#say(
      this.chips.length
        ? `${this.chips.length - waiting} posée${this.chips.length - waiting > 1 ? "s" : ""}, ${waiting} en attente. Glisse une pastille sur une note, ou touche-la puis touche la ligne de paroles sous la note (le trait orange montre où elle tombe).`
        : "Écris le texte du couplet puis « Découper », ou ajoute-le depuis « Paroles… ».",
    );
  }

  #say(t) {
    this.hint.textContent = t;
  }

  /** Une pastille posée sur la note de `res` (résultat de `positionAt` ou instant donné) : refus avec un message si l'ordre des mots l'interdit. */
  #drop(k, at) {
    const c = this.host.clip();
    if (!c) return false;
    const onsets = noteOnsets(c.d);
    const to = nearestOnset(onsets, at);
    if (!to) {
      this.#say("Ce clip n'a pas de note à laquelle accrocher une syllabe.");
      return false;
    }
    const next = placeChip(this.chips, k, to, onsets);
    if (!next) {
      this.#say("L'ordre des mots doit être respecté : une syllabe ne passe pas devant la précédente, et celles qui suivent doivent trouver une note libre.");
      return false;
    }
    const nextWaiting = next.findIndex((ch, i) => i > k && !ch.at);
    this.#setChips(next, nextWaiting >= 0 ? nextWaiting : null);
    return true;
  }

  /** Un tap sur la ligne de paroles du module (`lyricTap`) : pose la pastille choisie sur la note la plus proche, ou choisit la pastille touchée. */
  onTap(tap, d) {
    const at = fadd(d.start, fsub(tap.start, d.offset));
    if (this.sel != null) return this.#drop(this.sel, at);
    const k = this.chips.findIndex((c) => c.at && fcmp(c.at, nearestOnset(noteOnsets(d), at) ?? at) === 0);
    if (k >= 0) this.#setChips(this.chips, k, false);
    return k >= 0;
  }

  // ---- glisser (souris et doigt) -------------------------------------------------------------------------------------------------

  #down(e) {
    const el = e.target.closest?.(".lr-chip");
    if (!el || e.button > 0) return;
    this.chipsEl.setPointerCapture?.(e.pointerId);
    this.drag = { k: Number(el.dataset.k), id: e.pointerId, x: e.clientX, y: e.clientY, moved: false, ghost: null, target: null };
    e.preventDefault();
  }

  #move(e) {
    const g = this.drag;
    if (!g || e.pointerId !== g.id) return;
    if (!g.moved && Math.hypot(e.clientX - g.x, e.clientY - g.y) < DRAG_PX) return;
    if (!g.moved) {
      g.moved = true;
      g.ghost = document.createElement("div");
      g.ghost.className = "lr-chip lr-ghost";
      g.ghost.textContent = this.chips[g.k].extend ? "_ mélisme" : `${this.chips[g.k].text}${this.chips[g.k].hyphen ? "-" : ""}`;
      document.body.append(g.ghost);
    }
    const z = this.host.zoom();
    g.ghost.style.left = `${e.clientX / z + 8}px`;
    g.ghost.style.top = `${e.clientY / z + 8}px`;
    const view = this.host.view();
    let res = null;
    try {
      res = view?.positionAt?.(e.clientX, e.clientY, { snap: "note" }) ?? null;
    } catch {
      res = null;
    }
    g.target = res ? { raw: res, ...(this.host.resolve(res) ?? {}) } : null;
    if (g.target && g.target.at) view?.setDropMarker?.({ clip: res.clip, start: res.start });
    else view?.setDropMarker?.(null);
  }

  #up(e, cancelled) {
    const g = this.drag;
    if (!g || e.pointerId !== g.id) return;
    this.chipsEl.releasePointerCapture?.(e.pointerId);
    const { k, moved, target } = g;
    this.#endDrag();
    if (cancelled) return;
    if (!moved) this.#setChips(this.chips, k === this.sel ? null : k, false); // un tap choisit (ou relâche) la pastille
    else if (target?.at) this.#drop(k, target.at);
  }

  #endDrag() {
    this.drag?.ghost?.remove();
    this.host.view()?.setDropMarker?.(null);
    this.drag = null;
  }
}
