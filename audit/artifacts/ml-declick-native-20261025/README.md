# Native Declick data renderer

Issue 10, prerequisite for the optional offline repair discriminator. The new
`qa-declick-render` QA-feature binary renders the actual public plugin from
bounded interleaved float32-le input and exports untrimmed native output plus
metadata. It retains the latency prefix and calls public EOF drain until exact
completion. Both process and drain run under allocation guards. It does not
change production detection, repair, defaults, parameter metadata or latency.

The request supplies input/output paths, sample rate, channels, frame count,
callback frame size and native parameters. Bounds: 8–192 kHz, 1–8 channels,
1–4096 callback frames, 4 million input samples, 64 KiB JSON, finite input within
±32. Requests with unknown or clamped parameters, residual audition, incomplete
input geometry or existing output are rejected. Float PCM levels are preserved.
Output is created exclusively after successful native processing and drain;
metadata declares exact original/output frame counts and latency samples.

`build.json` records successful release build and strict Clippy. `build-r1.json`
preserves the resolved Clippy request to replace constant `chunks_exact(4)` with
array chunks; no DSP behavior changed. The earlier `build-initial.*` checkpoint
preceded strict QA parameter validation and is not the final gate.
`native-tests.json`/`.stdout.txt` record 131 successful tests from the full
Declick QA-feature suite, including existing finite-stream, repair, multiband,
periodic and drain contracts. The current `test_declick_model.py` additionally
runs the actual binary across rates, partial callbacks, disabled passthrough,
owned modes, widening, high channel counts, rejection and overwrite checks;
final results are in the model experiment's `final-checks.json`.

`candidate-probe.json` demonstrates that native groups can be either beneficial
injected-click repairs or harmful edits to legitimate impulses with identical
observations. `percussion-probe.json` is an initial illustrative fixed-one-sample
leading-pulse probe. Training broadened legitimate leading pulses to 1–4 samples
before its source freeze. These preliminary probes are not final quality data.
The smoke model, data identities and report are preserved here; its threshold1
abstained. Fresh smoke inference agreed with independent Torch operations.

Sources, input geometry and the final renderer hash are pinned in manifests.
Generated PCM remains in the external task cache, not in Git. No external audio,
pretrained weights, recordings or listening data were used. The optional ML
prototype and negative final result are described in the adjacent
`ml-declick-model-20261025` artifact and model README. All source/synthetic data
is GPL-3.0-or-later. The full audit remains in progress.
