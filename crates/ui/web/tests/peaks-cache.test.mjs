import test from "node:test";
import assert from "node:assert/strict";
import { PeaksCache, TILE_COLUMNS } from "../js/ui/peaks-cache.js";

// Client factice : répond tout de suite à `get_peaks` par des colonnes marquées avec la position de départ demandée.
function setup() {
  const listeners = [];
  const requests = [];
  const client = {
    on: (type, fn) => type === "peaks" && listeners.push(fn),
    send: (m) => {
      requests.push(m);
      for (const fn of listeners) fn({ clip: m.clip, from: m.from, to: m.to, columns: Array.from({ length: m.columns }, () => [-0.5, 0.5]) });
    },
  };
  return { cache: new PeaksCache(client, () => {}), requests };
}

const SPP = 100;
const TILE = SPP * TILE_COLUMNS; // 25 600 échantillons par tuile
const covered = (tiles) => tiles.flatMap((t) => [[t.from, t.to]]);

test("après un déplacement, la partie nouvellement couverte du clip est redemandée (pas de trou au début)", () => {
  const { cache, requests } = setup();
  const clip = { id: 1, start: 30_000, offset: 0, len: 60_000 };
  cache.tilesFor(clip, SPP, 0, 200_000); // première demande : tuiles partielles
  const first = cache.tilesFor(clip, SPP, 0, 200_000);
  assert.equal(first[0].from, 30_000);
  // Le clip est déplacé 10 000 échantillons plus tôt (même tuile de départ).
  const moved = { ...clip, start: 20_000 };
  const before = requests.length;
  cache.tilesFor(moved, SPP, 0, 200_000);
  assert.ok(requests.length > before, "de nouvelles tuiles sont demandées");
  const tiles = cache.tilesFor(moved, SPP, 0, 200_000);
  assert.equal(tiles[0].from, 20_000, "la première tuile commence au nouveau début du clip");
  assert.equal(tiles.at(-1).to, 80_000);
  // Aucun trou : les tuiles se suivent.
  const c = covered(tiles);
  for (let i = 1; i < c.length; i++) assert.equal(c[i][0], c[i - 1][1]);
  void TILE;
});

test("un rognage de la fin ou du début (décalage) invalide aussi les tuiles", () => {
  const { cache, requests } = setup();
  const clip = { id: 2, start: 0, offset: 0, len: 50_000 };
  cache.tilesFor(clip, SPP, 0, 100_000);
  cache.tilesFor(clip, SPP, 0, 100_000);
  const n = requests.length;
  cache.tilesFor({ ...clip, len: 60_000 }, SPP, 0, 100_000); // allongé
  assert.ok(requests.length > n);
  const m = requests.length;
  cache.tilesFor({ ...clip, offset: 500 }, SPP, 0, 100_000); // même place, autre contenu
  assert.ok(requests.length > m);
  const k = requests.length;
  cache.tilesFor({ ...clip, offset: 500 }, SPP, 0, 100_000); // inchangé : rien de nouveau
  assert.equal(requests.length, k);
});
