# Decision-directed feedback separation: rejected candidate

Base: `21806c0`. Production sources were restored exactly after the declared vowel guard failed. This directory preserves an experiment; it is not a shipping DSP change.

The existing recurrence feeds the applied user gain back into source-power prediction. The candidate separated that state using the natural Wiener estimate `H0 = xi / (1 + xi)` and `previous_clean_power = H0² × observed_power`. User attenuation, floor, curves and smoothing did not affect this private state. The primary equation reference and the guards were recorded in `predeclared.json` before measurement. The configured MarkItDown executable was absent; only publisher HTML and an already converted search excerpt were consulted.

All 136 functional tests passed, with four existing ignores. The independent 240-case f64 recurrence agreed within 1.6873e-7 scaled error. Five application-control configurations had exact estimator-state invariance and reset behavior. All 720 component traces retained their independent DFT checks (maximum scaled error 3.9500e-7) and exact noise-PSD invariance.

The 36 development full-source SDR changes ranged from -0.000994 to +2.534014 dB, all within the predeclared 0.25 dB regression limit. Both settled independent-noise attenuation checks were unchanged. However, the original 300–450 ms vowel projection was only 0.191373 in quality mode and 0.333957 in low-latency mode, below the required 0.5 in both modes. These are improvements over the original characterization, but they do not satisfy the declared requirement. The candidate was rejected without retuning.

`validation.json` contains full raw output, including the original tone-energy characterization and all recorded fixtures. `analysis.json` contains every signed SDR delta, noise guard and vowel result. `candidate-source/` and `candidate-diff.json` (losslessly encoded unified diff) preserve the complete changed sources. The legacy scalar baseline remains red for control-invariance, independently of this candidate's quality failure. `restored-validation.json` and `restored-clippy.json` record verification after restoration; their completion and exit codes must be inspected explicitly.

This experiment supports the diagnosis that user-controlled feedback can bias the prior estimate. It does not establish that removing that feedback resolves vowel loss. Held-out multilingual/noise recordings, listening, and native Linux/Windows DAW/CI evidence remain unavailable and open. No deadline acceptance is claimed.
