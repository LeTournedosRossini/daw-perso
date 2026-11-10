// Groupes de pistes : calculs purs, validation des commandes, faux moteur (création, annulation, mute / solo / niveau, retours).
import test from "node:test";
import assert from "node:assert/strict";

import { effectiveGroups, OFFSET_RANGE, anySendSolo, formatOffset, groupOffset, groupColor, groupFlags, groupMeanDb, groupMembers, groupable, GROUP_COLORS } from "../js/groups-model.js";
import { validateCommand, gainToDb } from "../js/protocol.js";
import { MockEngine } from "../js/mock-engine.js";

const node = (id, extra = {}) => ({ id, name: `n${id}`, kind: "stereo", group: 0, mute: false, solo: false, gain: 1, sends: [], ...extra });
const project = (nodes, groups = []) => ({ nodes: [{ id: 0, kind: "master", name: "Sortie", sends: [] }, ...nodes], groups });

test("a group's colour is its own, else a palette colour by position", () => {
  const p = project([], [{ id: 4, name: "A", color: "" }, { id: 9, name: "B", color: "#112233" }]);
  assert.equal(groupColor(p, p.groups[0]), GROUP_COLORS[0]);
  assert.equal(groupColor(p, p.groups[1]), "#112233");
});

test("only tracks and buses can be members, never the master or a monitoring return", () => {
  const p = project([node(1), node(2, { kind: "bus" }), node(3, { kind: "bus", is_return: true, phys: { first: 3 } })]);
  assert.deepEqual(groupable(p).map((n) => n.id), [1, 2]);
});

test("a group is muted or soloed when all its members are, on the tracks or on their sends to a return", () => {
  const p = project([node(1, { group: 1, mute: true, sends: [{ to: 9, gain: 1, mute: false, solo: true }] }), node(2, { group: 1, mute: true, sends: [{ to: 9, gain: 1, mute: false, solo: true }] }), node(3, { group: 1, mute: false })], [{ id: 1, name: "G" }]);
  assert.equal(groupFlags(p, 1).mute, false, "un membre n'est pas coupé");
  p.nodes[3].mute = true;
  assert.equal(groupFlags(p, 1).mute, true);
  assert.equal(groupFlags(p, 1, 9).solo, false, "le troisième n'a pas d'envoi");
  assert.equal(groupFlags(project([], [{ id: 5 }]), 5).any, false);
  assert.equal(anySendSolo(p, 9), true);
  assert.equal(anySendSolo(p, 8), false);
  assert.equal(groupMembers(p, 1).length, 3);
});

test("the mean level of a group in a return ignores members without a send", () => {
  const p = project([node(1, { group: 1, sends: [{ to: 9, gain: 1 }] }), node(2, { group: 1, sends: [{ to: 9, gain: 0.5 }] }), node(3, { group: 1 })], [{ id: 1 }]);
  assert.ok(Math.abs(groupMeanDb(p, 1, 9, gainToDb) - gainToDb(1) / 2 - gainToDb(0.5) / 2) < 1e-9);
  assert.equal(groupMeanDb(p, 1, 8, gainToDb), null);
});

test("a group offset is read per destination and written with a sign, a comma and one decimal", () => {
  const g = { id: 1, offsets: [{ to: null, db: -6 }, { to: 9, db: 3.04 }] };
  assert.equal(groupOffset(g), -6);
  assert.equal(groupOffset(g, 9), 3.04);
  assert.equal(groupOffset(g, 8), 0);
  assert.equal(groupOffset({ id: 2 }), 0);
  assert.equal(formatOffset(0), "0 dB");
  assert.equal(formatOffset(3.04), "+3,0 dB");
  assert.equal(formatOffset(-6.5), "−6,5 dB");
  assert.equal(formatOffset(-0.04), "0 dB");
  assert.equal(OFFSET_RANGE, 12);
});

