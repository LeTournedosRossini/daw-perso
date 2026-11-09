// Signaux de démonstration pour le faux moteur : des formes d'onde déterministes (aucun fichier audio), avec une
// enveloppe réaliste. Sert uniquement à voir l'interface avant que le vrai moteur lui soit branché.

const TAU = Math.PI * 2;

/** Bruit déterministe dans [-1, 1[ à partir d'un entier. */
export function noise(i) {
  let x = (i ^ 0x9e3779b9) >>> 0;
  x = Math.imul(x ^ (x >>> 16), 0x85ebca6b) >>> 0;
  x = Math.imul(x ^ (x >>> 13), 0xc2b2ae35) >>> 0;
  return ((x ^ (x >>> 16)) >>> 0) / 2 ** 31 - 1;
}

/** Fonction `(secondes) → échantillon [-1, 1]` pour un type de matière sonore. */
export function waveformFor(kind, seed = 0) {
  switch (kind) {
    case "drums":
      return (t) => {
        const beat = t * 2; // 120 bpm : 2 temps par seconde
        const phase = beat - Math.floor(beat);
        const kick = Math.exp(-phase * 9) * Math.sin(TAU * (60 + 90 * Math.exp(-phase * 25)) * phase * 0.5);
        const off = (beat * 2) % 1;
        const hat = Math.exp(-off * 30) * noise(Math.floor(t * 20000) + seed) * 0.35;
        const snare = Math.floor(beat) % 2 === 1 ? Math.exp(-phase * 14) * noise(Math.floor(t * 12000) + seed) * 0.6 : 0;
        return Math.max(-1, Math.min(1, kick * 0.9 + hat + snare));
      };
    case "bass":
      return (t) => {
        const note = [55, 55, 65.4, 49][Math.floor(t * 0.5) % 4];
        const env = 0.55 + 0.35 * Math.sin(TAU * t * 2) ** 2;
        return Math.sin(TAU * note * t) * env;
      };
    case "voice":
      return (t) => {
        const syll = Math.max(0, Math.sin(TAU * t * 1.7 + seed)) ** 1.5;
        return (Math.sin(TAU * 220 * t) * 0.5 + noise(Math.floor(t * 9000) + seed) * 0.35) * syll;
      };
    default: // pad / guitare
      return (t) => {
        const env = 0.5 + 0.3 * Math.sin(TAU * t * 0.4 + seed);
        return (Math.sin(TAU * 196 * t) * 0.45 + Math.sin(TAU * 294.3 * t) * 0.3 + noise(Math.floor(t * 8000) + seed) * 0.1) * env;
      };
  }
}

/**
 * Crêtes `[min, max]` de `columns` colonnes couvrant `[from, to[` échantillons (positions DANS la matière sonore),
 * comme `session::peaks::Peaks::query` côté moteur : chaque colonne couvre `(to - from) / columns` échantillons.
 */
export function columnPeaks(wave, sampleRate, from, to, columns) {
  const span = to - from;
  const out = [];
  for (let c = 0; c < columns; c++) {
    const a = from + (span * c) / columns;
    const b = from + (span * (c + 1)) / columns;
    const points = Math.max(2, Math.min(48, Math.ceil(b - a)));
    let lo = Infinity;
    let hi = -Infinity;
    for (let k = 0; k < points; k++) {
      const v = wave((a + ((b - a) * k) / points) / sampleRate);
      if (v < lo) lo = v;
      if (v > hi) hi = v;
    }
    out.push([lo, hi]);
  }
  return out;
}
