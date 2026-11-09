/* Couche C fine au-dessus de l'interface COM ASIO de Steinberg (Windows).
 * Rust ne voit que cette API C : tout le C++/COM reste dans asio_shim.cpp.
 * Toute la mémoire est allouée dans daw_asio_start ; le callback ne fait aucune allocation. */
#ifndef DAW_ASIO_SHIM_H
#define DAW_ASIO_SHIM_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
    char name[128];
    char clsid[64];
} DawAsioDriver;

typedef struct {
    int32_t input_channels, output_channels;
    int32_t buffer_min, buffer_max, buffer_preferred, buffer_granularity;
    int32_t input_latency, output_latency; /* latences annoncées par le pilote, en échantillons */
    double sample_rate;
    char driver_name[64];
} DawAsioInfo;

/* Appelée à chaque cycle depuis le thread du pilote. `inputs`/`outputs` : un tampon float par canal
 * choisi, `frames` échantillons chacun, valides pendant l'appel seulement. Les sorties sont mises à 0
 * avant l'appel. */
typedef void (*DawAsioProcessFn)(void* user, uint32_t frames, const float* const* inputs, float* const* outputs);

typedef struct DawAsio DawAsio;

/* Nombre de pilotes écrits dans `out` (au plus `max`), ou -1 en cas d'erreur. */
int daw_asio_list(DawAsioDriver* out, int max);

/* Charge et initialise le pilote (sans démarrer l'audio). NULL + message dans `err` en cas d'échec. */
DawAsio* daw_asio_open(const char* clsid, char* err, int err_len);
void daw_asio_info(DawAsio* a, DawAsioInfo* out);

/* Règle fréquence (0 = inchangée) et taille de tampon (0 = préférée), crée les tampons pour les
 * canaux choisis (indices à partir de 0) et démarre. Retourne 0 si tout va bien. */
int daw_asio_start(DawAsio* a, double sample_rate, int32_t buffer_size,
                   const int32_t* in_ch, int32_t n_in, const int32_t* out_ch, int32_t n_out,
                   DawAsioProcessFn fn, void* user, char* err, int err_len);
/* Taille de tampon et fréquence effectives (après daw_asio_start). */
int32_t daw_asio_buffer_size(const DawAsio* a);
double daw_asio_sample_rate(const DawAsio* a);
/* Latences entrée/sortie annoncées par le pilote, valides une fois les tampons créés (après daw_asio_start). */
void daw_asio_latencies(DawAsio* a, int32_t* input, int32_t* output);
/* Compteur de dépassements/pertes de synchro signalés par le pilote. */
uint32_t daw_asio_overloads(const DawAsio* a);

void daw_asio_stop(DawAsio* a);  /* arrête et libère les tampons */
void daw_asio_close(DawAsio* a); /* libère le pilote */

#ifdef __cplusplus
}
#endif
#endif
