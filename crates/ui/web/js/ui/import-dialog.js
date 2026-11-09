// Import audio : bouton « Importer… », glisser-déposer de fichiers (dans l'application : événements de glisser-déposer de Tauri, qui
// donnent des chemins ; dans un navigateur : événements HTML, qui ne donnent que des noms), et petite fenêtre qui dit ce qui va se
// passer (conversion de fréquence, fichiers à plusieurs canaux, copier ou référencer en place) avant d'envoyer `import_audio`.
// Le fichier est déposé sur la piste visée à la position visée (calée sur la grille) ; hors des pistes : une nouvelle piste.

import { buildImportCommand, describeFile, importProgressText, isAudioPath, needsChoice, referencable } from "../import-model.js";

const KEY = "daw.import";
const $ = (id) => document.getElementById(id);

export class ImportController {
  /** `say(lignes)` : affiche un message dans la barre d'état. */
  constructor(state, client, files, dialogs, timeline, say, tauri = globalThis.__TAURI__) {
    this.state = state;
    this.client = client;
    this.files = files;
    this.dialogs = dialogs;
    this.timeline = timeline;
    this.say = say;
    this.running = false;
    this.pref = { reference: false, multichannel: "split", ...this.#load() };
    $("btn-import")?.addEventListener("click", () => this.pick());
    const status = $("import-status");
    status?.addEventListener("click", () => this.client.send({ type: "cancel_import" }));
    client.on("import", (e) => {
      this.running = e.active;
      if (status) {
        status.hidden = !e.active;
        if (e.active) status.textContent = importProgressText(e);
      }
      if (!e.active) this.say([importProgressText(e), ...(e.warnings ?? [])]);
    });
    client.on("error", (e) => {
      if (e.command === "import_audio") this.say([e.message]);
    });
    this.#listenDrops(tauri);
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
      localStorage.setItem(KEY, JSON.stringify(this.pref));
    } catch {
      /* stockage indisponible : le choix n'est pas mémorisé */
    }
  }

  /** Bouton « Importer… » : choisir des fichiers ; ils vont sur de nouvelles pistes à la tête de lecture. */
  async pick() {
    const paths = await this.dialogs.pickFiles?.();
    if (paths?.length) await this.importFiles(paths, null);
  }

  #listenDrops(tauri) {
    const overlay = $("drop-overlay");
    const clear = () => {
      this.timeline.setDropHint(null);
      if (overlay) overlay.hidden = true;
    };
    if (tauri?.event?.listen) {
      // Positions physiques (pixels de l'écran) → pixels CSS de la page.
      const where = (p) => this.timeline.dropTarget(p.position.x / devicePixelRatio, p.position.y / devicePixelRatio);
      tauri.event.listen("tauri://drag-enter", (e) => {
        if (overlay) overlay.hidden = false;
        this.timeline.setDropHint(where(e.payload));
      });
      tauri.event.listen("tauri://drag-over", (e) => this.timeline.setDropHint(where(e.payload)));
      tauri.event.listen("tauri://drag-leave", clear);
      tauri.event.listen("tauri://drag-drop", (e) => {
        const target = where(e.payload);
        clear();
        const paths = (e.payload.paths ?? []).filter(isAudioPath);
        if (paths.length) this.importFiles(paths, target);
        else if ((e.payload.paths ?? []).length) this.say(["Rien à importer : seuls les fichiers WAV, FLAC et MP3 sont gérés."]);
      });
      return;
    }
    // Navigateur : on ne reçoit que les noms des fichiers (suffisant pour le faux moteur).
    const hasFiles = (e) => [...(e.dataTransfer?.types ?? [])].includes("Files");
    window.addEventListener("dragover", (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      if (overlay) overlay.hidden = false;
      this.timeline.setDropHint(this.timeline.dropTarget(e.clientX, e.clientY));
    });
    window.addEventListener("dragleave", (e) => {
      if (e.relatedTarget === null) clear();
    });
    window.addEventListener("drop", (e) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      const target = this.timeline.dropTarget(e.clientX, e.clientY);
      clear();
      const names = [...e.dataTransfer.files].map((f) => f.name).filter(isAudioPath);
      if (names.length) this.importFiles(names, target);
    });
  }

  /** Lit les en-têtes, montre la fenêtre, puis envoie l'import. `target` : `{ node, sample }` ou `null`. */
  async importFiles(paths, target) {
    if (this.running) {
      this.say(["Un import est déjà en cours."]);
      return;
    }
    const infos = await this.#probe(paths);
    if (!infos) {
      this.say(["Le moteur n'a pas répondu à la lecture des fichiers."]);
      return;
    }
    const choice = await this.#ask(infos);
    if (!choice) return;
    const reference = choice.reference && infos.some(referencable);
    if (!reference && !this.state.file?.path && !(await this.files.ensureSaved())) {
      this.say(["Copier les fichiers demande un dossier de projet : enregistre le projet, ou coche « référencer en place »."]);
      return;
    }
    this.client.send(buildImportCommand({ files: infos.filter((i) => !i.error).map((i) => i.path), target, reference: choice.reference, multichannel: needsChoice(infos) ? choice.multichannel : null }));
  }

  #probe(paths) {
    return new Promise((resolve) => {
      const timer = setTimeout(() => (off(), resolve(null)), 8000);
      const off = this.client.on("audio_info", (e) => {
        clearTimeout(timer);
        off();
        resolve(e.files);
      });
      this.client.send({ type: "probe_audio", files: paths });
    });
  }

  /** La fenêtre : liste des fichiers, choix multicanal, copier ou référencer. Renvoie les choix, ou `null` si on annule. */
  #ask(infos) {
    const dlg = $("import-dialog");
    const rate = this.state.project.sample_rate;
    const good = infos.filter((i) => !i.error);
    const list = $("im-files");
    list.replaceChildren(
      ...infos.map((i) => {
        const li = document.createElement("li");
        const name = document.createElement("strong");
        name.textContent = i.name;
        li.append(name, ` — ${describeFile(i, rate)}`);
        if (i.error) li.classList.add("bad");
        return li;
      }),
    );
    $("im-multi").hidden = !needsChoice(infos);
    $("im-multichannel").value = this.pref.multichannel;
    const canRef = good.some(referencable);
    const ref = $("im-ref");
    ref.disabled = !canRef;
    ref.checked = canRef && this.pref.reference;
    const mixed = good.some((i) => !referencable(i));
    $("im-ref-note").textContent = !canRef ? "Aucun de ces fichiers ne peut être référencé : fréquence différente de celle du projet, ou plus de deux canaux." : mixed ? "Ceux qui ne peuvent pas être référencés (autre fréquence, plus de deux canaux) seront copiés." : "";
    $("im-go").disabled = good.length === 0;
    return new Promise((resolve) => {
      const done = () => {
        dlg.removeEventListener("close", done);
        if (dlg.returnValue !== "go") return resolve(null);
        this.pref = { reference: ref.checked, multichannel: $("im-multichannel").value };
        this.#save();
        resolve({ ...this.pref });
      };
      dlg.returnValue = "cancel";
      dlg.addEventListener("close", done);
      dlg.showModal();
      $("im-go").focus();
    });
  }
}
