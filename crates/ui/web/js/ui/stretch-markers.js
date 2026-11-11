// Marqueurs d'étirement sur les clips audio de la timeline (à la Reaper) : on pose un marqueur (double-clic sur le clip), on le tire à droite
// ou à gauche : l'audio entre lui et ses voisins s'étire ou se comprime sans changer la hauteur. Le rendu a lieu **au relâchement** (un seul
// pas d'annulation) ; pendant le glissement la forme d'onde est déformée en aperçu. Un marqueur posé mais pas encore tiré n'est qu'un
// **brouillon** local (rien n'est envoyé, rien n'est rendu : à l'identique le son ne change pas).
//
// Cette classe tient l'état et le dessin ; la timeline lui passe les gestes (voir `timeline.js`). Calculs purs : `../stretch-model.js`.

import { sampleToX, xToSample, snapSample } from "../time.js";
import { ENGINES, ENGINE_LABELS, addMarker, dragBounds, moveMarker, plain, previewSegments, ratioText, ratios, removeMarker } from "../stretch-model.js";
import { clipRect } from "./clip-geometry.js";

const KEY_ENGINE = "daw-stretch-engine";
const MARKER_COLOR = "#ffd04d";

const get = (k) => {
  try {
    return localStorage.getItem(k);
  } catch {
    return null;
  }
};
const put = (k, v) => {
  try {
    localStorage.setItem(k, v);
  } catch {
    // Pas de mémorisation possible : le choix vaut pour cette session.
  }
};

/** Description d'un clip tel que le moteur la donne, avec l'indicateur « lissé » rangé en troisième élément de chaque marqueur. */
function baseOf(clip) {
  const st = clip.stretch;
  if (!st) return plain(clip.len);
  return { ...st, markers: st.markers.map((m, i) => [m[0], m[1], !!st.smooth?.[i]]) };
}

/** Commande `set_stretch` pour ces marqueurs (paires + indicateurs de lissage, ceux-ci omis quand aucun n'est lissé). */
function stretchMessage(clipId, markers, engine) {
  const msg = { type: "set_stretch", clip: clipId, markers: markers.map((m) => [m[0], m[1]]), engine };
  if (markers.some((m) => m[2])) msg.smooth = markers.map((m) => !!m[2]);
  return msg;
}

const sameMarkers = (a, b) => a.length === b.length && a.every((m, i) => m[0] === b[i][0] && m[1] === b[i][1] && !!m[2] === !!b[i][2]);

/** Vrai quand tous les rapports valent 1 et que la partie de source égale la longueur du clip : plus rien n'est étiré. */
function isIdentity(info, len) {
  return info.src_len === len && ratios(info, len).every((r) => Math.abs(r - 1) < 1e-9);
}

export class StretchMarkers {
  constructor(timeline) {
    this.tl = timeline;
    this.drag = null; // { clip, index, markers0, markers, x0, moved }
    this.draft = null; // { clipId, markers } : marqueurs posés ou tirés, pas encore (ou pas encore rendus) dans le clip
    this.sel = null; // { clipId, index } : marqueur sélectionné
    this.busy = null; // { clip, progress, name }
    this.engine = ENGINES.includes(get(KEY_ENGINE)) ? get(KEY_ENGINE) : "signalsmith";
    this.#bindUi();
  }

