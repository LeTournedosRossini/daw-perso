import test from "node:test";
import assert from "node:assert/strict";
import { MockEngine } from "../js/mock-engine.js";
import { validateCommand } from "../js/protocol.js";
import { hitTest, previewDrag, commandFor, clipRect, targetTrackAt, MIN_LEN } from "../js/ui/clip-geometry.js";
import { TempoMap, MeterMap, snapSample } from "../js/time.js";

const SR = 48_000;
const BAR = 96_000;
const collect = (e, ...types) => { const log = []; for (const t of types) e.on(t, (x) => log.push(x)); return log; };
const clipOf = (p, id) => p.nodes.flatMap((n) => n.clips).find((c) => c.id === id);

test("validation des commandes d'édition", () => {
  for (const ok of [
    { type: "move_clip", clip: 1, start: 0 }, { type: "trim_clip", clip: 1, start: 5 }, { type: "trim_clip", clip: 1, end: 9 }, { type: "trim_clip", clip: 1, start: 5, end: 9 },
    { type: "split_clip", clip: 1, at: 10 }, { type: "delete_clip", clip: 1 }, { type: "set_clip_fades", clip: 1, fade_in: 0 }, { type: "set_clip_fades", clip: 1, fade_out: 5 }, { type: "undo" }, { type: "redo" },
  ]) assert.equal(validateCommand(ok), null, JSON.stringify(ok));
  for (const bad of [
    { type: "move_clip", clip: 1 }, { type: "move_clip", clip: -1, start: 0 }, { type: "trim_clip", clip: 1 }, { type: "trim_clip", clip: 1, start: 9, end: 9 }, { type: "trim_clip", clip: 1, start: 1.5 },
    { type: "split_clip", clip: 1 }, { type: "delete_clip" }, { type: "set_clip_fades", clip: 1 }, { type: "set_clip_fades", clip: 1, fade_in: -1 },
  ]) assert.notEqual(validateCommand(bad), null, JSON.stringify(bad));
});

test("faux moteur : déplacer, rogner, découper, fondus, supprimer, annuler, rétablir", () => {
  const e = new MockEngine();
  const ev = collect(e, "project", "history", "error");
  const last = (t) => ev.filter((x) => x.type === t).at(-1);
  e.send({ type: "get_project" });
  assert.equal(last("history").can_undo, false);
  assert.ok(e.send({ type: "move_clip", clip: 10, start: 48_000 }));
  assert.equal(clipOf(last("project"), 10).start, 48_000);
  assert.deepEqual([last("history").can_undo, last("history").undo_label], [true, "Déplacer un clip"]);
  assert.ok(e.send({ type: "trim_clip", clip: 10, start: 100_000, end: 500_000 }));
  const t = clipOf(last("project"), 10);
  assert.deepEqual([t.start, t.len, t.offset], [100_000, 400_000, 52_000]);
  assert.ok(e.send({ type: "set_clip_fades", clip: 10, fade_in: 2_000, fade_out: 9e9 }));
  assert.deepEqual([clipOf(last("project"), 10).fade_in, clipOf(last("project"), 10).fade_out], [2_000, 400_000]);
  assert.ok(e.send({ type: "split_clip", clip: 10, at: 300_000 }));
  const p = last("project");
  const left = clipOf(p, 10);
  assert.deepEqual([left.start, left.len, left.fade_in, left.fade_out], [100_000, 200_000, 2_000, 0]);
  const right = p.nodes[1].clips.find((c) => c.start === 300_000);
  assert.deepEqual([right.len, right.offset, right.fade_in, right.fade_out, right.name], [200_000, 252_000, 0, 200_000, "Beat A"]);
  assert.ok(e.send({ type: "delete_clip", clip: right.id }));
  assert.equal(clipOf(last("project"), right.id), undefined);
  const labels = [];
  while (last("history").can_undo) {
    assert.ok(e.send({ type: "undo" }));
    labels.push(last("history").redo_label);
  }
  assert.deepEqual(labels, ["Supprimer un clip", "Découper", "Modifier un clip", "Rogner un clip", "Déplacer un clip"]);
  assert.deepEqual([clipOf(last("project"), 10).start, clipOf(last("project"), 10).len], [0, 8 * BAR]);
  assert.ok(e.send({ type: "redo" }));
  assert.equal(clipOf(last("project"), 10).start, 48_000);
  // Une nouvelle édition vide la pile de rétablissement.
  e.send({ type: "move_clip", clip: 20, start: 1 });
  assert.equal(last("history").can_redo, false);
});

