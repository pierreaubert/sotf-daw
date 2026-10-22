# sotf-plugin-xtc

SOTF XTC plugin for crosstalk cancellation.

Removes acoustic crosstalk to deliver a binaural-like listening experience from stereo speakers. Uses FFT-based processing with overlap-add for real-time operation.

Source topology is construction-time state. `source_mode`, `hrtf_file`,
`room_ir_file`, `recommended_matrix_file`, and `fft_size` must be changed by
rebuilding the plugin/graph; the callback only adopts same-width filter updates.

Rapid geometry and head-tracking changes are coalesced through one latest-only
worker per plugin instance. Completed filters use bounded mailboxes with nonblocking adoption; stale
requests are replaced before computation and no file loading occurs in the
audio callback.

Measured room IRs may be mono or stereo PCM WAV in integer or floating-point
encoding. The explicit `fft_size` early-reflection window rejects longer IRs
with a trimming/windowing error rather than silently losing their decay tail.

## Stream timing

Processing uses exactly `fft_size` frames of causal latency,
independent of callback size. Three zero-history windows preserve the first
programme samples; negative-time synthesis is discarded before the circular
output buffer can wrap. Every accepted input frame is consumed, including when
callbacks change size or exceed prepared staging. Initialization/reset starts a
fresh zero-history timeline. Invalid rate or buffer dimensions are rejected
before filter adoption, output writes, or audio-state changes.

The diagnostic `bypass_xtc_filters` path remains inside this delayed STFT and
provides a neutral reconstruction oracle. `enabled=false` now uses an exact
N-frame stereo dry delay (L/R in the first two outputs, zeros in extra outputs).
Wet filters, AutoGain and the limiter remain warm. Enable changes crossfade over
10 ms, rounded to the nearest positive integer frame count, starting on the next
output frame; repeated identical snapshots do not restart the fade. Reversals
start from the current wet coefficient. Complementary linear weights preserve
neutral unity, with exact dry/wet endpoint branches. Dry samples are neither
AutoGain compensated nor limited, so overrange bypass values remain unchanged.

This intentionally replaces immediate disabled routing and paused wet history:
disabled output now agrees with the existing fixed N-frame latency declaration,
and re-enabling cannot replay old programme samples. Disabled mode incurs the
wet processor's CPU cost plus a prepared stereo delay (8N bytes, 1–128 KiB over
the supported FFT range).

The wet limiter targets a 0.95 peak with a 0.2 ms attack and 50 ms release
envelope. During finite-attack overshoot, a shared instantaneous gain keeps all
speaker drives within a sample-peak ceiling of 1.0. This preserves interchannel
ratios instead of independently clipping each speaker; the final clamp only
bounds floating-point rounding. This is a sample-peak bound, not a reconstructed
true-peak guarantee. Diagnostic `bypass_xtc_filters` skips this limiter.

The default analytical head-shadow surrogate blends its low/high-frequency
curves with a smoothstep over `ka=0.5..2`, where `ka=2*pi*f*head_radius/343`.
It uses the same high-frequency exponent throughout the blend and above it,
removing the former magnitude jump at `ka=2`. Responses below `ka=0.5` and at
or above `ka=2` retain their existing formulas. This continuity correction
does not calibrate the surrogate to measured human HRTFs; its existing 8 kHz
ILD reference discrepancy remains unresolved.

Independent inverse tests use f64 point-source geometry and a Cholesky solve
against the production f32 determinant solver. They include lateral head
displacement, yaw and a single reflected path, plus signed residuals when a
fixed design is evaluated in a changed scene. These are controlled numerical
plants, not measured head/room validation. A separate rendered-output test now
projects actual `Plugin::process` speaker outputs at six coherent tones after
startup, using 48 kHz/FFT2048, quiet linear operation and fixed/irregular callbacks.
Those measured filter transfers are evaluated against independent displaced-head
point-source and one-echo plants. Of 360 controlled cases, 134 worsen normalized
separation; the worst is -5.01044 dB. This is not a human-HRTF, hardware or listening
result, and it is not directly comparable with the earlier core-inverse raw-depth
matrix because the filter source and normalization differ.

