import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";

test("retours : validation des commandes du serveur web", () => {
  assert.equal(validateCommand({ type: "start_monitor" }), null);
  assert.equal(validateCommand({ type: "start_monitor", port: 8780 }), null);
  assert.match(validateCommand({ type: "start_monitor", port: 70000 }), /port/);
  assert.match(validateCommand({ type: "start_monitor", port: "x" }), /port/);
  assert.equal(validateCommand({ type: "stop_monitor" }), null);
});

test("faux moteur : démarrer le serveur donne une adresse et un code QR à chaque retour, l'arrêter les retire", () => {
  const e = new MockEngine();
  const log = [];
  e.on("monitor", (m) => log.push(m));
  e.send({ type: "get_project" });
  const nodes = [];
  e.on("project", (p) => nodes.push(p));
  e.send({ type: "add_track", kind: "return", name: "Retour Marie" });
  const bus = nodes.at(-1).nodes.find((n) => n.name === "Retour Marie").id;
  e.send({ type: "set_return_output", node: bus, output: { first: 3, stereo: true, mono: false } });
  e.send({ type: "start_monitor" });
  const on = log.at(-1);
  assert.equal(on.active, true);
  assert.deepEqual(on.returns.map((r) => r.name), ["Retour Marie"]);
  assert.match(on.returns[0].url, /^http:\/\/.+\/r\/[0-9a-f]{32}$/);
  assert.match(on.returns[0].qr, /<svg/);
  e.send({ type: "stop_monitor" });
  assert.equal(log.at(-1).active, false);
  assert.deepEqual(log.at(-1).returns, []);
});

test("the mock engine keeps a return's address and renews it on demand", async () => {
  const { MockEngine } = await import("../js/mock-engine.js");
  const e = new MockEngine();
  let mon;
  let err;
  e.on("monitor", (m) => (mon = m));
  e.on("error", (x) => (err = x));
  let p;
  e.on("project", (x) => (p = x));
  e.send({ type: "get_project" });
  e.send({ type: "add_track", kind: "return", name: "Retour" });
  const ret = p.nodes.find((n) => n.name === "Retour").id;
  e.send({ type: "set_return_output", node: ret, output: { first: 3, stereo: true, mono: false } });
  e.send({ type: "start_monitor" });
  const first = mon.returns[0].url;
  e.send({ type: "stop_monitor" });
  e.send({ type: "start_monitor" });
  assert.equal(mon.returns[0].url, first, "même adresse après un arrêt");
  e.send({ type: "renew_monitor_address", node: ret });
  assert.notEqual(mon.returns[0].url, first);
  e.send({ type: "renew_monitor_address", node: p.nodes[1].id });
  assert.match(err.message, /retour de monitoring/);
});