test("faux moteur : refus sans effet ni trace", () => {
  const e = new MockEngine();
  const errors = collect(e, "error");
  const hist = collect(e, "history");
  const before = JSON.stringify(e.project.nodes.map((n) => n.clips));
  for (const msg of [
    { type: "move_clip", clip: 4242, start: 0 }, { type: "split_clip", clip: 10, at: 0 }, { type: "split_clip", clip: 10, at: 9e9 }, { type: "trim_clip", clip: 10, start: 9e9 },
    { type: "trim_clip", clip: 10, end: 9e9 }, { type: "trim_clip", clip: 30, start: 5 }, { type: "undo" }, { type: "redo" },
  ]) assert.equal(e.send(msg), false, JSON.stringify(msg));
  assert.equal(errors.length, 8);
  assert.equal(hist.length, 0);
  assert.equal(JSON.stringify(e.project.nodes.map((n) => n.clips)), before);
});

test("géométrie : zones touchées et priorité des clips superposés", () => {
  const vp = { start: 0, spp: 100 };
  const rowH = 92;
  const mk = (id, start, len, extra = {}) => ({ id, kind: "audio", name: "c", start, len, offset: 0, source_len: len + 50_000, fade_in: 0, fade_out: 0, ...extra });
  const nodes = [
    { clips: [mk(1, 10_000, 30_000), mk(2, 45_000, 30_000, { fade_in: 8_000 })] }, // lignes 0 : x 100–400 et 450–750
    { clips: [{ id: 3, kind: "midi", name: "m", start: 0, len: 20_000, notes: [] }] }, // ligne 1
    { clips: [mk(4, 0, 30_000), mk(5, 20_000, 30_000)] }, // ligne 2 : se chevauchent entre x 200 et 300
  ];
  const r1 = clipRect(vp, rowH, 0, nodes[0].clips[0]);
  const bodyY = r1.bodyTop + 30;
  assert.equal(hitTest(nodes, vp, rowH, 200, bodyY).zone, "body");
  assert.equal(hitTest(nodes, vp, rowH, 100 + 2, bodyY).zone, "trim-start");
  assert.equal(hitTest(nodes, vp, rowH, 400 - 3, bodyY).zone, "trim-end");
  // Poignée du fondu d'entrée du clip 2 : au bord (450) + 8 000 / 100 = 80 px.
  assert.equal(hitTest(nodes, vp, rowH, 450 + 80, r1.bodyTop).zone, "fade-in");
  // Chevauchement : dans la zone commune, le clip 5 (dessiné en dernier) gagne.
  const y2 = 2 * rowH + 60;
  assert.equal(hitTest(nodes, vp, rowH, 250, y2).clip.id, 5);
  assert.equal(hitTest(nodes, vp, rowH, 100, y2).clip.id, 4);
  // Dehors, entre les clips, hors des lignes.
  assert.equal(hitTest(nodes, vp, rowH, 900, bodyY), null);
  assert.equal(hitTest(nodes, vp, rowH, 200, 5 * rowH), null);
  // Un clip de notes a, comme un clip audio, des bords de rognage (et des poignées de fondu de vélocité).
  assert.equal(hitTest(nodes, vp, rowH, 2, rowH + 40).zone, "trim-start");
  assert.equal(hitTest(nodes, vp, rowH, 100, rowH + 40).zone, "body");
  // Trop étroit : plus de poignées.
  const tiny = [{ clips: [mk(9, 0, 3_000)] }];
  assert.equal(hitTest(tiny, vp, rowH, 15, clipRect(vp, rowH, 0, tiny[0].clips[0]).bodyTop + 20).zone, "body");
});

