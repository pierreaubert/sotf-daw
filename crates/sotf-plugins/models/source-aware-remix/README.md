# Centered-source remix qualification

This synthetic candidate is available only through the Downmix crate's `qa`
feature. It is a separate stereo `SourceRemix` stage after ordinary Downmix.
It has no production factory registration or automatic activation. The default
method is bypass. Production admission is false.

The 6→8→1 tanh/sigmoid network estimates centered foreground mid-energy fraction
from spatial features. It does not identify dialogue, voices or instruments.
Training uses project-generated complex mixtures: 64 training, 16 validation and
32 final scene seeds, with 512 rows per scene. Shift variants reuse final seeds
and are correlated probes, not independent datasets. No external audio was used.
The optimizer reached its fixed 300-iteration limit without reporting convergence.

Foreground and bed controls are bounded to ±6 dB, with mix in [0,1] and 5 ms
hop-rate smoothing. Only probabilities outside [0.15,0.85] drive separation;
uncertain, invalid, missing-model and unsupported-clock bins use neutral remix.
Equal foreground/bed gains deliberately apply the requested common gain without
requiring a source estimate. DC and Nyquist remain neutral otherwise.

The fixed 512-point, 256-hop WOLA stage adds 512 samples after matrix Downmix's
zero latency. The network adds no further lookahead. Supported learned clocks
are 44.1, 48 and 96 kHz. A frozen-gain finite drain preserves source counters.
Conditional positive M/S gains bound complete-stream energy by the largest
requested gain (including unity); this is not a sample-peak ceiling or limiter.

The learned candidate improves steady target-remix SNR in 3 of 10 generated
native waveform probes and regresses in 7. Deltas versus the analytic M/S
reference range from −4.55 to +1.90 dB. Centered bed ambiguity produces confident
misclassification in the held spectral probes. These results do not justify
deployment or semantic source-separation claims.

See `audit/ml-source-remix-20261025.md` and the adjacent manifest for evidence
and remaining product and validation work. Train with
`scripts/ml/train_source_remix.py`; evaluate actual native output with
`scripts/ml/evaluate_source_remix.py`. Their command-line help describes paths.
