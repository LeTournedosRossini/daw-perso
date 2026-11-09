// Implémentation de asio_shim.h. Les en-têtes ASIO viennent du SDK Steinberg (non redistribué ici,
// voir build.rs : variable ASIO_SDK_DIR).
#include <windows.h>
#include <objbase.h>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <vector>

#include "asiosys.h"  // doit précéder asio.h : définit IEEE754_64FLOAT (ASIOSampleRate = double)
#include "asio.h"
#include "iasiodrv.h"
#include "asio_shim.h"

struct DawAsio {
    IASIO* drv = nullptr;
    DawAsioInfo info{};
    long buffer_size = 0;
    double sample_rate = 0;
    bool started = false;
    bool buffers_created = false;
    bool output_ready = false;

    std::vector<ASIOBufferInfo> bufs;          // entrées puis sorties
    std::vector<ASIOSampleType> types;
    long n_in = 0, n_out = 0;
    std::vector<std::vector<float>> scratch;   // un tampon float par canal choisi
    std::vector<const float*> in_ptrs;
    std::vector<float*> out_ptrs;

    DawAsioProcessFn fn = nullptr;
    void* user = nullptr;
    std::atomic<uint32_t> overloads{0};
};

// Les rappels ASIO n'ont pas de pointeur utilisateur : un seul flux actif à la fois.
static DawAsio* g_active = nullptr;

static void set_err(char* err, int len, const char* msg) {
    if (err && len > 0) std::snprintf(err, (size_t)len, "%s", msg);
}

// ---------- conversions ----------
static bool supported(ASIOSampleType t) {
    return t == ASIOSTInt16LSB || t == ASIOSTInt24LSB || t == ASIOSTInt32LSB ||
           t == ASIOSTFloat32LSB || t == ASIOSTFloat64LSB;
}

static void to_float(ASIOSampleType t, const void* src, float* dst, long n) {
    switch (t) {
    case ASIOSTInt16LSB: {
        const int16_t* s = (const int16_t*)src;
        for (long i = 0; i < n; i++) dst[i] = (float)s[i] * (1.0f / 32768.0f);
    } break;
    case ASIOSTInt24LSB: {
        const uint8_t* s = (const uint8_t*)src;
        for (long i = 0; i < n; i++) {
            int32_t v = (int32_t)(((uint32_t)s[3 * i] << 8) | ((uint32_t)s[3 * i + 1] << 16) | ((uint32_t)s[3 * i + 2] << 24)) >> 8;
            dst[i] = (float)v * (1.0f / 8388608.0f);
        }
    } break;
    case ASIOSTInt32LSB: {
        const int32_t* s = (const int32_t*)src;
        for (long i = 0; i < n; i++) dst[i] = (float)((double)s[i] * (1.0 / 2147483648.0));
    } break;
    case ASIOSTFloat32LSB: std::memcpy(dst, src, (size_t)n * sizeof(float)); break;
    case ASIOSTFloat64LSB: {
        const double* s = (const double*)src;
        for (long i = 0; i < n; i++) dst[i] = (float)s[i];
    } break;
    default: std::memset(dst, 0, (size_t)n * sizeof(float));
    }
}

static inline float clamp1(float v) { return v > 1.0f ? 1.0f : (v < -1.0f ? -1.0f : v); }

static void from_float(ASIOSampleType t, const float* src, void* dst, long n) {
    switch (t) {
    case ASIOSTInt16LSB: {
        int16_t* d = (int16_t*)dst;
        for (long i = 0; i < n; i++) d[i] = (int16_t)std::lrintf(clamp1(src[i]) * 32767.0f);
    } break;
    case ASIOSTInt24LSB: {
        uint8_t* d = (uint8_t*)dst;
        for (long i = 0; i < n; i++) {
            int32_t v = (int32_t)std::lrintf(clamp1(src[i]) * 8388607.0f);
            d[3 * i] = (uint8_t)v; d[3 * i + 1] = (uint8_t)(v >> 8); d[3 * i + 2] = (uint8_t)(v >> 16);
        }
    } break;
    case ASIOSTInt32LSB: {
        int32_t* d = (int32_t*)dst;
        for (long i = 0; i < n; i++) d[i] = (int32_t)std::lrint((double)clamp1(src[i]) * 2147483647.0);
    } break;
    case ASIOSTFloat32LSB: std::memcpy(dst, src, (size_t)n * sizeof(float)); break;
    case ASIOSTFloat64LSB: {
        double* d = (double*)dst;
        for (long i = 0; i < n; i++) d[i] = (double)src[i];
    } break;
    default: std::memset(dst, 0, (size_t)n * 4);
    }
}