test("group commands are validated like the host does", () => {
  const bad = [
    { type: "create_group", name: " ", nodes: [1] },
    { type: "create_group", name: "x".repeat(41), nodes: [1] },
    { type: "create_group", name: "g", color: "rouge", nodes: [1] },
    { type: "create_group", name: "g", nodes: [1, 1] },
    { type: "create_group", name: "g", nodes: [0] },
    { type: "edit_group", group: 0, name: "g" },
    { type: "delete_group", group: 0 },
    { type: "set_group", group: 1 },
    { type: "set_group", group: 1, delta_db: 200 },
    { type: "set_group_offset", group: 0, db: 1 },
    { type: "set_group_offset", group: 1, db: 99 },
    { type: "set_group_offset", group: 1, db: "x" },
    { type: "set_send", node: 1, to: 2 },
    { type: "set_send", node: 1, to: 2, mute: "oui" },
  ];
  for (const m of bad) assert.notEqual(validateCommand(m), null, JSON.stringify(m));
  const ok = [
    { type: "create_group", name: "Batterie", color: "#ff8800", nodes: [1, 2] },
    { type: "edit_group", group: 3, nodes: [4] },
    { type: "delete_group", group: 3 },
    { type: "set_group", group: 3, mute: true, to: 5 },
    { type: "set_group_offset", group: 3, db: -6.5, to: 5 },
    { type: "set_send", node: 1, to: 2, solo: true },
  ];
  for (const m of ok) assert.equal(validateCommand(m), null, JSON.stringify(m));
});

function engine() {
  const e = new MockEngine();
  let project;
  let last;
  e.on("project", (p) => (project = p));
  e.on("error", (x) => (last = x));
  e.send({ type: "get_project" });
  return { e, p: () => project, err: () => last };
}

test("the mock engine creates, edits and deletes a group, and undo walks it back", () => {
  const { e, p } = engine();
  const [a, b, c] = p().nodes.filter((n) => n.kind !== "master" && n.kind !== "bus").map((n) => n.id);
  e.send({ type: "create_group", name: "Rythmique", color: "#ff8800", nodes: [a, b] });
  const g = p().groups[0];
  assert.equal(g.name, "Rythmique");
  assert.deepEqual(p().nodes.filter((n) => n.group === g.id).map((n) => n.id), [a, b]);
  e.send({ type: "edit_group", group: g.id, name: "Section", nodes: [b, c] });
  assert.deepEqual(p().nodes.filter((n) => n.group === g.id).map((n) => n.id), [b, c]);
  e.send({ type: "delete_group", group: g.id });
  assert.equal(p().groups.length, 0);
  assert.ok(p().nodes.every((n) => n.group === 0));
  e.send({ type: "undo" });
  assert.equal(p().groups.length, 1);
  e.send({ type: "undo" });
  e.send({ type: "undo" });
  assert.equal(p().groups.length, 0);
});

test("the mock engine acts on a whole group and on its sends to a return only", () => {
  const { e, p, err } = engine();
  e.send({ type: "add_track", kind: "return", name: "Retour" });
  const ret = p().nodes.find((n) => n.name === "Retour").id;
  const [a, b] = p().nodes.filter((n) => n.kind !== "master" && n.kind !== "bus").map((n) => n.id);
  e.send({ type: "create_group", name: "G", nodes: [a, b] });
  const gid = p().groups[0].id;
  e.send({ type: "set_group", group: gid, mute: true });
  assert.ok(p().nodes.filter((n) => n.group === gid).every((n) => n.mute));
  e.send({ type: "set_group", group: gid, mute: false, to: ret, solo: true });
  for (const n of p().nodes.filter((x) => x.group === gid)) {
    const s = n.sends.find((x) => x.to === ret);
    assert.ok(s && s.solo && s.gain === 0, "l'envoi est créé à niveau nul pour pouvoir être mis en solo");
    assert.equal(n.mute, true, "la piste n'est pas touchée : seulement son envoi");
  }
  e.send({ type: "set_send", node: a, to: ret, gain: 0.5 });
  e.send({ type: "set_group", group: gid, to: ret, delta_db: 6.0206 });
  assert.ok(Math.abs(p().nodes.find((n) => n.id === a).sends.find((s) => s.to === ret).gain - 1) < 1e-3);
  e.send({ type: "set_group", group: 999, mute: true });
  assert.match(err().message, /groupe inconnu/);
});

