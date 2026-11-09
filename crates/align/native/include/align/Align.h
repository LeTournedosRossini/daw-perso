#pragma once
// Alignement temporel d'une prise (« double ») sur une autre (« guide »), pour la voix chantée et les phrases de guitare.
// Principe : analyse spectrale des deux signaux (coefficients cepstraux sur échelle mel, un trait tous les 5 ms), puis recherche du meilleur
// chemin d'appariement (« dynamic time warping ») dans une bande autour de la diagonale, avec pente limitée (le double peut aller de deux fois
// plus vite à deux fois plus lentement que le guide). Le chemin est ensuite lissé selon le réglage de « serrage ».
// Aucune dépendance : le cœur se compile et se teste seul.
#include <cstddef>
#include <cstdint>
#include <vector>

namespace align
{
struct Features
{
    int dim = 0;               // nombre de coefficients par trait
    int frames = 0;            // nombre de traits
    double hopSeconds = 0.005; // durée entre deux traits
    std::vector<float> data;   // frames x dim
    std::vector<float> level;  // niveau (dB) de chaque trait, pour reconnaître les silences
    const float* row (int i) const { return data.data() + (size_t) i * dim; }
};

struct AnalysisSettings
{
    double hopSeconds = 0.005;
    double windowSeconds = 0.025;
    int bands = 28;            // filtres mel
    int coefficients = 13;     // MFCC conservés
    double lowHz = 80.0;
    double highHz = 8000.0;
    double silenceDb = -60.0;  // sous ce niveau relatif au maximum, le trait est considéré comme du silence
    double chromaWeight = 0.0; // poids des traits de hauteur (12 classes de notes) : pour deux notes de même timbre ; désactivés par défaut : sans effet mesuré sur les voix synthétiques, à essayer sur de la guitare
    double chromaWindowSeconds = 0.060; // fenêtre plus longue : il faut séparer deux notes voisines dans le grave
};

// Réglage pour les HARMONIES (mêmes paroles chantées à une autre hauteur : tierce, quinte, octave...). Avec peu de coefficients et des bandes plus larges, l'analyse
// décrit les voyelles et non la hauteur ; sur voix synthétiques, l'erreur reste à 3 ms pour tous les intervalles essayés (le réglage par défaut se trompe parfois
// d'une note entière à la quinte ou à l'octave).
inline AnalysisSettings harmonyAnalysis()
{
    AnalysisSettings s;
    s.coefficients = 6;
    s.bands = 20;
    return s;
}

// `x` : signal mono. Pour de la stéréo, l'appelant fait la somme (ou la voie la plus forte).
Features analyse (const float* x, size_t n, double sampleRate, const AnalysisSettings& s = {});

struct AlignSettings
{
    double maxShiftSeconds = 1.5;   // écart maximal entre guide et double, de part et d'autre de la diagonale
    double maxSlope = 2.0;          // vitesse maximale du double par rapport au guide, dans un sens comme dans l'autre (2 ou 3)
    double slopePenalty = 0.35;     // coût d'un pas qui n'est pas à vitesse normale : plus grand = chemin plus droit
    double tightness = 0.95;         // 0 = très lissé (suit à peine), 1 = serre le guide image par image
    bool openEnds = false;          // vrai : le double peut commencer / finir à un autre endroit que le guide (la bande suit alors la diagonale)
};

// Pour chaque trait du guide, la position (fractionnaire, en traits) du double qui lui correspond. Même longueur que le guide.
std::vector<double> alignPath (const Features& guide, const Features& dub, const AlignSettings& s = {});

// Analyse du guide, réutilisable pour plusieurs doubles (lot) : les traits du guide ne sont calculés qu'une fois. Le double doit être analysé avec les mêmes
// `AnalysisSettings` que le guide (mode « même hauteur » ou « harmonies »).
struct GuideAnalysis
{
    Features features;
    double sampleRate = 44100.0;
    std::size_t length = 0; // échantillons
    AnalysisSettings settings;
};
GuideAnalysis analyseGuide (const float* guideMono, size_t n, double sampleRate, const AnalysisSettings& an = {});
// Chemin du double (secondes, un point par trait du guide) à partir d'un guide déjà analysé.
std::vector<double> alignDubToGuide (const GuideAnalysis& guide, const float* dubMono, size_t nDub, const AlignSettings& a = {});

// Même chose avec des signaux : rend, pour chaque trait du guide, la position du double en SECONDES.
std::vector<double> alignSignals (const float* guide, size_t nGuide, const float* dub, size_t nDub, double sampleRate,
                                  const AlignSettings& a = {}, const AnalysisSettings& an = {});

// Consonnes : zones bruitées et riches en aigus (s, ch, t, k, f...) du signal. Une liste simple (début, fin en secondes) que le détecteur
// remplit et qu'une interface pourra modifier : ajouter une zone, en retirer une qui n'est pas une consonne.
struct Segment
{
    double start = 0, end = 0; // secondes, dans le signal analysé
};
struct ConsonantSettings
{
    double minLevelDb = -45.0;      // sous ce niveau (relatif au maximum), on ne cherche pas de consonne
    double minHighRatio = 0.55;     // part de l'énergie au-dessus de 3 kHz
    double minFlatness = 0.15;      // 0 = son tonal, 1 = bruit blanc (dans la bande 2-10 kHz)
    double minSeconds = 0.020;      // zones plus courtes ignorées
    double bridgeSeconds = 0.015;   // trous plus courts comblés
};
std::vector<Segment> findConsonants (const float* x, size_t n, double sampleRate, const ConsonantSettings& s = {});

// Force du recalage : 0 = le double reste tel quel (aucun décalage), 1 = calé complètement, entre les deux = une part du décalage trouvé.
// Agit sur le chemin, avant la carte de correspondance. Le décalage est l'écart entre la position trouvée et le temps du guide.
std::vector<double> applyStrength (const std::vector<double>& pathSeconds, double hopSeconds, double strength);

// Force variable dans le temps : une courbe de points (temps du guide en secondes, force de 0 à 1), jointe par des segments droits. Avant le premier
// point et après le dernier, la force est celle du point le plus proche. Sert à caler fort une phrase très décalée et à laisser le reste presque tel quel.
struct StrengthPoint
{
    double time = 0, strength = 1;
};
std::vector<double> applyStrength (const std::vector<double>& pathSeconds, double hopSeconds, const std::vector<StrengthPoint>& curve);

// Courbe de correspondance en échantillons : pour chaque échantillon de sortie (temps du guide), l'échantillon source du double.
// Sert aux étireurs. `hop` en secondes est celui de `path`.
struct WarpMap
{
    std::vector<double> sourceAtOutput; // un point tous les `stepSamples` échantillons de sortie
    size_t stepSamples = 0;
    size_t outputLength = 0;
    double sourceAt (double outputSample) const;
};
WarpMap makeWarpMap (const std::vector<double>& pathSeconds, double hopSeconds, double sampleRate, size_t outputLength, size_t stepSamples = 220);

// Même chose en protégeant les consonnes du double (`consonants`, en secondes dans le double). `protect` de 0 à 1 : à 1, la consonne est jouée à
// sa vitesse d'origine (ni étirée ni comprimée) et se termine là où elle se terminait dans la courbe d'alignement, donc l'attaque de la voyelle qui
// suit reste calée sur le guide. Le temps gagné ou perdu est reporté sur ce qui précède (silence, fin de la voyelle d'avant). À 0 : inchangé.
WarpMap makeWarpMap (const std::vector<double>& pathSeconds, double hopSeconds, double sampleRate, size_t outputLength,
                     const std::vector<Segment>& consonants, double protect, size_t stepSamples = 220);
} // namespace align
