# sotf-plugin-band-split

SOTF Band Split plugin for splitting a signal into frequency bands.

Splits an audio signal into multiple frequency bands using Linkwitz-Riley
crossover filters. Phase-compensated routing adds the later splits' allpass
responses to intermediate bands, so the sum preserves magnitude with allpass
phase. Legacy cascade mode preserves the original unequal-delay routing;
its three- and four-band sum need not have flat magnitude. Neither mode
promises raw sample identity with undelayed input.

The plugin supports 2–4 bands, LR24/LR48 slopes, 20 Hz through the lower of
20 kHz and 0.49 × sample rate, and every catalogued input layout through 12
channels. Output is band-major and latency is zero. Frequency and gain
automation are smoothed at audio rate; expensive IIR coefficient redesign is
bounded to a persistent eight-sample cadence (6 kHz at 48 kHz) so behavior is
independent of callback partition. Completed ramps adopt the exact cutoff
without clearing filter history, including edits below the shared LR setters'
normal small-change threshold. LR24/LR48, recombination mode and band-count
selection are structural and require a graph rebuild.
