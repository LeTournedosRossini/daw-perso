// Export : fenêtre « Exporter » (mixage ou stems en WAV / FLAC), avec avancement et annulation. L'export tourne dans le moteur sur
// un fil de travail : la lecture et l'édition restent possibles, et fermer la fenêtre ne l'arrête pas (un indicateur reste dans la
// barre). Les choix sont mémorisés dans le stockage local (avec repli si indisponible).

const KEY = "daw.export";
const FORMATS = [
  ["wav16", "WAV 16 bits", true],
  ["wav24", "WAV 24 bits", false],
  ["wav32f", "WAV 32 bits flottant", false],
  ["flac16", "FLAC 16 bits", true],
  ["flac24", "FLAC 24 bits", false],
];
const DEFAULTS = { mode: "mix", scope: "project", format: "wav24", dither: false, tail: 5, normalize: false, normalizeDb: -1 };

const $ = (id) => document.getElementById(id);

/** Commande `export` d'après les choix de la fenêtre. Fonction pure (testée). */
export function buildExportCommand(o, { clip = null, nodes = [], dir = null, name = "" } = {}) {
  const cmd = { type: "export", scope: o.scope === "clip" ? "range" : o.scope, mode: o.mode, format: o.format, tail_secs: Math.min(60, Math.max(0, Number(o.tail) || 0)) };
  if (o.scope === "clip") {
    cmd.start = clip.start;
    cmd.end = clip.start + clip.len;
  }
  if (o.mode === "stems") cmd.nodes = nodes;
  else if (o.normalize) cmd.normalize_db = Math.min(0, Math.max(-30, Number(o.normalizeDb)));
  if (FORMATS.find((f) => f[0] === o.format)?.[2] !== undefined && o.format !== "wav32f") cmd.dither = !!o.dither;
  if (dir) cmd.dir = dir;
  if (name.trim()) cmd.name = name.trim();
  return cmd;
}

/** Texte d'avancement : « 3 / 8 — Basse · 42 % ». */
export function progressText(e) {
  if (e.active) return `${Math.min(e.done + 1, e.total)} / ${e.total} — ${e.current} · ${Math.round(e.progress * 100)} %`;
  if (e.cancelled) return `Annulé — ${e.done} fichier(s) terminé(s)`;
  if (e.error) return `Échec — ${e.error}`;
  return `Terminé — ${e.done} fichier(s)`;
}

export class ExportDialog {
  constructor(state, client, files, dialogs) {
    this.state = state;
    this.client = client;
    this.files = files;
    this.dialogs = dialogs;
    this.dir = null; // dossier choisi à la main ; sinon `export/` dans le projet
    this.last = null; // dernier événement `export`
    this.o = { ...DEFAULTS, ...this.#load() };
    this.el = $("export-dialog");
    if (!this.el) return;
    this.#fill();
    client.on("export", (e) => {
      this.last = e;
      this.#showProgress();
    });
    client.on("error", (e) => {
      if (e.command === "export") this.#message(e.message, true);
    });
    $("btn-export")?.addEventListener("click", () => this.open());
    $("export-status")?.addEventListener("click", () => this.open());
    $("ex-go").addEventListener("click", () => this.#go());
    $("ex-cancel").addEventListener("click", () => this.client.send({ type: "cancel_export" }));
    $("ex-dir").addEventListener("click", () => this.#chooseDir());
    $("ex-dir-reset").addEventListener("click", () => {
      this.dir = null;
      this.#showDir();
    });
    for (const id of ["ex-mode-mix", "ex-mode-stems", "ex-scope", "ex-format", "ex-dither", "ex-tail", "ex-normalize", "ex-normalize-db"]) {
      $(id).addEventListener("change", () => this.#read(id));
    }
    this.el.addEventListener("close", () => this.#save());
  }

  #load() {
    try {
      return JSON.parse(localStorage.getItem(KEY) ?? "{}");
    } catch {
      return {};
    }
  }

  #save() {
    try {
      localStorage.setItem(KEY, JSON.stringify(this.o));
    } catch {
      /* stockage indisponible : les choix ne sont simplement pas mémorisés */
    }
  }

  #fill() {
    const fmt = $("ex-format");
    for (const [v, label] of FORMATS) fmt.append(new Option(label, v));
  }

  open() {
    if (!this.el) return;
    this.#sync();
    if (!this.el.open) this.el.showModal();
  }

  /** Remet la fenêtre d'accord avec les choix, le projet (pistes, boucle, clip sélectionné) et l'export en cours. */
  #sync() {
    const o = this.o;
    $("ex-mode-mix").checked = o.mode === "mix";
    $("ex-mode-stems").checked = o.mode === "stems";
    const scope = $("ex-scope");
    const clip = this.#selectedClip();
    scope.replaceChildren(new Option("Tout le projet", "project"), new Option(this.state.loop ? "La boucle" : "La boucle (aucune posée)", "loop"), new Option(clip ? `Le clip sélectionné (${clip.name})` : "Le clip sélectionné (aucun)", "clip"));
    scope.options[1].disabled = !this.state.loop;
    scope.options[2].disabled = !clip;
    if (scope.querySelector(`option[value="${o.scope}"]:not(:disabled)`)) scope.value = o.scope;
    else {
      o.scope = "project";
      scope.value = "project";
    }
    $("ex-format").value = o.format;
    $("ex-dither").checked = o.dither;
    $("ex-tail").value = String(o.tail);
    $("ex-normalize").checked = o.normalize;
    $("ex-normalize-db").value = String(o.normalizeDb);
    const stems = $("ex-stems");
    stems.replaceChildren();
    const previous = new Set(this.excluded ?? []);
    for (const n of this.state.project.nodes.filter((n) => n.kind !== "master")) {
      const l = document.createElement("label");
      const c = document.createElement("input");
      c.type = "checkbox";
      c.value = String(n.id);
      c.checked = !previous.has(n.id);
      c.addEventListener("change", () => {
        this.excluded = [...stems.querySelectorAll("input:not(:checked)")].map((i) => Number(i.value));
        this.#refreshEnabled();
      });
      l.append(c, ` ${n.name}`);
      stems.append(l);
    }
    $("ex-name").placeholder = this.state.file?.name || "nom du projet"; // vide = le nom du projet, quel qu'il soit à ce moment
    this.#showDir();
    this.#refreshEnabled();
    this.#showProgress();
  }

  #selectedClip() {
    const sel = this.state.selection;
    if (!sel) return null;
    for (const n of this.state.project.nodes) {
      const c = n.clips.find((c) => c.id === (sel.id ?? sel.clip ?? sel));
      if (c) return c;
    }
    return null;
  }

