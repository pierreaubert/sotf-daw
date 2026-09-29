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
the supported FFT range). The raw filter and limiter equations are unchanged.

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