`validation::predict_analytic_cancellation` reports the actual selected FFT bin,
left/right cue raw attenuation, intended-ear gain and normalized separation.
It honors the selected analytical head model, yaw, shadow/pinna tuning and LF
delay mode. It is a shared-model filter prediction, not a measurement of the
rendered plugin. Unsupported active resource modes return `None`; compatibility
scalar helpers return `NaN`. They do not load HRTF/RoomEQ/room resources and claim
an analytical fallback as the requested response.

`validation::evaluate_cancellation` instead accepts independent complex plant
and filter matrices, computes `T = H * W`, and preserves signed, uncapped values.
Raw crosstalk attenuation must be read alongside intended gain: reducing both
ears equally does not improve separation, and a muted cue has undefined separation.
Matrix entries use `[real, imaginary]` values with a coherent timing reference;
rows/columns are ears/speakers for `H` and speakers/intended inputs for `W`.
The caller must establish measurement provenance and calibration.

The legacy `measure_cancellation_depth_db` name now returns the worse signed raw
prediction across both inputs, using the configured FFT size. Its former 0..40 dB
clamp and arbitrary-frequency plant evaluation could hide degradation and create
false bin-dependent differences. Obsolete test-only display-depth bounds were replaced by actual coefficient-gain,
finite-value and independent arithmetic checks; historical
minimum thresholds and the failed 8 kHz ILD tolerance are unchanged.

Before continuous ear-path normalization, the default nominal report passed
15 of 17 comparisons: 8 kHz ILD was 36.124 dB versus 12 dB (20 dB tolerance),
and the 100 Hz request selected a 93.75 Hz bin with 4.986 dB versus a 5 dB minimum.
The continuous-path report initially passed 16 of 17: far/near shadow ILD is 28.627 dB,
within that existing broad tolerance, while 93.75 Hz cancellation was 4.954 dB
and still failed. Coordinated gain limiting retains 16/17 with 4.822 dB at that
bin; the existing 5 dB target remains failed. This is an analytical reference comparison, not calibrated HRTF
accuracy. The controlled 360-row point-source/echo mismatch matrix now has 140
degraded normalized-separation rows (previously 134), with worst -5.382 dB
(previously -5.010 dB). All rows remain; correcting model symmetry does not
establish improved robustness.

Prediction helpers allocate filter arrays and belong on a control/analysis thread,
not in `process()`. Matrix evaluation operates only on supplied values.

Stereo AutoGain ingests every frame. It compares the original input delayed by
N frames with the uncompensated wet output, then refreshes its target every
`max(1, floor(sample_rate/10))` frames. Each refresh affects the following frame
onward. Gain smoothing advances per sample, so callback sizes do not select or
retime the measurements. Initialization, reset and a newly enabled AutoGain
meter start a fresh measurement phase. AutoGain remains inactive for output
matrices wider than stereo.

This replaces the earlier once-per-ten-callback measurement, which skipped
audio and could leave a large-callback stream without a valid loudness reading.
AutoGain-enabled waveforms intentionally change; input/output level changes are
now compared on the same delayed clock. Metering every frame increases CPU work.

## Finite streams

`drain()` releases retained audio using the current filters and active transitions.
With N=`fft_size`, H=N/4, and S total accepted input frames since reset, enabled
or transitioning EOF emits `2N-H + ((H-S%H)%H)` frames. This conservative count
includes trailing zeros; the uniform tail bound is `2N-1`. Settled disabled EOF
emits exactly N delayed dry frames and declares N. A transition's accepted tail
bound remains latched through cached output and completion, even if its fade
finishes during a refill. All accepted frames advance both dry and wet clocks.

Each drain call processes at most one H-frame continuation and emits at most H
frames. A prepared cache accepts smaller frame-aligned destinations without
changing AutoGain's sample-clock measurement cadence. Ordinary processing and
drain use the same clock. Empty streams do not freeze. After accepted EOF
on a nonempty stream, reset or reinitialize before supplying new input or changing
controls; identical parameter snapshots remain no-ops. Reset clears dry/wet history and snaps the transition to the configured enabled
state. Successful initialization also prepares the ramp duration at the new rate.

EOF defers pending filter adoption while retaining the active filter/fade owners.
The worker can finish and reclaim its bounded retirement slots; no callback joins
it or destroys a final filter owner. Pending desired configuration survives reset
and may be adopted only by a subsequent validated nonempty normal callback.
Reset does not wait for asynchronous configuration work.

