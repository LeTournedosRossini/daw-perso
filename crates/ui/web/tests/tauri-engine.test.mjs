import test from "node:test";
import assert from "node:assert/strict";
import { TauriEngine } from "../js/tauri-engine.js";

function fakeTauri() {
  const calls = [];
  let handler = null;
  return {
    calls,
    emitFromEngine: (payload) => handler({ payload }),
    event: { listen: async (name, fn) => { assert.equal(name, "engine-event"); handler = fn; return () => {}; } },
    core: { invoke: async (cmd, args) => { calls.push([cmd, args]); if (args.msg.type === "boom") throw new Error("kaboom"); } },
  };
}

test("les événements du moteur sont republiés par type, les autres charges utiles ignorées", async () => {
  const t = fakeTauri();
  const e = new TauriEngine(t);
  const seen = [];
  e.on("position", (ev) => seen.push(ev));
  await e.start();
  t.emitFromEngine({ type: "position", pos: 5, playing: true, loop: null });
  t.emitFromEngine({ type: "inconnu-sans-abonne" });
  t.emitFromEngine(null);
  t.emitFromEngine({ pas_de_type: 1 });
  assert.deepEqual(seen, [{ type: "position", pos: 5, playing: true, loop: null }]);
});

test("les commandes valides partent par invoke, les invalides sont refusées localement", async () => {
  const t = fakeTauri();
  const e = new TauriEngine(t);
  const errors = [];
  e.on("error", (ev) => errors.push(ev));
  await e.start();
  assert.equal(e.send({ type: "seek", pos: 10 }), true);
  assert.deepEqual(t.calls, [["send_command", { msg: { type: "seek", pos: 10 } }]]);
  assert.equal(e.send({ type: "seek", pos: -1 }), false);
  assert.equal(t.calls.length, 1, "rien n'est envoyé au moteur");
  assert.equal(errors[0].command, "seek");
});

test("un échec de transmission devient une erreur du moteur", async () => {
  const t = fakeTauri();
  const e = new TauriEngine(t);
  const errors = [];
  e.on("error", (ev) => errors.push(ev));
  await e.start();
  // `boom` n'est pas une commande du protocole : on contourne la validation en appelant l'invoke directement via send valide.
  t.core.invoke = async () => { throw new Error("canal fermé"); };
  e.send({ type: "transport", action: "play" });
  await new Promise((r) => setTimeout(r, 0));
  assert.match(errors[0].message, /transmission impossible.*canal fermé/);
});
