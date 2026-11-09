#pragma once
// Progression et annulation des calculs longs. Le rappel reçoit une fraction de 0 à 1 et rend vrai pour ANNULER. Il est installé pour le fil courant
// (aucun état global : plusieurs calculs en parallèle sur des fils différents ont chacun le leur). Une annulation lève `align::Cancelled`, que l'interface C
// transforme en code d'erreur : aucune exception ne traverse le C.
#include <functional>

namespace align
{
struct Cancelled {};
using ProgressFn = std::function<bool (double)>;

namespace progress
{
// Installe `fn` (peut être nul) pour le fil courant pendant la durée de vie de l'objet.
class Scope
{
public:
    explicit Scope (const ProgressFn* fn);
    ~Scope();
    Scope (const Scope&) = delete;
    Scope& operator= (const Scope&) = delete;
private:
    const ProgressFn* previous_;
    double previousBase_, previousSpan_;
};

// Restreint les rapports qui suivent à l'intervalle [from, to] de la fraction courante : un calcul en plusieurs étapes rend ainsi une progression continue.
class Phase
{
public:
    Phase (double from, double to);
    ~Phase();
    Phase (const Phase&) = delete;
    Phase& operator= (const Phase&) = delete;
private:
    double previousBase_, previousSpan_;
};

// `fraction` : avancement (0 à 1) dans la phase courante. Lève Cancelled si le rappel le demande. Sans rappel installé : ne fait rien.
void report (double fraction);
} // namespace progress
} // namespace align