  #read(id) {
    const o = this.o;
    if (id === "ex-mode-mix" || id === "ex-mode-stems") o.mode = $("ex-mode-stems").checked ? "stems" : "mix";
    else if (id === "ex-scope") o.scope = $("ex-scope").value;
    else if (id === "ex-format") {
      o.format = $("ex-format").value;
      // Le dither suit le choix usuel du format (activé en 16 bits) ; l'utilisateur peut le changer ensuite.
      o.dither = FORMATS.find((f) => f[0] === o.format)[2];
      $("ex-dither").checked = o.dither;
    } else if (id === "ex-dither") o.dither = $("ex-dither").checked;
    else if (id === "ex-tail") o.tail = Number($("ex-tail").value);
    else if (id === "ex-normalize") o.normalize = $("ex-normalize").checked;
    else if (id === "ex-normalize-db") o.normalizeDb = Number($("ex-normalize-db").value);
    this.#refreshEnabled();
  }

  #refreshEnabled() {
    const o = this.o;
    const stems = o.mode === "stems";
    $("ex-stems-box").hidden = !stems;
    $("ex-normalize").disabled = stems;
    $("ex-normalize-db").disabled = stems || !o.normalize;
    $("ex-normalize-note").hidden = !stems;
    $("ex-dither").disabled = o.format === "wav32f";
    const running = !!this.last?.active;
    const none = stems && !this.#chosenNodes().length;
    $("ex-go").disabled = running || none;
    $("ex-cancel").hidden = !running;
  }

  #chosenNodes() {
    return [...$("ex-stems").querySelectorAll("input:checked")].map((i) => Number(i.value));
  }

  #showDir() {
    const path = this.state.file?.path;
    $("ex-dir-text").textContent = this.dir ?? (path ? `${path}/export` : "(le dossier du projet, une fois enregistré)");
    $("ex-dir-reset").hidden = this.dir === null;
  }

  async #chooseDir() {
    const picked = await this.dialogs.pickFolder("Dossier d'export");
    if (picked) {
      this.dir = picked;
      this.#showDir();
    }
  }

  async #go() {
    this.#message("");
    if (!this.dir && !this.state.file?.path && !(await this.files.ensureSaved())) {
      this.#message("L'export a besoin d'un dossier : enregistre le projet ou choisis un dossier d'export.", true);
      return;
    }
    this.last = null;
    const cmd = buildExportCommand(this.o, { clip: this.#selectedClip(), nodes: this.#chosenNodes(), dir: this.dir, name: $("ex-name").value });
    this.client.send(cmd);
  }

  #message(text, error = false) {
    const m = $("ex-message");
    m.textContent = text;
    m.classList.toggle("error", error);
    m.hidden = !text;
  }

  #showProgress() {
    const e = this.last;
    const bar = $("ex-progress");
    const status = $("export-status");
    if (status) {
      status.hidden = !e?.active;
      if (e?.active) status.textContent = `Export ${Math.round(e.progress * 100)} %`;
    }
    if (!this.el) return;
    bar.hidden = !e;
    $("ex-result").hidden = !e;
    if (e) {
      bar.value = e.progress;
      $("ex-progress-text").textContent = progressText(e);
      const list = $("ex-files");
      list.replaceChildren(
        ...e.files.map((f) => {
          const li = document.createElement("li");
          const peak = f.peak_db === null ? "silence" : `pic ${f.peak_db.toFixed(1)} dBFS`;
          li.textContent = `${f.path.replace(/^.*[\\/]/, "")} — ${peak}${f.applied_gain_db ? `, gain ${f.applied_gain_db > 0 ? "+" : ""}${f.applied_gain_db.toFixed(1)} dB` : ""}`;
          li.title = f.path;
          return li;
        }),
      );
      $("ex-warnings").textContent = e.warnings.join("\n");
      $("ex-warnings").hidden = !e.warnings.length;
      if (!e.active && e.error) this.#message(e.error, true);
    }
    this.#refreshEnabled();
  }
}