test("glissements : bornes du moteur, rognage et fondus", () => {
  const c = { id: 1, kind: "audio", start: 100_000, len: 80_000, offset: 20_000, source_len: 140_000, fade_in: 1_000, fade_out: 2_000 };
  // Déplacement : jamais de début négatif.
  assert.equal(previewDrag(c, "move", -999_999).start, 0);
  assert.equal(previewDrag(c, "move", 5_000).start, 105_000);
  // Rognage du début : ne dépasse ni le début des données (80 000 sur la ligne de temps) ni la fin moins MIN_LEN.
  let p = previewDrag(c, "trim-start", -999_999);
  assert.deepEqual([p.start, p.len, p.offset], [80_000, 100_000, 0]);
  p = previewDrag(c, "trim-start", 999_999);
  assert.deepEqual([p.start, p.len], [180_000 - MIN_LEN, MIN_LEN]);
  p = previewDrag(c, "trim-start", 10_000);
  assert.deepEqual([p.start, p.len, p.offset], [110_000, 70_000, 30_000]);
  // Rognage de la fin : données disponibles jusqu'à 80 000 + 140 000 = 220 000.
  assert.equal(previewDrag(c, "trim-end", 999_999).len, 120_000);
  assert.equal(previewDrag(c, "trim-end", -999_999).len, MIN_LEN);
  // Fondus : entre 0 et la longueur ; le fondu de sortie grandit quand on tire vers la gauche.
  assert.equal(previewDrag(c, "fade-in", 999_999).fade_in, 80_000);
  assert.equal(previewDrag(c, "fade-in", -5_000).fade_in, 0);
  assert.equal(previewDrag(c, "fade-out", -3_000).fade_out, 5_000);
  // Rogner en dessous d'un fondu le ramène à la longueur du clip.
  assert.equal(previewDrag({ ...c, fade_out: 70_000 }, "trim-end", -60_000).fade_out, 20_000);
});

test("glissement du corps : la zone « body » du hit-test déplace, et le bord qui tombe sur la grille l'emporte", () => {
  const c = { id: 1, kind: "audio", start: 1_000, len: 5_000, offset: 0, source_len: 9_000 };
  assert.equal(previewDrag(c, "body", 500).start, 1_500, "même résultat que « move »");
  assert.deepEqual(commandFor(c, "body", previewDrag(c, "body", 500)), { type: "move_clip", clip: 1, start: 1_500 });
  // Seul le début tombe près d'une ligne (à 40 échantillons) : c'est lui qu'on cale, la fin non calée n'« gagne » pas.
  const grid = (s) => (Math.abs(s - 2_000) < 100 ? 2_000 : s);
  assert.equal(previewDrag(c, "body", 1_040, grid).start, 2_000);
  // Seule la fin tombe près d'une ligne : c'est la fin qu'on cale.
  const gridEnd = (s) => (Math.abs(s - 9_000) < 100 ? 9_000 : s);
  assert.equal(previewDrag(c, "body", 2_960, gridEnd).start, 4_000);
  // Un glissement à la souris donne des échantillons fractionnaires : jamais de commande à virgule.
  for (const zone of ["body", "trim-start", "trim-end", "fade-in", "fade-out"]) {
    const p = previewDrag({ ...c, fade_in: 10, fade_out: 10 }, zone, 1234.567);
    for (const k of ["start", "len", "offset", "fade_in", "fade_out"]) assert.ok(Number.isInteger(p[k]), `${zone}.${k} = ${p[k]}`);
  }
  // Aucun bord près d'une ligne : position libre.
  assert.equal(previewDrag(c, "body", 777, (s) => s).start, 1_777);
});

