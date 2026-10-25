# Learned release fitting experiment

The explicit offline model and native-qualified fitter are implemented, with
classical fallback and report-only abstention. The general learned-initializer
adoption gate failed; this does not complete the Limiter ML item or the audit.

`protocol.json` froze training/evaluation sources and gates before the first
full evaluation. `training-report.json`, `feature-cache.npz` and `metrics.jsonl`
archive split identities, selected checkpoint and feature-level training data.
The full checkpoint lives in `crates/sotf-plugins/models/limiter-release`.
The initial smoke positive and negative control passed before full training.

Initial evaluation: seed 20471026, 20 excited synthetic native/shift probes all
admitted; all five negative controls rejected. Each search had at most 24
attempted evaluations; shared ambiguity probes and preset reopen are additional.
Neural and grid-plus-analytic searches both qualified all 20 positives. The
analytic raw estimate had lower median log error than the learned raw estimate.

Independent reconstruction checked 1,582 native outputs containing 652,981,308
samples, including sample ceilings. Maximum feature discrepancy was 7.08e-14,
portable versus Torch prediction discrepancy 2.38e-11 ms, and independently
recomputed waveform metric discrepancy 2.50e-15. Seven Python unit tests passed.
Six functional CLI checks cover learned/classical export, abstention, checksum
failure, unsupported profile and unchanged outputs after overwrite rejection.

That initial comparison did not isolate the cheaper analytic-only initializer.
An additional baseline comparison therefore used 20 fresh identities, seed
20481026, with unchanged model, features, optimizer and waveform gates. It used
the same endpoint+one-seed initialization and 24-evaluation cap for both methods,
without shared caches. **Analytic-only qualified 20/20 versus neural 19/20**, with
median 15.5 versus 17 attempted native evaluations. The neural short-context
failure had relative RMS 0.05149 and maximum error 0.01294. No weights or gates
were tuned after these results.

`analytic-independent.json` checks disjoint train/development/prior-evaluation
identities and independently verifies the additional 668 native outputs,
303,752,394 samples and waveform metrics (maximum discrepancy 6.94e-18).
The model remains explicitly experimental. Default native DSP is unchanged.

A single local macOS offline CLI profile took 1.10 seconds and reported
87,900,160 bytes maximum resident size (`time -l`). This includes subprocess/I/O
cost and process accounting; it is not a concurrent process-tree memory bound,
realtime budget, host measurement or cross-platform performance claim.

Large native arrays and source snapshots remain at
`/Volumes/home_tmp/tmp/ml-limiter-release-20261025`; they contain only generated
audio. Retained source recipes, seeds and hashes reproduce them. Native crate
tests and renderer allocation/streaming evidence from the preceding checkpoint
are in `../ml-limiter-native-20261025`; no Rust/native DSP sources changed here.

This evidence establishes synthetic fitting behavior in the documented native
profile. Natural material, independent tolerance/ambiguity calibration, other
profiles, perceptual/user-reference acceptance, hardware matching and host/UI
integration remain open. The full implementation ledger preserves those gaps.
