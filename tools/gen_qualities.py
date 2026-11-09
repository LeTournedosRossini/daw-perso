#!/usr/bin/env python3
"""Régénère crates/session/src/quality_catalog.rs : le dictionnaire de qualités d'accord de Scorsaire (31 qualités, intervalles sur deux
octaves, de 0 à 23 demi-tons) et les écritures acceptées à la lecture.

Usage : python3 tools/gen_qualities.py scorsaire-chord-qualities.json QualityAliases.inc
  - scorsaire-chord-qualities.json : docs/exemples-flibuste/ du dépôt de Scorsaire (référence unique du dictionnaire)
  - QualityAliases.inc : flibuste-neptune/core/src/ (écritures que Carte accepte en plus des alias de Scorsaire)
"""
import json, re, sys

d = json.load(open(sys.argv[1]))
qs = d["chordQualities"]["qualities"]
ids = {q["id"] for q in qs}
aliases = {}
for q in qs:
    for a in q.get("aliases", []):
        aliases[a] = q["id"]
for line in open(sys.argv[2], encoding="utf-8"):
    m = re.match(r'\s*\{\s*"((?:[^"\\]|\\.)*)"\s*,\s*"((?:[^"\\]|\\.)*)"\s*\}', line)
    if m:
        a, canon = m.group(1), m.group(2)
        if a not in ids and canon in ids:
            aliases.setdefault(a, canon)
esc = lambda t: t.replace("\\", "\\\\").replace('"', '\\"')
out = [
    "//! Dictionnaire de qualités d'accord de Scorsaire (référence unique pour Scorsaire, Carte et DAW), généré par `tools/gen_qualities.py`.",
    "//! Ne pas modifier à la main : régénérer. Les intervalles sont sur **deux octaves** (0 à 23 demi-tons) : une neuvième est 14, pas 2.",
    "",
    "/// (identifiant, demi-tons depuis la fondamentale) ; l'identifiant vide est l'accord majeur.",
    "pub const QUALITIES: &[(&str, &[u8])] = &[",
]
for q in qs:
    out.append(f'    ("{esc(q["id"])}", &[{",".join(map(str, q["intervals"]))}]),')
out += ["];", "", "/// (écriture acceptée à la lecture, identifiant). `2` et `add2` n'en font pas partie : une seconde ajoutée n'est pas une neuvième.", "pub const ALIASES: &[(&str, &str)] = &["]
for a, c in aliases.items():
    out.append(f'    ("{esc(a)}", "{esc(c)}"),')
out.append("];")
open("crates/session/src/quality_catalog.rs", "w").write("\n".join(out) + "\n")
print(len(qs), "qualités,", len(aliases), "alias")
