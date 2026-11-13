#!/usr/bin/env python3
"""Rassemble les plugins CLAP à livrer avec l'application.

    fetch_plugins.py --os macos|linux|windows --dest DOSSIER            # télécharge les Releases listées dans plugins.json (outil `gh`)
    fetch_plugins.py --os macos --dest DOSSIER --local DOSSIER_ARCHIVES  # utilise des archives déjà présentes (essais, sans réseau)

Chaque archive (.zip, .tar.gz) est dépaquetée dans un dossier temporaire ; tout `.clap` trouvé (fichier, ou dossier sous macOS) est copié
dans DEST. Deux plugins de même nom : le premier est gardé. Code de sortie 1 s'il n'y a aucun plugin pour ce système.
"""
import argparse, json, os, re, shutil, subprocess, sys, tarfile, tempfile, zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent


def archives_from_github(src, pattern, work):
    repo, tag = src["repo"], src.get("tag", "latest")
    cmd = ["gh", "release", "download", "-R", repo, "--dir", str(work), "--clobber", "--pattern", "*"]
    if tag != "latest":
        cmd.insert(3, tag)
    try:
        subprocess.run(cmd, check=True, capture_output=True, text=True)
    except subprocess.CalledProcessError as e:
        print(f"  {repo} ({tag}) : pas de Release téléchargeable ({e.stderr.strip() or 'erreur'})", file=sys.stderr)
        return []
    return [p for p in sorted(work.iterdir()) if re.search(pattern, p.name, re.I) and p.suffix in (".zip", ".gz")]


def unpack(archive, into):
    into.mkdir(parents=True, exist_ok=True)
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as z:
            z.extractall(into)
            # `zipfile` ne restitue pas les droits d'exécution : nécessaire pour l'exécutable d'un bundle macOS.
            for info in z.infolist():
                mode = info.external_attr >> 16
                if mode:
                    os.chmod(into / info.filename, mode & 0o777)
    else:
        with tarfile.open(archive) as t:
            t.extractall(into, filter="tar") if sys.version_info >= (3, 12) else t.extractall(into)


def normalized(name):
    """Nom comparable : sans casse, sans espaces, sans extension .clap / archive."""
    name = re.sub(r"(\.clap|\.zip|\.tar\.gz|\.gz)$", "", name, flags=re.I)
    return re.sub(r"[\s_-]+", "", name).lower()


def is_excluded(name, excluded):
    """Vrai si `name` (fichier .clap ou archive) commence par un nom exclu (ex. `EssaiCarte` : « Essai Carte.clap », `EssaiCarte-macOS.tar.gz`)."""
    n = normalized(name)
    return any(n.startswith(e) for e in excluded)


def find_claps(root):
    claps = []
    for dirpath, dirnames, filenames in os.walk(root):
        for d in list(dirnames):
            if d.endswith(".clap"):
                claps.append(Path(dirpath) / d)
                dirnames.remove(d)  # un bundle macOS n'est pas fouillé
        claps += [Path(dirpath) / f for f in filenames if f.endswith(".clap")]
    return sorted(claps, key=lambda p: (len(p.parts), str(p)))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--os", required=True, choices=["macos", "linux", "windows"])
    ap.add_argument("--dest", required=True)
    ap.add_argument("--manifest", default=str(HERE / "plugins.json"))
    ap.add_argument("--local", help="dossier d'archives à utiliser à la place des Releases GitHub")
    args = ap.parse_args()
    manifest = json.load(open(args.manifest, encoding="utf-8"))
    pattern = manifest["match"][args.os]
    excluded = [normalized(e) for e in manifest.get("exclude", [])]
    dest = Path(args.dest)
    dest.mkdir(parents=True, exist_ok=True)
    kept = {}
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        sources = []
        if args.local:
            sources = [[p for p in sorted(Path(args.local).iterdir()) if re.search(pattern, p.name, re.I) and p.suffix in (".zip", ".gz")]]
        else:
            for i, src in enumerate(manifest["sources"]):
                work = tmp / f"dl{i}"
                work.mkdir()
                print(f"{src['repo']} ({src.get('tag', 'latest')}) ...")
                sources.append(archives_from_github(src, pattern, work))
        for n, archives in enumerate(sources):
            for a in archives:
                if is_excluded(a.name, excluded):
                    print(f"  - {a.name}  (exclu)")
                    continue
                out = tmp / f"x{n}-{a.name}"
                try:
                    unpack(a, out)
                except Exception as e:  # archive abîmée : on le dit et on continue
                    print(f"  {a.name} : illisible ({e})", file=sys.stderr)
                    continue
                for c in find_claps(out):
                    if c.name in kept or is_excluded(c.name, excluded):
                        continue
                    target = dest / c.name
                    if target.exists():
                        shutil.rmtree(target) if target.is_dir() else target.unlink()
                    shutil.copytree(c, target, symlinks=True) if c.is_dir() else shutil.copy2(c, target)
                    kept[c.name] = a.name
                    print(f"  + {c.name}  ({a.name})")
    if not kept:
        print(f"aucun plugin pour {args.os}", file=sys.stderr)
        return 1
    print(f"{len(kept)} plugin(s) dans {dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