test("the mock engine keeps a group offset in place and gives the levels back at zero", () => {
  const { e, p } = engine();
  const [a, b] = p().nodes.filter((n) => n.kind !== "master" && n.kind !== "bus").map((n) => n.id);
  e.send({ type: "create_group", name: "G", nodes: [a, b] });
  const gid = p().groups[0].id;
  const gain = (id) => p().nodes.find((n) => n.id === id).gain;
  const [ga, gb] = [gain(a), gain(b)];
  e.send({ type: "set_group_offset", group: gid, db: -6 });
  assert.ok(Math.abs(gain(a) - ga * 10 ** (-6 / 20)) < 1e-9);
  assert.deepEqual(p().groups[0].offsets, [{ to: null, db: -6 }]);
  e.send({ type: "set_group_offset", group: gid, db: -6 });
  assert.ok(Math.abs(gain(a) - ga * 10 ** (-6 / 20)) < 1e-9, "le même décalage demandé deux fois n'est appliqué qu'une fois");
  e.send({ type: "set_group_offset", group: gid, db: 0 });
  assert.ok(Math.abs(gain(a) - ga) < 1e-9 && Math.abs(gain(b) - gb) < 1e-9);
  assert.deepEqual(p().groups[0].offsets, []);
});

test("the mock engine moves a track in the shared display order", () => {
  const { e, p } = engine();
  const names = () => p().nodes.filter((n) => n.kind !== "master").map((n) => n.name);
  const [first, , third] = p().nodes.filter((n) => n.kind !== "master");
  const before = names();
  e.send({ type: "move_track", node: third.id, before: first.id });
  assert.equal(names()[0], third.name);
  assert.deepEqual(names().slice(1), before.filter((n) => n !== third.name));
  e.send({ type: "move_track", node: third.id });
  assert.equal(names().at(-1), third.name);
});

test("effectiveGroups gives a return its own groups when it has some, else the project ones", () => {
  const p = project([node(1, { group: 1 }), node(2), node(9, { kind: "bus", is_return: true, phys: { first: 3 }, return_groups: { groups: [{ id: 5, name: "Perso" }], members: [[2, 5]] } }), node(8, { kind: "bus", is_return: true, phys: { first: 5 } })], [{ id: 1, name: "Projet" }]);
  const own = effectiveGroups(p, 9);
  assert.equal(own.custom, true);
  assert.deepEqual(own.groups.map((g) => g.name), ["Perso"]);
  assert.equal(own.groupOf(2), 5);
  assert.equal(own.groupOf(1), 0, "le groupe du projet n'existe pas dans ce mix");
  for (const ret of [8, null, undefined]) {
    const e = effectiveGroups(p, ret);
    assert.equal(e.custom, false);
    assert.deepEqual(e.groups.map((g) => g.name), ["Projet"]);
    assert.equal(e.groupOf(1), 1);
  }
  assert.deepEqual(groupMembers(p, 5, 9).map((n) => n.id), [2]);
  assert.deepEqual(groupMembers(p, 1).map((n) => n.id), [1]);
  assert.equal(groupFlags(p, 5, 9).any, true);
});

test("return group commands are validated", () => {
  for (const m of [{ type: "customize_return_groups" }, { type: "reset_return_groups", ret: 0 }, { type: "create_group", name: "g", nodes: [1], ret: "x" }]) assert.notEqual(validateCommand(m), null, JSON.stringify(m));
  for (const m of [{ type: "customize_return_groups", ret: 4 }, { type: "reset_return_groups", ret: 4 }, { type: "create_group", name: "g", nodes: [1], ret: 4 }, { type: "edit_group", group: 2, name: "x", ret: 4 }, { type: "delete_group", group: 2, ret: 4 }]) assert.equal(validateCommand(m), null, JSON.stringify(m));
});

