// Paroles : saisie d'un texte en syllabes posées sur la ligne de temps (fractions exactes de ronde) — calculs purs, sans DOM.
// Une syllabe : { at: [n, d], text, hyphen, verse } ; `hyphen` : elle continue le mot (elle s'écrit « ma- »). Couplets 1 à 8.

import { fadd, fsub, fcmp, fnorm } from "./notes-model.js";

export const MAX_VERSE = 8;
export const MAX_TEXT = 40;

/**
 * Découpe un texte saisi en syllabes : les mots sont séparés par des espaces, les syllabes d'un mot par des tirets
 * (« ma-man est la plus bel-le » → ma- man est la plus bel- le). Un tiret final garde la syllabe liée à la suivante (« ma- »).
 */
export function parseLyricsText(text) {
  const out = [];
  for (const word of String(text ?? "").split(/\s+/)) {
    if (!word) continue;
    if (word === "_") {
      out.push({ extend: true }); // « _ » : la syllabe précédente se prolonge sur la note suivante (mélisme)
      continue;
    }
    const parts = word.split("-");
    parts.forEach((part, i) => {
      if (!part) return;
      const joined = i < parts.length - 1; // un tiret suit cette partie (au milieu ou à la fin du mot)
      out.push({ text: part.slice(0, MAX_TEXT), hyphen: joined });
    });
  }
  return out;
}

/** Les « _ » d'un texte ne sont pas des syllabes : leur nombre ne compte pas parmi les syllabes. */
export const syllableCount = (items) => items.filter((it) => !it.extend).length;

/** Une syllabe telle qu'on la saisit et l'affiche : « ma- » quand elle continue le mot. */
export const syllableLabel = (s) => `${s.text}${s.hyphen ? "-" : ""}`;

/** Pose `items` (résultat de `parseLyricsText`) à partir de `at`, une syllabe tous les `step` (fraction de ronde), dans le couplet `verse`. */
export function placeSyllables(items, at, step, verse) {
  return items.map((it, k) => {
    let pos = fnorm(at);
    for (let i = 0; i < k; i++) pos = fadd(pos, step);
    return { at: pos, text: it.text, hyphen: !!it.hyphen, verse };
  });
}

/** Ajoute `added` aux syllabes existantes : une syllabe déjà posée au même instant dans le même couplet est remplacée. Résultat trié. */
export function mergeSyllables(existing, added) {
  const clash = (s) => added.some((a) => a.verse === s.verse && fcmp(a.at, s.at) === 0);
  return [...existing.filter((s) => !clash(s)), ...added].sort((a, b) => fcmp(a.at, b.at) || a.verse - b.verse);
}

/** Les syllabes d'un couplet, avec leur indice dans la liste complète (que le protocole adresse). */
export const verseSyllables = (list, verse) => list.map((s, index) => ({ ...s, index })).filter((s) => (s.verse ?? 1) === verse);

/** Nombre de couplets à proposer : jusqu'au plus grand couplet utilisé + 1 (au plus 8). */
export const verseCount = (list) => Math.min(MAX_VERSE, Math.max(1, ...list.map((s) => s.verse ?? 1)) + 1);

/**
 * Paroles d'un clip de notes pour le module de partition : les syllabes dont l'instant est dans la partie visible, en positions
 * « depuis le début de la matière » (comme les notes). `d` : événement `notes` (start, len, offset).
 */
export function lyricsForClip(list, d) {
  const from = d.start;
  const to = fadd(d.start, d.len);
  return list
    .filter((s) => fcmp(s.at, from) >= 0 && fcmp(s.at, to) < 0)
    .map((s) => {
      const rel = fadd(fnorm([s.at[0] * from[1] - from[0] * s.at[1], s.at[1] * from[1]]), d.offset);
      const out = { start: rel, text: s.text, hyphen: !!s.hyphen, verse: s.verse ?? 1 };
      if (s.end && fcmp(s.end, s.at) > 0) out.end = fadd(fsub(s.end, from), d.offset); // trait de prolongation (même repère que `start`)
      return out;
    });
}

/** Le texte d'un couplet tel qu'on le saisit : « ma-man est la plus bel-le » (une syllabe liée s'écrit collée à la suivante). */
export function lyricsToText(list, verse, onsets = null) {
  const sorted = list.filter((s) => (s.verse ?? 1) === verse).sort((a, b) => fcmp(a.at, b.at));
  const tokens = [];
  sorted.forEach((s) => {
    // Un mélisme se relit en « _ » : un par note dont le début tombe sous le trait de prolongation.
    const under = s.end && onsets ? onsets.filter((o) => fcmp(o, s.at) > 0 && fcmp(o, s.end) < 0).length : 0;
    tokens.push({ label: syllableLabel(s), glued: s.hyphen && !under });
    for (let k = 0; k < under; k++) tokens.push({ label: "_", glued: false });
  });
  return tokens.map((t, i) => (i < tokens.length - 1 && !t.glued ? `${t.label} ` : t.label)).join("");
}