// ---------- rappels ASIO ----------
static void cb_buffer_switch(long index, ASIOBool) {
    DawAsio* a = g_active;
    if (!a || !a->started) return;
    const long n = a->buffer_size;
    for (long c = 0; c < a->n_in; c++) to_float(a->types[c], a->bufs[c].buffers[index], a->scratch[c].data(), n);
    for (long c = 0; c < a->n_out; c++) std::memset(a->scratch[a->n_in + c].data(), 0, (size_t)n * sizeof(float));
    a->fn(a->user, (uint32_t)n, a->in_ptrs.data(), a->out_ptrs.data());
    for (long c = 0; c < a->n_out; c++)
        from_float(a->types[a->n_in + c], a->scratch[a->n_in + c].data(), a->bufs[a->n_in + c].buffers[index], n);
    if (a->output_ready) a->drv->outputReady();
}

static ASIOTime* cb_buffer_switch_time_info(ASIOTime* t, long index, ASIOBool direct) {
    cb_buffer_switch(index, direct);
    return t;
}

static void cb_sample_rate_changed(ASIOSampleRate) {}

static long cb_message(long selector, long value, void*, double*) {
    switch (selector) {
    case kAsioSelectorSupported:
        return (value == kAsioEngineVersion || value == kAsioResetRequest || value == kAsioResyncRequest ||
                value == kAsioLatenciesChanged || value == kAsioOverload) ? 1 : 0;
    case kAsioEngineVersion: return 2;
    case kAsioResyncRequest:
    case kAsioOverload:
        if (g_active) g_active->overloads.fetch_add(1, std::memory_order_relaxed);
        return 1;
    case kAsioResetRequest:
    case kAsioLatenciesChanged: return 1;
    default: return 0;
    }
}

static ASIOCallbacks g_callbacks = {cb_buffer_switch, cb_sample_rate_changed, cb_message, cb_buffer_switch_time_info};

// ---------- API C ----------
extern "C" int daw_asio_list(DawAsioDriver* out, int max) {
    HKEY root;
    if (RegOpenKeyExA(HKEY_LOCAL_MACHINE, "SOFTWARE\\ASIO", 0, KEY_READ, &root) != ERROR_SUCCESS) return 0;
    int count = 0;
    for (DWORD i = 0; count < max; i++) {
        char name[128];
        DWORD nlen = sizeof(name);
        if (RegEnumKeyExA(root, i, name, &nlen, nullptr, nullptr, nullptr, nullptr) != ERROR_SUCCESS) break;
        HKEY sub;
        if (RegOpenKeyExA(root, name, 0, KEY_READ, &sub) != ERROR_SUCCESS) continue;
        char clsid[64];
        DWORD clen = sizeof(clsid), type = 0;
        if (RegQueryValueExA(sub, "CLSID", nullptr, &type, (LPBYTE)clsid, &clen) == ERROR_SUCCESS && type == REG_SZ) {
            std::snprintf(out[count].name, sizeof(out[count].name), "%s", name);
            std::snprintf(out[count].clsid, sizeof(out[count].clsid), "%s", clsid);
            count++;
        }
        RegCloseKey(sub);
    }
    RegCloseKey(root);
    return count;
}

extern "C" DawAsio* daw_asio_open(const char* clsid_str, char* err, int err_len) {
    HRESULT co = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    if (FAILED(co) && co != RPC_E_CHANGED_MODE) { set_err(err, err_len, "CoInitializeEx a échoué"); return nullptr; }
    wchar_t wide[64];
    if (!MultiByteToWideChar(CP_ACP, 0, clsid_str, -1, wide, 64)) { set_err(err, err_len, "CLSID illisible"); return nullptr; }
    CLSID clsid;
    if (FAILED(CLSIDFromString(wide, &clsid))) { set_err(err, err_len, "CLSID invalide"); return nullptr; }
    IASIO* drv = nullptr;
    // Particularité ASIO : l'identifiant d'interface est le CLSID du pilote lui-même.
    if (FAILED(CoCreateInstance(clsid, nullptr, CLSCTX_INPROC_SERVER, clsid, (void**)&drv)) || !drv) {
        set_err(err, err_len, "impossible de créer le pilote (installé ? même architecture 64 bits ?)");
        return nullptr;
    }
    if (!drv->init(GetDesktopWindow())) {
        char msg[256] = {0};
        drv->getErrorMessage(msg);
        set_err(err, err_len, msg[0] ? msg : "init du pilote refusée (carte absente ou occupée ?)");
        drv->Release();
        return nullptr;
    }
    DawAsio* a = new DawAsio();
    a->drv = drv;
    drv->getDriverName(a->info.driver_name);
    long in = 0, out = 0, mn = 0, mx = 0, pref = 0, gran = 0, il = 0, ol = 0;
    drv->getChannels(&in, &out);
    drv->getBufferSize(&mn, &mx, &pref, &gran);
    drv->getLatencies(&il, &ol);
    ASIOSampleRate sr = 0;
    drv->getSampleRate(&sr);
    a->info.input_channels = in; a->info.output_channels = out;
    a->info.buffer_min = mn; a->info.buffer_max = mx; a->info.buffer_preferred = pref; a->info.buffer_granularity = gran;
    a->info.input_latency = il; a->info.output_latency = ol;
    a->info.sample_rate = sr;
    return a;
}

