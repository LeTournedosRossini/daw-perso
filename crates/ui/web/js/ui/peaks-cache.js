// Cache de crêtes de formes d'onde par tuiles de 256 colonnes (une colonne = un pixel au zoom courant). Les tuiles sont
// demandées au moteur (`get_peaks`) et rangées dès que la réponse arrive ; un tour de dessin sans réponse laisse la place vide.

export const TILE_COLUMNS = 256;
const MAX_TILES = 600;

export class PeaksCache {
  constructor(client, onUpdate) {
    this.client = client;
    this.onUpdate = onUpdate;
    this.listeners = new Set();
    this.tiles = new Map(); // clé → { from, to, spp, columns } ou "pending"
    this.byRequest = new Map();
    client.on("peaks", (e) => this.#receive(e));
  }

  /**
   * Une tuile dépend de l'étendue demandée (`from`, `to`) et de la place du fichier sur la ligne de temps (`shift` =
   * début − décalage) : sans cela, après un déplacement ou un rognage la tuile partielle de l'ancienne position restait
   * en cache et la partie nouvellement couverte du clip n'était jamais redemandée (trou dans la forme d'onde). `data` (identité des données
   * du clip, `data_id`) : un clip dont le **fichier** change sans que son identifiant ni sa géométrie changent (étirement rendu, retrait,
   * annulation) ne doit pas garder l'ancienne image.
   */
  #key(clip, spp, tile, from, to, shift, data) {
    return `${clip}|${spp}|${tile}|${from}|${to}|${shift}|${data}`;
  }

  /** Appelle `fn` à chaque tuile reçue ; rend la fonction qui l'arrête. */
  addListener(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /** Tuiles `[{ from, to, columns }]` disponibles qui recouvrent `[s0, s1[` pour ce clip, en demandant les manquantes. */
  tilesFor(clip, spp, s0, s1) {
    const first = Math.max(0, Math.floor(Math.max(s0, clip.start) / (spp * TILE_COLUMNS)));
    const last = Math.floor((Math.min(s1, clip.start + clip.len) - 1) / (spp * TILE_COLUMNS));
    const out = [];
    for (let k = first; k <= last; k++) {
      const from = Math.max(clip.start, Math.round(k * spp * TILE_COLUMNS));
      const to = Math.min(clip.start + clip.len, Math.round((k + 1) * spp * TILE_COLUMNS));
      if (to <= from) continue;
      const key = this.#key(clip.id, spp, k, from, to, clip.start - (clip.offset ?? 0), clip.data_id ?? 0);
      const t = this.tiles.get(key);
      if (t && t !== "pending") {
        out.push(t);
      } else if (!t) {
        const columns = Math.max(1, Math.round((to - from) / spp));
        this.tiles.set(key, "pending");
        this.byRequest.set(`${clip.id}|${from}|${to}`, { key, spp });
        if (this.tiles.size > MAX_TILES) this.#evict();
        this.client.send({ type: "get_peaks", clip: clip.id, from, to, columns });
      }
    }
    return out;
  }

  #receive(e) {
    const req = this.byRequest.get(`${e.clip}|${e.from}|${e.to}`);
    if (!req) return;
    this.byRequest.delete(`${e.clip}|${e.from}|${e.to}`);
    this.tiles.set(req.key, { from: e.from, to: e.to, spp: req.spp, columns: e.columns });
    this.onUpdate();
    for (const f of this.listeners) f();
  }

  #evict() {
    // Les plus anciennes d'abord (ordre d'insertion de la Map), hors demandes en cours.
    for (const [k, v] of this.tiles) {
      if (this.tiles.size <= MAX_TILES * 0.8) break;
      if (v !== "pending") this.tiles.delete(k);
    }
  }

  clear() {
    this.tiles.clear();
    this.byRequest.clear();
  }
}
