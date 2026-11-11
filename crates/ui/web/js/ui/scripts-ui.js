// Menu « Scripts » (liste des scripts enregistrés, lancement par clic ou raccourci) et console (taper un script, le lancer, lire sa
// sortie, l'enregistrer). L'exécution est celle de `ScriptRunner` : un Worker, une seule action annulable par lancement.

import { ScriptRunner, parseHeader, parseShortcut, shortcutMatches } from "../script-runner.js";
import { createScriptStore, toFileName } from "../script-store.js";
import { makeFloatable } from "./float-frame.js";
import { ScriptFs, createFsBackend, consentText, hashText, unknownPermissions } from "../script-fs.js";
import { makeDialogs } from "../project-files.js";

const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

const CONSOLE_HINT = `// Un script est le corps d'une fonction asynchrone. Exemple :
const { project } = await daw.state();
daw.log(project.nodes.length + " nœuds");
// await daw.send({ type: "rename_track", node: 1, name: "Kick" });
`;

export class ScriptsUI {
  #runner;
  #store;
  #notify;
  #state;
  #dialogs;
  #fsBackend;
  #scripts = []; // { file, source, meta, shortcut }
  #menu = null;
  #console = null;
  #lines = []; // sortie gardée même console fermée : elle s'affiche à l'ouverture
  #status = document.getElementById("script-status");

  constructor(engine, state, notify, store = createScriptStore(), runner = new ScriptRunner(engine, state), { dialogs = makeDialogs(), fsBackend = createFsBackend() } = {}) {
    this.#state = state;
    this.#dialogs = dialogs;
    this.#fsBackend = fsBackend;
    this.#runner = runner;
    this.#store = store;
    this.#notify = notify;
    document.getElementById("btn-scripts")?.addEventListener("click", (e) => {
      e.stopPropagation();
      this.#toggleMenu();
    });
    this.#status?.addEventListener("click", () => this.#runner.abort());
    document.addEventListener("click", (e) => {
      if (this.#menu && !this.#menu.contains(e.target)) this.#closeMenu();
    });
    this.reload();
  }

  /** Relit la liste des scripts. */
  async reload() {
    try {
      const list = await this.#store.list();
      this.#scripts = list.map((s) => {
        const meta = parseHeader(s.source);
        return { ...s, meta, shortcut: parseShortcut(meta.shortcut) };
      });
    } catch (e) {
      this.#scripts = [];
      this.#notify(`Scripts : ${e?.message ?? e}`);
    }
    if (this.#menu) this.#fillMenu();
  }

  /** Raccourci clavier : lance le script correspondant. Rend vrai si la touche a été prise. */
  handleKey(e) {
    const s = this.#scripts.find((x) => shortcutMatches(x.shortcut, e));
    if (!s) return false;
    e.preventDefault();
    this.runScript(s);
    return true;
  }

  async runScript(s) {
    const name = s.meta?.name || s.file.replace(/\.js$/, "");
    return this.#run(s.source, name, s.meta?.permissions ?? []);
  }

