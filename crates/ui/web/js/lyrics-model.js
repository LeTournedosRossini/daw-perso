// Paroles : saisie d'un texte en syllabes posées sur la ligne de temps (fractions exactes de ronde) — calculs purs, sans DOM.
// Une syllabe : { at: [n, d], text, hyphen, verse } ; `hyphen` : elle continue le mot (elle s'écrit « ma- »). Couplets 1 à 8.

import { fadd, fcmp, fnorm } from "./notes-model.js";

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
