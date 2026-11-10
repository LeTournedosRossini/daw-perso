#include "align/Align.h"
#include "align/Progress.h"

#include <algorithm>
#include <cmath>
#include <complex>
#include <cstdint>
#include <limits>

namespace align
{
namespace
{
constexpr double kPi = 3.14159265358979323846;

// FFT itérative de taille puissance de 2 (en place).
void fft (std::vector<std::complex<double>>& a)
{
    const size_t n = a.size();
    for (size_t i = 1, j = 0; i < n; ++i)
    {
        size_t bit = n >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j ^= bit;
        if (i < j) std::swap (a[i], a[j]);
    }
    for (size_t len = 2; len <= n; len <<= 1)
    {
        const double ang = -2.0 * kPi / (double) len;
        const std::complex<double> wl (std::cos (ang), std::sin (ang));
        for (size_t i = 0; i < n; i += len)
        {
            std::complex<double> w (1.0, 0.0);
            for (size_t k = 0; k < len / 2; ++k)
            {
                const auto u = a[i + k], v = a[i + k + len / 2] * w;
                a[i + k] = u + v;
                a[i + k + len / 2] = u - v;
                w *= wl;
            }
        }
    }
}

double hzToMel (double f) { return 2595.0 * std::log10 (1.0 + f / 700.0); }
double melToHz (double m) { return 700.0 * (std::pow (10.0, m / 2595.0) - 1.0); }
} // namespace

Features analyse (const float* x, size_t n, double sr, const AnalysisSettings& s)
{
    Features f;
    f.hopSeconds = s.hopSeconds;
    const int nMfcc = s.coefficients, nChroma = s.chromaWeight > 0.0 ? 12 : 0;
    f.dim = nMfcc + nChroma;
    const size_t hop = std::max<size_t> (1, (size_t) std::llround (s.hopSeconds * sr));
    const size_t win = std::max<size_t> (16, (size_t) std::llround (s.windowSeconds * sr));
    size_t nfft = 1;
    while (nfft < win) nfft <<= 1;
    f.frames = n == 0 ? 0 : (int) (n / hop) + 1;
    f.data.assign ((size_t) f.frames * f.dim, 0.f);
    f.level.assign ((size_t) f.frames, -200.f);
    if (f.frames == 0) return f;

    // Filtres mel triangulaires.
    const int nb = s.bands, half = (int) nfft / 2 + 1;
    const double hi = std::min (s.highHz, sr * 0.5 - 1.0);
    std::vector<double> edges ((size_t) nb + 2);
    for (int b = 0; b < nb + 2; ++b)
        edges[(size_t) b] = melToHz (hzToMel (s.lowHz) + (hzToMel (hi) - hzToMel (s.lowHz)) * b / (nb + 1));
    std::vector<std::vector<float>> filt ((size_t) nb, std::vector<float> ((size_t) half, 0.f));
    for (int b = 0; b < nb; ++b)
        for (int k = 0; k < half; ++k)
        {
            const double fr = k * sr / (double) nfft;
            const double lo = edges[(size_t) b], mid = edges[(size_t) b + 1], up = edges[(size_t) b + 2];
            if (fr > lo && fr <= mid) filt[(size_t) b][(size_t) k] = (float) ((fr - lo) / (mid - lo));
            else if (fr > mid && fr < up) filt[(size_t) b][(size_t) k] = (float) ((up - fr) / (up - mid));
        }
    std::vector<double> window (win);
    for (size_t i = 0; i < win; ++i) window[i] = 0.5 - 0.5 * std::cos (2.0 * kPi * (double) i / (double) (win - 1));

    // DCT-II des log-énergies.
    std::vector<double> dct ((size_t) nMfcc * nb);
    for (int c = 0; c < nMfcc; ++c)
        for (int b = 0; b < nb; ++b) dct[(size_t) c * nb + b] = std::cos (kPi * c * (b + 0.5) / nb);

    // Traits de hauteur : spectre sur une fenêtre plus longue, replié en 12 classes de notes (la = 440 Hz).
    const size_t cwin = std::max<size_t> (64, (size_t) std::llround (s.chromaWindowSeconds * sr));
    size_t cfft = 1;
    while (cfft < cwin * 2) cfft <<= 1;
    std::vector<double> cwindow (cwin);
    for (size_t i = 0; i < cwin; ++i) cwindow[i] = 0.5 - 0.5 * std::cos (2.0 * kPi * (double) i / (double) (cwin - 1));
    std::vector<std::complex<double>> cbuf (cfft);
    const int ck0 = std::max (1, (int) std::ceil (100.0 * (double) cfft / sr)), ck1 = std::min ((int) cfft / 2 - 1, (int) (2000.0 * (double) cfft / sr));
    std::vector<int> cLow ((size_t) std::max (0, ck1 + 1), 0);
    std::vector<double> cFrac ((size_t) std::max (0, ck1 + 1), 0.0);
    for (int k = ck0; k <= ck1; ++k)
    {
        const double pitch = 12.0 * std::log2 ((double) k * sr / (double) cfft / 440.0) + 69.0; // numéro MIDI fractionnaire
        const double cl = std::fmod (std::fmod (pitch, 12.0) + 12.0, 12.0);
        cLow[(size_t) k] = (int) std::floor (cl) % 12;
        cFrac[(size_t) k] = cl - std::floor (cl);
    }
    std::vector<std::complex<double>> buf (nfft);
    std::vector<double> logE ((size_t) nb);
    std::vector<double> frameDb ((size_t) f.frames);
    double maxDb = -200.0;
    for (int i = 0; i < f.frames; ++i)
    {
        if ((i & 127) == 0) progress::report ((double) i / (double) f.frames);
        // Trait centré sur i * hop.
        const long start = (long) ((size_t) i * hop) - (long) win / 2;
        double energy = 0.0;
        std::fill (buf.begin(), buf.end(), std::complex<double> (0.0, 0.0));
        for (size_t k = 0; k < win; ++k)
        {
            const long p = start + (long) k;
            const double v = (p >= 0 && (size_t) p < n) ? x[p] : 0.0;
            buf[k] = v * window[k];
            energy += v * v * window[k];
        }
        fft (buf);
        for (int b = 0; b < nb; ++b)
        {
            double e = 0.0;
            for (int k = 0; k < half; ++k)
                if (filt[(size_t) b][(size_t) k] > 0.f) e += filt[(size_t) b][(size_t) k] * std::norm (buf[(size_t) k]);
            logE[(size_t) b] = std::log (e + 1e-10);
        }
        for (int c = 0; c < nMfcc; ++c)
        {
            double a = 0.0;
            for (int b = 0; b < nb; ++b) a += dct[(size_t) c * nb + b] * logE[(size_t) b];
            f.data[(size_t) i * f.dim + c] = (float) (a / nb);
        }
        if (nChroma)
        {
            const long cs = (long) ((size_t) i * hop) - (long) cwin / 2;
            std::fill (cbuf.begin(), cbuf.end(), std::complex<double> (0.0, 0.0));
            for (size_t k = 0; k < cwin; ++k)
            {
                const long p = cs + (long) k;
                cbuf[k] = ((p >= 0 && (size_t) p < n) ? x[p] : 0.0) * cwindow[k];
            }
            fft (cbuf);
            double chroma[12] = {};
            for (int k = ck0; k <= ck1; ++k)
            {
                const double m = std::sqrt (std::norm (cbuf[(size_t) k]));
                chroma[cLow[(size_t) k]] += m * (1.0 - cFrac[(size_t) k]);
                chroma[(cLow[(size_t) k] + 1) % 12] += m * cFrac[(size_t) k];
            }
            double nrm = 0;
            for (double v : chroma) nrm += v * v;
            nrm = std::sqrt (nrm) + 1e-9;
            for (int q = 0; q < 12; ++q) f.data[(size_t) i * f.dim + nMfcc + q] = (float) (s.chromaWeight * chroma[q] / nrm);
        }
        frameDb[(size_t) i] = 10.0 * std::log10 (energy / (double) win + 1e-12);
        maxDb = std::max (maxDb, frameDb[(size_t) i]);
    }
    for (int i = 0; i < f.frames; ++i) f.level[(size_t) i] = (float) (frameDb[(size_t) i] - maxDb);

    // c0 (niveau global) : gardé à faible poids ; les autres coefficients sont les « formes » de la voix.
    // En dessous du seuil de silence, le trait devient neutre : on ne cherche pas à apparier du bruit de fond.
    for (int i = 0; i < f.frames; ++i)
    {
        float* r = f.data.data() + (size_t) i * f.dim;
        if (f.level[(size_t) i] < (float) s.silenceDb)
            std::fill (r + 1, r + f.dim, 0.f);
        r[0] *= 0.3f;
    }
    return f;
}

namespace
{
inline float distance (const float* a, const float* b, int dim, float la, float lb, double silenceDb)
{
    const bool sa = la < (float) silenceDb, sb = lb < (float) silenceDb;
    float d = 0.f;
    for (int k = 0; k < dim; ++k)
    {
        const float t = a[k] - b[k];
        d += t * t;
    }
    d = std::sqrt (d);
    if (sa != sb) d += 2.0f; // silence contre son : toujours mal apparié
    return d;
}
} // namespace

std::vector<double> alignPath (const Features& g, const Features& d, const AlignSettings& s)
{
    const int N = g.frames, M = d.frames;
    std::vector<double> out ((size_t) N, 0.0);
    if (N == 0 || M == 0) return out;
    if (N == 1 || M == 1) { std::fill (out.begin(), out.end(), 0.0); return out; }

    const int W = std::max (2, (int) std::lround (s.maxShiftSeconds / g.hopSeconds));
    const int span = 2 * W + 1;
    // Centre de la bande : diagonale i -> i (ouvert) ou diagonale « de coin à coin » (fermé).
    auto centre = [&] (int i) -> int {
        return s.openEnds ? i : (int) std::lround ((double) i * (M - 1) / (double) (N - 1));
    };
    const float kInf = std::numeric_limits<float>::infinity();
    std::vector<float> cost ((size_t) N * span, kInf);
    std::vector<uint8_t> from ((size_t) N * span, 0);
    auto at = [&] (int i, int j) -> long {
        const int o = j - centre (i) + W;
        return (o < 0 || o >= span || j < 0 || j >= M) ? -1 : (long) i * span + o;
    };
    auto dist = [&] (int i, int j) {
        return distance (g.row (i), d.row (j), g.dim, g.level[(size_t) i], d.level[(size_t) j], -60.0);
    };
    // Pas : (di, dj) = (1,1) diagonal, (1,2) double plus rapide, (2,1) double plus lent. Poids = moyenne de di et dj.
    struct Step { int di, dj; float w, pen; };
    const float pen = (float) s.slopePenalty;
    const Step steps[5] = { { 1, 1, 1.0f, 0.f }, { 1, 2, 1.5f, pen }, { 2, 1, 1.5f, pen }, { 1, 3, 2.0f, 2.f * pen }, { 3, 1, 2.0f, 2.f * pen } };
    const int nSteps = s.maxSlope >= 3.0 ? 5 : 3;

    // Départ : fermé = (0,0) ; ouvert = toute la bande de la première ligne.
    for (int j = std::max (0, centre (0) - W); j <= std::min (M - 1, centre (0) + W); ++j)
        if (s.openEnds || j == 0) { const long a = at (0, j); if (a >= 0) cost[(size_t) a] = dist (0, j) * 1.f; }

    for (int i = 1; i < N; ++i)
    {
        if ((i & 127) == 0) progress::report ((double) i / (double) N);
        for (int o = 0; o < span; ++o)
        {
            const int j = centre (i) - W + o;
            if (j < 0 || j >= M) continue;
            const float dij = dist (i, j);
            float best = kInf;
            uint8_t arg = 0;
            for (int si = 0; si < nSteps; ++si)
            {
                const long p = (i - steps[si].di >= 0 && j - steps[si].dj >= 0) ? at (i - steps[si].di, j - steps[si].dj) : -1;
                if (p < 0 || cost[(size_t) p] == kInf) continue;
                const float c = cost[(size_t) p] + dij * steps[si].w + steps[si].pen;
                if (c < best) { best = c; arg = (uint8_t) si; }
            }
            if (best != kInf) { cost[(size_t) i * span + o] = best; from[(size_t) i * span + o] = arg; }
        }
    }

    // Arrivée : fermé = (N-1, M-1) (ou, s'il est hors de la bande, le meilleur point de la dernière ligne) ; ouvert = meilleur de la dernière ligne.
    int ej = -1;
    float eb = kInf;
    for (int o = 0; o < span; ++o)
    {
        const int j = centre (N - 1) - W + o;
        if (j < 0 || j >= M) continue;
        float c = cost[(size_t) (N - 1) * span + o];
        if (c == kInf) continue;
        if (s.openEnds) c /= (float) (N + std::abs (j - (N - 1)));
        else if (j != M - 1) c += 1e6f;
        if (c < eb) { eb = c; ej = j; }
    }
    if (ej < 0)
        for (int i = 0; i < N; ++i) out[(size_t) i] = centre (i);
    else
    {
        int i = N - 1, j = ej;
        std::vector<double> sum ((size_t) N, 0.0);
        std::vector<int> cnt ((size_t) N, 0);
        while (true)
        {
            sum[(size_t) i] += j; ++cnt[(size_t) i];
            if (i == 0) break;
            const Step& st = steps[from[(size_t) at (i, j)]];
            // Les lignes sautées par un pas de 2 sont interpolées linéairement.
            for (int k = 1; k < st.di; ++k) { sum[(size_t) (i - k)] += j - st.dj * (double) k / st.di; ++cnt[(size_t) (i - k)]; }
            i -= st.di; j -= st.dj;
            if (i < 0) break;
        }
        for (int k = 0; k < N; ++k) out[(size_t) k] = cnt[(size_t) k] ? sum[(size_t) k] / cnt[(size_t) k] : (k ? out[(size_t) k - 1] : 0.0);
    }

    // Serrage : on lisse l'écart (position - diagonale) par un noyau gaussien ; tightness 1 = pas de lissage, 0 = jusqu'à ~0,3 s.
    const double sigmaFrames = (1.0 - std::clamp (s.tightness, 0.0, 1.0)) * (0.3 / g.hopSeconds);
    if (sigmaFrames > 0.3)
    {
        const int R = (int) std::ceil (sigmaFrames * 3);
        std::vector<double> ker ((size_t) (2 * R + 1)), res ((size_t) N);
        double ks = 0;
        for (int k = -R; k <= R; ++k) { ker[(size_t) (k + R)] = std::exp (-0.5 * k * k / (sigmaFrames * sigmaFrames)); ks += ker[(size_t) (k + R)]; }
        for (int i = 0; i < N; ++i)
        {
            double a = 0, wsum = 0;
            for (int k = -R; k <= R; ++k)
            {
                const int p = std::clamp (i + k, 0, N - 1);
                a += ker[(size_t) (k + R)] * (out[(size_t) p] - p);
                wsum += ker[(size_t) (k + R)];
            }
            res[(size_t) i] = i + a / wsum;
        }
        out = res;
    }
    // Le chemin ne recule jamais.
    for (int i = 1; i < N; ++i) out[(size_t) i] = std::max (out[(size_t) i], out[(size_t) i - 1]);
    return out;
}

std::vector<double> applyStrength (const std::vector<double>& p, double hop, double strength)
{
    strength = std::clamp (strength, 0.0, 1.0);
    std::vector<double> out (p.size());
    for (size_t i = 0; i < p.size(); ++i)
    {
        const double t = (double) i * hop;
        out[i] = t + strength * (p[i] - t);
    }
    return out;
}

std::vector<double> applyStrength (const std::vector<double>& p, double hop, const std::vector<StrengthPoint>& curveIn)
{
    if (curveIn.empty()) return p;
    std::vector<StrengthPoint> curve = curveIn;
    std::stable_sort (curve.begin(), curve.end(), [] (const StrengthPoint& a, const StrengthPoint& b) { return a.time < b.time; });
    std::vector<double> out (p.size());
    size_t j = 0;
    for (size_t i = 0; i < p.size(); ++i)
    {
        const double t = (double) i * hop;
        while (j + 1 < curve.size() && curve[j + 1].time <= t) ++j;
        double s;
        if (t <= curve.front().time) s = curve.front().strength;
        else if (j + 1 >= curve.size()) s = curve.back().strength;
        else
        {
            const double f = (t - curve[j].time) / std::max (1e-9, curve[j + 1].time - curve[j].time);
            s = curve[j].strength + f * (curve[j + 1].strength - curve[j].strength);
        }
        out[i] = t + std::clamp (s, 0.0, 1.0) * (p[i] - t);
    }
    return out;
}

GuideAnalysis analyseGuide (const float* g, size_t n, double sr, const AnalysisSettings& an)
{
    GuideAnalysis out;
    out.sampleRate = sr;
    out.length = n;
    out.settings = an;
    out.features = analyse (g, n, sr, an);
    return out;
}

std::vector<double> alignDubToGuide (const GuideAnalysis& guide, const float* d, size_t nD, const AlignSettings& a)
{
    Features df;
    {
        progress::Phase ph (0.0, 0.25); // traits du double
        df = analyse (d, nD, guide.sampleRate, guide.settings);
    }
    std::vector<double> p;
    {
        progress::Phase ph (0.25, 1.0); // recherche du chemin
        p = alignPath (guide.features, df, a);
    }
    for (auto& v : p) v *= guide.features.hopSeconds;
    return p;
}

std::vector<double> alignSignals (const float* guide, size_t nGuide, const float* dub, size_t nDub, double sr,
                                  const AlignSettings& a, const AnalysisSettings& an)
{
    const Features g = analyse (guide, nGuide, sr, an), d = analyse (dub, nDub, sr, an);
    auto p = alignPath (g, d, a);
    for (auto& v : p) v *= g.hopSeconds;
    return p;
}

std::vector<Segment> findConsonants (const float* x, size_t n, double sr, const ConsonantSettings& s)
{
    std::vector<Segment> out;
    const double hopSec = 0.005;
    const size_t hop = std::max<size_t> (1, (size_t) std::llround (hopSec * sr)), win = std::max<size_t> (64, (size_t) std::llround (0.020 * sr));
    size_t nfft = 1;
    while (nfft < win) nfft <<= 1;
    const int frames = n == 0 ? 0 : (int) (n / hop) + 1;
    if (frames == 0) return out;
    const int half = (int) nfft / 2;
    auto bin = [&] (double hz) { return std::clamp ((int) std::lround (hz * (double) nfft / sr), 1, half); };
    const int kLo = bin (80), kHi = bin (3000), kFlatA = bin (2000), kFlatB = bin (std::min (10000.0, sr * 0.45)), kTop = bin (std::min (12000.0, sr * 0.45));
    std::vector<double> window (win);
    for (size_t i = 0; i < win; ++i) window[i] = 0.5 - 0.5 * std::cos (2.0 * kPi * (double) i / (double) (win - 1));
    std::vector<std::complex<double>> buf (nfft);
    std::vector<double> levelDb ((size_t) frames), hiRatio ((size_t) frames), flat ((size_t) frames);
    double maxDb = -200.0;
    for (int i = 0; i < frames; ++i)
    {
        const long start = (long) ((size_t) i * hop) - (long) win / 2;
        std::fill (buf.begin(), buf.end(), std::complex<double> (0.0, 0.0));
        double e = 0;
        for (size_t k = 0; k < win; ++k)
        {
            const long p = start + (long) k;
            const double v = (p >= 0 && (size_t) p < n) ? x[p] : 0.0;
            buf[k] = v * window[k];
            e += v * v * window[k];
        }
        fft (buf);
        double tot = 0, hi = 0, logSum = 0, linSum = 0;
        int cnt = 0;
        for (int k = kLo; k <= kTop; ++k)
        {
            const double p = std::norm (buf[(size_t) k]);
            tot += p;
            if (k >= kHi) hi += p;
            if (k >= kFlatA && k <= kFlatB) { logSum += std::log (p + 1e-12); linSum += p; ++cnt; }
        }
        levelDb[(size_t) i] = 10.0 * std::log10 (e / (double) win + 1e-12);
        maxDb = std::max (maxDb, levelDb[(size_t) i]);
        hiRatio[(size_t) i] = tot > 1e-12 ? hi / tot : 0.0;
        flat[(size_t) i] = (cnt > 0 && linSum > 1e-12) ? std::exp (logSum / cnt) / (linSum / cnt + 1e-12) : 0.0;
    }
    std::vector<char> on ((size_t) frames, 0);
    for (int i = 0; i < frames; ++i)
        on[(size_t) i] = levelDb[(size_t) i] - maxDb > s.minLevelDb && hiRatio[(size_t) i] > s.minHighRatio && flat[(size_t) i] > s.minFlatness;
    // Combler les petits trous, puis écarter les zones trop courtes.
    const int bridge = (int) std::lround (s.bridgeSeconds / hopSec), minFrames = (int) std::lround (s.minSeconds / hopSec);
    for (int i = 0; i < frames;)
    {
        if (on[(size_t) i]) { ++i; continue; }
        int j = i;
        while (j < frames && ! on[(size_t) j]) ++j;
        if (i > 0 && j < frames && j - i <= bridge) std::fill (on.begin() + i, on.begin() + j, 1);
        i = j;
    }
    for (int i = 0; i < frames;)
    {
        if (! on[(size_t) i]) { ++i; continue; }
        int j = i;
        while (j < frames && on[(size_t) j]) ++j;
        if (j - i >= minFrames) out.push_back ({ std::max (0.0, ((double) i - 0.5) * hopSec), ((double) j - 0.5) * hopSec });
        i = j;
    }
    return out;
}

double WarpMap::sourceAt (double u) const
{
    if (sourceAtOutput.empty()) return u;
    const double x = u / (double) stepSamples;
    if (x <= 0) return sourceAtOutput.front() + x * (sourceAtOutput.size() > 1 ? sourceAtOutput[1] - sourceAtOutput[0] : stepSamples);
    const size_t i = (size_t) x;
    if (i + 1 >= sourceAtOutput.size())
        return sourceAtOutput.back() + (x - (double) (sourceAtOutput.size() - 1)) * (sourceAtOutput.size() > 1 ? sourceAtOutput.back() - sourceAtOutput[sourceAtOutput.size() - 2] : stepSamples);
    const double t = x - (double) i;
    return sourceAtOutput[i] * (1.0 - t) + sourceAtOutput[i + 1] * t;
}

WarpMap makeLinearWarpMap (const std::vector<std::pair<double, double>>& keys, double sr)
{
    WarpMap m;
    m.stepSamples = 64; // fin : un point de rupture tombe à moins d'un pas, donc à moins de 1,5 ms d'un point échantillonné
    m.outputLength = keys.empty() ? 0 : (size_t) std::llround (keys.back().second * sr);
    if (keys.size() < 2) return m;
    for (const auto& k : keys) m.anchors.emplace_back (k.first * sr, k.second * sr);
    m.anchors.back().second = (double) m.outputLength; // la fin exacte, à l'échantillon près
    const size_t n = m.outputLength / m.stepSamples + 3; // un peu au-delà de la fin : le moteur lit un peu après
    m.sourceAtOutput.resize (n);
    size_t seg = 0;
    for (size_t i = 0; i < n; ++i)
    {
        const double u = (double) (i * m.stepSamples);
        while (seg + 2 < m.anchors.size() && u >= m.anchors[seg + 1].second) ++seg; // après la fin : le dernier tronçon se prolonge
        const auto& a = m.anchors[seg];
        const auto& b = m.anchors[seg + 1];
        m.sourceAtOutput[i] = a.first + (u - a.second) * (b.first - a.first) / (b.second - a.second);
    }
    return m;
}

WarpMap makeWarpMap (const std::vector<double>& p, double hop, double sr, size_t outLen, size_t step)
{
    WarpMap m;
    m.stepSamples = step;
    m.outputLength = outLen;
    const size_t pts = outLen / step + 2;
    m.sourceAtOutput.resize (pts);
    for (size_t k = 0; k < pts; ++k)
    {
        const double t = (double) (k * step) / sr / hop; // en traits
        double v;
        if (p.empty()) v = (double) (k * step);
        else if (t <= 0) v = (p.front() + t * hop) * sr;
        else if (t >= (double) p.size() - 1) v = (p.back() + (t - ((double) p.size() - 1)) * hop) * sr;
        else
        {
            const size_t i = (size_t) t;
            const double f = t - (double) i;
            v = (p[i] * (1 - f) + p[i + 1] * f) * sr;
        }
        m.sourceAtOutput[k] = v;
    }
    return m;
}

WarpMap makeWarpMap (const std::vector<double>& p, double hop, double sr, size_t outLen, const std::vector<Segment>& cons, double protect, size_t step)
{
    protect = std::clamp (protect, 0.0, 1.0);
    if (p.size() < 2 || cons.empty() || protect <= 0.0) return makeWarpMap (p, hop, sr, outLen, step);
    // Points de contrôle (temps du guide, temps du double), croissants. On part de la courbe d'alignement telle quelle.
    struct Pt { double g, d; };
    std::vector<Pt> pts;
    pts.reserve (p.size() + 2 * cons.size());
    for (size_t i = 0; i < p.size(); ++i) pts.push_back ({ (double) i * hop, p[i] });
    // Temps du guide où le double vaut d : premier / dernier instant atteignant cette valeur.
    auto guideAtFirst = [&] (double d) {
        for (size_t i = 0; i < pts.size(); ++i)
            if (pts[i].d >= d) return i == 0 ? pts[0].g : pts[i - 1].g + (pts[i].g - pts[i - 1].g) * (d - pts[i - 1].d) / std::max (1e-12, pts[i].d - pts[i - 1].d);
        return pts.back().g + (d - pts.back().d);
    };
    auto guideAtLast = [&] (double d) {
        for (size_t i = pts.size(); i-- > 0;)
            if (pts[i].d <= d) return i + 1 >= pts.size() ? pts[i].g + (d - pts[i].d) : pts[i].g + (pts[i + 1].g - pts[i].g) * (d - pts[i].d) / std::max (1e-12, pts[i + 1].d - pts[i].d);
        return pts.front().g + (d - pts.front().d);
    };
    std::vector<Segment> sorted = cons;
    std::sort (sorted.begin(), sorted.end(), [] (const Segment& a, const Segment& b) { return a.start < b.start; });
    double prevEndG = -1e9, prevEndD = -1e9;
    std::vector<Pt> placed; // points fixés par les consonnes
    std::vector<std::pair<double, double>> inside; // intervalles de temps du guide dont les points d'alignement d'origine disparaissent
    for (const auto& c : sorted)
    {
        if (c.end <= c.start) continue;
        const double ga = guideAtFirst (c.start), gb = guideAtLast (c.end);
        const double dur = c.end - c.start, originalOut = std::max (1e-4, gb - ga);
        double outLen2 = protect * dur + (1.0 - protect) * originalOut;
        double startG = gb - outLen2;
        // Ne pas empiéter sur la consonne précédente : on rogne alors la protection (pente différente de 1, mais courbe toujours croissante).
        const double minG = std::max (prevEndG + 1e-3, 0.0);
        if (startG < minG) { startG = minG; if (gb - startG < 1e-3) continue; }
        if (c.start <= prevEndD + 1e-4) continue;
        placed.push_back ({ startG, c.start });
        inside.push_back ({ std::min (startG, ga), gb });
        placed.push_back ({ gb, c.end });
        prevEndG = gb; prevEndD = c.end;
    }
    // Fusion : les points d'alignement qui tombent à l'intérieur d'un intervalle protégé disparaissent.
    std::vector<Pt> merged;
    merged.reserve (pts.size() + placed.size());
    for (const auto& q : pts)
    {
        bool in = false;
        for (const auto& iv : inside) if (q.g > iv.first - 1e-9 && q.g < iv.second + 1e-9) { in = true; break; }
        if (! in) merged.push_back (q);
    }
    for (const auto& q : placed) merged.push_back (q);
    std::sort (merged.begin(), merged.end(), [] (const Pt& a, const Pt& b) { return a.g < b.g || (a.g == b.g && a.d < b.d); });
    // Courbe croissante : un point dont le temps du double recule est ramené au précédent.
    std::vector<Pt> mono;
    for (const auto& q : merged)
    {
        if (! mono.empty() && q.g <= mono.back().g + 1e-9) { mono.back().d = std::max (mono.back().d, q.d); continue; }
        mono.push_back ({ q.g, std::max (q.d, mono.empty() ? q.d : mono.back().d) });
    }
    WarpMap m;
    m.stepSamples = step;
    m.outputLength = outLen;
    const size_t cnt = outLen / step + 2;
    m.sourceAtOutput.resize (cnt);
    size_t j = 0;
    for (size_t k = 0; k < cnt; ++k)
    {
        const double g = (double) (k * step) / sr;
        while (j + 1 < mono.size() && mono[j + 1].g < g) ++j;
        double d;
        if (mono.size() < 2) d = g;
        else if (g <= mono.front().g) d = mono.front().d + (g - mono.front().g);
        else if (g >= mono.back().g) d = mono.back().d + (g - mono.back().g);
        else
        {
            const double f = (g - mono[j].g) / std::max (1e-12, mono[j + 1].g - mono[j].g);
            d = mono[j].d + f * (mono[j + 1].d - mono[j].d);
        }
        m.sourceAtOutput[k] = d * sr;
    }
    return m;
}
} // namespace align