extern "C" void daw_asio_info(DawAsio* a, DawAsioInfo* out) { *out = a->info; }

extern "C" int daw_asio_start(DawAsio* a, double sample_rate, int32_t buffer_size,
                              const int32_t* in_ch, int32_t n_in, const int32_t* out_ch, int32_t n_out,
                              DawAsioProcessFn fn, void* user, char* err, int err_len) {
    if (g_active) { set_err(err, err_len, "un flux ASIO est déjà actif"); return 1; }
    if (sample_rate > 0) {
        if (a->drv->canSampleRate(sample_rate) != ASE_OK || a->drv->setSampleRate(sample_rate) != ASE_OK) {
            set_err(err, err_len, "fréquence d'échantillonnage refusée par le pilote");
            return 2;
        }
    }
    ASIOSampleRate sr = 0;
    a->drv->getSampleRate(&sr);
    a->sample_rate = sr;
    long size = buffer_size > 0 ? buffer_size : a->info.buffer_preferred;
    if (size < a->info.buffer_min || size > a->info.buffer_max) { set_err(err, err_len, "taille de tampon hors des limites du pilote"); return 3; }

    a->n_in = n_in; a->n_out = n_out;
    a->bufs.assign((size_t)(n_in + n_out), ASIOBufferInfo{});
    a->types.assign((size_t)(n_in + n_out), ASIOSTInt32LSB);
    for (long c = 0; c < n_in + n_out; c++) {
        bool is_in = c < n_in;
        long idx = is_in ? in_ch[c] : out_ch[c - n_in];
        a->bufs[c].isInput = is_in ? ASIOTrue : ASIOFalse;
        a->bufs[c].channelNum = idx;
        ASIOChannelInfo ci{};
        ci.channel = idx; ci.isInput = a->bufs[c].isInput;
        if (a->drv->getChannelInfo(&ci) != ASE_OK) { set_err(err, err_len, "canal inexistant"); return 4; }
        if (!supported(ci.type)) { set_err(err, err_len, "format d'échantillon du pilote non géré"); return 5; }
        a->types[c] = ci.type;
    }
    a->scratch.assign((size_t)(n_in + n_out), std::vector<float>((size_t)size, 0.0f));
    a->in_ptrs.assign((size_t)n_in, nullptr);
    a->out_ptrs.assign((size_t)n_out, nullptr);
    for (long c = 0; c < n_in; c++) a->in_ptrs[c] = a->scratch[c].data();
    for (long c = 0; c < n_out; c++) a->out_ptrs[c] = a->scratch[n_in + c].data();
    a->fn = fn; a->user = user;
    a->buffer_size = size;

    if (a->drv->createBuffers(a->bufs.data(), n_in + n_out, size, &g_callbacks) != ASE_OK) {
        char msg[256] = {0};
        a->drv->getErrorMessage(msg);
        set_err(err, err_len, msg[0] ? msg : "createBuffers a échoué");
        return 6;
    }
    a->buffers_created = true;
    a->output_ready = (a->drv->outputReady() == ASE_OK);
    g_active = a;
    a->started = true;
    if (a->drv->start() != ASE_OK) {
        a->started = false;
        g_active = nullptr;
        a->drv->disposeBuffers();
        a->buffers_created = false;
        set_err(err, err_len, "start() a échoué");
        return 7;
    }
    return 0;
}

extern "C" int32_t daw_asio_buffer_size(const DawAsio* a) { return (int32_t)a->buffer_size; }
extern "C" double daw_asio_sample_rate(const DawAsio* a) { return a->sample_rate; }
extern "C" void daw_asio_latencies(DawAsio* a, int32_t* input, int32_t* output) {
    long il = 0, ol = 0;
    a->drv->getLatencies(&il, &ol);
    *input = (int32_t)il;
    *output = (int32_t)ol;
}
extern "C" uint32_t daw_asio_overloads(const DawAsio* a) { return a->overloads.load(std::memory_order_relaxed); }

extern "C" void daw_asio_stop(DawAsio* a) {
    if (a->started) {
        a->started = false;
        a->drv->stop();
    }
    if (g_active == a) g_active = nullptr;
    if (a->buffers_created) {
        a->drv->disposeBuffers();
        a->buffers_created = false;
    }
}

extern "C" void daw_asio_close(DawAsio* a) {
    daw_asio_stop(a);
    a->drv->Release();
    delete a;
}