Successful synchronous initialization invalidates older worker generations,
including work that passed its check before initialization completed. Invalid
sample rate zero and rates outside the existing stereo AutoGain meter range
16..=2822400 Hz are rejected before clock/generation changes.

Initialization prepares all source filters, room data and AutoGain at the requested
rate before installing them. Missing or invalid matrix/HRTF/room IR files, source
rate mismatches and changed output width return an error, preserving the previous
clock, filter owners, buffered audio, EOS cache and pending desired generation.
Restore the artifact or rebuild the plugin for a changed layout, then retry.
Successful initialization starts fresh audio and meter history, including the
measurement frame phase. It reloads active artifacts once and uses those
in-memory results; room spectra are recomputed for the requested rate.
Initialization remains a control-thread operation that may allocate and perform
file I/O. The asynchronous setter/publication mechanism is unchanged.

## SOFA delay support

SOFA `Data.Delay` uses source-rate sample counts. Shared `[I,R]` and
measurement-specific `[M,R]` nonnegative integer delays are materialized exactly
once into the HRIR before filter preparation. Missing/all-zero delay preserves
raw samples and length; existing SQLite caches are loaded unchanged. Fractional
or negative nonzero delay returns an explicit unsupported-capability error.
Newly materialized IR storage is limited to 256 MiB for the entire dataset,
separately from the renderer's per-IR support limit. Same-rate integer timing is
verified; this change alone does not establish cross-rate phase accuracy.

When delay was materialized, selected plant IRs with nonzero support beyond the
FFT are rejected. Trailing zeros and long unselected measurements are safe.
Existing absent/zero-delay and SQLite long-IR truncation remains unchanged and is
a separate limitation. Old caches that discarded delay metadata must be
reimported from their source SOFA file.

## Continuous analytical ear paths

Synthetic filter preparation now evaluates all four speaker-to-ear paths from
the signed source projection onto the interaural axis. Near-ear ownership follows
the source across either median plane. Each source's two magnitudes share its
near-ear head-shadow/pinna reference, so they converge at a crossing. Both source
columns use the mean center range as one smooth propagation reference. The
explicit LF mode blends geometric/shadow magnitude toward unity before pinna
shaping, retaining its existing 300 Hz crossover and 50 Hz width.

This intentionally changes centered/default as well as displaced filters. The
historical default coefficient fixture remains alongside a separate continuous
path fixture. Per-source normalization and center-range plus far-ear extra arc
remain relative analytical approximations; they are not exact finite-distance
ear-surface paths or measured HRTFs. At a quarter turn the spherical plant can
be rank deficient, so no inverse can independently reproduce both ear cues.

Geometry and inverse preparation use f64 internally; audio and stored filters
remain f32. Stable condition-number arithmetic avoids subtracting close singular
value roots. Design and diagnostics share the plant; the callback still adopts
prepared filters through the existing bounded worker exchange. Parameter and
preset schemas, FFT latency, gain limits and callback ownership stay unchanged.

## Prepared-filter gain limits

Each inverse input column uses one soft gain chosen by its largest complex
coefficient. Both speaker drives retain their relative amplitude and phase.
Spectral-normalization limiting uses the same column rule, including measured
HRTF filter preparation. The half-ceiling tanh knee and coefficient ceiling are
unchanged; independently compressing coefficients could rotate the cancellation
steering vector even when each coefficient stayed within budget.

The final per-bin effort cap uses one common matrix gain set by the larger
loudspeaker-row power. Every row still satisfies its sum-of-squared-coefficients
budget, while both input columns keep their steering ratios. A quieter speaker
can retain unused headroom. Power calculations and scaling use f64 preparation
arithmetic so finite f32 extremes do not overflow or evade the cap.

These changes preserve normalized ear ratios through the limiting operations for
a fixed linear plant. They do not preserve cancellation through the intentional
band-edge identity blend, establish correct measured transfer data, or guarantee
time-domain/reconstructed peak levels. Frequency blends, regularization, gain
ceilings, parameter schemas, worker ownership and FFT latency are unchanged.
Default responses intentionally change; earlier coefficient fixtures and nominal
failures remain alongside the current coordinated-limit fixture.
