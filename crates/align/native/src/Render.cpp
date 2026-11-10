#include "align/Render.h"
#include "align/Progress.h"

#include <algorithm>
#include <cmath>
#include <exception>
#include <map>

#include "rubberband/RubberBandStretcher.h"
#include "signalsmith-stretch/signalsmith-stretch.h"

namespace align
{
namespace
{
using Chans = std::vector<std::vector<float>>;

float at (const std::vector<float>& x, long i) { return (i >= 0 && (size_t) i < x.size()) ? x[(size_t) i] : 0.f; }

// Rubber Band hors temps réel, carte de points clés entrée -> sortie (un point tous les keyEvery pas de la carte, ~45 ms).
Chans renderRubberBand (const Chans& dub, const WarpMap& m, double sr, bool finer)
{
    const size_t nch = dub.size(), dubLen = dub[0].size();
    const double s0d = std::max (0.0, m.sourceAt (0.0)), s1d = std::min ((double) dubLen, m.sourceAt ((double) m.outputLength));
    const long s0 = (long) std::floor (s0d), s1 = std::max (s0 + 64, (long) std::ceil (s1d));
    const size_t inLen = (size_t) (s1 - s0);
    Chans in (nch, std::vector<float> (inLen));
    for (size_t c = 0; c < nch; ++c)
        for (size_t i = 0; i < inLen; ++i) in[c][i] = at (dub[c], s0 + (long) i);

    std::map<size_t, size_t> keys;
    long lastIn = -1;
    const size_t keyEvery = 8;
    for (const auto& a : m.anchors) // carte par marqueurs : ses points de rupture exacts suffisent, la carte est linéaire entre eux
    {
        const long inC = std::clamp ((long) std::llround (a.first - (double) s0), 0L, (long) inLen);
        if (inC <= lastIn || inC == 0) continue;
        keys[(size_t) inC] = (size_t) std::llround (a.second);
        lastIn = inC;
    }
    for (size_t k = 0; m.anchors.empty() && k * m.stepSamples <= m.outputLength && k < m.sourceAtOutput.size(); k += keyEvery)
    {
        const long inC = std::clamp ((long) std::llround (m.sourceAtOutput[k] - (double) s0), 0L, (long) inLen);
        if (inC <= lastIn || inC == 0 || k == 0) continue; // le point (0, 0) est implicite : Rubber Band R3 en tire un rapport NaN
        keys[(size_t) inC] = k * m.stepSamples;
        lastIn = inC;
    }
    keys[inLen] = m.outputLength; // la fin exacte

    using RB = RubberBand::RubberBandStretcher;
    const RB::Options opt = RB::OptionProcessOffline | (finer ? RB::OptionEngineFiner : RB::OptionEngineFaster) | RB::OptionThreadingNever
                          | RB::OptionPitchHighQuality;
    RB st ((size_t) sr, nch, opt, (double) m.outputLength / (double) inLen, 1.0);
    st.setExpectedInputDuration (inLen);
    st.setKeyFrameMap (keys);
    std::vector<const float*> ip (nch);
    for (size_t c = 0; c < nch; ++c) ip[c] = in[c].data();
    st.study (ip.data(), inLen, true);

    Chans out (nch);
    std::vector<std::vector<float>> tmp (nch, std::vector<float> (4096));
    std::vector<float*> tp (nch);
    for (size_t c = 0; c < nch; ++c) tp[c] = tmp[c].data();
    auto drain = [&] {
        int av;
        while ((av = st.available()) > 0)
        {
            const size_t got = st.retrieve (tp.data(), std::min<size_t> ((size_t) av, 4096));
            for (size_t c = 0; c < nch; ++c) out[c].insert (out[c].end(), tmp[c].begin(), tmp[c].begin() + (long) got);
            if (got == 0) break;
        }
    };
    for (size_t pos = 0; pos < inLen;)
    {
        progress::report ((double) pos / (double) inLen);
        const size_t n = std::min<size_t> (1024, inLen - pos);
        for (size_t c = 0; c < nch; ++c) ip[c] = in[c].data() + pos;
        pos += n;
        st.process (ip.data(), n, pos == inLen);
        drain();
    }
    drain();
    return out;
}

// Signalsmith Stretch : cadence variable en donnant à chaque bloc de sortie la quantité d'entrée voulue.
Chans renderSignalsmith (const Chans& dub, const WarpMap& m, double sr)
{
    const int nch = (int) dub.size();
    signalsmith::stretch::SignalsmithStretch<float> st (12345); // graine fixe : deux rendus identiques donnent les mêmes échantillons
    st.presetDefault (nch, (float) sr);
    const int inL = st.inputLatency(), outL = st.outputLatency();
    // Retard constant mesuré sur des doublures à correspondance connue (calage résiduel < 1 ms) : 56,2 ms, soit un peu moins que inL.
    const long delay = std::lround (0.0562 * sr);
    const long s0 = std::llround (m.sourceAt (0.0));
    std::vector<std::vector<float>> seekBuf ((size_t) nch, std::vector<float> ((size_t) inL));
    std::vector<float*> cp ((size_t) nch);
    for (int c = 0; c < nch; ++c)
    {
        for (int i = 0; i < inL; ++i) seekBuf[(size_t) c][(size_t) i] = at (dub[(size_t) c], s0 + i);
        cp[(size_t) c] = seekBuf[(size_t) c].data();
    }
    st.seek (cp, inL, 1.0);

    const size_t total = m.outputLength + (size_t) outL + 4096, B = 256;
    std::vector<std::vector<float>> stream ((size_t) nch, std::vector<float> (total, 0.f)), inBuf ((size_t) nch);
    long given = s0 + inL;
    for (size_t v = 0; v < total; v += B)
    {
        if (((v / B) & 63) == 0) progress::report ((double) v / (double) total);
        const size_t nOut = std::min (B, total - v);
        const double uEnd = std::max (0.0, (double) (v + nOut) - (double) outL + (double) delay);
        const long want = std::llround (m.sourceAt (std::min (uEnd, (double) m.outputLength + 2048.0))) + inL;
        const long nIn = std::max (0L, want - given);
        std::vector<float*> ci ((size_t) nch), co ((size_t) nch);
        for (int c = 0; c < nch; ++c)
        {
            inBuf[(size_t) c].resize ((size_t) nIn + 1);
            for (long i = 0; i < nIn; ++i) inBuf[(size_t) c][(size_t) i] = at (dub[(size_t) c], given + i);
            ci[(size_t) c] = inBuf[(size_t) c].data();
            co[(size_t) c] = stream[(size_t) c].data() + v;
        }
        given += nIn;
        st.process (ci, (int) nIn, co, (int) nOut);
    }
    Chans out ((size_t) nch, std::vector<float> (m.outputLength));
    for (size_t c = 0; c < (size_t) nch; ++c)
        for (size_t i = 0; i < m.outputLength; ++i) out[c][i] = stream[c][i + (size_t) outL];
    return out;
}

bool sane (const Chans& out, size_t n)
{
    for (const auto& c : out)
    {
        if (c.size() < n / 2) return false; // un moteur qui rend moins de la moitié de la durée a échoué
        for (float v : c) if (! std::isfinite (v)) return false;
    }
    return true;
}
} // namespace

const char* engineName (Engine e)
{
    return e == Engine::Signalsmith ? "Signalsmith" : e == Engine::RubberBandR2 ? "RubberBand-R2" : "RubberBand-R3";
}

RenderResult render (const Chans& dub, const WarpMap& map, double sr, Engine engine)
{
    RenderResult r;
    r.engineUsed = engine;
    const size_t n = map.outputLength;
    if (dub.empty() || dub[0].empty() || n == 0 || map.sourceAtOutput.empty())
    {
        r.channels.assign (std::max<size_t> (1, dub.size()), std::vector<float> (n, 0.f));
        return r;
    }
    auto run = [&] (Engine e) -> Chans {
        try
        {
            return e == Engine::Signalsmith ? renderSignalsmith (dub, map, sr) : renderRubberBand (dub, map, sr, e == Engine::RubberBandR3);
        }
        catch (const std::exception&) { return {}; }
    };
    Chans out = run (engine);
    if (! sane (out, n) && engine != Engine::Signalsmith)
    {
        out = run (Engine::Signalsmith);
        r.engineUsed = Engine::Signalsmith;
        r.fellBack = true;
    }
    if (out.empty()) out.assign (dub.size(), std::vector<float> (n, 0.f));
    for (auto& c : out)
    {
        c.resize (n, 0.f);
        for (float& v : c) if (! std::isfinite (v)) v = 0.f;
    }
    r.channels = std::move (out);
    return r;
}
} // namespace align
