#include "align/Progress.h"

#include <algorithm>

namespace align::progress
{
namespace
{
thread_local const ProgressFn* tFn = nullptr;
thread_local double tBase = 0.0, tSpan = 1.0;
} // namespace

Scope::Scope (const ProgressFn* fn) : previous_ (tFn), previousBase_ (tBase), previousSpan_ (tSpan)
{
    tFn = fn;
    tBase = 0.0;
    tSpan = 1.0;
}

Scope::~Scope()
{
    tFn = previous_;
    tBase = previousBase_;
    tSpan = previousSpan_;
}

Phase::Phase (double from, double to) : previousBase_ (tBase), previousSpan_ (tSpan)
{
    tBase = tBase + from * tSpan;
    tSpan = std::max (0.0, to - from) * tSpan;
}

Phase::~Phase()
{
    tBase = previousBase_;
    tSpan = previousSpan_;
}

void report (double fraction)
{
    if (tFn == nullptr || ! *tFn) return;
    const double overall = std::clamp (tBase + std::clamp (fraction, 0.0, 1.0) * tSpan, 0.0, 1.0);
    if ((*tFn) (overall)) throw Cancelled {};
}
} // namespace align::progress
