#include "align/Pipeline.h"
#include "align/Progress.h"

#include <algorithm>
#include <cmath>

namespace align
{
Analysis analysePair (const float* g, size_t nG, const float* d, size_t nD, double sr, const AlignSettings& a, const AnalysisSettings& an)
{
    Analysis out;
    out.sampleRate = sr;
    out.length = nG;
    out.path = alignSignals (g, nG, d, nD, sr, a, an);
    out.consonants = findConsonants (d, nD, sr);
    return out;
}

Analysis analyseAgainstGuide (const GuideAnalysis& g, const float* d, size_t nD, const AlignSettings& a)
{
    Analysis out;
    out.sampleRate = g.sampleRate;
    out.length = g.length;
    {
        progress::Phase ph (0.0, 0.9);
        out.path = alignDubToGuide (g, d, nD, a);
    }
    {
        progress::Phase ph (0.9, 1.0);
        out.consonants = findConsonants (d, nD, g.sampleRate);
    }
    return out;
}

namespace
{
std::vector<double> strengthPath (const Analysis& a, const RenderSettings& r)
{
    return r.strengthCurve.empty() ? applyStrength (a.path, a.hopSeconds, r.strength) : applyStrength (a.path, a.hopSeconds, r.strengthCurve);
}
} // namespace

RenderResult renderAnalysis (const std::vector<std::vector<float>>& dub, const Analysis& a, const RenderSettings& r)
{
    const auto path = strengthPath (a, r);
    const auto map = makeWarpMap (path, a.hopSeconds, a.sampleRate, a.length, a.consonants, r.protect);
    return render (dub, map, a.sampleRate, r.engine);
}

std::vector<double> appliedShift (const Analysis& a, const RenderSettings& r)
{
    const auto path = strengthPath (a, r);
    std::vector<double> s (path.size());
    for (size_t i = 0; i < path.size(); ++i) s[i] = path[i] - (double) i * a.hopSeconds;
    return s;
}

std::vector<StrengthPoint> suggestStrengthCurve (const Analysis& a, const SuggestSettings& s)
{
    std::vector<StrengthPoint> out;
    const size_t n = a.path.size();
    if (n < 2) return out;
    const double total = (double) (n - 1) * a.hopSeconds;
    const double enter = s.thresholdMs / 1000.0, leave = 0.6 * enter;
    struct Region { double start, end; };
    std::vector<Region> regions;
    bool inside = false;
    double start = 0;
    for (size_t i = 0; i < n; ++i)
    {
        const double t = (double) i * a.hopSeconds, m = std::fabs (a.path[i] - t);
        if (! inside && m > enter) { inside = true; start = t; }
        else if (inside && m < leave) { inside = false; regions.push_back ({ start, t }); }
    }
    if (inside) regions.push_back ({ start, total });
    // Ignorer les trop courts, réunir les proches, élargir.
    std::vector<Region> kept;
    for (const auto& r : regions)
        if (r.end - r.start >= s.minSeconds) kept.push_back (r);
    std::vector<Region> merged;
    for (const auto& r : kept)
    {
        if (! merged.empty() && r.start - merged.back().end <= s.mergeGapSeconds) merged.back().end = r.end;
        else merged.push_back (r);
    }
    for (auto& r : merged) { r.start = std::max (0.0, r.start - s.padSeconds); r.end = std::min (total, r.end + s.padSeconds); }
    // Les élargissements peuvent se chevaucher : on réunit de nouveau.
    std::vector<Region> fin;
    for (const auto& r : merged)
    {
        if (! fin.empty() && r.start <= fin.back().end + 2.0 * s.rampSeconds + 0.2) fin.back().end = std::max (fin.back().end, r.end);
        else fin.push_back (r);
    }
    if (fin.empty()) return out;
    if (fin.size() == 1 && fin.front().start <= 0.0 && fin.front().end >= total) return out; // tout le morceau : force constante
    auto add = [&] (double t, double v) {
        t = std::clamp (t, 0.0, total);
        if (! out.empty() && t <= out.back().time) t = out.back().time + 1e-3;
        out.push_back ({ t, v });
    };
    // Avant le premier point et après le dernier, la force est celle du point le plus proche : pas besoin de points aux deux bouts.
    for (const auto& r : fin)
    {
        if (r.start - s.rampSeconds > 0.0) add (r.start - s.rampSeconds, s.base);
        else if (out.empty() || out.back().strength != s.high) { /* le passage commence au tout début : on part directement de la force haute */ }
        add (r.start, s.high);
        add (r.end, s.high);
        if (r.end + s.rampSeconds < total) add (r.end + s.rampSeconds, s.base);
    }
    return out;
}
} // namespace align
