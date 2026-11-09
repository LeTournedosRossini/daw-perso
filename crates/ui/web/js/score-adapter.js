// Adaptateur entre le DAW et le module « vue partition » de Scorsaire (voir docs/demande-a-scorsaire-vue-partition.md) : pur JavaScript, sans DOM.
//  - `scoreClipInput` : l'événement `notes` d'un clip → l'entrée de `setClips` ;
//  - `editToCommands` : un `onEdit` du module → nos commandes (`add_notes`, `edit_notes`, `delete_notes`, `preview_note`), toutes annulables.
// Le module n'a pas de mémoire propre : il dit ce que l'utilisateur veut, le DAW exécute et renvoie le clip.
// Les notes gardent leurs **indices** (liste triée du DAW) : on ne filtre jamais les notes cachées par le rognage, le module ne dessine que la partie visible.

import { fadd, fsub, fcmp, fnorm } from "./notes-model.js";

export const STAVES = ["treble", "bass", "grand"];
export const DEFAULT_VELOCITY = 0.8;

/** Réglages de vue par défaut d'un clip (affichage seulement : rien de cela n'est dans le MIDI). */
// Grille d'écriture : `'auto'` laisse le module choisir, mesure par mesure, la plus grossière qui porte tous les débuts de notes à un tick près
// (plancher 1/16 binaire et 1/24 de triolet ; une prise jouée en direct retombe sur 1/16 arrondi).
// autoTolerance : marge en ticks (960 par noire) de la grille automatique ; 4 reconnaît un triolet joué à la main (par défaut le module tolère 1 tick).
export const defaultPresentation = () => ({ staff: "treble", grid: "auto", autoTolerance: 4, keyFifths: 0 });

/**
 * Position, dans la mesure qui contient `start` (fraction de ronde), où un morceau commence : `null` quand il commence sur un début de mesure.
 * `meters` : les chiffrages du projet `[{ bar, num, den }]` (mesure 0 = la première), calculés en fractions exactes. Le module en fait une
 * mesure raccourcie qui garde son numéro (`clip.startInBar`).
 */
export function startInBarOf(start, meters) {
  const list = [...(meters?.length ? meters : [{ bar: 0, num: 4, den: 4 }])].sort((x, y) => x.bar - y.bar);
  let segStart = [0, 1];
  let seg = list[0];
  for (let i = 0; i < list.length; i++) {
    const cur = list[i];
    if (i > 0) {
      const prev = list[i - 1];
      const next = fadd(segStart, fnorm([(cur.bar - prev.bar) * prev.num, prev.den]));
      if (fcmp(next, start) > 0) break;
      segStart = next;
    }
    seg = cur;
  }
  const rel = fsub(start, segStart);
  if (rel[0] <= 0) return null;
  // k = nombre de mesures entières dans `rel` (division exacte par la longueur de mesure num/den).
  const k = Math.floor((rel[0] * seg.den) / (rel[1] * seg.num));
  const within = fsub(rel, fnorm([k * seg.num, seg.den]));
  return within[0] > 0 ? within : null;
}

/** Portée proposée d'après la tessiture : notes graves → clé de fa, étendues → deux portées, sinon clé de sol. */
export function suggestStaff(notes) {
  if (!notes.length) return "treble";
  const lo = Math.min(...notes.map((n) => n.key));
  const hi = Math.max(...notes.map((n) => n.key));
  if (lo < 60 && hi >= 60 && hi - lo > 14) return "grand";
  return hi < 60 ? "bass" : "treble";
}

/** L'événement `notes` d'un clip (positions `[n, d]` depuis le début de la matière) et ses réglages de vue → entrée de `setClips`. */
export function scoreClipInput(detail, { name = "", node = null, presentation = defaultPresentation(), lyrics = [], startInBar = null } = {}) {
  return {
    id: detail.clip,
    name,
    node,
    start: detail.start,
    len: detail.len,
    offset: detail.offset,
    notes: detail.notes.map((n) => ({ start: n.start, len: n.len, key: n.key, velocity: n.velocity, channel: n.channel ?? 0, voice: n.voice ?? 1 })),
    selection: [...(detail.selection ?? [])],
    ...(lyrics.length ? { lyrics } : {}),
    ...(startInBar ? { startInBar } : {}),
    presentation: { ...defaultPresentation(), ...presentation },
  };
}

/** Carte des tempos et chiffrages du projet → ce que `setTiming` reçoit. */
// Le DAW numérote les mesures à partir de 0 (première mesure = 0) ; le module de Scorsaire à partir de 1 : un chiffrage posé à la mesure 5
// (index 4) doit lui arriver avec `bar: 5`. (Sans ce décalage tout changement de chiffrage tombait une mesure trop tôt.)
export const scoreTiming = (project) => ({ tempoMap: project?.tempo_map ?? [], meters: (project?.meters ?? []).map((m) => ({ ...m, bar: m.bar + 1 })) });

