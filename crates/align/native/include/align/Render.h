#pragma once
// Rendu du double recalé : applique une WarpMap (temps du guide -> échantillon du double) avec l'étireur choisi.
// Trois moteurs au choix : Signalsmith Stretch (par défaut), Rubber Band R2 (« faster ») et Rubber Band R3 (« finer »).
// Quel que soit le moteur : la sortie a EXACTEMENT la longueur demandée, ne contient que des valeurs finies, et un moteur qui échoue
// (exception, rapport invalide) est remplacé par Signalsmith. `engineUsed` dit lequel a réellement servi.
#include <vector>

#include "align/Align.h"

namespace align
{
enum class Engine { Signalsmith, RubberBandR2, RubberBandR3 };

struct RenderResult
{
    std::vector<std::vector<float>> channels; // même nombre de voies que l'entrée, chacune de map.outputLength échantillons
    Engine engineUsed = Engine::Signalsmith;
    bool fellBack = false;
};

// `dub` : une voie par vecteur (toutes de même longueur). La même courbe est appliquée à toutes les voies (image stéréo conservée).
RenderResult render (const std::vector<std::vector<float>>& dub, const WarpMap& map, double sampleRate, Engine engine = Engine::Signalsmith);
const char* engineName (Engine e);
} // namespace align
