#pragma once
// Fiche de réglages d'un calage, lisible et écrivable en JSON (format « flibuste-align-settings », version 1).
//
//   { "format": "flibuste-align-settings", "version": 1,
//     "general": { "analysis_mode": "same_pitch" | "harmony", "tightness": 0.95, "max_shift_seconds": 1.5,
//                  "engine": "signalsmith" | "rubberband_r2" | "rubberband_r3", "strength": 1.0, "protect": 1.0 },
//     "local":   { "strength_curve": [[temps_s, force], ...] } }
//
// « general » est ce qui se copie d'un double à l'autre ; « local » dépend de l'endroit où chaque chanteur était décalé et ne se copie JAMAIS (décision de Jean).
// Les consonnes modifiées à la main sont aussi « locales » : elles vivent dans l'analyse (voir Pipeline.h), pas dans la fiche.
// Lecture tolérante : champ absent = valeur par défaut, champ inconnu ignoré ; valeur invalide ou JSON abîmé = erreur avec message lisible.
#include <string>
#include <vector>

#include "align/Pipeline.h"

namespace align
{
struct GeneralSettings
{
    bool harmony = false;           // analysis_mode : « harmony » = analyse insensible à la hauteur
    double tightness = 0.95;        // 0 à 1
    double maxShiftSeconds = 1.5;   // 0,1 à 10
    Engine engine = Engine::Signalsmith;
    double strength = 1.0;          // 0 à 1 (force constante, utilisée si la courbe de force est vide)
    double protect = 1.0;           // 0 à 1 (protection des consonnes)
};

struct SettingsSheet
{
    GeneralSettings general;
    std::vector<StrengthPoint> strengthCurve; // « local »
};

AlignSettings alignSettingsOf (const GeneralSettings&);
AnalysisSettings analysisSettingsOf (const GeneralSettings&);
RenderSettings renderSettingsOf (const SettingsSheet&);

// Texte JSON <-> fiche. `error` est rempli en cas d'échec (parse rend faux).
std::string toJson (const SettingsSheet&);
bool parseSettings (const std::string& json, SettingsSheet& out, std::string& error);

// Rend `dst` avec le bloc « general » de `src` et le bloc « local » de `dst` inchangé (copier-coller des réglages généraux). Faux si un texte est invalide.
bool mergeGeneralSettings (const std::string& dstJson, const std::string& srcJson, std::string& outJson, std::string& error);

// Analyse sérialisée en octets (pour l'enregistrer avec la prise et refaire le rendu plus tard) : format « ALN1 », petit-boutiste.
std::vector<unsigned char> serializeAnalysis (const Analysis&);
bool deserializeAnalysis (const unsigned char* data, std::size_t size, Analysis& out, std::string& error);
} // namespace align
