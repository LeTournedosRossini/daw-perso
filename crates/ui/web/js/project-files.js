// Nouveau, ouvrir, enregistrer : boutons et raccourcis, titre de la fenêtre, confirmation avant de perdre des modifications.
// Les dialogues sont ceux du système dans l'application (plugin Tauri « dialog ») ; dans un navigateur ordinaire, de simples
// invites (le faux moteur ne lit pas le disque : il garde en mémoire ce qu'il a « enregistré »).

export function makeDialogs(tauri = globalThis.__TAURI__) {
  const d = tauri?.dialog;
  if (d) {
    return {
      confirm: (message, labels = {}) => d.ask(message, { title: "DAW", kind: "warning", okLabel: labels.ok ?? "Continuer", cancelLabel: labels.cancel ?? "Annuler" }),
      pickFolder: (title = "Ouvrir un projet (dossier .daw)") => d.open({ directory: true, multiple: false, title }),
      pickSave: (defaultName) => d.save({ title: "Enregistrer le projet sous", defaultPath: defaultName }),
      pickJson: async () => {
        const r = await d.open({ multiple: false, directory: false, title: "Importer une carte d'accords", filters: [{ name: "Carte (flibuste-carte, JSON)", extensions: ["json"] }] });
        return Array.isArray(r) ? r[0] : r;
      },
      pickSaveJson: (defaultName) => d.save({ title: "Exporter la carte d'accords", defaultPath: defaultName, filters: [{ name: "Carte (JSON)", extensions: ["json"] }] }),
      pickFiles: async () => {
        const r = await d.open({ multiple: true, directory: false, title: "Importer de l'audio", filters: [{ name: "Audio (WAV, FLAC, MP3)", extensions: ["wav", "flac", "mp3"] }] });
        return Array.isArray(r) ? r : r ? [r] : [];
      },
    };
  }
  return {
    confirm: async (message) => globalThis.confirm(message),
    pickFolder: async (title) => globalThis.prompt(title ? `${title} :` : "Dossier du projet à ouvrir (.daw) :", title ? "export" : "Chanson.daw"),
    pickSave: async (defaultName) => globalThis.prompt("Enregistrer le projet sous (nom du dossier) :", defaultName),
    pickJson: async () => globalThis.prompt("Fichier de carte à importer (chemin) :", "carte.json"),
    pickSaveJson: async (defaultName) => globalThis.prompt("Exporter la carte vers (chemin) :", defaultName),
    // Navigateur ordinaire : le sélecteur de fichiers ne donne pas de chemins, seulement des noms (le faux moteur s'en contente).
    pickFiles: () =>
      new Promise((resolve) => {
        const input = document.createElement("input");
        input.type = "file";
        input.multiple = true;
        input.accept = ".wav,.flac,.mp3,audio/*";
        input.addEventListener("change", () => resolve([...input.files].map((f) => f.name)));
        input.addEventListener("cancel", () => resolve([]));
        input.click();
      }),
  };
}

const LOSS = "Des modifications ne sont pas enregistrées. Les perdre ?";

/** Titre de la fenêtre pour un état de fichier. */
export const windowTitle = (f) => `${f.dirty ? "● " : ""}${f.name} — DAW`;

