#include "align/Settings.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>

namespace align
{
namespace
{
// ---- JSON minimal : objets, tableaux, nombres, chaînes, booléens, null. Suffit pour la fiche (aucune dépendance).
struct Json
{
    enum Type { Null, Bool, Number, String, Array, Object } type = Null;
    bool b = false;
    double n = 0;
    std::string s;
    std::vector<Json> a;
    std::vector<std::pair<std::string, Json>> o;
    const Json* find (const std::string& key) const
    {
        for (const auto& kv : o) if (kv.first == key) return &kv.second;
        return nullptr;
    }
};

class Parser
{
public:
    explicit Parser (const std::string& t) : t_ (t) {}
    bool parse (Json& out, std::string& err)
    {
        try { skip(); out = value(); skip(); if (i_ != t_.size()) fail ("texte en trop après la fin du JSON"); }
        catch (const std::string& e) { err = e + " (position " + std::to_string (i_) + ")"; return false; }
        return true;
    }
private:
    [[noreturn]] void fail (const std::string& m) { throw m; }
    void skip() { while (i_ < t_.size() && (t_[i_] == ' ' || t_[i_] == '\n' || t_[i_] == '\r' || t_[i_] == '\t')) ++i_; }
    char peek() { skip(); if (i_ >= t_.size()) fail ("JSON incomplet"); return t_[i_]; }
    bool eat (char c) { if (peek() == c) { ++i_; return true; } return false; }
    void expect (char c) { if (! eat (c)) fail (std::string ("« ") + c + " » attendu"); }
    std::string str()
    {
        expect ('"');
        std::string r;
        while (i_ < t_.size() && t_[i_] != '"')
        {
            if (t_[i_] == '\\')
            {
                if (++i_ >= t_.size()) fail ("chaîne incomplète");
                switch (t_[i_]) { case 'n': r += '\n'; break; case 't': r += '\t'; break; case 'r': r += '\r'; break; case 'b': r += '\b'; break; case 'f': r += '\f'; break;
                                  case '"': case '\\': case '/': r += t_[i_]; break;
                                  case 'u': // les caractères \uXXXX ne servent pas dans la fiche : on les garde tels quels
                                      r += "\\u"; break;
                                  default: fail ("échappement inconnu"); }
                ++i_;
            }
            else r += t_[i_++];
        }
        if (i_ >= t_.size()) fail ("chaîne incomplète");
        ++i_;
        return r;
    }
    Json value()
    {
        Json j;
        const char c = peek();
        if (c == '{')
        {
            ++i_; j.type = Json::Object;
            if (eat ('}')) return j;
            do { std::string k = str(); expect (':'); j.o.emplace_back (std::move (k), value()); } while (eat (','));
            expect ('}');
        }
        else if (c == '[')
        {
            ++i_; j.type = Json::Array;
            if (eat (']')) return j;
            do { j.a.push_back (value()); } while (eat (','));
            expect (']');
        }
        else if (c == '"') { j.type = Json::String; j.s = str(); }
        else if (t_.compare (i_, 4, "true") == 0) { i_ += 4; j.type = Json::Bool; j.b = true; }
        else if (t_.compare (i_, 5, "false") == 0) { i_ += 5; j.type = Json::Bool; j.b = false; }
        else if (t_.compare (i_, 4, "null") == 0) { i_ += 4; }
        else
        {
            char* end = nullptr;
            const double v = std::strtod (t_.c_str() + i_, &end);
            if (end == t_.c_str() + i_) fail ("valeur JSON inattendue");
            i_ = (std::size_t) (end - t_.c_str());
            j.type = Json::Number; j.n = v;
        }
        return j;
    }
    const std::string& t_;
    std::size_t i_ = 0;
};

std::string num (double v)
{
    char buf[40];
    std::snprintf (buf, sizeof buf, "%.9g", v);
    return buf;
}

const char* engineId (Engine e) { return e == Engine::Signalsmith ? "signalsmith" : e == Engine::RubberBandR2 ? "rubberband_r2" : "rubberband_r3"; }

bool readNumber (const Json& obj, const char* key, double lo, double hi, double& out, std::string& err)
{
    const Json* v = obj.find (key);
    if (! v || v->type == Json::Null) return true; // absent : valeur par défaut
    if (v->type != Json::Number || ! std::isfinite (v->n)) { err = std::string ("« ") + key + " » doit être un nombre"; return false; }
    if (v->n < lo || v->n > hi) { err = std::string ("« ") + key + " » hors limites (" + num (lo) + " à " + num (hi) + ")"; return false; }
    out = v->n;
    return true;
}
} // namespace

AlignSettings alignSettingsOf (const GeneralSettings& g)
{
    AlignSettings a;
    a.tightness = g.tightness;
    a.maxShiftSeconds = g.maxShiftSeconds;
    return a;
}

AnalysisSettings analysisSettingsOf (const GeneralSettings& g) { return g.harmony ? harmonyAnalysis() : AnalysisSettings {}; }

RenderSettings renderSettingsOf (const SettingsSheet& s)
{
    RenderSettings r;
    r.engine = s.general.engine;
    r.strength = s.general.strength;
    r.protect = s.general.protect;
    r.strengthCurve = s.strengthCurve;
    return r;
}

std::string toJson (const SettingsSheet& s)
{
    std::string j = "{\n  \"format\": \"flibuste-align-settings\",\n  \"version\": 1,\n  \"general\": {\n";
    j += std::string ("    \"analysis_mode\": \"") + (s.general.harmony ? "harmony" : "same_pitch") + "\",\n";
    j += "    \"tightness\": " + num (s.general.tightness) + ",\n";
    j += "    \"max_shift_seconds\": " + num (s.general.maxShiftSeconds) + ",\n";
    j += std::string ("    \"engine\": \"") + engineId (s.general.engine) + "\",\n";
    j += "    \"strength\": " + num (s.general.strength) + ",\n";
    j += "    \"protect\": " + num (s.general.protect) + "\n  },\n  \"local\": {\n    \"strength_curve\": [";
    for (std::size_t i = 0; i < s.strengthCurve.size(); ++i)
        j += (i ? ", [" : "[") + num (s.strengthCurve[i].time) + ", " + num (s.strengthCurve[i].strength) + "]";
    j += "]\n  }\n}\n";
    return j;
}

bool parseSettings (const std::string& text, SettingsSheet& out, std::string& error)
{
    Json root;
    if (! Parser (text).parse (root, error)) { error = "JSON invalide : " + error; return false; }
    if (root.type != Json::Object) { error = "la fiche doit être un objet JSON"; return false; }
    if (const Json* f = root.find ("format"))
        if (f->type != Json::String || f->s != "flibuste-align-settings") { error = "format inconnu (attendu « flibuste-align-settings »)"; return false; }
    if (const Json* v = root.find ("version"))
        if (v->type == Json::Number && v->n > 1.0) { error = "fiche plus récente que cette bibliothèque (version " + num (v->n) + ")"; return false; }
    SettingsSheet s;
    if (const Json* g = root.find ("general"))
    {
        if (g->type != Json::Object) { error = "« general » doit être un objet"; return false; }
        if (const Json* m = g->find ("analysis_mode"))
        {
            if (m->type != Json::String || (m->s != "same_pitch" && m->s != "harmony")) { error = "« analysis_mode » : same_pitch ou harmony"; return false; }
            s.general.harmony = m->s == "harmony";
        }
        if (const Json* e = g->find ("engine"))
        {
            if (e->type != Json::String || (e->s != "signalsmith" && e->s != "rubberband_r2" && e->s != "rubberband_r3")) { error = "« engine » : signalsmith, rubberband_r2 ou rubberband_r3"; return false; }
            s.general.engine = e->s == "signalsmith" ? Engine::Signalsmith : e->s == "rubberband_r2" ? Engine::RubberBandR2 : Engine::RubberBandR3;
        }
        if (! readNumber (*g, "tightness", 0.0, 1.0, s.general.tightness, error)) return false;
        if (! readNumber (*g, "max_shift_seconds", 0.1, 10.0, s.general.maxShiftSeconds, error)) return false;
        if (! readNumber (*g, "strength", 0.0, 1.0, s.general.strength, error)) return false;
        if (! readNumber (*g, "protect", 0.0, 1.0, s.general.protect, error)) return false;
    }
    if (const Json* l = root.find ("local"))
    {
        if (l->type != Json::Object) { error = "« local » doit être un objet"; return false; }
        if (const Json* c = l->find ("strength_curve"))
        {
            if (c->type != Json::Array) { error = "« strength_curve » doit être un tableau de [temps, force]"; return false; }
            for (const auto& p : c->a)
            {
                if (p.type != Json::Array || p.a.size() != 2 || p.a[0].type != Json::Number || p.a[1].type != Json::Number || ! std::isfinite (p.a[0].n) || ! std::isfinite (p.a[1].n))
                { error = "« strength_curve » : chaque point est [temps, force]"; return false; }
                s.strengthCurve.push_back ({ p.a[0].n, std::clamp (p.a[1].n, 0.0, 1.0) });
            }
            std::stable_sort (s.strengthCurve.begin(), s.strengthCurve.end(), [] (const StrengthPoint& a, const StrengthPoint& b) { return a.time < b.time; });
        }
    }
    out = std::move (s);
    return true;
}

bool mergeGeneralSettings (const std::string& dstJson, const std::string& srcJson, std::string& outJson, std::string& error)
{
    SettingsSheet dst, src;
    std::string e;
    if (! parseSettings (dstJson, dst, e)) { error = "fiche de destination : " + e; return false; }
    if (! parseSettings (srcJson, src, e)) { error = "fiche source : " + e; return false; }
    dst.general = src.general; // « local » de la destination reste tel quel
    outJson = toJson (dst);
    return true;
}

// ---- analyse en octets
namespace
{
constexpr unsigned kMagic = 0x314E4C41; // « ALN1 »
template <class T> void put (std::vector<unsigned char>& v, T x)
{
    unsigned char b[sizeof (T)];
    std::memcpy (b, &x, sizeof (T));
    v.insert (v.end(), b, b + sizeof (T));
}
template <class T> bool get (const unsigned char*& p, const unsigned char* end, T& x)
{
    if ((std::size_t) (end - p) < sizeof (T)) return false;
    std::memcpy (&x, p, sizeof (T));
    p += sizeof (T);
    return true;
}
} // namespace

std::vector<unsigned char> serializeAnalysis (const Analysis& a)
{
    std::vector<unsigned char> v;
    put<unsigned> (v, kMagic);
    put<unsigned> (v, 1u); // version
    put<double> (v, a.sampleRate);
    put<double> (v, a.hopSeconds);
    put<unsigned long long> (v, (unsigned long long) a.length);
    put<unsigned> (v, (unsigned) a.path.size());
    for (double x : a.path) put<double> (v, x);
    put<unsigned> (v, (unsigned) a.consonants.size());
    for (const auto& s : a.consonants) { put<double> (v, s.start); put<double> (v, s.end); }
    return v;
}

bool deserializeAnalysis (const unsigned char* data, std::size_t size, Analysis& out, std::string& error)
{
    const unsigned char* p = data;
    const unsigned char* end = data + size;
    unsigned magic = 0, version = 0, count = 0, nCons = 0;
    unsigned long long len = 0;
    Analysis a;
    if (! data || ! get (p, end, magic) || magic != kMagic) { error = "ce ne sont pas les octets d'une analyse (marque ALN1 absente)"; return false; }
    if (! get (p, end, version) || version != 1) { error = "version d'analyse inconnue"; return false; }
    if (! get (p, end, a.sampleRate) || ! get (p, end, a.hopSeconds) || ! get (p, end, len) || ! get (p, end, count)) { error = "analyse tronquée"; return false; }
    if (! (a.sampleRate > 0) || ! (a.hopSeconds > 0) || count > 100000000u) { error = "analyse invalide"; return false; }
    a.length = (std::size_t) len;
    a.path.resize (count);
    for (auto& x : a.path) { if (! get (p, end, x)) { error = "analyse tronquée"; return false; } }
    if (! get (p, end, nCons) || nCons > 100000000u) { error = "analyse tronquée"; return false; }
    a.consonants.resize (nCons);
    for (auto& s : a.consonants) if (! get (p, end, s.start) || ! get (p, end, s.end)) { error = "analyse tronquée"; return false; }
    out = std::move (a);
    return true;
}
} // namespace align