test("commandes issues d'un glissement : rien si rien n'a changé", () => {
  const c = { id: 7, kind: "audio", start: 1_000, len: 5_000, offset: 0, source_len: 9_000, fade_in: 0, fade_out: 0 };
  assert.equal(commandFor(c, "move", previewDrag(c, "move", 0)), null);
  assert.deepEqual(commandFor(c, "move", previewDrag(c, "move", 500)), { type: "move_clip", clip: 7, start: 1_500 });
  assert.deepEqual(commandFor(c, "trim-start", previewDrag(c, "trim-start", 200)), { type: "trim_clip", clip: 7, start: 1_200 });
  assert.deepEqual(commandFor(c, "trim-end", previewDrag(c, "trim-end", -300)), { type: "trim_clip", clip: 7, end: 5_700 });
  assert.deepEqual(commandFor(c, "fade-in", previewDrag(c, "fade-in", 400)), { type: "set_clip_fades", clip: 7, fade_in: 400 });
  assert.deepEqual(commandFor(c, "fade-out", previewDrag(c, "fade-out", -400)), { type: "set_clip_fades", clip: 7, fade_out: 400 });
  assert.equal(commandFor(c, "fade-out", previewDrag(c, "fade-out", 0)), null);
});

test("magnétisme : colle à la grille visible dans le seuil, sinon laisse tel quel", () => {
  const t = new TempoMap([{ pos: [0, 1], num: 120, den: 1 }], SR);
  const m = new MeterMap([{ bar: 0, num: 4, den: 4 }]);
  const vp = { start: 0, spp: 100 }; // 960 px par mesure, 240 par temps
  assert.equal(snapSample(vp, 1000, t, m, 24_000 + 300, 8), 24_000, "3 px d'une ligne de temps : calé dessus");
  assert.equal(snapSample(vp, 1000, t, m, 24_000 + 5_000, 8), 24_000 + 5_000, "50 px : trop loin, inchangé");
  assert.equal(snapSample(vp, 1000, t, m, 96_000 - 200, 8), 96_000, "calé sur la barre de mesure");
  // Très dézoomé : seules les barres existent, le magnétisme n'invente pas de subdivisions.
  const far = { start: 0, spp: 4_000 };
  assert.equal(snapSample(far, 1000, t, m, 60_000, 8), 60_000, "pas de ligne de temps à ce zoom (barres à 0 et 24 px) : inchangé");
  assert.equal(snapSample(far, 1000, t, m, 24_000, 8), 0, "à 6 px de la barre 1 : calé dessus");
});

test("piste visée par un glissement vertical : seulement les vraies pistes", () => {
  const nodes = [{ id: 1, kind: "stereo" }, { id: 2, kind: "mono" }, { id: 7, kind: "bus" }, { id: 0, kind: "master" }];
  assert.equal(targetTrackAt(nodes, 92, 10), 1);
  assert.equal(targetTrackAt(nodes, 92, 100), 2);
  assert.equal(targetTrackAt(nodes, 92, 2 * 92 + 10), null, "un bus ne reçoit pas de clip");
  assert.equal(targetTrackAt(nodes, 92, 3 * 92 + 10), null, "ni la sortie principale");
  assert.equal(targetTrackAt(nodes, 92, -50), 1, "au-dessus de la première ligne : la première");
  assert.equal(targetTrackAt(nodes, 92, 9999), null, "sous la dernière : la sortie principale, donc rien");
});

test("commande de déplacement : la piste n'est envoyée que si elle change", () => {
  const c = { id: 5, kind: "audio", start: 1_000, len: 5_000, offset: 0, source_len: 9_000 };
  const same = previewDrag(c, "body", 0);
  assert.equal(commandFor(c, "body", same, 2, 2), null, "ni position ni piste changées");
  assert.deepEqual(commandFor(c, "body", same, 3, 2), { type: "move_clip", clip: 5, start: 1_000, node: 3 }, "même position, autre piste");
  assert.deepEqual(commandFor(c, "body", previewDrag(c, "body", 200), 2, 2), { type: "move_clip", clip: 5, start: 1_200 }, "pas de champ node si la piste ne change pas");
  assert.equal(validateCommand({ type: "move_clip", clip: 5, start: 1, node: 3 }), null);
  assert.notEqual(validateCommand({ type: "move_clip", clip: 5, start: 1, node: -1 }), null);
  assert.equal(validateCommand({ type: "set_auto_crossfade", enabled: false }), null);
  assert.notEqual(validateCommand({ type: "set_auto_crossfade", enabled: "oui" }), null);
});

