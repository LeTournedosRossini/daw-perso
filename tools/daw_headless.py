"""Client Python de `daw-headless` : le moteur du DAW sans fenêtre ni carte son, piloté par le protocole JSON de l'interface.

    from daw_headless import Daw
    with Daw() as d:                      # lance target/debug/daw-headless (ou DAW_HEADLESS=chemin)
        d.send({"type": "add_track", "kind": "stereo"})
        d.advance(48000)                  # une seconde d'audio, hors temps réel
        print(d.last("meters"))

Chaque commande renvoie la liste des événements reçus avant l'accusé de réception ; une commande refusée lève `DawError`.
"""
import json
import os
import subprocess
import sys


class DawError(Exception):
    pass


class Daw:
    def __init__(self, exe=None, rate=48000, block=64, demo=False):
        exe = exe or os.environ.get("DAW_HEADLESS") or os.path.join(os.path.dirname(__file__), "..", "target", "debug", "daw-headless")
        args = [exe, "--rate", str(rate), "--block", str(block)] + (["--demo"] if demo else [])
        self.p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self.events = []
        self._next = 1
        self._read_until_ack(None)  # prêt

    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.close()

    def close(self):
        if self.p.poll() is None:
            try:
                self.p.stdin.write('{"type":"quit"}\n')
                self.p.stdin.flush()
            except BrokenPipeError:
                pass
            self.p.wait(timeout=10)

    def _read_until_ack(self, want):
        got = []
        for line in self.p.stdout:
            e = json.loads(line)
            if e.get("type") == "ack":
                if e.get("error") and want is not None:
                    raise DawError(e["error"])
                break
            got.append(e)
        else:
            raise DawError("daw-headless s'est arrêté")
        self.events += got
        return got

    def send(self, cmd):
        """Envoie une commande ; rend les événements reçus (le premier `error` lève `DawError`)."""
        cid = self._next
        self._next += 1
        self.p.stdin.write(json.dumps({**cmd, "id": cid}) + "\n")
        self.p.stdin.flush()
        return self._read_until_ack(cid)

    def advance(self, frames):
        """Fait avancer le moteur de `frames` échantillons ; rend les événements (dont position et niveaux)."""
        return self.send({"type": "advance", "frames": int(frames)})

    def wait_for(self, event, timeout_ms=60000):
        """Attend un événement de tâche de fond (`import`, `export`, `plugins`…)."""
        return self.send({"type": "wait_for", "event": event, "timeout_ms": timeout_ms})

    def last(self, kind, events=None):
        for e in reversed(events if events is not None else self.events):
            if e.get("type") == kind:
                return e
        return None


if __name__ == "__main__":
    with Daw() as d:
        ev = d.send({"type": "get_project"})
        proj = d.last("project", ev)
        print("pistes :", [n["name"] for n in proj["nodes"]], file=sys.stderr)
