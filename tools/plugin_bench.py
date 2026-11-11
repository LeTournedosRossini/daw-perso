#!/usr/bin/env python3
"""Banc d'essai de plugin CLAP par l'hôte sans interface.

    tools/plugin_bench.py AnalogSynth            # instrument : joue une note, mesure le niveau
    tools/plugin_bench.py Compresseur --params   # liste aussi les paramètres

Charge le plugin (celui dont le nom ou l'identifiant contient l'argument) sur la piste d'un projet vide, affiche latence, paramètres et, pour un
instrument, le niveau de crête de la sortie pendant et après une note (la queue doit retomber). Les plugins sont cherchés dans les dossiers CLAP du système.
"""
import sys
import math

from daw_headless import Daw, DawError


def db(x):
    return -120.0 if x <= 1e-6 else 20 * math.log10(x)


def peak(events):
    m = 0.0
    for e in events:
        if e.get("type") == "meters":
            for l, r in e["levels"].values():
                m = max(m, l, r)
    return m


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    want = argv[1].lower()
    with Daw() as d:
        d.send({"type": "list_plugins"})
        listing = d.wait_for("plugins")
        plugins = d.last("plugins", listing)["plugins"]
        while plugins is not None and d.last("plugins", listing).get("scanning"):
            listing = d.wait_for("plugins")
        plugins = d.last("plugins", listing)["plugins"]
        hits = [p for p in plugins if want in p["name"].lower() or want in p["clap_id"].lower()]
        if not hits:
            print(f"aucun plugin ne correspond à « {argv[1]} » ({len(plugins)} plugins trouvés)")
            return 1
        p = hits[0]
        print(f"plugin : {p['name']} ({p.get('vendor', '?')}), {p['clap_id']}")
        proj = d.last("project", d.send({"type": "get_project"}))
        track = next(n["id"] for n in proj["nodes"] if n["kind"] in ("mono", "stereo"))
        try:
            ev = d.send({"type": "add_plugin", "node": track, "clap_id": p["clap_id"], "file": p.get("file")})
        except DawError as e:
            print("refusé :", e)
            return 1
        proj = d.last("project", ev) or d.last("project", d.send({"type": "get_project"}))
        ins = next(n for n in proj["nodes"] if n["id"] == track)["inserts"][-1]
        print(f"latence annoncée : {ins['latency']} échantillons, interface native : {'oui' if ins['gui'] else 'non'}")
        params = d.last("plugin_params", d.send({"type": "get_plugin_params", "insert": ins["id"]}))
        print(f"paramètres : {len(params['params']) if params else 0}")
        if "--params" in argv and params:
            for q in params["params"][:60]:
                print("  ", q.get("name"), q.get("min"), q.get("max"), q.get("default"))
        # Une note de La3 pendant une seconde, puis le silence ; on regarde les crêtes.
        d.send({"type": "preview_note", "node": track, "key": 57, "velocity": 0.8, "on": True})
        during = peak(d.advance(48000))
        d.send({"type": "preview_note", "node": track, "key": 57, "velocity": 0.8, "on": False})
        d.advance(48000)
        after = peak(d.advance(9600))
        print(f"niveau pendant la note : {db(during):.1f} dBFS ; une seconde après la fin : {db(after):.1f} dBFS")
        print("sans son : ce n'est pas un instrument (ou il attend du signal)" if during < 1e-4 else ("la queue ne retombe pas : note bloquée ?" if after > 0.5 * during else "la note s'éteint"))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
