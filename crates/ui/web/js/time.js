// Temps musical, tempo, zoom et grille — pur JavaScript, sans DOM (testé avec `node --test`).
// Ici tout sert à l'AFFICHAGE : les flottants suffisent. Le moteur, lui, calcule en rationnels exacts.

export const TICKS_PER_QUARTER = 960;

/** Fraction de ronde `[n, d]` → nombre. */
export const frac = ([n, d]) => n / d;

/** Carte des tempos : conversion ronde ↔ échantillons par paliers. */
export class TempoMap {
  constructor(changes, sampleRate) {
    this.sampleRate = sampleRate;
    const sorted = [...changes].sort((a, b) => frac(a.pos) - frac(b.pos));
    this.segments = [];
    let samples = 0;
    for (let i = 0; i < sorted.length; i++) {
      const start = frac(sorted[i].pos);
      const spw = (sampleRate * 240 * sorted[i].den) / sorted[i].num; // échantillons par ronde
      if (i > 0) {
        const prev = this.segments[i - 1];
        samples = prev.samples + (start - prev.start) * prev.spw;
      }
      this.segments.push({ start, samples, spw, bpm: sorted[i].num / sorted[i].den });
    }
  }

  samplesAt(whole) {
    const i = this.#byWhole(whole);
    const s = this.segments[i];
    return s.samples + (whole - s.start) * s.spw;
  }

  wholeAt(sample) {
    let i = this.segments.length - 1;
    while (i > 0 && this.segments[i].samples > sample) i--;
    const s = this.segments[i];
    return s.start + (sample - s.samples) / s.spw;
  }

  bpmAt(sample) {
    return this.segments[this.#segmentBySample(sample)].bpm;
  }

  #byWhole(whole) {
    let i = this.segments.length - 1;
    while (i > 0 && this.segments[i].start > whole) i--;
    return i;
  }

  #segmentBySample(sample) {
    let i = this.segments.length - 1;
    while (i > 0 && this.segments[i].samples > sample) i--;
    return i;
  }
}

/** Carte des chiffrages (changements en début de mesure). */
export class MeterMap {
  constructor(changes) {
    const sorted = [...changes].sort((a, b) => a.bar - b.bar);
    this.segments = [];
    let start = 0;
    for (let i = 0; i < sorted.length; i++) {
      if (i > 0) {
        const prev = sorted[i - 1];
        start += (sorted[i].bar - prev.bar) * (prev.num / prev.den);
      }
      this.segments.push({ bar: sorted[i].bar, start, num: sorted[i].num, den: sorted[i].den });
    }
  }

  #seg(bar) {
    let i = this.segments.length - 1;
    while (i > 0 && this.segments[i].bar > bar) i--;
    return this.segments[i];
  }

  meterAtBar(bar) {
    const s = this.#seg(bar);
    return { num: s.num, den: s.den };
  }

  /** Position (en rondes) du début de la mesure `bar` (0 = première). */
  barStart(bar) {
    const s = this.#seg(bar);
    return s.start + (bar - s.bar) * (s.num / s.den);
  }

  /** Mesure qui contient la position `whole`, et position dans la mesure (en rondes). */
  locate(whole) {
    let i = this.segments.length - 1;
    while (i > 0 && this.segments[i].start > whole) i--;
    const s = this.segments[i];
    const len = s.num / s.den;
    const k = Math.floor((whole - s.start) / len + 1e-12);
    return { bar: s.bar + k, within: whole - (s.start + k * len) };
  }
}

/** Fenêtre de la timeline : `start` = premier échantillon visible, `spp` = échantillons par pixel. */
export function sampleToX(vp, sample) {
  return (sample - vp.start) / vp.spp;
}
export function xToSample(vp, x) {
  return vp.start + x * vp.spp;
}

/** Zoom centré sur le pixel `anchorX` : l'échantillon sous le curseur reste sous le curseur. */
export function zoomAt(vp, anchorX, factor, limits) {
  const spp = Math.min(limits.maxSpp, Math.max(limits.minSpp, vp.spp * factor));
  const anchor = xToSample(vp, anchorX);
  return { start: Math.max(0, anchor - anchorX * spp), spp };
}

/** Ramène la fenêtre dans les bornes : pas de début négatif, pas au-delà de `length` + une marge. */
export function clampView(vp, widthPx, length) {
  const maxStart = Math.max(0, length - widthPx * vp.spp * 0.25);
  return { spp: vp.spp, start: Math.min(maxStart, Math.max(0, vp.start)) };
}

/**
 * Lignes de grille visibles : `{ x, kind: "bar"|"beat"|"sub", label? }`. Le niveau de détail dépend de l'espace
 * disponible : les temps n'apparaissent que s'ils sont à ≥ 14 px les uns des autres, les subdivisions à ≥ 60 px par
 * temps, et les numéros de mesure sont espacés pour ne jamais se chevaucher (≥ 44 px entre deux étiquettes).
 */
