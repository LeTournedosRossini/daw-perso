// Import audio : calculs purs (testés) de la fenêtre d'import et de la commande envoyée au moteur.

export const AUDIO_EXTENSIONS = ["wav", "flac", "mp3"];

/** Le chemin a-t-il l'extension d'un format géré ? (le moteur reconnaît le format par le contenu : ce n'est qu'un tri à l'entrée) */
export function isAudioPath(path) {
  const m = /\.([^./\\]+)$/.exec(path ?? "");
  return !!m && AUDIO_EXTENSIONS.includes(m[1].toLowerCase());
}

/** Un fichier peut être référencé en place s'il se lit sans conversion : mono ou stéréo, à la fréquence du projet. */
export function referencable(info) {
  return !info.error && info.channels > 0 && info.channels <= 2 && !info.needs_conversion;
}

/** Vrai s'il faut demander quoi faire des fichiers de plus de deux canaux. */
export function needsChoice(infos) {
  return infos.some((i) => !i.error && i.channels > 2);
}

const kHz = (hz) => `${(hz / 1000).toFixed(hz % 1000 === 0 ? 0 : 1).replace(".", ",")} kHz`;

function duration(frames, rate) {
  if (!frames || !rate) return "";
  const s = Math.round(frames / rate);
  return ` · ${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Ligne de description d'un fichier : « stéréo · 44,1 kHz → 48 kHz · 0:12 », ou son erreur. */
export function describeFile(info, projectRate) {
  if (info.error) return `illisible : ${info.error}`;
  const ch = info.channels === 1 ? "mono" : info.channels === 2 ? "stéréo" : `${info.channels} canaux`;
  const rate = info.needs_conversion ? `${kHz(info.sample_rate)} → ${kHz(projectRate)} (converti)` : kHz(info.sample_rate);
  return `${ch} · ${rate}${duration(info.frames, info.sample_rate)}`;
}

/** Commande `import_audio`. `target` : `{ node, sample }` du dépôt (ou `null`). */
export function buildImportCommand({ files, target = null, reference = false, multichannel = null }) {
  const cmd = { type: "import_audio", files, reference: !!reference };
  if (target?.node != null) cmd.node = target.node;
  if (target?.sample != null) cmd.pos = Math.max(0, Math.round(target.sample));
  if (multichannel) cmd.multichannel = multichannel;
  return cmd;
}

/** Texte d'avancement : « 2 / 5 — Basse · 40 % ». */
export function importProgressText(e) {
  if (e.active) return `Import ${Math.min(e.done + 1, e.total)} / ${e.total} — ${e.current} · ${Math.round(e.progress * 100)} %`;
  if (e.cancelled) return "Import annulé";
  if (e.error) return `Import impossible — ${e.error}`;
  return `Importé : ${e.clips.length} clip${e.clips.length > 1 ? "s" : ""}`;
}
