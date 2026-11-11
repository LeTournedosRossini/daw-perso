import test from "node:test";
import assert from "node:assert/strict";
import { plain, keysOf, srcAt, outAt, ratios, ratioAt, validate, addMarker, removeMarker, dragBounds, moveMarker, previewSegments, rewindow, ratioText } from "../js/stretch-model.js";

const RATE = 48_000;
const LEN = 144_000; // 3 s

test("un clip ordinaire est l'identité : la source est lui-même, aucun marqueur, rapport 1", () => {
  const p = plain(LEN);
  assert.deepEqual(keysOf(p, LEN), [[0, 0], [LEN, LEN]]);
  assert.equal(srcAt(p, LEN, 50_000), 50_000);
  assert.deepEqual(ratios(p, LEN), [1]);
});

test("poser un marqueur ne déplace pas le son : sa position source est celle que la carte y met déjà", () => {
  const p = plain(LEN);
  const m = addMarker(p, LEN, 72_000, RATE);
  assert.deepEqual(m, [[72_000, 72_000, false]]);
  // Sur un clip déjà étiré (×1,5 puis ×0,75), un marqueur posé dans le second tronçon prend la source de ce tronçon.
  const s = { engine: "signalsmith", src_len: LEN, markers: [[72_000, 48_000]] };
  const m2 = addMarker(s, LEN, 108_000, RATE);
  assert.deepEqual(m2.map((x) => x[0]), [72_000, 108_000]);
  assert.equal(m2[1][1], 48_000 + Math.round(36_000 / 0.75), "source = 48 000 + 36 000 / 0,75");
  assert.equal(srcAt({ ...s, markers: m2 }, LEN, 108_000), m2[1][1], "la carte passe bien par le nouveau marqueur");
});

test("poser trop près d'un bord ou d'un autre marqueur est refusé (10 ms de source au moins)", () => {
  const s = { engine: "signalsmith", src_len: LEN, markers: [[72_000, 72_000]] };
  assert.equal(addMarker(s, LEN, 100, RATE), null);
  assert.equal(addMarker(s, LEN, LEN - 100, RATE), null);
  assert.equal(addMarker(s, LEN, 72_200, RATE), null);
  assert.notEqual(addMarker(s, LEN, 72_000 + 481, RATE), null);
});

test("les rapports de chaque tronçon, et celui qui contient une position", () => {
  const s = { engine: "signalsmith", src_len: LEN, markers: [[72_000, 48_000]] };
  const r = ratios(s, LEN);
  assert.ok(Math.abs(r[0] - 1.5) < 1e-9 && Math.abs(r[1] - 0.75) < 1e-9);
  assert.equal(ratioText(r[0]), "150 %");
  assert.ok(Math.abs(ratioAt(s, LEN, 10_000) - 1.5) < 1e-9 && Math.abs(ratioAt(s, LEN, 100_000) - 0.75) < 1e-9);
});