const fracOk = (f) => Array.isArray(f) && f.length === 2 && Number.isInteger(f[0]) && Number.isInteger(f[1]) && f[1] > 0;
const clampKey = (k) => Math.min(127, Math.max(0, Math.round(k)));

/**
 * Un `onEdit` du module → liste de commandes du protocole (vide : rien à envoyer, ou geste refusé). `detail` : l'événement `notes` le plus
 * récent du clip ; `ctx.node` : la piste du clip (pour l'écoute). Le numérateur d'un `dstart` est **signé** (déplacement vers la gauche).
 */
export function editToCommands(edit, detail, ctx = {}) {
  if (!edit || !detail) return [];
  const clip = detail.clip;
  const notes = detail.notes;
  const valid = (indices) => Array.isArray(indices) && indices.length > 0 && indices.every((i) => Number.isInteger(i) && notes[i]);
  switch (edit.type) {
    case "add": {
      if (!fracOk(edit.start) || !fracOk(edit.len) || edit.len[0] <= 0 || edit.start[0] < 0 || !Number.isInteger(edit.key) || edit.key < 0 || edit.key > 127) return [];
      const note = { start: fnorm(edit.start), len: fnorm(edit.len), key: edit.key, velocity: edit.velocity ?? DEFAULT_VELOCITY };
      if (Number.isInteger(edit.voice) && edit.voice >= 2 && edit.voice <= 4) note.voice = edit.voice; // la voix 1 est la valeur par défaut
      return [{ type: "add_notes", clip, notes: [note] }];
    }
    case "move": {
      if (!valid(edit.indices) || !fracOk(edit.dstart)) return [];
      const dk = Number.isInteger(edit.dkey) ? edit.dkey : 0;
      const edits = [];
      for (const index of edit.indices) {
        const start = fadd(notes[index].start, edit.dstart);
        if (start[0] < 0) return []; // une note ne passe pas avant le début de la matière : le geste est refusé en bloc
        edits.push({ index, start, key: clampKey(notes[index].key + dk) });
      }
      return [{ type: "edit_notes", clip, edits, label: "Déplacer des notes" }];
    }
    case "resize": {
      if (!valid([edit.index]) || !fracOk(edit.len) || edit.len[0] <= 0) return [];
      return [{ type: "edit_notes", clip, edits: [{ index: edit.index, len: fnorm(edit.len) }], label: "Changer la durée" }];
    }
    case "pitch": {
      if (!valid(edit.indices)) return [];
      const key = (n) => (Number.isInteger(edit.key) ? clampKey(edit.key) : clampKey(n.key + (Number.isInteger(edit.dkey) ? edit.dkey : 0)));
      return [{ type: "edit_notes", clip, edits: edit.indices.map((index) => ({ index, key: key(notes[index]) })), label: "Changer la hauteur" }];
    }
    case "voice": {
      if (!valid(edit.indices) || !Number.isInteger(edit.voice) || edit.voice < 1 || edit.voice > 4) return [];
      return [{ type: "edit_notes", clip, edits: edit.indices.map((index) => ({ index, voice: edit.voice })), label: "Changer de voix" }];
    }
    case "delete":
      return valid(edit.indices) ? [{ type: "delete_notes", clip, indices: [...edit.indices].sort((a, b) => a - b) }] : [];
    case "preview":
      return ctx.node != null && Number.isInteger(edit.key) && edit.key >= 0 && edit.key <= 127 ? [{ type: "preview_note", node: ctx.node, key: edit.key, velocity: DEFAULT_VELOCITY, on: !!edit.on }] : [];
    default:
      return []; // `select` et inconnues : rien à envoyer au DAW (la sélection est renvoyée par le DAW après chaque commande)
  }
}

/** La note `index` est-elle (au moins en partie) dans la partie visible du clip ? */
export function isVisible(detail, index) {
  const n = detail.notes[index];
  if (!n) return false;
  const end = fadd(detail.offset, detail.len);
  return fcmp(n.start, end) < 0 && fcmp(fadd(n.start, n.len), detail.offset) > 0;
}

/** Dénominateur des positions de lecture envoyées au module (un tick de 960 par noire). */
const TICKS = 3840;

/** Texte d'une armure en quintes : 0 → « Do majeur / La mineur », 2 → « 2 dièses », −3 → « 3 bémols ». */
export const keyText = (n) => (n === 0 ? "Aucune altération" : `${Math.abs(n)} ${n > 0 ? (n > 1 ? "dièses" : "dièse") : (n < -1 ? "bémols" : "bémol")}`);

/** Position de lecture (échantillons) → fraction de ronde `[n, d]` au tick. */
export const playheadFraction = (tempo, sample) => [Math.max(0, Math.round(tempo.wholeAt(sample) * TICKS)), TICKS];
