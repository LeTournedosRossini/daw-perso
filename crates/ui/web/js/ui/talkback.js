// Talkback de la régie : bouton « Parler » (maintenir) dans l'en-tête du mixeur et fenêtre de réglage (micro, niveau, baisse du mix,
// retours visés). Maintien : on redit « je parle » toutes les 500 ms ; l'hôte coupe la voix sans nouvelle pendant 1,5 s.

import { desiredTargets, inputChoices, reconcile, returnsOf, talkLabel } from "../talkback-model.js";

const KEY = "daw-talkback";
const HOLD_MS = 500;

const load = () => {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return { channel: Number.isInteger(v.channel) ? v.channel : null, level_db: Number.isFinite(v.level_db) ? v.level_db : 0, duck_db: Number.isFinite(v.duck_db) ? v.duck_db : 12, unchecked: Array.isArray(v.unchecked) ? v.unchecked.filter((x) => typeof x === "string") : [] };
  } catch {
    return { channel: null, level_db: 0, duck_db: 12, unchecked: [] };
  }
};

const h = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

export class Talkback {
  constructor(state, client) {
    this.state = state;
    this.client = client;
    this.cfg = load();
    this.host = { channel: null, level_db: 0, duck_db: 12, targets: [], active: false };
    this.button = document.getElementById("mx-talk");
    this.gear = document.getElementById("mx-talk-cfg");
    this.timer = null;
    this.holding = false;
    if (!this.button) return;
    this.button.addEventListener("pointerdown", (e) => {
      if (e.button > 0) return;
      e.preventDefault();
      this.button.setPointerCapture?.(e.pointerId);
      this.start();
    });
    for (const ev of ["pointerup", "pointercancel", "lostpointercapture"]) this.button.addEventListener(ev, () => this.stop());
    this.button.addEventListener("keydown", (e) => {
      if ((e.key === " " || e.key === "Enter") && !e.repeat) {
        e.preventDefault();
        this.start();
      }
    });
    this.button.addEventListener("keyup", (e) => (e.key === " " || e.key === "Enter") && this.stop());
    window.addEventListener("blur", () => this.stop());
    this.gear?.addEventListener("click", () => this.openDialog());
    client.on("talkback", (e) => {
      this.host = e;
      this.paint();
    });
    client.on("project", () => this.sync());
    this.paint();
  }

  wanted() {
    return { channel: this.cfg.channel, level_db: this.cfg.level_db, duck_db: this.cfg.duck_db, targets: desiredTargets(this.state.project, this.cfg.unchecked) };
  }

  /** Remet l'hôte d'accord avec ce que la page retient (au démarrage, après un changement de structure : l'hôte oublie les retours visés). */
  sync() {
    if (this.holding) return;
    const msg = reconcile(this.host, this.wanted());
    if (msg) this.client.send(msg);
    this.paint();
  }

  save() {
    try {
      localStorage.setItem(KEY, JSON.stringify(this.cfg));
    } catch {
      // Pas de mémorisation possible : le réglage vaut pour cette session.
    }
  }

  start() {
    if (this.holding) return;
    this.holding = true;
    this.client.send({ type: "talk", active: true });
    this.timer = setInterval(() => this.client.send({ type: "talk", active: true }), HOLD_MS);
    this.paint();
  }

  stop() {
    if (!this.holding) return;
    this.holding = false;
    clearInterval(this.timer);
    this.timer = null;
    this.client.send({ type: "talk", active: false });
    this.paint();
  }

  paint() {
    const returns = returnsOf(this.state.project);
    const root = this.button?.parentElement;
    if (root) root.hidden = returns.length === 0;
    if (!this.button) return;
    const talking = this.holding || this.host.active;
    this.button.textContent = talkLabel({ channel: this.host.channel, targets: this.host.targets, active: talking });
    this.button.classList.toggle("talking", talking);
    this.button.setAttribute("aria-pressed", talking ? "true" : "false");
  }

  /** Fenêtre de réglage : chaque modification est appliquée tout de suite. */
  openDialog() {
    const project = this.state.project;
    const returns = returnsOf(project);
    const dlg = h("dialog", "group-dialog");
    const form = h("form");
    form.method = "dialog";
    form.append(h("h2", "", "Talkback"));
    form.append(h("p", "hint", "Un canal d'entrée de la carte sert de micro. Quand on maintient « Parler », la voix part dans les retours cochés et le reste de leur mix baisse."));

    const mic = h("select");
    mic.setAttribute("aria-label", "Entrée du micro");
    mic.append(new Option("Aucun micro", ""));
    for (const c of inputChoices(project?.audio_inputs ?? 0)) mic.append(new Option(`Entrée ${c}`, String(c)));
    mic.value = this.cfg.channel == null ? "" : String(this.cfg.channel);

    const range = (min, max, step, value, fmt) => {
      const input = h("input");
      input.type = "range";
      Object.assign(input, { min, max, step, value });
      const out = h("output", "", fmt(value));
      input.addEventListener("input", () => (out.textContent = fmt(Number(input.value))));
      return { input, out };
    };
    const level = range(-30, 12, 0.5, this.cfg.level_db, (v) => `${v > 0 ? "+" : ""}${v.toFixed(1)} dB`);
    const duck = range(0, 30, 1, this.cfg.duck_db, (v) => (v === 0 ? "pas de baisse" : `−${v} dB`));
    const row = (label, ...els) => {
      const r = h("label", "row");
      r.append(h("span", "", label), ...els);
      return r;
    };
    form.append(row("Micro", mic), row("Niveau de la voix", level.input, level.out), row("Baisse du mix", duck.input, duck.out));

    const list = h("fieldset");
    list.append(h("legend", "", "Retours qui entendent la voix"));
    if (!returns.length) list.append(h("p", "hint", "Aucun retour de monitoring dans ce projet."));
    const boxes = returns.map((n) => {
      const l = h("label", "row");
      const cb = h("input");
      cb.type = "checkbox";
      cb.checked = !this.cfg.unchecked.includes(n.name);
      l.append(cb, h("span", "", n.name));
      list.append(l);
      return { cb, name: n.name };
    });
    form.append(list);

    const apply = () => {
      this.cfg.channel = mic.value === "" ? null : Number(mic.value);
      this.cfg.level_db = Number(level.input.value);
      this.cfg.duck_db = Number(duck.input.value);
      const here = new Set(boxes.map((b) => b.name));
      this.cfg.unchecked = [...this.cfg.unchecked.filter((n) => !here.has(n)), ...boxes.filter((b) => !b.cb.checked).map((b) => b.name)];
      this.save();
      if (this.cfg.channel == null) this.client.send({ type: "set_talkback", channel: null, level_db: this.cfg.level_db, duck_db: this.cfg.duck_db, targets: [] });
      else this.sync();
    };
    for (const el of [mic, level.input, duck.input, ...boxes.map((b) => b.cb)]) el.addEventListener("change", apply);
    for (const el of [level.input, duck.input]) el.addEventListener("input", apply);

    const close = h("button", "", "Fermer");
    close.value = "close";
    form.append(close);
    dlg.append(form);
    dlg.addEventListener("close", () => dlg.remove());
    document.body.append(dlg);
    dlg.showModal();
  }
}
