#include "align/align_c.h"

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <memory>
#include <string>

#include "align/Pipeline.h"
#include "align/Progress.h"
#include "align/Settings.h"

struct align_guide
{
    align::GuideAnalysis guide;
    bool harmony = false;
};

struct align_analysis
{
    align::Analysis analysis;
};

namespace
{
thread_local std::string tLastError;

int fail (int code, const std::string& msg)
{
    tLastError = msg;
    return code;
}

int ok()
{
    tLastError.clear();
    return ALIGN_OK;
}

template <class F> int guarded (F&& f)
{
    try { return f(); }
    catch (const align::Cancelled&) { return fail (ALIGN_ERR_CANCELLED, "opération annulée"); }
    catch (const std::bad_alloc&) { return fail (ALIGN_ERR_FAILED, "mémoire insuffisante"); }
    catch (const std::exception& e) { return fail (ALIGN_ERR_FAILED, std::string ("échec interne : ") + e.what()); }
    catch (...) { return fail (ALIGN_ERR_FAILED, "échec interne inconnu"); }
}

bool loadSettings (const char* json, align::SettingsSheet& s, int& code)
{
    if (json == nullptr || *json == '\0') { s = {}; return true; }
    std::string err;
    if (! align::parseSettings (json, s, err)) { code = fail (ALIGN_ERR_FORMAT, "réglages : " + err); return false; }
    return true;
}

bool checkAudio (const float* data, size_t frames, int channels, double sr, const char* what, int& code)
{
    if (data == nullptr) { code = fail (ALIGN_ERR_ARGUMENT, std::string (what) + " : tampon absent"); return false; }
    if (channels < 1 || channels > 16) { code = fail (ALIGN_ERR_ARGUMENT, std::string (what) + " : nombre de voies hors limites (1 à 16)"); return false; }
    if (! (sr >= 8000.0 && sr <= 384000.0)) { code = fail (ALIGN_ERR_ARGUMENT, std::string (what) + " : fréquence d'échantillonnage hors limites"); return false; }
    if (frames < (size_t) (0.5 * sr)) { code = fail (ALIGN_ERR_ARGUMENT, std::string (what) + " : trop court (moins d'une demi-seconde)"); return false; }
    return true;
}

std::vector<float> monoOf (const float* x, size_t frames, int channels)
{
    std::vector<float> m (frames);
    if (channels == 1) { std::copy (x, x + frames, m.begin()); return m; }
    const float inv = 1.0f / (float) channels;
    for (size_t i = 0; i < frames; ++i)
    {
        float s = 0.f;
        for (int c = 0; c < channels; ++c) s += x[i * (size_t) channels + (size_t) c];
        m[i] = s * inv;
    }
    return m;
}

void copyOut (const std::vector<double>& v, double* out, size_t capacity, size_t* count, int& code)
{
    if (count) *count = v.size();
    if (out == nullptr) { code = ok(); return; }
    if (capacity < v.size()) { code = fail (ALIGN_ERR_ARGUMENT, "tampon trop petit (taille nécessaire dans *count)"); return; }
    std::copy (v.begin(), v.end(), out);
    code = ok();
}
} // namespace

