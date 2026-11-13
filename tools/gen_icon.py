#!/usr/bin/env python3
"""Génère l'icône PROVISOIRE de l'application (PNG 1024 × 1024, bibliothèque standard seulement).

    python3 tools/gen_icon.py crates/app/icons/provisoire-1024.png

Fond bleu nuit, barres blanches de hauteurs variées (une forme d'onde stylisée). À remplacer par une vraie icône.
"""
import struct, sys, zlib

N = 1024
BARS = [0.30, 0.55, 0.85, 0.45, 0.70, 0.95, 0.60, 0.35]


def pixel(x, y):
    # coins arrondis (rayon 180) : transparent à l'extérieur
    r = 180
    cx = min(max(x, r), N - 1 - r)
    cy = min(max(y, r), N - 1 - r)
    if (x - cx) ** 2 + (y - cy) ** 2 > r * r:
        return (0, 0, 0, 0)
    margin, gap = 150, 28
    w = (N - 2 * margin - gap * (len(BARS) - 1)) // len(BARS)
    for i, h in enumerate(BARS):
        x0 = margin + i * (w + gap)
        bh = int(h * (N - 2 * margin))
        y0 = (N - bh) // 2
        if x0 <= x < x0 + w and y0 <= y < y0 + bh:
            return (240, 244, 250, 255)
    return (24, 38, 74, 255)


rows = bytearray()
for y in range(N):
    rows.append(0)
    for x in range(N):
        rows += bytes(pixel(x, y))


def chunk(tag, data):
    c = struct.pack(">I", len(data)) + tag + data
    return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)


png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", N, N, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