test("les refus de l'hôte sont les mêmes ici", () => {
  const v = (m) => validate(m, LEN, LEN, RATE);
  assert.equal(v([[72_000, 48_000]]), null);
  assert.match(v([[1_000, 48_000]]), /étiré à 2 %/);
  assert.match(v([[48_000, 1_000]]), /4800 %/);
  assert.match(v([[0, 100]]), /à l'intérieur/);
  assert.match(v([[LEN, 100]]), /à l'intérieur/);
  assert.match(v([[50_000, 50_000], [40_000, 60_000]]), /dans l'ordre/);
  assert.match(v([[50_000, 50_000], [50_100, 50_100]]), /trop court/);
  assert.match(validate(Array.from({ length: 300 }, (_, i) => [i + 1, i + 1]), LEN, LEN, RATE), /trop de marqueurs/);
});

test("bornes d'un glissement : les deux tronçons voisins restent entre 25 % et 400 %", () => {
  const s = { engine: "signalsmith", src_len: LEN, markers: [[72_000, 72_000]] };
  const [lo, hi] = dragBounds(s, LEN, 0);
  // Tronçon gauche : 72 000 de source, sortie dans [18 000, 288 000] ; tronçon droit : 72 000 de source, de sortie dans [18 000, 288 000] aussi.
  // Un échantillon de marge sous le rapport limite : la bibliothèque calcule ses rapports en secondes (flottants), un rapport exactement égal
  // à 25 % pourrait être refusé par l'arrondi.
  assert.equal(lo, Math.max(18_000, LEN - 288_000) + 1);
  assert.equal(hi, Math.min(288_000, LEN - 18_000) - 1);
  assert.equal(moveMarker(s, LEN, 0, 10).at(0)[0], lo, "ramené à la borne basse");
  assert.equal(moveMarker(s, LEN, 0, 10_000_000).at(0)[0], hi, "ramené à la borne haute");
  assert.deepEqual(moveMarker(s, LEN, 0, 90_000), [[90_000, 72_000]], "la source ne change pas, seule la sortie bouge");
  assert.equal(validate(moveMarker(s, LEN, 0, 90_000), LEN, LEN, RATE), null);
  assert.equal(dragBounds(s, LEN, 3), null);
});

test("retirer un marqueur rend la place au tronçon voisin", () => {
  const s = { engine: "signalsmith", src_len: LEN, markers: [[40_000, 40_000], [90_000, 80_000]] };
  assert.deepEqual(removeMarker(s, 0), [[90_000, 80_000]]);
  assert.deepEqual(removeMarker(s, 1), [[40_000, 40_000]]);
});

test("aperçu : chaque tronçon de la nouvelle carte montre le tronçon de source qu'il montrait avant", () => {
  const s = { engine: "signalsmith", src_len: LEN, markers: [[72_000, 72_000]] };
  const next = moveMarker(s, LEN, 0, 90_000);
  assert.deepEqual(previewSegments(s, LEN, next), [{ from: [0, 72_000], to: [0, 90_000] }, { from: [72_000, LEN], to: [90_000, LEN] }]);
});

test("aperçu : un marqueur qu'on vient de poser montre tout le clip tel quel, de chaque côté (défaut : moitié gauche écrasée, droite vide)", () => {
  const p = plain(LEN);
  const segs = previewSegments(p, LEN, [[72_000, 72_000]]);
  assert.equal(segs.length, 2);
  for (const s of segs) assert.deepEqual(s.from, s.to, "à l'identique, chaque tronçon se montre à sa place");
  assert.deepEqual(segs.map((s) => s.to), [[0, 72_000], [72_000, LEN]]);
  // Un marqueur ajouté sur un clip déjà étiré : les tronçons de source sont relus là où la carte actuelle les met.
  const s1 = { engine: "signalsmith", src_len: LEN, markers: [[72_000, 48_000]] };
  const two = previewSegments(s1, LEN, [[72_000, 48_000], [108_000, 48_000 + 48_000]]);
  assert.equal(two.length, 3);
  assert.deepEqual(two[0], { from: [0, 72_000], to: [0, 72_000] });
  assert.ok(Math.abs(two[1].from[1] - outAt(s1, LEN, 96_000)) < 1e-6);
  // Un marqueur retiré : un seul tronçon qui montre tout le clip.
  const none = previewSegments(s1, LEN, []);
  assert.deepEqual(none, [{ from: [0, LEN], to: [0, LEN] }]);
});

test("outAt est l'inverse de srcAt", () => {
  const s = { engine: "signalsmith", src_len: LEN, markers: [[72_000, 48_000], [100_000, 90_000]] };
  for (const out of [1, 30_000, 72_000, 85_000, 100_000, 140_000]) assert.ok(Math.abs(outAt(s, LEN, srcAt(s, LEN, out)) - out) < 1e-6, `${out}`);
});

test("découpe : chaque moitié garde ses marqueurs et la partie de la source qu'elle couvre", () => {
  const s = { engine: "rubberband_r2", src_len: LEN, markers: [[72_000, 48_000]] };
  const left = rewindow(s, LEN, 0, 100_000);
  const right = rewindow(s, LEN, 100_000, LEN);
  assert.deepEqual(left.markers, [[72_000, 48_000]]);
  assert.equal(left.engine, "rubberband_r2");
  assert.deepEqual(right.markers, [], "le marqueur est dans l'autre moitié");
  // La droite couvre la source de 1,7778 s à 3 s : 58 667 échantillons, comme côté hôte.
  assert.ok(Math.abs(right.src_len - 58_667) <= 2, `${right.src_len}`);
  assert.equal(rewindow(s, LEN, 5, 5), null);
  // Un rognage du début décale les marqueurs.
  const trimmed = rewindow(s, LEN, 10_000, LEN);
  assert.equal(trimmed.markers[0][0], 62_000);
});

import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";

function mock() {
  const e = new MockEngine();
  const log = { projects: [], errors: [], history: [], stretch: [] };
  e.on("project", (x) => log.projects.push(x));
  e.on("error", (x) => log.errors.push(x.message));
  e.on("history", (x) => log.history.push(x));
  e.on("stretch", (x) => log.stretch.push(x));
  e.send({ type: "get_project" });
  const clip = () => log.projects.at(-1).nodes.flatMap((n) => n.clips).find((c) => c.kind === "audio" && c.len > 100_000);
  return { e, log, clip, project: () => log.projects.at(-1) };
}

test("protocole : set_stretch et cancel_stretch sont validés comme par l'hôte", () => {
  assert.equal(validateCommand({ type: "set_stretch", clip: 3, markers: [[100, 50], [200, 150]] }), null);
  assert.equal(validateCommand({ type: "set_stretch", clip: 3, markers: [], engine: "rubberband_r3" }), null);
  assert.ok(validateCommand({ type: "set_stretch", clip: 3, markers: [[100, 50], [90, 150]] }));
  assert.ok(validateCommand({ type: "set_stretch", clip: 3, markers: [[0, 50]] }));
  assert.ok(validateCommand({ type: "set_stretch", clip: 3, markers: [[1.5, 5]] }));
  assert.ok(validateCommand({ type: "set_stretch", clip: 3, markers: [[100, 50]], engine: "zplane" }));
  assert.ok(validateCommand({ type: "set_stretch", markers: [] }));
  assert.equal(validateCommand({ type: "cancel_stretch" }), null);
});

test("faux moteur : étirer, annuler en un pas, retirer, refuser sans trace", () => {
  const { e, log, clip } = mock();
  const c = clip();
  assert.ok(!c.stretch, "clip ordinaire");
  // Refus sans trace : rapport impossible, clip inconnu, rien à étirer.
  e.send({ type: "set_stretch", clip: c.id, markers: [[1_000, 48_000]] });
  e.send({ type: "set_stretch", clip: 987_654, markers: [[50_000, 50_000]] });
  e.send({ type: "set_stretch", clip: c.id, markers: [] });
  assert.equal(log.errors.length, 3);
  assert.equal(log.history.at(-1).undo_label ?? null, null);
  // Étirement : rendu simulé de 0,4 s, une action annulable.
  const m = [[Math.round(c.len / 2), Math.round(c.len / 3)]];
  e.send({ type: "set_stretch", clip: c.id, markers: m, engine: "rubberband_r2" });
  assert.equal(log.stretch.at(-1).active, true);
  e.send({ type: "set_stretch", clip: c.id, markers: m });
  assert.equal(log.errors.length, 4, "un seul étirement à la fois");
  e.advance(500);
  assert.equal(log.stretch.at(-1).active, false);
  const after = clip();
  assert.deepEqual([after.stretch.engine, after.stretch.src_len, after.stretch.markers], ["rubberband_r2", c.len, m]);
  assert.equal(log.history.at(-1).undo_label, "Étirer l'audio");
  e.send({ type: "undo" });
  assert.ok(!clip().stretch, "annulé d'un coup");
  e.send({ type: "redo" });
  // Retirer tous les marqueurs : retour immédiat au clip ordinaire.
  e.send({ type: "set_stretch", clip: c.id, markers: [] });
  assert.ok(!clip().stretch);
  assert.equal(log.history.at(-1).undo_label, "Retirer l'étirement");
});

test("faux moteur : annuler le rendu ne pose rien, et la découpe répartit les marqueurs", () => {
  const { e, log, clip, project } = mock();
  const c = clip();
  const m = [[Math.round(c.len / 2), Math.round(c.len / 3)]];
  e.send({ type: "set_stretch", clip: c.id, markers: m });
  e.send({ type: "cancel_stretch" });
  e.advance(100);
  assert.equal(log.stretch.at(-1).cancelled, true);
  assert.ok(!clip().stretch);
  e.send({ type: "set_stretch", clip: c.id, markers: m });
  e.advance(500);
  const cut = c.start + Math.round(c.len * 0.8);
  e.send({ type: "split_clip", clip: c.id, at: cut });
  const halves = project().nodes.flatMap((n) => n.clips).filter((x) => x.kind === "audio" && x.start >= c.start && x.start < c.start + c.len && x.name === c.name);
  assert.equal(halves.length, 2);
  const [left, right] = halves.sort((a, b) => a.start - b.start);
  assert.deepEqual(left.stretch.markers, m, "la gauche garde le marqueur");
  assert.deepEqual(right.stretch.markers, [], "la droite n'en a pas mais reste étirée");
  assert.ok(right.stretch.src_len > 0 && right.stretch.src_len < left.stretch.src_len);
});

test("set_stretch accepte le clip d'identifiant 0 (le premier clip du projet) et refuse un identifiant négatif", () => {
  assert.equal(validateCommand({ type: "set_stretch", clip: 0, markers: [[100, 100]] }), null);
  assert.match(validateCommand({ type: "set_stretch", clip: -1, markers: [] }), /clip/);
});

test("set_stretch : l'indicateur de lissage doit avoir un booléen par marqueur", () => {
  assert.equal(validateCommand({ type: "set_stretch", clip: 1, markers: [[100, 100]], smooth: [true] }), null);
  assert.match(validateCommand({ type: "set_stretch", clip: 1, markers: [[100, 100]], smooth: [true, false] }), /smooth/);
  assert.match(validateCommand({ type: "set_stretch", clip: 1, markers: [[100, 100]], smooth: ["oui"] }), /smooth/);
});

test("déplacer un marqueur garde son indicateur de lissage", () => {
  const info = { engine: "signalsmith", src_len: LEN, markers: [[72_000, 48_000, true]] };
  assert.equal(moveMarker(info, LEN, 0, 80_000)[0][2], true);
});

test("detect_onsets : clip valide et sensibilité entre 0 et 1", () => {
  assert.equal(validateCommand({ type: "detect_onsets", clip: 0 }), null);
  assert.equal(validateCommand({ type: "detect_onsets", clip: 3, sensitivity: 0.5 }), null);
  assert.match(validateCommand({ type: "detect_onsets", clip: 3, sensitivity: 2 }), /sensitivity/);
});