  #bindUi() {
    const select = document.getElementById("stretch-engine");
    if (select) {
      for (const e of ENGINES) select.append(new Option(ENGINE_LABELS[e], e));
      select.value = this.engine;
      select.addEventListener("change", () => {
        this.engine = select.value;
        put(KEY_ENGINE, this.engine);
        // Un clip étiré sélectionné est rerendu avec le nouveau moteur (mêmes marqueurs) : on compare à l'oreille.
        const clip = this.#selectedAudio();
        if (clip?.stretch) this.tl.client.send(stretchMessage(clip.id, baseOf(clip).markers, this.engine));
      });
    }
    const status = document.getElementById("stretch-status");
    status?.addEventListener("click", () => this.tl.client.send({ type: "cancel_stretch" }));
    this.tl.client.on("stretch", (e) => {
      if (e.active) this.busy = { clip: e.clip, progress: e.progress, name: e.name };
      else {
        this.busy = null;
        this.draft = null;
        if (e.error) this.tl.onNotice?.(`Étirement : ${e.error}`);
        else if (e.fell_back) this.tl.onNotice?.("Rubber Band a échoué : Signalsmith l'a remplacé.");
      }
      if (status) {
        status.hidden = !e.active;
        if (e.active) status.textContent = `Étirement ${Math.round((e.progress ?? 0) * 100)} % — ${e.name}`;
      }
      this.tl.dirty = true;
    });
    this.tl.client.on("error", (e) => {
      if (e.command === "set_stretch") {
        this.draft = null;
        this.tl.dirty = true;
      }
    });
    // Un nouveau projet ou un clip qui a changé ailleurs : le brouillon ne vaut plus.
    this.tl.client.on("project", () => {
      if (this.draft && !this.busy && !this.drag) {
        const clip = this.#clipById(this.draft.clipId);
        if (!clip) this.draft = null;
      }
      if (this.sel && !this.#clipById(this.sel.clipId)) this.sel = null;
    });
  }

