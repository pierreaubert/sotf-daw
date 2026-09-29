# AUD009 Gate modes: verified implementation

Date: 2026-09-28. Crate source/tests/docs frozen after the focused gate below.
The earlier AUD065 evidence and AUD009 proposal remain in
`/tmp/sotf-gate-knee-and-modes.md`; this report records the implementation.

## Signal and timing conventions

The original downward sample-processing branches are retained. A separate
`src/lib/above_threshold.rs` path implements Upward and Duck. All three use
the existing detector, channel linking, optional sidechain filter, sample-rate
contract, threshold/mix smoothers, and program-only lookahead delay.

Let x=L-T for detector level L and threshold T in dB. The hinge H is zero for
x<=-K/2, (x+K/2)^2/(2K) within the knee, and x above K/2. Widths below 0.1 dB
use max(x,0). Upward gain is +min(max_boost,(R-1)H), Duck gain is
-min(range,(R-1)H). Duck's ratio is the reflected expansion slope, not a
conventional compressor ratio. A zero attenuation range retains the existing
finite 240 dB ceiling. A zero boost ceiling disables boost. Max Boost reduction
immediately bounds both the held target and applied envelope, including during
hold/release. f32 output overflow saturates to the finite numeric range;
ordinary floating-point headroom above 0 dBFS is not clipped.

Upward/Duck activate at the lower knee edge T-K/2, where effect starts from
zero. At/above activation, the target follows the curve. Within the hysteresis
band below activation, the most recent target is retained. Below activation-H,
hold retains that target for exactly its configured sample count, then targets
zero. Attack follows increasing effect magnitude and release follows decreasing
effect, using the documented one-pole dB time constants. Downward keeps its
existing opposite attenuation-envelope direction. Hysteresis/hold are explicitly
history-dependent, including smaller retained targets after gradual knee exits.

The feature comparison is limited to above-threshold boost and sidechain
ducking functions described by [FabFilter Pro-G's primary documentation](https://www.fabfilter.com/help/pro-g/using/timecontrols),
checked 2026-09-28. The SOTF equations and fixed one-pole timing are independently
specified; proprietary transfer/timing equivalence is not claimed.

## Parameter, preset and telemetry contracts

- Existing indices 0..14 and defaults are unchanged. Index15 `mode` is a
  structural choice: exported `GateMode::{Downward,Upward,Duck}` = 0/1/2.
  Runtime exact no-ops succeed; actual changes and mixed batches reject before
  settings or audio history mutate. Construction/pre-initialize configuration
  supports the new modes.
- Index16 `max_boost_db` is realtime, default12 dB, range0..24 dB. Both typed
  `GatePluginParams` and serializable `params::Params` carry the new fields.
  Constructor boost is f32; stored Params boost is f64. Empty/old JSON selects
  Downward. Mode accepts case-insensitive labels and integral numeric indices,
  including `1.0`; invalid/null/fractional values reject.
- Signed wet `GateData.gain_db` is positive for Upward and negative for
  Downward/Duck. Legacy `attenuation_db` remains nonnegative and zero for
  Upward. `effect_active` tests any wet magnitude >=0.1 dB. `gate_open` exposes
  the detector hysteresis latch, excluding subsequent hold. `mode` identifies
  its semantics. Legacy `is_open` keeps the low-attenuation meaning and remains
  true during upward boost. All describe wet processing before Mix.
- Snapshot arrays are preallocated and held published snapshots remain
  immutable. External sidechain samples stay byte-for-byte unchanged.

## Independent numerical and lifecycle evidence

Eight new tests in `tests/above_threshold_audio.rs`:

1. 264 settled transfer configurations: 2 modes ×2 link settings ×3 rates
   (44.1/48/96 kHz) ×2 knee widths ×11 detector levels, including both edges,
   center and adjacent probes. Both channel gains are checked separately.
   Another20 configurations cover ratio1, zero/3/12/24 dB caps, dry/wet mixing
   and the distinct zero-range convention.
2. Four sample-by-sample timing configurations (2 modes ×2 link settings):
   independent exponential attack, retained target within hysteresis, exact
   96-frame hold and exponential release, checking every sample.
3. Stable indices/update modes, typed JSON/defaults, invalid values, constructor
   validation, unchanged settings/history after failed mode or mixed updates,
   and immediate zero boost.
4. Signed/nonnegative telemetry, linked-channel gain mirroring, state meanings,
   and immutable held snapshots.
5. Eight automation/partition/reset configurations (2 modes ×2 links ×peak or
   filtered RMS), with lookahead and callback sizes1/7/31/256 vs2048. Outputs
   match exactly, including reset versus a fresh matching configuration.
6. Two independent lookahead impulse checks: 48-frame program delay, with gain
   computed from the undelayed sidechain at the delayed impulse's output time.
7. Four finite peak-extreme configurations, including +/-f32::MAX, nonfinite
   program sanitation, unchanged nonfinite sidechain storage, and recovery.
8. Old JSON/default Downward and explicit Downward produce identical waveforms
   under different callback partitions. The existing79-test downward suite
   remains green.

The existing explicit TLS allocator/deallocator regression now covers all3
modes ×2 link settings ×2 sidechain layouts ×2 detector configurations =24
cold threads. It measures first processing, mode no-op, live boost/knee/threshold
updates, reset, and subsequent processing: **zero allocations and zero frees**.

The gain oracle uses independent f64 equations and standard log/pow rather than
production fast math. Settled waveform error bound is0.02 dB, reflecting the
existing fast logarithm/exponential and f32 envelope. Partition/restoration
comparisons are exact. No numerical tolerance was loosened to pass.

## Verification and ownership

- `cargo test -p sotf-plugin-gate --all-features`: **87 passed**, zero failed or
  skipped; includes QA target compilation. Log `/tmp/sotf-gate-modes-tests.log`.
- `cargo clippy -p sotf-plugin-gate --all-targets --all-features -- -D warnings`:
  passed. Log `/tmp/sotf-gate-modes-clippy.log`.
- `cargo fmt -p sotf-plugin-gate` and scoped `git diff --check`: passed.

This agent edited only the Gate crate for implementation. Required external
wiring was handed to `/root/plugin_chain`: add typed mode/boost fields to engine
settings and reconstruction/accessor paths, reexport GateMode through the
facade, include both appended controls in bridge/native/FFI mappings, and test
defaults plus nondefault preset/audio round trips. External wiring has its own
verification and is not claimed by the87-test crate result. Native external-key
sidechain availability is separately tracked as AUD068.

## Separate shared-detector limitation (AUD067)

The pinned shared `math-dsp::LevelDetector` RMS implementation squares samples
in f32 before promoting to f64 and stores squared history in f32. Extremely
large finite sidechain samples can overflow this history. This predates the new
Gate modes and is not masked or changed in the new path. Ordinary RMS/reset and
nonfinite sanitation tests pass; the finite-extreme feature guarantee above is
specifically tested with Peak. Root owns disposition of the shared dependency;
the isolated reproduction and exact source provenance are recorded separately.
