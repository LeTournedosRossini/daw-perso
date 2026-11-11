// Ce que le bandeau montre du pilote audio, en petit et à la manière de Reaper : fréquence d'échantillonnage et tampon, puis les latences
// d'entrée et de sortie QUE LE PILOTE ANNONCE (et non la latence de compensation des plugins, qui passe dans l'infobulle).
// Fonction pure, testée.

const ms = (samples, rate) => (samples / rate) * 1000;
const fmt = (x) => x.toFixed(1).replace(".", ",");

/** « 44,1 kHz », « 48 kHz ». */
export function khz(rate) {
  const k = rate / 1000;
  return `${Number.isInteger(k) ? k : k.toFixed(1).replace(".", ",")} kHz`;
}

/**
 * `audio` : `project.audio` (nom, fréquence, tampon, latences en échantillons ou `null`) ; absent sans pilote.
 * Rend `{ main, sub, title }` : deux courtes lignes et l'infobulle détaillée.
 */
export function audioInfo(audio, project) {
  const pdc = project?.latency_samples ? ms(project.latency_samples, project.sample_rate) : 0;
  const pdcLine = pdc > 0 ? `Compensation de latence des plugins : ${fmt(pdc)} ms (alignement de la lecture, pas une latence d'écoute).` : "";
  if (!audio) {
    return { main: khz(project?.sample_rate ?? 44100), sub: "sans pilote", title: ["Aucun pilote audio ouvert : le moteur tourne sans son.", pdcLine].filter(Boolean).join("\n") };
  }
  const rate = audio.sample_rate;
  const main = audio.buffer > 0 ? `${khz(rate)} · ${audio.buffer} éch.` : khz(rate);
  const known = audio.in_latency != null && audio.out_latency != null;
  const sub = known ? `E ${fmt(ms(audio.in_latency, rate))} · S ${fmt(ms(audio.out_latency, rate))} ms` : "latence : —";
  const lines = [`${audio.driver} — ${khz(rate)}${audio.buffer > 0 ? `, tampon de ${audio.buffer} échantillons (${fmt(ms(audio.buffer, rate))} ms)` : ""}`];
  if (known) {
    const rt = ms(audio.in_latency + audio.out_latency, rate);
    lines.push(`Latences annoncées par le pilote : entrée ${audio.in_latency} éch. (${fmt(ms(audio.in_latency, rate))} ms), sortie ${audio.out_latency} éch. (${fmt(ms(audio.out_latency, rate))} ms) — aller-retour ≈ ${fmt(rt)} ms avant les convertisseurs.`);
  } else {
    lines.push("Latences du pilote : pas encore connues (elles sont lues une fois le flux démarré).");
  }
  if (pdcLine) lines.push(pdcLine);
  return { main, sub, title: lines.join("\n") };
}