/**
 * Les instants où commence au moins une note de la partie visible d'un clip (accords = un seul instant), en positions absolues, triés.
 * `d` : événement `notes` ; `only` : indices de notes à considérer (sélection), tous si absent ou vide.
 */
export function noteOnsets(d, only = null) {
  const lo = d.offset;
  const hi = fadd(d.offset, d.len);
  const onsets = [];
  d.notes.forEach((n, i) => {
    if (only?.length && !only.includes(i)) return;
    if (fcmp(n.start, lo) < 0 || fcmp(n.start, hi) >= 0) return;
    const at = fadd(d.start, fsub(n.start, d.offset));
    if (!onsets.some((o) => fcmp(o, at) === 0)) onsets.push(at);
  });
  return onsets.sort(fcmp);
}

/** Comme `noteOnsets`, avec la fin de chaque instant (la fin de la note la plus longue qui y commence) : [{ at, end }]. */
export function noteSpans(d, only = null) {
  const lo = d.offset;
  const hi = fadd(d.offset, d.len);
  const spans = [];
  d.notes.forEach((n, i) => {
    if (only?.length && !only.includes(i)) return;
    if (fcmp(n.start, lo) < 0 || fcmp(n.start, hi) >= 0) return;
    const at = fadd(d.start, fsub(n.start, d.offset));
    const end = fadd(at, n.len);
    const same = spans.find((s) => fcmp(s.at, at) === 0);
    if (same) same.end = fcmp(end, same.end) > 0 ? end : same.end;
    else spans.push({ at, end });
  });
  return spans.sort((a, b) => fcmp(a.at, b.at));
}

/**
 * Pose une syllabe par instant de note (`spans` : [{ at, end }] triés, ou simples instants `[n, d]`) dans le couplet `verse`. Un « _ » (mélisme) consomme
 * la note suivante sans syllabe : la syllabe précédente reçoit un trait de prolongation (`end`) jusqu'à la fin de cette note. Les éléments en trop (plus que
 * de notes, ou un « _ » sans syllabe devant) sont comptés dans `extra` ; les notes sans élément restent muettes.
 */
export function placeOnNotes(items, spans, verse) {
  const slots = spans.map((s) => (Array.isArray(s) ? { at: s, end: null } : s));
  const placed = [];
  let k = 0;
  let extra = 0;
  for (const it of items) {
    if (k >= slots.length) {
      extra++;
      continue;
    }
    if (it.extend) {
      const prev = placed[placed.length - 1];
      if (!prev) {
        extra++;
        continue;
      }
      if (slots[k].end) prev.end = slots[k].end;
      k++;
    } else {
      placed.push({ at: slots[k].at, text: it.text, hyphen: !!it.hyphen, verse });
      k++;
    }
  }
  return { placed, extra };
}

/** Retire les syllabes du couplet `verse` dont l'instant est dans [from, to[ (ce qu'un nouveau texte posé sur ce clip remplace). */
export function clearVerseRange(list, verse, from, to) {
  return list.filter((s) => (s.verse ?? 1) !== verse || fcmp(s.at, from) < 0 || fcmp(s.at, to) >= 0);
}

// ---- réserve de syllabes (pastilles posées sur les notes) -----------------------------------------------------------------------
// Une pastille : { text, hyphen, at (fraction absolue de ronde, ou null : elle attend), end? }. L'ordre des mots ne change jamais.

/** Les pastilles d'un texte (« ma-man est la plus bel-le ») : toutes en attente. Les « _ » ne sont pas des pastilles (le trait se règle par la fenêtre « Paroles… »). */
export function chipsFromText(text) {
  return parseLyricsText(text).filter((i) => !i.extend).map((i) => ({ text: i.text, hyphen: !!i.hyphen, at: null }));
}

/** Les pastilles d'un couplet déjà posé sur une plage du projet : toutes placées, dans l'ordre. */
export function chipsFromLyrics(list, verse, from, to) {
  return list
    .filter((s) => (s.verse ?? 1) === verse && fcmp(s.at, from) >= 0 && fcmp(s.at, to) < 0)
    .sort((a, b) => fcmp(a.at, b.at))
    .map((s) => ({ text: s.text, hyphen: !!s.hyphen, at: s.at, ...(s.end ? { end: s.end } : {}) }));
}