test("faux moteur : clips entre pistes, clips de notes (rognage, découpe, fondus), fondus enchaînés automatiques", () => {
  const e = new MockEngine();
  const ev = collect(e, "project", "history", "error");
  const last = (t) => ev.filter((x) => x.type === t).at(-1);
  const nodeOf = (p, id) => p.nodes.find((n) => n.clips.some((c) => c.id === id)).id;
  // Entre pistes.
  assert.ok(e.send({ type: "move_clip", clip: 10, start: 5_000, node: 2 }));
  assert.equal(nodeOf(last("project"), 10), 2);
  assert.ok(e.send({ type: "move_clip", clip: 30, start: 384_000, node: 5 }), "un clip de notes aussi");
  assert.equal(nodeOf(last("project"), 30), 5);
  assert.equal(e.send({ type: "move_clip", clip: 10, start: 0, node: 0 }), false, "pas sur la sortie principale");
  assert.match(ev.filter((x) => x.type === "error").at(-1).message, /pas une piste/);
  assert.equal(e.send({ type: "move_clip", clip: 10, start: 0, node: 99 }), false);
  e.send({ type: "undo" });
  e.send({ type: "undo" });
  assert.equal(nodeOf(last("project"), 10), 1);
  assert.equal(nodeOf(last("project"), 30), 3);
  // Clip de notes : rogner la fin garde les notes cachées, étendre les remontre.
  const notes = (id) => last("project").nodes.flatMap((n) => n.clips).find((c) => c.id === id).notes;
  assert.equal(notes(30).length, 64);
  e.send({ type: "trim_clip", clip: 30, end: 480_000 });
  assert.equal(notes(30).length, 8);
  e.send({ type: "trim_clip", clip: 30, end: 1_152_000 });
  assert.equal(notes(30).length, 64, "rien n'a été détruit");
  e.send({ type: "trim_clip", clip: 30, start: 408_000 });
  assert.equal(notes(30).length, 62);
  assert.equal(e.send({ type: "trim_clip", clip: 30, start: 100 }), false, "avant le début de la matière");
  // Découpe : chaque note n'est jouée que d'un côté.
  e.send({ type: "undo" });
  assert.ok(e.send({ type: "split_clip", clip: 30, at: 480_000 }));
  const halves = last("project").nodes[3].clips.filter((c) => c.kind === "midi");
  assert.equal(halves.length, 2);
  assert.equal(halves.reduce((n, c) => n + c.notes.length, 0), 64);
  // Fondus de vélocité : la première note est plus douce que celle du milieu.
  e.send({ type: "undo" });
  e.send({ type: "set_clip_fades", clip: 30, fade_in: 48_000 });
  assert.ok(notes(30)[0].velocity < notes(30)[10].velocity);
  // Fondu enchaîné automatique : remonter Beat B de 96 000 sur Beat A (même piste).
  e.send({ type: "move_clip", clip: 11, start: 672_000 });
  const p = last("project");
  const a = p.nodes[1].clips.find((c) => c.id === 10);
  const b = p.nodes[1].clips.find((c) => c.id === 11);
  assert.deepEqual([a.fade_out, b.fade_in], [96_000, 96_000]);
  assert.equal(p.auto_crossfade, true);
  e.send({ type: "set_auto_crossfade", enabled: false });
  assert.equal(last("project").auto_crossfade, false);
  e.send({ type: "move_clip", clip: 11, start: 700_000 });
  assert.equal(last("project").nodes[1].clips.find((c) => c.id === 10).fade_out, 96_000, "désactivé : les fondus ne sont plus recalculés");
});

