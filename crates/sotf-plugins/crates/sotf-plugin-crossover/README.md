# sotf-plugin-crossover

SOTF Crossover plugin for frequency band splitting.

Splits audio into frequency bands using Linkwitz-Riley filters for use in multiband processing or multi-way speaker management.

LR24 coefficients and delay state use double precision internally, with the
existing interleaved `f32` audio and parameter boundaries unchanged. This avoids
low-bass transfer error from single-precision recursive state. Processing remains
allocation-free; FIR behavior and preset schemas are unchanged.

`CrossoverPlugin::fir_memory_report()` reports the compiled FIR coefficient,
history, alignment-delay, scratch, and total byte counts before graph admission.
LR crossovers return `None`. Processing uses allocation-free interleaved block
kernels with the generic scalar multiband/per-channel paths retained.