  async #run(source, name, permissions = []) {
    let fs = null;
    if (permissions.length) {
      const unknown = unknownPermissions(permissions);
      if (unknown.length) {
        this.#out(`« ${name} » demande une permission inconnue (${unknown.join(", ")}) : le script n'est pas lancé.`);
        this.#notify(`Script « ${name} » : permission inconnue`);
        return null;
      }
      if (!(await this.#authorize(name, source))) {
        this.#out(`« ${name} » : accès aux fichiers non autorisé, le script n'est pas lancé.`);
        return null;
      }
      fs = new ScriptFs({ name, backend: this.#fsBackend, confirm: (m) => this.#dialogs.confirm(m), projectDir: this.#state.file?.path ?? null, scriptsDir: this.#store.dir || null });
    }
    if (this.#runner.running) {
      this.#notify(this.#runner.listening ? `« ${this.#runner.listening} » écoute des événements : arrête-le d'abord (bouton de la barre)` : "Un script est déjà en cours");
      return null;
    }
    if (this.#status) {
      this.#status.hidden = false;
      this.#status.textContent = `Script « ${name} »… (cliquer pour l'arrêter)`;
    }
    const onEnd = (reason) => {
      if (this.#status) this.#status.hidden = true;
      this.#out(`■ « ${name} » n'écoute plus : ${reason}`);
      this.#notify(`Script « ${name} » arrêté : ${reason}`);
    };
    const r = await this.#runner.run(source, { name, onLog: (line) => this.#out(line), onEnd, fs });
    if (r.ok && r.listening?.length) {
      // Le script reste actif : le bouton de la barre l'arrête.
      if (this.#status) this.#status.textContent = `Script « ${name} » écoute : ${r.listening.join(", ")} (cliquer pour l'arrêter)`;
      this.#out(`✓ prêt, écoute : ${r.listening.join(", ")} (une réaction = un pas d'annulation)`);
      this.#notify(`Script « ${name} » à l'écoute`);
      return r;
    }
    if (this.#status) this.#status.hidden = true;
    if (r.ok) {
      this.#out(`✓ terminé (${r.commands} commande${r.commands > 1 ? "s" : ""})${r.result != null ? ` → ${JSON.stringify(r.result)}` : ""}`);
      this.#notify(`Script « ${name} » terminé`);
    } else {
      this.#out(`✗ ${r.error} — ce que le script avait fait est défait`);
      this.openConsole(); // l'erreur doit se voir : le bandeau du bas ne reste que quelques secondes
      this.#notify(`Script « ${name} » : ${r.error}`);
    }
    return r;
  }

  /** Première exécution d'un script qui demande les fichiers : confirmation, retenue pour ce texte exact (modifié, il est redemandé). */
  async #authorize(name, source) {
    const key = `${name}:${hashText(source)}`;
    let known = [];
    try {
      known = JSON.parse(localStorage.getItem("daw.scriptApprovals") ?? "[]");
    } catch { /* stockage indisponible : on redemande à chaque fois */ }
    if (known.includes(key)) return true;
    if (!(await this.#dialogs.confirm(consentText(name), { ok: "Autoriser", cancel: "Refuser" }))) return false;
    try {
      localStorage.setItem("daw.scriptApprovals", JSON.stringify([...known.slice(-199), key]));
    } catch { /* non mémorisé */ }
    return true;
  }

  // ---- menu ----------------------------------------------------------------------------------------------------------------

  #toggleMenu() {
    if (this.#menu) return this.#closeMenu();
    const btn = document.getElementById("btn-scripts");
    const menu = h("div", "scripts-menu");
    menu.setAttribute("role", "menu");
    document.body.append(menu);
    this.#menu = menu;
    const r = btn.getBoundingClientRect();
    const z = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--ui-zoom")) || 1;
    menu.style.left = `${r.left / z}px`;
    menu.style.top = `${r.bottom / z + 2}px`;
    this.#fillMenu();
  }

  #fillMenu() {
    const menu = this.#menu;
    menu.replaceChildren();
    if (!this.#scripts.length) menu.append(h("div", "scripts-empty", "Aucun script enregistré"));
    for (const s of this.#scripts) {
      const b = h("button", "scripts-item");
      b.type = "button";
      b.setAttribute("role", "menuitem");
      b.title = s.meta.description || s.file;
      b.append(h("span", "", s.meta.name || s.file.replace(/\.js$/, "")));
      if (s.shortcut) b.append(h("kbd", "", s.shortcut));
      else if (s.meta.shortcut) b.append(h("kbd", "scripts-bad", "raccourci non permis"));
      b.addEventListener("click", () => {
        this.#closeMenu();
        this.runScript(s);
      });
      menu.append(b);
    }
    menu.append(h("hr"));
    const con = h("button", "scripts-item");
    con.type = "button";
    con.textContent = "Console…";
    con.addEventListener("click", () => {
      this.#closeMenu();
      this.openConsole();
    });
    const re = h("button", "scripts-item");
    re.type = "button";
    re.textContent = "Actualiser la liste";
    re.addEventListener("click", () => this.reload());
    menu.append(con, re);
    if (this.#store.dir) menu.append(h("div", "scripts-dir", this.#store.dir));
  }

  #closeMenu() {
    this.#menu?.remove();
    this.#menu = null;
  }

  // ---- console -------------------------------------------------------------------------------------------------------------

  openConsole() {
    if (this.#console) {
      this.#console.root.hidden = false;
      this.#console.code.focus();
      return;
    }
    const root = h("section", "script-console");
    root.setAttribute("aria-label", "Console de scripts");
    const head = h("header", "sc-head");
    head.append(h("strong", "", "Console de scripts"));
    const close = h("button", "sc-close", "✕");
    close.type = "button";
    close.setAttribute("aria-label", "Fermer");
    close.addEventListener("click", () => (root.hidden = true));
    head.append(close);

    const code = h("textarea", "sc-code");
    code.spellcheck = false;
    code.value = CONSOLE_HINT;
    code.setAttribute("aria-label", "Code du script");
    const out = h("pre", "sc-out");
    out.setAttribute("aria-live", "polite");

    const bar = h("div", "sc-bar");
    const run = h("button", "text", "Exécuter");
    run.type = "button";
    run.title = "Exécuter (Ctrl+Entrée)";
    const stop = h("button", "text", "Arrêter");
    stop.type = "button";
    const name = h("input");
    name.type = "text";
    name.placeholder = "nom du script";
    name.maxLength = 60;
    name.setAttribute("aria-label", "Nom pour l'enregistrement");
    const save = h("button", "text", "Enregistrer");
    save.type = "button";
    save.title = "Enregistrer dans le dossier des scripts";
    bar.append(run, stop, name, save);

    run.addEventListener("click", () => this.#run(code.value, "console", parseHeader(code.value).permissions));
    stop.addEventListener("click", () => this.#runner.abort());
    code.addEventListener("keydown", (e) => {
      e.stopPropagation(); // la saisie ne déclenche aucun raccourci du DAW
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        run.click();
      } else if (e.key === "Tab") {
        e.preventDefault();
        const { selectionStart: a, selectionEnd: b } = code;
        code.setRangeText("  ", a, b, "end");
      }
    });
    name.addEventListener("keydown", (e) => e.stopPropagation());
    save.addEventListener("click", async () => {
      const file = toFileName(name.value || parseHeader(code.value).name || "script");
      const exists = this.#scripts.some((s) => s.file === file);
      try {
        await this.#store.save(file, code.value, exists);
        this.#out(`enregistré : ${file}${exists ? " (remplacé)" : ""}`);
        await this.reload();
      } catch (e) {
        this.#out(`✗ enregistrement : ${e?.message ?? e}`);
      }
    });

    root.append(head, code, bar, out);
    document.body.append(root);
    makeFloatable(root, { key: "daw-script-console", minW: 360, minH: 260 });
    this.#console = { root, code, out };
    out.textContent = this.#lines.map((l) => `${l}\n`).join("");
    out.scrollTop = out.scrollHeight;
    code.focus();
  }

  #out(line) {
    this.#lines.push(line);
    if (this.#lines.length > 300) this.#lines.shift();
    const c = this.#console;
    if (!c) return;
    c.out.textContent += `${line}\n`;
    c.out.scrollTop = c.out.scrollHeight;
  }
}