test("courbes de fondu : mêmes valeurs que le moteur (formes de base, courbure, adoucissement)", async () => {
  const { fadeGain, crossfadeCurves, curvePoints, PRESETS, presetOf, sameCurve } = await import("../js/fades.js");
  const c = (shape, bend = 0, smooth = 0) => ({ shape, bend, smooth });
  // x = 0,25 et 0,75 — les mêmes valeurs que le test Rust `fade_curves_have_the_documented_values…`.
  const near = (got, want, what) => assert.ok(Math.abs(got - want) < 1e-6, `${what} : ${got} au lieu de ${want}`);
  for (const [curve, [a, b], what] of [
    [c("linear"), [0.25, 0.75], "linéaire"], [c("equal_power"), [0.5, 0.8660254], "puissance constante"], [c("linear", 0, 1000), [0.15625, 0.84375], "S complet"],
    [c("linear", 0, 500), [0.5 * (0.25 + 0.15625), 0.5 * (0.75 + 0.84375)], "S à moitié"], [c("linear", 1000, 0), [0.0625, 0.5625], "convexe"], [c("linear", -1000, 0), [0.4375, 0.9375], "concave"],
    [c("linear", 500, 0), [0.25 - 0.5 * 0.25 * 0.75, 0.75 - 0.5 * 0.75 * 0.25], "demi-convexe"],
  ]) {
    near(fadeGain(curve, 0.25), a, `${what} à 0,25`);
    near(fadeGain(curve, 0.75), b, `${what} à 0,75`);
  }
  near(fadeGain(c("equal_power", 1000, 1000), 0.25), Math.sqrt(0.15625 - 0.15625 * (1 - 0.15625)), "S + convexe + puissance");
  // Toute courbe est croissante de 0 à 1.
  for (const shape of ["linear", "equal_power"]) for (const bend of [-1000, -300, 0, 300, 1000]) for (const smooth of [0, 400, 1000]) {
    const pts = curvePoints(c(shape, bend, smooth), 50);
    assert.ok(Math.abs(pts[0][1]) < 1e-12 && Math.abs(pts[50][1] - 1) < 1e-12, `${shape} ${bend} ${smooth} : bornes`);
    assert.ok(pts.every((p, i) => i === 0 || p[1] >= pts[i - 1][1] - 1e-12), `${shape} ${bend} ${smooth} croissante`);
  }
  // Fondu enchaîné : courbures opposées de même adoucissement = somme des gains constante ; puissance constante = somme des carrés.
  for (const bend of [-1000, -400, 0, 400, 1000]) for (const smooth of [0, 500, 1000]) {
    for (const p of crossfadeCurves(c("linear", bend, smooth), c("linear", -bend, smooth))) assert.ok(Math.abs(p.sum - 1) < 1e-9, `${bend}/${smooth} à ${p.x}`);
  }
  for (const p of crossfadeCurves(c("equal_power"), c("equal_power"))) assert.ok(Math.abs(p.power - 1) < 1e-9);
  assert.ok(Math.abs(crossfadeCurves(c("linear"), c("linear"))[24].power - Math.SQRT1_2) < 1e-9, "linéaire : -3 dB de volume perçu au milieu pour des signaux sans rapport");
  // Réglages rapides : retrouvés par leur courbe, et une courbe personnalisée n'en désigne aucun.
  for (const p of PRESETS) assert.equal(presetOf(p.curve), p.id);
  assert.equal(presetOf(c("linear", 250, 0)), null);
  assert.ok(sameCurve({ shape: "linear" }, c("linear", 0, 0)), "bend et smooth absents = 0");
  assert.equal(PRESETS.length, 5);
});

