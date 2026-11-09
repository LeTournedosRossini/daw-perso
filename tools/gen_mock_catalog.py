#!/usr/bin/env python3
"""Génère crates/ui/web/js/mock-chord-catalog.js (dictionnaires du faux moteur) depuis les fichiers Rust générés.
Usage : python3 tools/gen_mock_catalog.py"""
import json, re
q = open("crates/session/src/quality_catalog.rs", encoding="utf-8").read()
qualities = [(m.group(1), [int(x) for x in m.group(2).split(",") if x.strip()]) for m in re.finditer(r'\("((?:[^"\\]|\\.)*)", &\[([\d,]*)\]\),', q.split("pub const ALIASES")[0])]
aliases = re.findall(r'\("((?:[^"\\]|\\.)*)", "((?:[^"\\]|\\.)*)"\),', q.split("pub const ALIASES")[1])
sc = open("crates/session/src/scale_catalog.rs", encoding="utf-8").read()
scales = [{"id": m.group(1), "fr": m.group(2), "en": m.group(3), "intervals": [int(x) for x in m.group(4).split(",")]} for m in re.finditer(r'\("([^"]+)", "([^"]*)", "([^"]*)", &\[([\d,]+)\]\),', sc)]
out = "// Dictionnaires du faux moteur (qualités de Scorsaire, alias, 72 gammes). Généré par tools/gen_mock_catalog.py : ne pas modifier à la main.\nexport const MOCK_CHORD_CATALOG = " + json.dumps({"qualities": qualities, "aliases": [list(a) for a in aliases], "scales": scales}, ensure_ascii=False) + ";\n"
open("crates/ui/web/js/mock-chord-catalog.js", "w", encoding="utf-8").write(out)
print(len(qualities), len(aliases), len(scales))
