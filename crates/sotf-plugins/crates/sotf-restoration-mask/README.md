# Experimental restoration mask evaluator

Shared fixed-storage evaluator for explicit QA denoising/restoration modes.
DSP consumers depend on this crate only through their optional `qa` feature.
It has no factory plugin, automatic activation or admitted production model.

This extracts the expander's existing six-feature, eight-hidden-unit causal
music/noise mask and state without changing its coefficient or feature contract.
The compatibility JSON format remains `sotf-expander-denoising-mask-v1`.
Using its frozen synthetic model in another plugin is cross-plugin qualification;
it does not create independent training evidence or remove its clean-material
distribution failure.

The evaluator owns fixed arrays for 513 bins: FFT1024, hop256, supported clocks
44.1/48/96kHz, linked unnormalized FFT magnitudes and periodic Hann window sum.
The caller owns FFTs, classical histories/gains, final mix and finite EOF support.
Uncertain/missing/invalid evidence keeps exact classical gains. DC/Nyquist are
classical. Selected gains stay within [0.1,1], with fixed5/30ms smoothing.
EOF freeze preserves learned history, gains and source counters.