/** L'instant de `onsets` le plus proche de `at` (`null` s'il n'y en a pas). */
export function nearestOnset(onsets, at) {
  let best = null;
  let bestD = Infinity;
  for (const o of onsets) {
    const dist = Math.abs(o[0] / o[1] - at[0] / at[1]);
    if (dist < bestD) [best, bestD] = [o, dist];
  }
  return best;
}

/**
 * Pose la pastille `k` à l'instant `at` (un des `onsets` des notes). Règle de l'ordre : une pastille ne passe jamais devant une pastille placée
 * avant elle (refus : `null`) ; celles qui la suivent et qui se trouveraient au même instant ou avant sont poussées sur les notes suivantes
 * (refus s'il n'y en a plus). Une pastille qui change de place perd son trait de prolongation.
 */
export function placeChip(chips, k, at, onsets) {
  if (!chips[k]) return null;
  for (let i = 0; i < k; i++) if (chips[i].at && fcmp(chips[i].at, at) >= 0) return null;
  const out = chips.map((c) => ({ ...c }));
  const move = (c, to) => {
    if (!c.at || fcmp(c.at, to) !== 0) delete c.end;
    c.at = to;
  };
  move(out[k], at);
  let cur = at;
  for (let j = k + 1; j < out.length; j++) {
    if (!out[j].at) continue;
    if (fcmp(out[j].at, cur) > 0) {
      cur = out[j].at;
      continue;
    }
    const next = onsets.find((o) => fcmp(o, cur) > 0);
    if (!next) return null;
    move(out[j], next);
    cur = next;
  }
  return out;
}

/** Retire la pastille `k` de la portée : elle attend de nouveau. */
export function unplaceChip(chips, k) {
  return chips.map((c, i) => (i === k ? { text: c.text, hyphen: c.hyphen, at: null } : { ...c }));
}

/** Les syllabes du projet que donnent les pastilles placées, dans le couplet `verse`. */
export function chipsToSyllables(chips, verse) {
  return chips.filter((c) => c.at).map((c) => ({ at: c.at, text: c.text, hyphen: !!c.hyphen, verse, ...(c.end ? { end: c.end } : {}) }));
}

/**
 * Saisie d'une syllabe au clic dans la partition (`lyricTap` du module : `start` depuis le début de la matière du clip, `verse`, `end` éventuel).
 * Rend la nouvelle liste de syllabes du projet et son libellé d'historique, ou `null` s'il n'y a rien à changer. Un texte vide supprime la syllabe
 * de cet instant ; sinon la première syllabe saisie se pose à l'instant touché (remplaçant celle du même couplet qui y était) et les suivantes,
 * si le texte en contient plusieurs (« ma-man »), sur les notes qui suivent ; un « _ » prolonge la syllabe précédente. Une syllabe retouchée garde
 * son trait de prolongation s'il n'y a pas de « _ » dans le texte.
 */
export function applyLyricTap(list, d, tap, typed) {
  const verse = tap.verse ?? 1;
  const abs = (rel) => fadd(d.start, fsub(rel, d.offset));
  const at = abs(tap.start);
  const items = parseLyricsText(typed);
  if (!items.length) {
    const there = list.some((s) => (s.verse ?? 1) === verse && fcmp(s.at, at) === 0);
    return there ? { syllables: list.filter((s) => !((s.verse ?? 1) === verse && fcmp(s.at, at) === 0)), label: "Effacer une syllabe", extra: 0 } : null;
  }
  const spans = noteSpans(d);
  const here = spans.find((s) => fcmp(s.at, at) === 0);
  const slots = [{ at, end: here?.end ?? null }, ...spans.filter((s) => fcmp(s.at, at) > 0)];
  const { placed, extra } = placeOnNotes(items, slots, verse);
  if (!placed.length) return null;
  if (tap.end && !placed[0].end && !items.some((i) => i.extend)) placed[0].end = abs(tap.end);
  const base = list.filter((s) => !((s.verse ?? 1) === verse && placed.some((p) => fcmp(p.at, s.at) === 0)));
  const syllables = [...base, ...placed].sort((a, b) => fcmp(a.at, b.at) || (a.verse ?? 1) - (b.verse ?? 1));
  return { syllables, label: "Écrire une syllabe", extra };
}

/** Les indices des notes qui commencent à l'instant de la première note sélectionnée ou après (là où les syllabes démarrent). */
export function notesFromFirstSelected(d, sel) {
  const first = sel.map((i) => d.notes[i]).filter(Boolean).sort((a, b) => fcmp(a.start, b.start))[0];
  if (!first) return null;
  return d.notes.map((n, i) => (fcmp(n.start, first.start) >= 0 ? i : -1)).filter((i) => i >= 0);
}
