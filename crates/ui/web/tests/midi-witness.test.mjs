import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";

test("le faux moteur annonce l'activité MIDI d'une piste MIDI armée en lecture, puis son extinction", () => {
  const e = new MockEngine();
  const seen = [];
  e.on("midi_activity", (ev) => seen.push(ev.active));
  e.send({ type: "get_project" });
  const id = e.project.nodes.find((n) => n.kind === "stereo" || n.kind === "mono").id;
  e.send({ type: "set_track_input", node: id, input: { kind: "midi" } });
  e.send({ type: "set_node", node: id, arm: true });
  e.send({ type: "transport", action: "play" });
  e.advance(100);
  assert.deepEqual(seen.at(-1), { [id]: 1 });
  e.advance(100);
  assert.equal(seen.length, 1, "inchangé : pas de nouvel événement");
  e.send({ type: "transport", action: "stop" });
  e.advance(100);
  assert.deepEqual(seen.at(-1), {});
});
