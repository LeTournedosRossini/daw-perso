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
    const parts = word.split("-");
    parts.forEach((part, i) => {
      if (!part) return;
      const joined = i < parts.length - 1; // un tiret suit cette partie (au milieu ou à la fin du mot)
      out.push({ text: part.slice(0, MAX_TEXT), hyphen: joined });
    });
  }
  return out;
}

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
      return { start: rel, text: s.text, hyphen: !!s.hyphen, verse: s.verse ?? 1 };
    });
}

/** Le texte d'un couplet tel qu'on le saisit : « ma-man est la plus bel-le » (une syllabe liée s'écrit collée à la suivante). */
export function lyricsToText(list, verse) {
  const sorted = list.filter((s) => (s.verse ?? 1) === verse).sort((a, b) => fcmp(a.at, b.at));
  let out = "";
  sorted.forEach((s, i) => {
    out += syllableLabel(s);
    if (!s.hyphen && i < sorted.length - 1) out += " ";
  });
  return out;
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

/**
 * Pose une syllabe par instant de note (`onsets`, triés) dans le couplet `verse` ; les syllabes en trop (plus que de notes) sont comptées
 * dans `extra`, les notes sans syllabe restent muettes. Aucune syllabe ne dépasse le dernier instant fourni.
 */
export function placeOnNotes(items, onsets, verse) {
  const n = Math.min(items.length, onsets.length);
  return {
    placed: items.slice(0, n).map((it, k) => ({ at: onsets[k], text: it.text, hyphen: !!it.hyphen, verse })),
    extra: items.length - n,
  };
}

/** Retire les syllabes du couplet `verse` dont l'instant est dans [from, to[ (ce qu'un nouveau texte posé sur ce clip remplace). */
export function clearVerseRange(list, verse, from, to) {
  return list.filter((s) => (s.verse ?? 1) !== verse || fcmp(s.at, from) < 0 || fcmp(s.at, to) >= 0);
}

/** Les indices des notes qui commencent à l'instant de la première note sélectionnée ou après (là où les syllabes démarrent). */
export function notesFromFirstSelected(d, sel) {
  const first = sel.map((i) => d.notes[i]).filter(Boolean).sort((a, b) => fcmp(a.start, b.start))[0];
  if (!first) return null;
  return d.notes.map((n, i) => (fcmp(n.start, first.start) >= 0 ? i : -1)).filter((i) => i >= 0);
}