test("fondu enchaîné sous la souris : deux clips qui se chevauchent, avec une tolérance de visée", async () => {
  const { crossfadeAt } = await import("../js/ui/clip-geometry.js");
  const mk = (id, start, len, extra = {}) => ({ id, kind: "audio", start, len, offset: 0, source_len: len + 1e5, fade_in: 0, fade_out: 0, ...extra });
  const node = { clips: [mk(1, 0, 1_000, { fade_out: 300 }), mk(2, 700, 1_000, { fade_in: 300 })] };
  const cf = crossfadeAt(node, 800);
  assert.deepEqual([cf.a.id, cf.b.id, cf.start, cf.end], [1, 2, 700, 1_000]);
  assert.equal(crossfadeAt(node, 500), null, "hors de la zone commune");
  assert.equal(crossfadeAt(node, 1_200), null);
  assert.ok(crossfadeAt({ clips: [mk(1, 0, 1_000, { fade_out: 100 }), mk(2, 700, 1_000, { fade_in: 300 })] }, 800), "fondus qui ne correspondent pas à la zone : l'éditeur les remet d'accord");
  assert.ok(crossfadeAt({ clips: [mk(1, 0, 1_000), mk(2, 700, 1_000)] }, 800), "clips qui se chevauchent sans fondu");
  assert.equal(crossfadeAt(node, 690), null);
  assert.deepEqual([crossfadeAt(node, 690, 20)?.a.id, crossfadeAt(node, 1_015, 20)?.b.id], [1, 2], "tolérance de visée de chaque côté");
  assert.equal(crossfadeAt({ clips: [mk(1, 0, 1_000), mk(2, 2_000, 500)] }, 500), null);
  const { crossfadeOf } = await import("../js/ui/clip-geometry.js");
  assert.deepEqual([crossfadeOf(node, 1)?.b.id, crossfadeOf(node, 2)?.a.id, crossfadeOf(node, 9)], [2, 1, null]);
});

test("faux moteur : courbes de fondu, éditeur de fondu enchaîné, conservation à travers un déplacement", () => {
  const e = new MockEngine();
  const ev = collect(e, "project", "history", "error");
  const last = (t) => ev.filter((x) => x.type === t).at(-1);
  const clip = (id) => last("project").nodes.flatMap((n) => n.clips).find((c) => c.id === id);
  const c = (shape, bend = 0, smooth = 0) => ({ shape, bend, smooth });
  e.send({ type: "move_clip", clip: 11, start: 672_000 });
  assert.deepEqual([clip(10).fade_out_curve, clip(11).fade_in_curve], [c("equal_power"), c("equal_power")]);
  assert.ok(e.send({ type: "set_crossfade", clip_out: 10, clip_in: 11, curve_out: c("linear", 800, 200), curve_in: c("linear", -800, 200) }));
  assert.deepEqual([clip(10).fade_out_curve, clip(11).fade_in_curve], [c("linear", 800, 200), c("linear", -800, 200)]);
  assert.equal(last("history").undo_label, "Courbe du fondu enchaîné");
  e.send({ type: "move_clip", clip: 11, start: 720_000 });
  assert.deepEqual([clip(10).fade_out_curve, clip(11).fade_in_curve, clip(10).fade_out], [c("linear", 800, 200), c("linear", -800, 200), 48_000], "courbes conservées, longueurs recalculées");
  e.send({ type: "undo" });
  e.send({ type: "undo" });
  assert.deepEqual([clip(10).fade_out_curve, clip(11).fade_in_curve], [c("equal_power"), c("equal_power")]);
  // Un seul fondu ; bend et smooth absents valent 0.
  assert.ok(e.send({ type: "set_clip_fades", clip: 20, fade_in: 4_000, curve_in: { shape: "linear", smooth: 1000 } }));
  assert.deepEqual([clip(20).fade_in, clip(20).fade_in_curve], [4_000, c("linear", 0, 1000)]);
  // Refus : forme inconnue, courbure hors plage, clip de notes, clips non audio.
  assert.equal(e.send({ type: "set_clip_fades", clip: 20, curve_in: { shape: "zigzag" } }), false);
  assert.equal(e.send({ type: "set_clip_fades", clip: 20, curve_in: { shape: "linear", bend: 1500 } }), false);
  assert.equal(e.send({ type: "set_clip_fades", clip: 20, curve_in: { shape: "linear", smooth: -1 } }), false);
  assert.equal(e.send({ type: "set_clip_fades", clip: 30, curve_in: c("linear") }), false);
  assert.match(ev.filter((x) => x.type === "error").at(-1).message, /linéaires/);
  assert.equal(e.send({ type: "set_crossfade", clip_out: 10, clip_in: 30, curve_out: c("linear") }), false);
  assert.equal(e.send({ type: "set_crossfade", clip_out: 10, clip_in: 10, curve_out: c("linear") }), false);
  assert.equal(e.send({ type: "set_crossfade", clip_out: 10, clip_in: 11 }), false);
});