test("the mock engine gives a return its own groups that replace the project ones in its mix only", () => {
  const { e, p, err } = engine();
  e.send({ type: "add_track", kind: "return", name: "Retour" });
  const ret = p().nodes.find((n) => n.name === "Retour").id;
  e.send({ type: "set_return_output", node: ret, output: { first: 3, stereo: true, mono: false } });
  const [a, b, c] = p().nodes.filter((n) => n.kind !== "master" && !n.phys && n.kind !== "bus").map((n) => n.id);
  e.send({ type: "create_group", name: "Projet", nodes: [a, b] });
  const pg = p().groups[0].id;
  e.send({ type: "create_group", name: "X", nodes: [c], ret });
  assert.match(err().message, /pas de groupes propres/);
  e.send({ type: "customize_return_groups", ret });
  const rg = () => p().nodes.find((n) => n.id === ret).return_groups;
  assert.equal(rg().groups.length, 1);
  assert.notEqual(rg().groups[0].id, pg);
  assert.deepEqual(rg().members.map(([n]) => n).sort(), [a, b].sort());
  e.send({ type: "create_group", name: "Perso", nodes: [c], ret });
  assert.equal(rg().groups.length, 2);
  assert.equal(p().groups.length, 1, "le projet ne bouge pas");
  const perso = rg().groups[1].id;
  e.send({ type: "set_group", group: perso, to: ret, solo: true });
  assert.equal(p().nodes.find((n) => n.id === c).sends.find((s) => s.to === ret).solo, true);
  assert.equal((p().nodes.find((n) => n.id === a).sends ?? []).some((s) => s.solo), false);
  e.send({ type: "set_group", group: pg, to: ret, mute: true });
  assert.match(err().message, /groupe inconnu/);
  e.send({ type: "reset_return_groups", ret });
  assert.equal(rg(), null);
  e.send({ type: "undo" });
  assert.equal(rg().groups.length, 2);
});

import { describeFilter, rowsFromRules, rulesFromRows } from "../js/ui/midi-filter-dialog.js";

test("midi filter rows and rules convert both ways and are described", () => {
  const rules = [{ device: "Clavier A", channels: [1, 2] }, { device: "Pad B", channels: [] }, { device: "", channels: [10] }];
  const rows = rowsFromRules(rules);
  assert.deepEqual(rows, [{ device: "Clavier A", channel: 1 }, { device: "Clavier A", channel: 2 }, { device: "Pad B", channel: 0 }, { device: "", channel: 10 }]);
  assert.deepEqual(rulesFromRows(rows), [{ device: "Clavier A", channels: [1] }, { device: "Clavier A", channels: [2] }, { device: "Pad B", channels: [] }, { device: "", channels: [10] }]);
  assert.deepEqual(rulesFromRows([{ device: "X", channel: 3 }, { device: "X", channel: 3 }]), [{ device: "X", channels: [3] }], "lignes identiques fusionnées");
  assert.equal(describeFilter([]), "tout passe");
  assert.equal(describeFilter(rules), "Clavier A (canal 1, 2) ; Pad B ; tout appareil (canal 10)");
  assert.equal(validateCommand({ type: "set_track_midi_filter", node: 2, rules }), null);
  for (const bad of [{ node: 2, rules: [{ channels: [17] }] }, { node: 0, rules: [] }, { node: 2, rules: Array(17).fill({}) }, { node: 2, rules: [{ device: 4 }] }]) assert.notEqual(validateCommand({ type: "set_track_midi_filter", ...bad }), null);
});

test("the mock engine keeps a track's midi filter", () => {
  const { e, p, err } = engine();
  const t = p().nodes.find((n) => n.kind === "stereo").id;
  assert.deepEqual(p().nodes.find((n) => n.id === t).midi_filter, []);
  assert.ok(p().midi_devices.length >= 1);
  e.send({ type: "set_track_midi_filter", node: t, rules: [{ device: "Clavier MIDI (aperçu)", channels: [1] }] });
  assert.deepEqual(p().nodes.find((n) => n.id === t).midi_filter, [{ device: "Clavier MIDI (aperçu)", channels: [1] }]);
  e.send({ type: "set_track_midi_filter", node: 0, rules: [] });
  assert.ok(err());
});

import { FIELDS, DEFAULT_DYNAMICS, describeDynamics, formatValue } from "../js/ui/dynamics-dialog.js";

