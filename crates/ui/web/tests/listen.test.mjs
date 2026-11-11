import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { isListenable, listenedName } from "../js/ui/listen.js";

function setup() {
  const e = new MockEngine();
  const log = { errors: [], projects: [] };
  e.on("error", (x) => log.errors.push(x));
  e.on("project", (x) => log.projects.push(x));
  e.send({ type: "get_project" });
  return { e, log, last: () => log.projects.at(-1) };
}

test("set_listen : le numéro d'un nœud ou null", () => {
  assert.equal(validateCommand({ type: "set_listen", node: 5 }), null);
  assert.equal(validateCommand({ type: "set_listen", node: null }), null);
  for (const bad of [{}, { node: 0 }, { node: -1 }, { node: 1.5 }, { node: "3" }]) {
    assert.match(validateCommand({ type: "set_listen", ...bad }) ?? "", /set_listen/, JSON.stringify(bad));
  }
});

test("seuls les bus et les retours s'écoutent", () => {
  assert.equal(isListenable({ kind: "bus" }), true);
  assert.equal(isListenable({ kind: "stereo" }), false);
  assert.equal(isListenable({ kind: "mono" }), false);
  assert.equal(isListenable({ kind: "master" }), false);
});

test("faux moteur : écouter un bus, le changer, l'arrêter ; une piste est refusée", () => {
  const { e, log, last } = setup();
  e.send({ type: "add_track", kind: "return", name: "Retour Marie" });
  const ret = last().nodes.find((n) => n.name === "Retour Marie").id;
  assert.equal(last().listen, null);
  e.send({ type: "set_listen", node: ret });
  assert.equal(last().listen, ret);
  assert.equal(listenedName(last()), "Retour Marie");
  e.send({ type: "add_track", kind: "bus", name: "Sous-groupe" });
  assert.equal(last().listen, null, "un changement de structure défait l'écoute (les numéros bougent)");
  const bus = last().nodes.find((n) => n.name === "Sous-groupe").id;
  e.send({ type: "set_listen", node: bus });
  assert.equal(last().listen, bus);
  e.send({ type: "set_listen", node: null });
  assert.equal(last().listen, null);
  assert.equal(listenedName(last()), "");
  const track = last().nodes.find((n) => n.kind === "stereo" || n.kind === "mono").id;
  e.send({ type: "set_listen", node: track });
  assert.equal(log.errors.at(-1).command, "set_listen");
  assert.equal(last().listen, null);
  e.send({ type: "set_listen", node: 0 });
  assert.equal(log.errors.length, 2, "la sortie principale non plus");
});