extern "C"
{
int align_c_api_version(void) { return ALIGN_C_API_VERSION; }
const char* align_version(void) { return "0.1.0"; }
const char* align_last_error(void) { return tLastError.c_str(); }
void align_free(void* p) { std::free (p); }

const char* align_settings_default_json(void)
{
    static const std::string s = align::toJson (align::SettingsSheet {});
    return s.c_str();
}

int align_settings_validate(const char* json)
{
    return guarded ([&] {
        align::SettingsSheet s;
        int code = ALIGN_OK;
        if (json == nullptr) return fail (ALIGN_ERR_ARGUMENT, "réglages absents");
        if (! loadSettings (json, s, code)) return code;
        return ok();
    });
}

int align_settings_merge_general(const char* dst, const char* src, char** out)
{
    return guarded ([&] {
        if (! dst || ! src || ! out) return fail (ALIGN_ERR_ARGUMENT, "argument absent");
        std::string merged, err;
        if (! align::mergeGeneralSettings (dst, src, merged, err)) return fail (ALIGN_ERR_FORMAT, err);
        char* buf = (char*) std::malloc (merged.size() + 1);
        if (! buf) return fail (ALIGN_ERR_FAILED, "mémoire insuffisante");
        std::memcpy (buf, merged.c_str(), merged.size() + 1);
        *out = buf;
        return ok();
    });
}

int align_guide_create(const float* x, size_t frames, int channels, double sr, const char* json, align_progress_fn progress, void* user, align_guide** out)
{
    return guarded ([&] {
        if (! out) return fail (ALIGN_ERR_ARGUMENT, "pointeur de sortie absent");
        *out = nullptr;
        int code = ALIGN_OK;
        if (! checkAudio (x, frames, channels, sr, "guide", code)) return code;
        align::SettingsSheet sheet;
        if (! loadSettings (json, sheet, code)) return code;
        const align::ProgressFn fn = [&] (double f) { return progress != nullptr && progress (f, user) != 0; };
        align::progress::Scope scope (&fn);
        const auto mono = monoOf (x, frames, channels);
        auto g = std::make_unique<align_guide>();
        g->harmony = sheet.general.harmony;
        g->guide = align::analyseGuide (mono.data(), frames, sr, align::analysisSettingsOf (sheet.general));
        align::progress::report (1.0);
        *out = g.release();
        return ok();
    });
}

void align_guide_free(align_guide* g) { delete g; }

int align_analyse(const align_guide* guide, const float* x, size_t frames, int channels, double sr, const char* json, align_progress_fn progress, void* user, align_analysis** out)
{
    return guarded ([&] {
        if (! out) return fail (ALIGN_ERR_ARGUMENT, "pointeur de sortie absent");
        *out = nullptr;
        if (! guide) return fail (ALIGN_ERR_ARGUMENT, "guide absent");
        int code = ALIGN_OK;
        if (! checkAudio (x, frames, channels, sr, "double", code)) return code;
        if (std::fabs (sr - guide->guide.sampleRate) > 0.5) return fail (ALIGN_ERR_ARGUMENT, "le double et le guide n'ont pas la même fréquence d'échantillonnage");
        align::SettingsSheet sheet;
        if (! loadSettings (json, sheet, code)) return code;
        if (sheet.general.harmony != guide->harmony)
            return fail (ALIGN_ERR_ARGUMENT, "le mode d'analyse (same_pitch / harmony) du double diffère de celui du guide : créer un guide pour ce mode");
        const align::ProgressFn fn = [&] (double f) { return progress != nullptr && progress (f, user) != 0; };
        align::progress::Scope scope (&fn);
        const auto mono = monoOf (x, frames, channels);
        auto a = std::make_unique<align_analysis>();
        a->analysis = align::analyseAgainstGuide (guide->guide, mono.data(), frames, align::alignSettingsOf (sheet.general));
        align::progress::report (1.0);
        *out = a.release();
        return ok();
    });
}

void align_analysis_free(align_analysis* a) { delete a; }

int align_render(const align_analysis* an, const float* x, size_t frames, int channels, double sr, const char* json, align_progress_fn progress, void* user,
                 float** outBuf, size_t* outFrames, align_render_info* info)
{
    return guarded ([&] {
        if (! outBuf || ! outFrames) return fail (ALIGN_ERR_ARGUMENT, "pointeur de sortie absent");
        *outBuf = nullptr;
        *outFrames = 0;
        if (! an) return fail (ALIGN_ERR_ARGUMENT, "analyse absente");
        int code = ALIGN_OK;
        if (! checkAudio (x, frames, channels, sr, "double", code)) return code;
        if (std::fabs (sr - an->analysis.sampleRate) > 0.5) return fail (ALIGN_ERR_ARGUMENT, "le double n'a pas la fréquence d'échantillonnage de l'analyse");
        align::SettingsSheet sheet;
        if (! loadSettings (json, sheet, code)) return code;
        const align::ProgressFn fn = [&] (double f) { return progress != nullptr && progress (f, user) != 0; };
        align::progress::Scope scope (&fn);
        std::vector<std::vector<float>> planar ((size_t) channels, std::vector<float> (frames));
        for (size_t i = 0; i < frames; ++i)
            for (int c = 0; c < channels; ++c) planar[(size_t) c][i] = x[i * (size_t) channels + (size_t) c];
        auto r = align::renderAnalysis (planar, an->analysis, align::renderSettingsOf (sheet));
        const size_t n = an->analysis.length;
        float* buf = (float*) std::malloc (n * (size_t) channels * sizeof (float));
        if (! buf) return fail (ALIGN_ERR_FAILED, "mémoire insuffisante");
        for (size_t i = 0; i < n; ++i)
            for (int c = 0; c < channels; ++c) buf[i * (size_t) channels + (size_t) c] = i < r.channels[(size_t) c].size() ? r.channels[(size_t) c][i] : 0.f;
        align::progress::report (1.0);
        *outBuf = buf;
        *outFrames = n;
        if (info) { info->engine_used = (int) r.engineUsed == 0 ? 0 : (r.engineUsed == align::Engine::RubberBandR2 ? 1 : 2); info->fell_back = r.fellBack ? 1 : 0; }
        return ok();
    });
}

int align_warp_render(const float* x, size_t frames, int channels, double sr, const double* keys, size_t keyCount, const char* json, align_progress_fn progress,
                      void* user, float** outBuf, size_t* outFrames, align_render_info* info)
{
    return guarded ([&] {
        if (! outBuf || ! outFrames) return fail (ALIGN_ERR_ARGUMENT, "pointeur de sortie absent");
        *outBuf = nullptr;
        *outFrames = 0;
        if (! x) return fail (ALIGN_ERR_ARGUMENT, "audio : tampon absent");
        if (channels < 1 || channels > 16) return fail (ALIGN_ERR_ARGUMENT, "audio : nombre de voies hors limites (1 à 16)");
        if (! (sr >= 8000.0 && sr <= 384000.0)) return fail (ALIGN_ERR_ARGUMENT, "audio : fréquence d'échantillonnage hors limites");
        if (frames < (size_t) (0.05 * sr)) return fail (ALIGN_ERR_ARGUMENT, "audio : trop court (moins de 50 ms)");
        if (! keys || keyCount < 2) return fail (ALIGN_ERR_ARGUMENT, "marqueurs : au moins deux couples (début et fin) sont nécessaires");
        std::vector<std::pair<double, double>> k (keyCount);
        for (size_t i = 0; i < keyCount; ++i)
        {
            k[i] = { keys[2 * i], keys[2 * i + 1] };
            if (! std::isfinite (k[i].first) || ! std::isfinite (k[i].second)) return fail (ALIGN_ERR_ARGUMENT, "marqueurs : valeur non finie");
        }
        if (k[0].first != 0.0 || k[0].second != 0.0) return fail (ALIGN_ERR_ARGUMENT, "marqueurs : le premier couple doit être (0, 0)");
        const double srcDur = (double) frames / sr;
        if (std::fabs (k.back().first - srcDur) > 1.5 / sr) return fail (ALIGN_ERR_ARGUMENT, "marqueurs : le dernier temps source doit être la durée de la source");
        k.back().first = srcDur;
        for (size_t i = 1; i < keyCount; ++i)
        {
            const double ds = k[i].first - k[i - 1].first, dout = k[i].second - k[i - 1].second;
            const std::string where = "marqueurs : tronçon " + std::to_string (i) + " (source " + std::to_string (k[i - 1].first) + " s à " + std::to_string (k[i].first) + " s)";
            if (! (ds > 0.0 && dout > 0.0)) return fail (ALIGN_ERR_ARGUMENT, where + " : les temps doivent être strictement croissants");
            if (ds < 0.010) return fail (ALIGN_ERR_ARGUMENT, where + " : moins de 10 ms de source");
            const double ratio = dout / ds;
            if (ratio < 0.25 || ratio > 4.0)
                return fail (ALIGN_ERR_ARGUMENT, where + " : rapport sortie / source " + std::to_string (ratio) + " hors de [0,25 ; 4]");
        }
        int code = ALIGN_OK;
        align::SettingsSheet sheet;
        if (! loadSettings (json, sheet, code)) return code;
        const auto engine = align::renderSettingsOf (sheet).engine;
        const align::ProgressFn fn = [&] (double f) { return progress != nullptr && progress (f, user) != 0; };
        align::progress::Scope scope (&fn);
        std::vector<std::vector<float>> planar ((size_t) channels, std::vector<float> (frames));
        for (size_t i = 0; i < frames; ++i)
            for (int c = 0; c < channels; ++c) planar[(size_t) c][i] = x[i * (size_t) channels + (size_t) c];
        const auto map = align::makeLinearWarpMap (k, sr);
        auto r = align::render (planar, map, sr, engine);
        if (r.channels.empty() || r.channels[0].size() != map.outputLength) return fail (ALIGN_ERR_FAILED, "le moteur d'étirement a échoué");
        // Un moteur qui n'a rien rendu est remplacé par du silence par render() : on ne le laisse pas passer pour un résultat.
        bool allZero = true;
        for (const auto& ch : r.channels) for (float v : ch) if (v != 0.f) { allZero = false; break; }
        bool srcZero = true;
        for (const auto& ch : planar) for (float v : ch) if (v != 0.f) { srcZero = false; break; }
        if (allZero && ! srcZero) return fail (ALIGN_ERR_FAILED, "le moteur d'étirement n'a rien rendu");
        const size_t n = map.outputLength;
        float* buf = (float*) std::malloc (n * (size_t) channels * sizeof (float));
        if (! buf) return fail (ALIGN_ERR_FAILED, "mémoire insuffisante");
        for (size_t i = 0; i < n; ++i)
            for (int c = 0; c < channels; ++c) buf[i * (size_t) channels + (size_t) c] = r.channels[(size_t) c][i];
        align::progress::report (1.0);
        *outBuf = buf;
        *outFrames = n;
        if (info) { info->engine_used = r.engineUsed == align::Engine::Signalsmith ? 0 : (r.engineUsed == align::Engine::RubberBandR2 ? 1 : 2); info->fell_back = r.fellBack ? 1 : 0; }
        return ok();
    });
}

int align_analysis_serialize(const align_analysis* a, unsigned char** outBytes, size_t* outSize)
{
    return guarded ([&] {
        if (! a || ! outBytes || ! outSize) return fail (ALIGN_ERR_ARGUMENT, "argument absent");
        const auto v = align::serializeAnalysis (a->analysis);
        unsigned char* buf = (unsigned char*) std::malloc (v.size());
        if (! buf) return fail (ALIGN_ERR_FAILED, "mémoire insuffisante");
        std::memcpy (buf, v.data(), v.size());
        *outBytes = buf;
        *outSize = v.size();
        return ok();
    });
}

int align_analysis_deserialize(const unsigned char* bytes, size_t size, align_analysis** out)
{
    return guarded ([&] {
        if (! out) return fail (ALIGN_ERR_ARGUMENT, "pointeur de sortie absent");
        *out = nullptr;
        auto a = std::make_unique<align_analysis>();
        std::string err;
        if (! align::deserializeAnalysis (bytes, size, a->analysis, err)) return fail (ALIGN_ERR_FORMAT, err);
        *out = a.release();
        return ok();
    });
}

double align_analysis_hop_seconds(const align_analysis* a) { return a ? a->analysis.hopSeconds : 0.0; }
double align_analysis_sample_rate(const align_analysis* a) { return a ? a->analysis.sampleRate : 0.0; }
size_t align_analysis_length_frames(const align_analysis* a) { return a ? a->analysis.length : 0; }

int align_analysis_get_path(const align_analysis* a, double* out, size_t capacity, size_t* count)
{
    return guarded ([&] {
        if (! a) return fail (ALIGN_ERR_ARGUMENT, "analyse absente");
        int code = ALIGN_OK;
        copyOut (a->analysis.path, out, capacity, count, code);
        return code;
    });
}

int align_analysis_get_consonants(const align_analysis* a, double* out, size_t capacityPairs, size_t* count)
{
    return guarded ([&] {
        if (! a) return fail (ALIGN_ERR_ARGUMENT, "analyse absente");
        std::vector<double> flat;
        for (const auto& s : a->analysis.consonants) { flat.push_back (s.start); flat.push_back (s.end); }
        size_t c2 = 0;
        int code = ALIGN_OK;
        copyOut (flat, out, capacityPairs * 2, &c2, code);
        if (count) *count = c2 / 2;
        return code;
    });
}

int align_analysis_set_consonants(align_analysis* a, const double* pairs, size_t count)
{
    return guarded ([&] {
        if (! a) return fail (ALIGN_ERR_ARGUMENT, "analyse absente");
        if (count > 0 && ! pairs) return fail (ALIGN_ERR_ARGUMENT, "tableau absent");
        std::vector<align::Segment> v;
        for (size_t i = 0; i < count; ++i)
        {
            const double s = pairs[2 * i], e = pairs[2 * i + 1];
            if (! std::isfinite (s) || ! std::isfinite (e) || e <= s || s < 0.0) return fail (ALIGN_ERR_ARGUMENT, "consonne invalide (début < fin, positif)");
            v.push_back ({ s, e });
        }
        std::sort (v.begin(), v.end(), [] (const align::Segment& x, const align::Segment& y) { return x.start < y.start; });
        a->analysis.consonants = std::move (v);
        return ok();
    });
}

int align_applied_shift(const align_analysis* a, const char* json, double* out, size_t capacity, size_t* count)
{
    return guarded ([&] {
        if (! a) return fail (ALIGN_ERR_ARGUMENT, "analyse absente");
        int code = ALIGN_OK;
        align::SettingsSheet sheet;
        if (! loadSettings (json, sheet, code)) return code;
        copyOut (align::appliedShift (a->analysis, align::renderSettingsOf (sheet)), out, capacity, count, code);
        return code;
    });
}

int align_suggest_strength(const align_analysis* a, double thresholdMs, double* out, size_t capacityPairs, size_t* count)
{
    return guarded ([&] {
        if (! a) return fail (ALIGN_ERR_ARGUMENT, "analyse absente");
        if (! (thresholdMs > 0.0 && thresholdMs <= 2000.0)) return fail (ALIGN_ERR_ARGUMENT, "seuil hors limites (0 à 2000 ms)");
        align::SuggestSettings ss;
        ss.thresholdMs = thresholdMs;
        std::vector<double> flat;
        for (const auto& p : align::suggestStrengthCurve (a->analysis, ss)) { flat.push_back (p.time); flat.push_back (p.strength); }
        size_t c2 = 0;
        int code = ALIGN_OK;
        copyOut (flat, out, capacityPairs * 2, &c2, code);
        if (count) *count = c2 / 2;
        return code;
    });
}
} // extern "C"