test("input dynamics: ranges match the host, values are written readably, commands are validated", () => {
  const spec = { ...DEFAULT_DYNAMICS, enabled: true };
  assert.equal(validateCommand({ type: "set_track_dynamics", node: 2, dynamics: spec }), null);
  for (const f of FIELDS) {
    assert.equal(validateCommand({ type: "set_track_dynamics", node: 2, dynamics: { ...spec, [f.key]: f.min } }), null, `${f.key} au minimum`);
    assert.equal(validateCommand({ type: "set_track_dynamics", node: 2, dynamics: { ...spec, [f.key]: f.max } }), null, `${f.key} au maximum`);
    assert.notEqual(validateCommand({ type: "set_track_dynamics", node: 2, dynamics: { ...spec, [f.key]: f.max + 1 } }), null, `${f.key} trop grand`);
    assert.notEqual(validateCommand({ type: "set_track_dynamics", node: 2, dynamics: { ...spec, [f.key]: f.min - 1 } }), null, `${f.key} trop petit`);
  }
  assert.notEqual(validateCommand({ type: "set_track_dynamics", node: 0, dynamics: spec }), null);
  assert.notEqual(validateCommand({ type: "set_track_dynamics", node: 2, dynamics: { ...spec, enabled: "oui" } }), null);
  assert.equal(formatValue(FIELDS[0], -90), "—");
  assert.equal(formatValue(FIELDS[0], -45), "−45 dB");
  assert.equal(formatValue(FIELDS[2], 3), "3 :1");
  assert.equal(formatValue(FIELDS[3], 0.5), "0,5 ms");
  assert.equal(describeDynamics(DEFAULT_DYNAMICS), "désactivée");
  assert.equal(describeDynamics({ ...spec, gate_db: -50, record: true }), "porte −50 dB · compresseur −18 dB 3 :1 · enregistrée");
});

test("the mock engine keeps a track's input dynamics", () => {
  const { e, p, err } = engine();
  const t = p().nodes.find((n) => n.kind === "stereo").id;
  assert.equal(p().nodes.find((n) => n.id === t).dynamics.enabled, false);
  e.send({ type: "set_track_dynamics", node: t, dynamics: { ...DEFAULT_DYNAMICS, enabled: true, ratio: 6 } });
  assert.equal(p().nodes.find((n) => n.id === t).dynamics.ratio, 6);
  e.send({ type: "set_track_dynamics", node: t, dynamics: { ...DEFAULT_DYNAMICS, ratio: 99 } });
  assert.ok(err(), "hors plage refusé");
});

import { sidechainSources } from "../js/ui/plugins-panel.js";

test("side chain: sources exclude the master, the node itself and monitoring returns; commands are validated", () => {
  const p = { nodes: [{ id: 0, kind: "master" }, { id: 1, kind: "stereo" }, { id: 2, kind: "mono" }, { id: 3, kind: "bus" }, { id: 4, kind: "bus", is_return: true, phys: { first: 3 } }] };
  assert.deepEqual(sidechainSources(p, 2).map((n) => n.id), [1, 3]);
  assert.equal(validateCommand({ type: "set_insert_sidechain", insert: 3, source: 1 }), null);
  assert.equal(validateCommand({ type: "set_insert_sidechain", insert: 3, source: null }), null);
  for (const bad of [{ insert: -1, source: 1 }, { insert: 3, source: 0 }, { insert: 3, source: "a" }, { insert: 3 }]) assert.notEqual(validateCommand({ type: "set_insert_sidechain", ...bad }), null, JSON.stringify(bad));
});

test("the mock engine sets a plugin's side chain, refuses loops and forgets it with the source", () => {
  const { e, p, err } = engine();
  const [a, b] = p().nodes.filter((n) => n.kind === "stereo" || n.kind === "mono").map((n) => n.id);
  e.send({ type: "add_plugin", node: b, clap_id: "mock.compresseur" });
  const ins = () => p().nodes.find((n) => n.id === b).inserts[0];
  assert.equal(ins().has_sidechain, true);
  e.send({ type: "set_insert_sidechain", insert: ins().id, source: a });
  assert.equal(ins().sidechain, a);
  e.send({ type: "set_insert_sidechain", insert: ins().id, source: b });
  assert.match(err().message, /refusée/);
  e.send({ type: "set_insert_sidechain", insert: ins().id, source: 0 });
  assert.match(err().message, /source/, "la sortie principale est refusée dès la validation");
  e.send({ type: "set_insert_sidechain", insert: ins().id, source: null });
  assert.equal(ins().sidechain, null);
  e.send({ type: "set_insert_sidechain", insert: ins().id, source: a });
  e.send({ type: "delete_track", node: a });
  assert.equal(ins().sidechain, null, "la source supprimée emporte la chaîne latérale");
  e.send({ type: "add_plugin", node: b, clap_id: "mock.egaliseur" });
  assert.equal(p().nodes.find((n) => n.id === b).inserts[1].has_sidechain, false);
});
