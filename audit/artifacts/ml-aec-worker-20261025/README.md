# Opt-in native AEC hybrid worker checkpoint

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

This checkpoint wires the pinned residual model into explicit native AEC
configuration. Default audio remains classical. The learned stage augments the
classical spectral post-filter with bounded additional cuts; it never replaces
adaptive cancellation, relaxes classical cuts or overrides current near-end
protection. Natural/device/listening and cross-platform runtime acceptance remain
open. The AEC proposal and full 31-plugin/five-program ML goal remain active.

## Contracts

- Optional `AecPluginParams.residual_suppression` roundtrips through serde and
  the existing plugin factory. No new scalar automation IDs or default fields
  are emitted. The facade maps `neural-aec` to the AEC `neural-residual` feature.
- The shared `plugins-inference::AsyncInference` bridge owns a two-packet queue.
  Packets and outputs contain fixed arrays and source endpoints. Reset invalidates
  queued and in-flight work; owned result destruction stays on the worker.
- Realtime callbacks do not run inference, wait for a result, load weights,
  create/join threads or allocate. Contention, full queues, unavailable models,
  invalid/future/old observations and current near-end/ref-silent protection
  retain classical processing. No inference or thread is created by default.
- Age is source samples since the observation endpoint, separate from the
  256-sample observation interval and 50 ms causal EMA time constant. Valid
  observation warmup is 50 ms. Default endpoint age limit is 40 ms.
- Extra spectral attenuation is capped at 6 dB by default (configurable 0..12),
  attacks over 10 ms and releases immediately. Zero extra cut is exactly
  classical. This is a spectral bound, not a per-sample amplitude guarantee.
- Explicit Offline initialization chooses synchronous fixed-size inference;
  callback partitions match exactly. Reset preserves mode; direct `initialize`
  selects Realtime. Loading/replacing/dropping workers is control-thread work.
- Host-position discontinuities retire learned history; a mixed partial native
  block is skipped. Once a nonzero host clock is established, seeks to zero keep
  clock tracking, including repeated loops. Legacy all-zero positions use a local
  monotonic clock and require an ordinary plugin reset when sources change.
  Learned invalidation alone does not reset the adaptive canceller.
- First valid EOF freezes the last extra coefficients and invalidates late
  worker publications. EOF silence does not update features or decisions. The
  native 256-sample latency, finite tail and drain bounds remain unchanged.

## Verification evidence

- `enabled-tests.*`: initial full enabled suite, 59 library plus 19 integration
  tests pass. The six new integration cases cover serde/old presets, exact missing
  model fallback, offline partition/reset equivalence, zero-cut and reference
  silence, frozen EOF/transactional rejection, and paced realtime publication.
  Allocation assertions surround cold/live/reset/EOF callback operations.
- `disabled-tests.*`: 54 library plus 15 integration tests pass without the model
  feature, including exact feature-unavailable fallback and retained settings.
- `stage-tests-final.*`, `worker-integration-final.*`: after the seek-to-zero
  review fix, four stage tests and all six worker integration tests pass. The new
  clock case adds one distinct library regression to the initial enabled suite.
- `bridge-tests.*`: all 18 shared bridge tests pass, including a controlled
  in-flight reset. Its old result is discarded before publication and the next
  generation recovers. The shared production bridge itself was not changed.
- `clippy*`: enabled and disabled all-target strict Clippy pass. The first
  attempt (`clippy-r1.*`) exposed an eight-argument function; the final code uses
  a named block context instead of a lint suppression.
- `facade-check.*`: plugin facade builds with `--no-default-features --features
  neural-aec`; the existing serde factory consumes the new structural setting.
- `python-checks.json`: Ruff and formatting pass for evaluation and reduction.
- `asset-eol-check.json`: simulated autocrlf checkout filters preserve the pinned
  AEC and AAE model bytes. Scoped `.gitattributes` rules prevent line-ending
  conversion from invalidating runtime SHA checks; this is not Windows host QA.

## Fresh quality comparison

`evaluate.json` records terminal success (3.34 seconds). `fresh-final/` contains
24 new six-second cases at 16/44.1/48 kHz, seed 20274025, disjoint from all 64
original prototype inputs. The model is unchanged. The hybrid policy was chosen
after initial prototype development and frozen before these new cases existed;
no weights or policy were tuned from the fresh final scores.

`evaluation-source/` and `evaluation-source-sha256.json` capture the exact
evaluation preimages. `independent-quality.json` recomputes source/output hashes,
native scores, mode/availability/endpoint accounting and exact downstream-only
behavior. All adaptive error, spectrum, feature and oracle arrays hash-identical
between classical and hybrid for every case. The model actually applies cuts in
3,708 blocks; the comparison cannot silently substitute an unavailable model.

Across 18 steady echo intervals, the delta ranges from 0 to +4.88 dB; three drift
cases improve by more than 1 dB. Near-only output is bit-identical in all three
rates. The worst near-target SNR delta is -0.123 dB, retained with all other
negative results. Some absolute quiet-double-talk scores remain poor in both
methods. These synthetic offline results do not establish general speech/device
or realtime-worker quality, and numerical-noise deltas are not substantive wins.

The seek-to-zero fix happened after evaluation. `final-source-replay.json`
verifies all five relevant output-array hashes exactly on classical/hybrid
replays at all three rates with final sources. Only host-clock discontinuity
tracking changed; monotonic-clock evaluation behavior and weights did not.

Full generated audio/features remain in the external cache at
`/Volumes/home_tmp/tmp/ml-aec-worker-20261025/final`, with hashes and reproduction
sources here. No external recordings or pretrained weights were added. Model and
synthetic data retain the workspace GPL-3.0-or-later license.
