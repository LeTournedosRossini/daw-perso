// Aides des scripts pour les notes et les clips, construites sur `daw.state()` / `daw.send()` : rien ici ne touche au DAW autrement
// que par les commandes ordinaires. La fonction est recopiée telle quelle dans le Worker (`toString`) : elle ne doit donc dépendre
// d'aucun import ni d'aucune variable du module. Voir docs/scripts.md.

export function scriptLib(daw, call) {
  const TICKS = 3840; // fraction de ronde = ticks / 3840 (960 par noire, comme le DAW)
  const gcd = (a, b) => (b === 0 ? a : gcd(b, a % b));
  const reduce = (n, d) => {
    const g = gcd(Math.abs(n), d) || 1;
    return [n / g, d / g];
  };
  const frac = {
    /** Durée de `ticks` ticks (960 par noire) en fraction de ronde. */
    ticks: (t) => reduce(Math.round(t), TICKS),
    toTicks: (f) => (f[0] * TICKS) / f[1],
    add: (a, b) => reduce(a[0] * b[1] + b[0] * a[1], a[1] * b[1]),
    value: (f) => f[0] / f[1],
  };
  /** Générateur pseudo-aléatoire déterministe (mulberry32) : un script « humaniser » rend le même résultat à graine égale. */
  const rng = (seed) => {
    let a = seed >>> 0;
    return () => {
      a = (a + 0x6d2b79f5) >>> 0;
      let t = a;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  };
  const need = (cond, message) => {
    if (!cond) throw new Error(message);
  };
  const pick = (all, indices) => (indices ? all.filter((n) => indices.includes(n.index)) : all);

  const selection = async ({ kind } = {}) => {
    const { project, selection: ids } = await daw.state();
    if (kind === undefined) return ids;
    const want = kind === "notes" ? "midi" : kind;
    const kinds = new Map(project.nodes.flatMap((n) => n.clips.map((c) => [c.id, c.kind])));
    return ids.filter((id) => kinds.get(id) === want);
  };

  const clips = {
    /** Tous les clips du projet (`node` : identifiant de la piste, `kind` : « audio » ou « midi »), filtrables. */
    list: async ({ kind, node, selected } = {}) => {
      const { project, selection: sel } = await daw.state();
      const out = [];
      for (const n of project.nodes) {
        if (node !== undefined && n.id !== node) continue;
        for (const c of n.clips) {
          if (kind !== undefined && c.kind !== kind) continue;
          if (selected && !sel.includes(c.id)) continue;
          out.push({ ...c, node: n.id });
        }
      }
      return out;
    },
    move: (ids, delta, nodeDelta) => daw.send({ type: "move_clips", clips: ids, delta, ...(nodeDelta ? { node_delta: nodeDelta } : {}) }),
    split: (ids, at) => daw.send({ type: "split_clips", clips: ids, at }),
    remove: (ids) => daw.send({ type: "delete_clips", clips: ids }),
    duplicate: (ids, delta, nodeDelta) => daw.send({ type: "duplicate_clips", clips: ids, ...(delta !== undefined ? { delta } : {}), ...(nodeDelta ? { node_delta: nodeDelta } : {}) }),
  };

  const notes = {
    /** Notes d'un clip de notes : `{ clip, start, len, offset, notes: [{ index, start, len, key, channel, velocity, voice }] }` (fractions `[n, d]`). */
    get: async (clip) => {
      need(Number.isInteger(clip), "daw.notes.get : l'identifiant d'un clip de notes est attendu");
      const r = await call("notes", clip);
      return { ...r, notes: r.notes.map((n, index) => ({ ...n, index })) };
    },
    /** Transpose (demi-tons). Une note qui sortirait de 0-127 est laissée en place. Rend le nombre de notes modifiées. */
    transpose: async (clip, semitones, { indices } = {}) => {
      need(Number.isInteger(semitones), "daw.notes.transpose : un nombre entier de demi-tons");
      const edits = pick((await notes.get(clip)).notes, indices)
        .filter((n) => n.key + semitones >= 0 && n.key + semitones <= 127 && semitones !== 0)
        .map((n) => ({ index: n.index, key: n.key + semitones }));
      if (edits.length) await daw.send({ type: "edit_notes", clip, edits, label: "Transposer" });
      return edits.length;
    },
    /** Multiplie les vélocités par `factor` (bornées à 1/127 … 1). Rend le nombre de notes modifiées. */
    scaleVelocity: async (clip, factor, { indices } = {}) => {
      need(factor > 0, "daw.notes.scaleVelocity : un facteur > 0");
      const edits = [];
      for (const n of pick((await notes.get(clip)).notes, indices)) {
        const v = Math.min(1, Math.max(1 / 127, n.velocity * factor));
        if (Math.abs(v - n.velocity) > 1e-9) edits.push({ index: n.index, velocity: v });
      }
      if (edits.length) await daw.send({ type: "edit_notes", clip, edits, label: "Vélocité" });
      return edits.length;
    },
    /** Fixe la vélocité (0 < v ≤ 1). */
    setVelocity: async (clip, velocity, { indices } = {}) => {
      need(velocity > 0 && velocity <= 1, "daw.notes.setVelocity : 0 < vélocité ≤ 1");
      const edits = pick((await notes.get(clip)).notes, indices).map((n) => ({ index: n.index, velocity }));
      if (edits.length) await daw.send({ type: "edit_notes", clip, edits, label: "Vélocité" });
      return edits.length;
    },
    /**
     * Humanise : décale chaque début de note d'au plus `timing` ticks (960 par noire) et change sa vélocité d'au plus `velocity`
     * (0-1), au hasard mais de façon reproductible (`seed`). Un début ne passe jamais avant 0.
     */
    humanize: async (clip, { timing = 0, velocity = 0, seed = 1, indices } = {}) => {
      const rand = rng(seed);
      const edits = [];
      for (const n of pick((await notes.get(clip)).notes, indices)) {
        const e = { index: n.index };
        if (timing > 0) {
          const dt = Math.round((rand() * 2 - 1) * timing);
          const moved = frac.add(n.start, frac.ticks(dt));
          if (moved[0] >= 0 && dt !== 0) e.start = moved;
        }
        if (velocity > 0) e.velocity = Math.min(1, Math.max(1 / 127, n.velocity + (rand() * 2 - 1) * velocity));
        if (Object.keys(e).length > 1) edits.push(e);
      }
      if (edits.length) await daw.send({ type: "edit_notes", clip, edits, label: "Humaniser" });
      return edits.length;
    },
    /** Décale les débuts de `ticks` ticks (négatif : plus tôt ; une note qui passerait avant 0 est laissée en place). */
    shift: async (clip, ticks, { indices } = {}) => {
      const edits = pick((await notes.get(clip)).notes, indices)
        .map((n) => ({ index: n.index, start: frac.add(n.start, frac.ticks(ticks)) }))
        .filter((e) => e.start[0] >= 0 && ticks !== 0);
      if (edits.length) await daw.send({ type: "edit_notes", clip, edits, label: "Décaler des notes" });
      return edits.length;
    },
    /** Aimante les débuts sur une grille (fraction de ronde, 1/16 par défaut), positions absolues. */
    quantize: (clip, grid = [1, 16]) => daw.send({ type: "quantize_notes", clip, grid }),
    /** Ajoute des notes `{ start: [n, d], len: [n, d], key, velocity }` (début depuis le début de la matière du clip). */
    add: (clip, list) => daw.send({ type: "add_notes", clip, notes: list }),
    /** Supprime des notes par indice (ceux de `notes.get`). */
    remove: (clip, indices) => daw.send({ type: "delete_notes", clip, indices }),
  };
  /** Fichiers texte (lot 4) : demande `// @permissions files` et l'autorisation de l'utilisateur ; voir docs/scripts.md. */
  const fs = {
    projectDir: () => call("fs", { op: "projectDir" }),
    scriptsDir: () => call("fs", { op: "scriptsDir" }),
    /** Dialogue « ouvrir » : chemin choisi (ou null). Le choix donne accès à ce fichier. `{ title?, extensions?: ["txt"] }` */
    pickFile: (o = {}) => call("fs", { op: "pickFile", ...o }),
    /** Dialogue « enregistrer sous » : chemin choisi (ou null). `{ title?, defaultName?, extensions? }` */
    pickSave: (o = {}) => call("fs", { op: "pickSave", ...o }),
    /** Dialogue « dossier » : chemin choisi (ou null). Le choix donne accès à ce dossier. */
    pickFolder: (o = {}) => call("fs", { op: "pickFolder", ...o }),
    readText: (path) => call("fs", { op: "readText", path }),
    /** Écrit un texte (1 Mo au plus). Un fichier qui existe déjà est refusé, sauf `{ overwrite: true }` (confirmé par l'utilisateur). */
    writeText: (path, text, o = {}) => call("fs", { op: "writeText", path, text, overwrite: !!o.overwrite }),
    /** Contenu d'un dossier : `[{ name, is_dir, size }]` (sans descendre dans les sous-dossiers). */
    list: (path) => call("fs", { op: "list", path }),
    join: (...parts) => parts.filter((x) => x !== "" && x != null).map((x, i) => (i === 0 ? String(x).replace(/[\\/]+$/, "") : String(x).replace(/^[\\/]+|[\\/]+$/g, ""))).join("/"),
  };

  return { selection, clips, notes, frac, fs };
}