export class ProjectFiles {
  /** `hooks` : `onLoaded()` (un autre projet vient d'être chargé : vider les caches), `onWarnings(liste)`. */
  constructor(engine, state, dialogs, hooks = {}, tauri = globalThis.__TAURI__) {
    this.engine = engine;
    this.state = state;
    this.dialogs = dialogs;
    this.hooks = hooks;
    this.tauri = tauri;
    state.file = { name: "Sans titre", path: null, dirty: false, generation: 0, warnings: [] };
    engine.on("file", (f) => {
      const changed = f.generation !== this.state.file.generation;
      this.state.file = f;
      this.#show();
      if (changed) this.hooks.onLoaded?.();
      if (f.warnings?.length) this.hooks.onWarnings?.(f.warnings);
    });
    // Un projet à une autre fréquence que le moteur : rien n'a changé ; on propose une copie convertie (l'original reste intact).
    engine.on("rate_mismatch", (m) => this.#offerConversion(m));
    const status = document.getElementById("convert-status");
    status?.addEventListener("click", () => engine.send({ type: "cancel_convert" }));
    engine.on("convert", (e) => {
      if (status) {
        status.hidden = !e.active;
        if (e.active) status.textContent = `Conversion ${Math.round(e.progress * 100)} %`;
      }
      if (!e.active) this.hooks.onWarnings?.([e.cancelled ? "Conversion annulée." : e.error ? `Conversion impossible — ${e.error}` : `Copie convertie à ${this.state.project.sample_rate} Hz ouverte : ${e.current}`]);
    });
    // Une sauvegarde automatique plus récente que le projet (ou d'un projet sans titre) : on propose de la reprendre.
    engine.on("autosave", (a) => this.#offerRecovery(a));
    for (const [id, action] of [["btn-new", () => this.newProject()], ["btn-open", () => this.open()], ["btn-save", () => this.save()], ["btn-saveas", () => this.saveAs()]]) {
      document.getElementById(id)?.addEventListener("click", action);
    }
    this.#guardClose();
  }

  async #offerRecovery({ path, untitled, modified }) {
    const when = new Date(modified * 1000).toLocaleString("fr-FR");
    const what = untitled ? "d'un projet qui n'avait pas été enregistré" : "plus récente que le projet enregistré";
    const message = `Une sauvegarde automatique ${what} existe (${when}). La reprendre ?`;
    if (await this.dialogs.confirm(message, { ok: "Reprendre", cancel: "Ignorer" })) this.engine.send({ type: "open_project", path, recover: true });
    else this.engine.send({ type: "discard_autosave", path });
  }

  async #offerConversion({ path, project_rate: from, engine_rate: to }) {
    const name = path.replace(/^.*[\\/]/, "").replace(/\.daw$/i, "");
    const message = `« ${name} » est à ${from} Hz et le moteur audio tourne à ${to} Hz. Créer une copie convertie à ${to} Hz (dans un nouveau dossier voisin, l'original n'est pas modifié) et l'ouvrir ?`;
    if (await this.dialogs.confirm(message, { ok: "Convertir", cancel: "Annuler" })) this.engine.send({ type: "convert_project", path });
  }

  #show() {
    const f = this.state.file;
    const title = windowTitle(f);
    document.title = title;
    const label = document.getElementById("project-name");
    if (label) {
      label.textContent = `${f.dirty ? "● " : ""}${f.name}`;
      label.title = f.path ? `${f.path}${f.dirty ? " — modifications non enregistrées" : ""}` : "Projet non enregistré";
      label.classList.toggle("dirty", f.dirty);
    }
    this.tauri?.window?.getCurrentWindow?.().setTitle(title).catch?.(() => {});
  }

  /** Dans l'application : la fermeture de la fenêtre passe par la page, qui confirme s'il y a des modifications. */
  #guardClose() {
    const t = this.tauri;
    if (!t?.core?.invoke || !t?.event?.listen) return;
    t.event
      .listen("close-requested", async () => {
        if (!this.state.file.dirty || (await this.dialogs.confirm(LOSS))) t.core.invoke("quit");
      })
      .then(() => t.core.invoke("close_guard", { enabled: true }))
      .catch(() => {}); // sans le garde, la fenêtre se ferme normalement
  }

  async #discard() {
    return !this.state.file.dirty || (await this.dialogs.confirm(LOSS));
  }

  async newProject() {
    if (await this.#discard()) this.engine.send({ type: "new_project" });
  }

  async open() {
    if (!(await this.#discard())) return;
    const path = await this.dialogs.pickFolder();
    if (path) this.engine.send({ type: "open_project", path });
  }

  /** Enregistre dans le dossier du projet ; s'il n'en a pas encore, demande où (« Enregistrer sous »). */
  async save() {
    if (this.state.file.path) this.engine.send({ type: "save_project" });
    else await this.saveAs();
  }

  /** Demande où enregistrer et enregistre ; rend le chemin choisi, ou `null` si on a annulé. */
  async saveAs() {
    const path = await this.dialogs.pickSave(this.state.file.name === "Sans titre" || this.state.file.name === "Démonstration" ? "Mon projet" : this.state.file.name);
    if (path) this.engine.send({ type: "save_project", path });
    return path || null;
  }

  /** Les prises s'écrivent dans le dossier du projet : s'il n'en a pas encore, on le demande d'abord. Vrai quand il existe. */
  async ensureSaved() {
    if (this.state.file.path) return true;
    // On s'abonne avant d'envoyer : la réponse (événement `file` avec un chemin) peut arriver avant la fin de l'attente.
    let off = () => {};
    let timer;
    const saved = new Promise((resolve) => {
      timer = setTimeout(() => resolve(false), 8000);
      off = this.engine.on("file", (f) => {
        if (f.path) resolve(true);
      });
    });
    const path = await this.saveAs();
    const ok = path ? await saved : false;
    clearTimeout(timer);
    off();
    return ok;
  }
}
