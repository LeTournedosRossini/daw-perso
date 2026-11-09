#!/usr/bin/env python3
"""Régénère crates/session/src/scale_catalog.rs depuis le scales.json de Scorsaire.
Usage : python3 tools/gen_scales.py /chemin/vers/scorsaire/src/scales/scales.json"""
import json, sys
d = json.load(open(sys.argv[1]))
out = ["//! Catalogue de gammes de Scorsaire (72), généré par `tools/gen_scales.py` depuis `src/scales/scales.json` de Scorsaire.",
       "//! Ne pas modifier à la main : régénérer. Scorsaire fait foi pour le catalogue ; les `intervals` écrits dans un fichier font foi pour le morceau.",
       "", "/// (identifiant, nom français, nom anglais, demi-tons depuis la fondamentale)", "pub const SCALES: &[(&str, &str, &str, &[u8])] = &["]
esc = lambda t: t.replace("\\", "\\\\").replace('"', '\\"')
for s in d["scales"]:
    iv = ",".join(str(i) for i in s["intervals"])
    out.append(f'    ("{s["id"]}", "{esc(s["name"]["fr"])}", "{esc(s["name"]["en"])}", &[{iv}]),')
out.append("];")
open("crates/session/src/scale_catalog.rs", "w").write("\n".join(out) + "\n")
