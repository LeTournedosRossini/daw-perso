#pragma once
// Chaîne complète en deux temps, pour que l'outil en ligne de commande et le plugin fassent exactement la même chose :
//   1. analyse (lente, une fois) : chemin d'alignement + consonnes du double ;
//   2. rendu (rapide à refaire) : étireur, force (constante ou variable), protection des consonnes. On peut changer ces réglages sans refaire l'analyse.
#include <vector>

#include "align/Render.h"

namespace align
{
struct Analysis
{
    std::vector<double> path;       // pour chaque trait de 5 ms du guide, position (secondes) du double
    std::vector<Segment> consonants; // consonnes du double (secondes) ; modifiable par l'utilisateur avant le rendu
    double sampleRate = 44100.0;
    size_t length = 0;              // durée de la sortie en échantillons (= celle du guide)
    double hopSeconds = 0.005;
};

struct RenderSettings
{
    Engine engine = Engine::Signalsmith;
    double strength = 1.0;                 // utilisée si `strengthCurve` est vide
    std::vector<StrengthPoint> strengthCurve;
    double protect = 0.0;                  // protection des consonnes, 0 à 1
};

Analysis analysePair (const float* guideMono, size_t nGuide, const float* dubMono, size_t nDub, double sampleRate,
                      const AlignSettings& a = {}, const AnalysisSettings& an = {});

// Même chose à partir d'un guide déjà analysé (lot : une analyse de guide pour N doubles).
Analysis analyseAgainstGuide (const GuideAnalysis& guide, const float* dubMono, size_t nDub, const AlignSettings& a = {});

RenderResult renderAnalysis (const std::vector<std::vector<float>>& dub, const Analysis& analysis, const RenderSettings& r);

// Suggestion de force variable : repère les passages où le double est très décalé du guide et y pose la force à fond, ailleurs au plus bas.
// Un passage commence quand le décalage dépasse `thresholdMs` et finit quand il redescend à 60 % du seuil (hystérésis) ; les passages trop courts sont
// ignorés, ceux qui sont proches sont réunis, chacun est élargi de `padSeconds`, et les transitions durent `rampSeconds`.
struct SuggestSettings
{
    double thresholdMs = 40.0;
    double high = 1.0, base = 0.0;
    double padSeconds = 0.25, rampSeconds = 0.15, minSeconds = 0.20, mergeGapSeconds = 0.6;
};
// Rend une courbe prête pour `RenderSettings::strengthCurve`, ou une liste vide si aucun passage ne dépasse le seuil (ou si tout le morceau le dépasse :
// une force constante suffit alors).
std::vector<StrengthPoint> suggestStrengthCurve (const Analysis& analysis, const SuggestSettings& s = {});

// Décalage (secondes) appliqué à chaque trait après force et protection : pour dessiner la courbe de décalage.
std::vector<double> appliedShift (const Analysis& analysis, const RenderSettings& r);
} // namespace align
