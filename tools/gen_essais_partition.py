#!/usr/bin/env python3
"""Fabrique des entrées `setClips` pour le module de Scorsaire : un clip de triolets « joués » (décalés de quelques ticks) et un projet chargé.
Positions en fractions de ronde [n, d] ; un tick = 1/3840 de ronde (960 par noire), comme l'enregistrement MIDI du DAW.
Usage : tools/gen_essais_partition.py DOSSIER_DE_SORTIE"""
import json, random, sys
from fractions import Fraction
from math import gcd

TICK = Fraction(1, 3840)

def fr(x):
    x = Fraction(x)
    return [x.numerator, x.denominator]

def note(start, ln, key, vel=0.7, voice=1):
    n = {"start": fr(start), "len": fr(ln), "key": key, "velocity": vel, "channel": 0}
    if voice != 1:
        n["voice"] = voice
    return n

def pres(tol=4):
    return {"staff": "treble", "grid": "auto", "autoTolerance": tol, "keyFifths": 0}

def clip(cid, name, start, length, notes, **extra):
    return {"id": cid, "name": name, "node": cid, "start": fr(start), "len": fr(length), "offset": [0, 1], "notes": notes, "selection": [],
            "presentation": pres(), **extra}

def played_triplets(jitter, seed):
    """4 mesures en 4/4 : triolets de croches, croches binaires, triolet de noires, doubles croches. Positions décalées d'au plus `jitter` ticks."""
    rnd = random.Random(seed)
    j = lambda: TICK * rnd.randint(-jitter, jitter) if jitter else 0
    notes = []
    keys = [72, 74, 76, 77, 79, 77, 76, 74]
    k = 0
    def add(pos, ln):
        nonlocal k
        s = max(Fraction(0), Fraction(pos) + j())
        e = Fraction(pos) + Fraction(ln) + j()
        notes.append(note(s, max(e - s, TICK * 40), keys[k % len(keys)]))
        k += 1
    for beat in range(4):                       # mesure 1 : quatre temps de triolets de croches
        for i in range(3):
            add(Fraction(beat, 4) + Fraction(i, 12), Fraction(1, 12))
    for i in range(8):                          # mesure 2 : croches binaires
        add(1 + Fraction(i, 8), Fraction(1, 8))
    for beat in range(2):                       # mesure 3 : triolets de noires sur deux temps, puis double croches
        for i in range(3):
            add(2 + Fraction(beat, 2) + Fraction(i, 6), Fraction(1, 6))
    for i in range(8):
        add(2 + Fraction(1, 2) + Fraction(i, 16), Fraction(1, 16))
    for beat in range(4):                       # mesure 4 : un temps de triolets au milieu de croches
        if beat == 2:
            for i in range(3):
                add(3 + Fraction(beat, 4) + Fraction(i, 12), Fraction(1, 12))
        else:
            for i in range(2):
                add(3 + Fraction(beat, 4) + Fraction(i, 8), Fraction(1, 8))
    notes.sort(key=lambda n: (Fraction(*n["start"]), n["key"]))
    return notes

def loaded(measures, tracks, sections):
    clips = []
    per = measures // sections
    names = [("Intro", "intro"), ("Couplet 1", "verse"), ("Refrain", "chorus"), ("Couplet 2", "verse"), ("Refrain", "chorus"), ("Pont", "bridge"), ("Refrain", "chorus"), ("Outro", "outro")]
    rnd = random.Random(7)
    for t in range(tracks):
        notes = []
        for m in range(measures):
            for i in range(8):
                base = 48 + 12 * (t % 4) + [0, 2, 4, 5, 7, 9, 11, 12][(i + m + t) % 8]
                notes.append(note(Fraction(m) + Fraction(i, 8), Fraction(1, 8), base, 0.5 + 0.4 * rnd.random()))
                if i % 4 == 0:
                    notes.append(note(Fraction(m) + Fraction(i, 8), Fraction(1, 4), base + 7, 0.6))
        clips.append(clip(1000 + t, f"Piste {t + 1}", 0, measures, notes))
    return clips, per, names

def split_sections(clips, per, names):
    out = []
    for c in clips:
        for s in range(len(names)):
            a, b = Fraction(s * per), Fraction((s + 1) * per)
            sub = [n for n in c["notes"] if a <= Fraction(*n["start"]) < b]
            nm, cat = names[s]
            out.append({**c, "id": f"{c['id']}:{s}", "start": fr(a), "len": fr(b - a), "offset": fr(a), "notes": sub, "section": {"name": nm, "category": cat}})
    return out

if __name__ == "__main__":
    d = sys.argv[1]
    json.dump([clip(1, "Triolets parfaits", 0, 4, played_triplets(0, 1))], open(f"{d}/triolets-parfaits.json", "w"), ensure_ascii=False)
    for jit in (2, 5):
        json.dump([clip(1, f"Triolets joues +-{jit} ticks", 0, 4, played_triplets(jit, jit))], open(f"{d}/triolets-joues-{jit}-ticks.json", "w"), ensure_ascii=False)
    clips, per, names = loaded(128, 10, 8)
    json.dump(clips, open(f"{d}/projet-charge-10-pistes-128-mesures.json", "w"), ensure_ascii=False)
    json.dump(split_sections(clips, per, names), open(f"{d}/projet-charge-par-sections.json", "w"), ensure_ascii=False)