  #clipById(id) {
    return this.tl.state.project?.nodes.flatMap((n) => n.clips).find((c) => c.id === id) ?? null;
  }

  #selectedAudio() {
    const c = this.#clipById(this.tl.state.selection);
    return c?.kind === "audio" ? c : null;
  }

  #rate() {
    return this.tl.state.project?.sample_rate ?? 48_000;
  }

  /** Description du clip (étiré ou non) avec ses marqueurs affichés : le brouillon et le glissement en cours l'emportent. */
  describe(clip) {
    const base = baseOf(clip);
    if (this.drag?.clip.id === clip.id) return { ...base, markers: this.drag.markers };
    if (this.draft?.clipId === clip.id) return { ...base, markers: this.draft.markers };
    return base;
  }

  /** Marqueur touché par la souris en (x, y) : `{ clip, index }` ou `null`. */
  hit(nodes, rowH, x, y) {
    const row = Math.floor(y / rowH);
    const node = nodes[row];
    if (!node?.clips) return null;
    const { view } = this.tl.state;
    const tol = window.matchMedia?.("(pointer: coarse)").matches ? 14 : 6;
    for (let i = node.clips.length - 1; i >= 0; i--) {
      const clip = node.clips[i];
      if (clip.kind !== "audio" || clip.muted) continue;
      const r = clipRect(view, rowH, row * rowH, clip);
      if (y < r.y || y > r.y + r.h || x < r.x0 - 2 || x > r.x1 + 2) continue;
      const markers = this.describe(clip).markers;
      let best = null;
      markers.forEach(([out], index) => {
        const d = Math.abs(sampleToX(view, clip.start + out) - x);
        if (d <= tol && (!best || d < best.d)) best = { clip, index, d };
      });
      if (best) return { clip: best.clip, index: best.index };
    }
    return null;
  }

  begin(hit, x) {
    this.sel = { clipId: hit.clip.id, index: hit.index };
    const markers0 = this.describe(hit.clip).markers.map((m) => [...m]);
    this.drag = { clip: hit.clip, index: hit.index, markers0, markers: markers0, x0: x, moved: false };
    this.tl.dirty = true;
  }

  move(x, shift) {
    const d = this.drag;
    if (!d) return;
    const { view, tempo, meters_ } = this.tl.state;
    let s = xToSample(view, x);
    if (!shift) s = snapSample(view, this.tl.width, tempo, meters_, s, 8);
    const base = { ...baseOf(d.clip), markers: d.markers0 };
    d.markers = moveMarker(base, d.clip.len, d.index, Math.round(s - d.clip.start));
    d.moved = d.moved || Math.abs(x - d.x0) > 3;
    this.tl.dirty = true;
  }

  /** Relâchement (`commit`) ou abandon. Envoie `set_stretch` seulement si le glissement a réellement changé quelque chose. */
  end(commit) {
    const d = this.drag;
    this.drag = null;
    this.tl.dirty = true;
    if (!d || !commit || !d.moved) return;
    this.#commit(d.clip, d.markers);
  }

  #commit(clip, markers) {
    const base = baseOf(clip);
    const info = { ...base, markers };
    if (sameMarkers(markers, base.markers)) {
      this.draft = null;
      return;
    }
    // Plus rien d'étiré : on le dit au moteur par une liste vide, il rend alors la source telle quelle (aucun rendu, aucune altération).
    const identity = isIdentity(info, clip.len);
    this.draft = { clipId: clip.id, markers };
    if (identity && !clip.stretch) return; // rien n'est étiré : le brouillon reste local, rien à envoyer
    const ok = this.tl.client.send(stretchMessage(clip.id, identity ? [] : markers, this.engine));
    if (!ok) this.draft = null;
  }

  /** Double-clic dans un clip audio : pose un marqueur (brouillon local) à cette position. Rend vrai si un marqueur a été posé. */
  addAt(clip, x, shift) {
    if (clip.kind !== "audio" || clip.muted) return false;
    const { view, tempo, meters_ } = this.tl.state;
    let s = xToSample(view, x);
    if (!shift) s = snapSample(view, this.tl.width, tempo, meters_, s, 8);
    const next = addMarker(this.describe(clip), clip.len, Math.round(s - clip.start), this.#rate());
    if (!next) {
      this.tl.onNotice?.("Trop près d'un bord ou d'un autre marqueur (10 ms au moins).");
      return false;
    }
    this.draft = { clipId: clip.id, markers: next };
    this.sel = { clipId: clip.id, index: next.findIndex((m) => !this.describe(clip).markers.includes(m)) };
    this.tl.dirty = true;
    return true;
  }

  /** Supprime un marqueur (double-clic dessus, ou Suppr s'il est sélectionné). */
  remove(clip, index) {
    const base = baseOf(clip);
    const shown = this.describe(clip);
    const gone = shown.markers[index];
    if (!gone) return;
    const next = removeMarker(shown, index);
    const committed = base.markers.some((m) => m[0] === gone[0] && m[1] === gone[1]);
    this.sel = null;
    if (!committed) {
      // Marqueur posé mais jamais tiré : il n'existait que dans le brouillon.
      this.draft = next.length || base.markers.length ? { clipId: clip.id, markers: next } : null;
      this.tl.dirty = true;
      return;
    }
    this.#commit(clip, next);
  }

  /** Lisse (ou non) le marqueur sélectionné : la vitesse d'étirement y varie progressivement au lieu de changer d'un coup. Rend vrai si traité. */
  toggleSmoothSelected() {
    if (!this.sel) return false;
    const clip = this.#clipById(this.sel.clipId);
    const shown = clip && this.describe(clip).markers;
    if (!clip || !shown?.[this.sel.index]) return false;
    const next = shown.map((m, i) => (i === this.sel.index ? [m[0], m[1], !m[2]] : m));
    this.#commit(clip, next);
    this.tl.dirty = true;
    return true;
  }

  /** Suppr : retire le marqueur sélectionné ; rend vrai si c'était le cas (la touche est alors consommée). */
  deleteSelected() {
    if (!this.sel) return false;
    const clip = this.#clipById(this.sel.clipId);
    if (!clip) {
      this.sel = null;
      return false;
    }
    this.remove(clip, this.sel.index);
    return true;
  }

  /** Échap : abandonne le glissement et le brouillon ; demande l'annulation d'un rendu en cours. */
  escape() {
    const had = this.drag || this.draft || this.busy || this.sel;
    this.drag = null;
    if (!this.busy) this.draft = null;
    this.sel = null;
    if (this.busy) this.tl.client.send({ type: "cancel_stretch" });
    this.tl.dirty = true;
    return !!had;
  }

  /** Marqueur sous le curseur : pour le curseur de la souris. */
  hovering(nodes, rowH, x, y) {
    return !!this.hit(nodes, rowH, x, y);
  }

  /**
   * Aperçu de la forme d'onde déformée (pendant un glissement, puis jusqu'à la fin du rendu) : tronçons `{ from, to }` en échantillons
   * **absolus** de la ligne de temps, ou `null` quand rien n'est déformé.
   */
  warpFor(clip) {
    const pending = this.drag?.clip.id === clip.id ? this.drag.markers : this.draft?.clipId === clip.id ? this.draft.markers : null;
    if (!pending) return null;
    const base = baseOf(clip);
    if (sameMarkers(pending, base.markers)) return null;
    return previewSegments(base, clip.len, pending).map((s) => ({ from: [clip.start + s.from[0], clip.start + s.from[1]], to: [clip.start + s.to[0], clip.start + s.to[1]] }));
  }

  /** Dessine les marqueurs, le rapport de chaque tronçon et l'état d'un rendu en cours, sur le corps du clip. */
  draw(g, clip, geom) {
    const info = this.describe(clip);
    const { view } = this.tl.state;
    const { x0, x1, y, h, bodyTop, bodyH } = geom;
    if (clip.kind !== "audio") return;
    const w = x1 - x0;
    // Teinte des tronçons étirés (bleu) ou comprimés (rouge) et leur rapport.
    if (info.markers.length || info.src_len !== clip.len) {
      const k = [0, ...info.markers.map((m) => m[0]), clip.len];
      const r = ratios(info, clip.len);
      g.font = "600 10px system-ui, sans-serif";
      g.textBaseline = "middle";
      g.textAlign = "center";
      for (let i = 0; i < r.length; i++) {
        const a = sampleToX(view, clip.start + k[i]);
        const b = sampleToX(view, clip.start + k[i + 1]);
        if (Math.abs(r[i] - 1) > 0.005) {
          g.fillStyle = r[i] > 1 ? "#4da3ff26" : "#ff7a4d26";
          g.fillRect(a, bodyTop, b - a, bodyH);
        }
        if (b - a > 54 && (info.markers.length || r[i] !== 1)) {
          const label = ratioText(r[i]);
          const lw = g.measureText(label).width + 8;
          g.fillStyle = "#000000a0";
          g.fillRect((a + b) / 2 - lw / 2, bodyTop + bodyH - 15, lw, 12);
          g.fillStyle = Math.abs(r[i] - 1) > 0.005 ? "#fff" : "#ffffffaa";
          g.fillText(label, (a + b) / 2, bodyTop + bodyH - 9);
        }
      }
      g.textAlign = "start";
    }
    // Les marqueurs : un trait sur toute la hauteur du clip et une poignée en triangle au sommet du corps.
    const committed = clip.stretch?.markers ?? [];
    const smoothAt = (i) => !!info.markers[i]?.[2];
    info.markers.forEach((m, index) => {
      const mx = sampleToX(view, clip.start + m[0]);
      if (mx < x0 - 1 || mx > x1 + 1) return;
      const isDraft = !committed.some((c) => c[0] === m[0] && c[1] === m[1]);
      const selected = this.sel?.clipId === clip.id && this.sel.index === index;
      g.strokeStyle = MARKER_COLOR;
      g.lineWidth = selected || this.drag?.index === index ? 2.5 : 1.5;
      g.setLineDash(isDraft ? [4, 3] : []);
      g.beginPath();
      g.moveTo(Math.round(mx) + 0.5, y);
      g.lineTo(Math.round(mx) + 0.5, y + h);
      g.stroke();
      g.setLineDash([]);
      g.fillStyle = MARKER_COLOR;
      g.beginPath();
      if (smoothAt(index)) {
        g.arc(mx, bodyTop + 5, 5.5, 0, Math.PI * 2); // rond : marqueur lissé (la vitesse y change progressivement)
      } else {
        g.moveTo(mx - 5.5, bodyTop);
        g.lineTo(mx + 5.5, bodyTop);
        g.lineTo(mx, bodyTop + 8);
        g.closePath();
      }
      g.fill();
    });
    // Rendu en cours : le clip est voilé et dit où il en est.
    if (this.busy?.clip === clip.id) {
      g.fillStyle = "#0000008c";
      g.fillRect(x0, bodyTop, w, bodyH);
      if (w > 90) {
        g.fillStyle = "#fff";
        g.font = "600 11px system-ui, sans-serif";
        g.textBaseline = "middle";
        g.textAlign = "center";
        g.fillText(`Étirement… ${Math.round((this.busy.progress ?? 0) * 100)} %`, (x0 + x1) / 2, bodyTop + bodyH / 2);
        g.textAlign = "start";
      }
    }
  }

  /** Bornes d'un marqueur pendant son glissement, en positions absolues : pour afficher les butées si on veut. */
  boundsOf(clip, index) {
    const b = dragBounds(this.describe(clip), clip.len, index);
    return b ? [clip.start + b[0], clip.start + b[1]] : null;
  }
}