export function gridLines(vp, widthPx, tempo, meters) {
  const out = [];
  const s0 = vp.start;
  const s1 = vp.start + widthPx * vp.spp;
  const w0 = Math.max(0, tempo.wholeAt(s0));
  const w1 = tempo.wholeAt(s1);
  const firstBar = meters.locate(w0).bar;
  const barPx = (bar) => {
    const a = meters.barStart(bar);
    const b = meters.barStart(bar + 1);
    return (tempo.samplesAt(b) - tempo.samplesAt(a)) / vp.spp;
  };
  const refBar = barPx(firstBar);
  let every = 1;
  while (refBar * every < 44 && every < 4096) every *= 2;
  for (let bar = firstBar; ; bar++) {
    const bw = meters.barStart(bar);
    if (bw > w1) break;
    const m = meters.meterAtBar(bar);
    const beat = 1 / m.den;
    const beatPx = (tempo.samplesAt(bw + beat) - tempo.samplesAt(bw)) / vp.spp;
    for (let k = 0; k < m.num; k++) {
      const w = bw + k * beat;
      const x = sampleToX(vp, tempo.samplesAt(w));
      if (x < -1 || x > widthPx + 1) continue;
      if (k === 0) {
        out.push({ x, kind: "bar", label: bar % every === 0 ? String(bar + 1) : undefined });
      } else if (beatPx >= 14) {
        out.push({ x, kind: "beat" });
      }
      if (beatPx >= 60) {
        for (let j = 1; j < 4; j++) {
          const xs = sampleToX(vp, tempo.samplesAt(w + (beat * j) / 4));
          if (xs >= -1 && xs <= widthPx + 1) out.push({ x: xs, kind: "sub" });
        }
      }
    }
  }
  return out;
}

/** Position affichée « mesure.temps.tick » (à partir de 1). */
export function formatBBT(sample, tempo, meters) {
  const whole = Math.max(0, tempo.wholeAt(sample));
  const { bar, within } = meters.locate(whole);
  const m = meters.meterAtBar(bar);
  const beat = 1 / m.den;
  const k = Math.min(m.num - 1, Math.floor(within / beat + 1e-9));
  const ticksPerBeat = (TICKS_PER_QUARTER * 4) / m.den;
  const tick = Math.min(ticksPerBeat - 1, Math.floor((within - k * beat) / beat * ticksPerBeat + 1e-6));
  return `${bar + 1}.${k + 1}.${String(tick).padStart(3, "0")}`;
}

/** Temps écoulé « mm:ss.mmm ». */
export function formatTime(sample, sampleRate) {
  const ms = Math.round((sample / sampleRate) * 1000);
  const m = Math.floor(ms / 60000);
  const s = Math.floor((ms % 60000) / 1000);
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}.${String(ms % 1000).padStart(3, "0")}`;
}

/** Gain affiché : « -∞ » sous -60 dB, sinon une décimale. */
export function formatDb(db) {
  return db <= -60 || !Number.isFinite(db) ? "-∞" : `${db >= 0 ? "+" : ""}${db.toFixed(1)}`;
}

/**
 * Magnétisme : ramène `sample` sur la ligne de grille visible la plus proche si elle est à moins de `thresholdPx` pixels,
 * sinon le laisse tel quel. Les lignes de grille sont celles qu'on voit à l'écran (mesures, temps, subdivisions selon le zoom).
 */
export function snapSample(vp, widthPx, tempo, meters, sample, thresholdPx = 8) {
  let best = null;
  let bestD = thresholdPx;
  for (const l of gridLines(vp, widthPx, tempo, meters)) {
    const d = Math.abs(l.x - sampleToX(vp, sample));
    if (d < bestD) {
      bestD = d;
      best = l.x;
    }
  }
  return best === null ? sample : Math.round(xToSample(vp, best));
}

/**
 * Marqueurs de tempo et de chiffrage d'un projet, du plus ancien au plus récent : un marqueur par mesure où quelque chose change
 * (tempo et chiffrage d'une même mesure sont réunis). Un tempo posé en milieu de mesure (import MIDI) a son propre marqueur
 * (`bar` = la mesure qui le contient, `midBar` vrai). `bar` est numéroté à partir de 1.
 */
export function buildMarkers(project) {
  const tempo = new TempoMap(project.tempo_map, project.sample_rate);
  const meters = new MeterMap(project.meters);
  const list = [];
  const at = (whole, bar, midBar) => {
    let m = list.find((x) => x.whole === whole && x.midBar === midBar);
    if (!m) {
      m = { whole, bar, midBar, sample: tempo.samplesAt(whole), tempo: null, meter: null };
      list.push(m);
    }
    return m;
  };
  for (const s of meters.segments) at(meters.barStart(s.bar), s.bar + 1, false).meter = { num: s.num, den: s.den };
  for (const t of project.tempo_map) {
    const whole = frac(t.pos);
    const { bar, within } = meters.locate(whole);
    const m = at(within < 1e-9 ? meters.barStart(bar) : whole, bar + 1, within >= 1e-9);
    m.tempo = { bpm: t.num / t.den, pos: t.pos };
  }
  return list.sort((a, b) => a.whole - b.whole);
}

/** Texte d'un marqueur : « ♩ = 90 · 3/4 » (seulement ce qui change à cet endroit). */
export function markerLabel(m) {
  const parts = [];
  if (m.tempo) parts.push(`♩ = ${Math.round(m.tempo.bpm * 100) / 100}`);
  if (m.meter) parts.push(`${m.meter.num}/${m.meter.den}`);
  return parts.join(" · ");
}
