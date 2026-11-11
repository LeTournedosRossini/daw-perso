// Résumé des tâches en cours pour la pastille du bandeau (fonction pure, testée). `texts` : le texte de chaque tâche visible
// (« Import 2/5 · 40 % », « Export 12 % »…). Aucune tâche : null (la pastille se cache) ; une seule : son texte ; plusieurs : leur nombre.
export function tasksSummary(texts) {
  const list = (texts ?? []).map((t) => String(t ?? "").trim()).filter(Boolean);
  if (list.length === 0) return null;
  if (list.length === 1) return { count: 1, label: list[0], title: `${list[0]} — appuyer pour agir` };
  return { count: list.length, label: `${list.length} tâches`, title: list.join("\n") };
}
