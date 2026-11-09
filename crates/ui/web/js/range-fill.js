// Curseurs linéaires : tient à jour `--p` (part parcourue du rail) de tous les `input[type=range]` de la page, pour que la partie parcourue
// soit en couleur d'accent comme sur les faders du mixeur. Les valeurs changées par le code (sans événement) sont rattrapées par un
// balayage léger tant que la page est visible.

const pct = (r) => {
  const min = Number(r.min || 0);
  const max = Number(r.max || 100);
  const v = Number(r.value);
  return max > min ? Math.max(0, Math.min(100, ((v - min) / (max - min)) * 100)) : 0;
};

export function syncRange(r) {
  const p = `${pct(r).toFixed(1)}%`;
  if (r.style.getPropertyValue("--p") !== p) r.style.setProperty("--p", p);
}

export function startRangeFill(root = document) {
  const all = () => root.querySelectorAll('input[type="range"]');
  root.addEventListener("input", (e) => e.target?.type === "range" && syncRange(e.target), true);
  const sweep = () => {
    if (!document.hidden) all().forEach(syncRange);
  };
  sweep();
  return setInterval(sweep, 150);
}
