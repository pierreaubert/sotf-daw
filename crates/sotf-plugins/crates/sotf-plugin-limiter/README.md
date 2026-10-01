# sotf-plugin-limiter

SOTF Limiter plugin for peak limiting.

Prevents audio from exceeding a set ceiling by applying fast gain reduction to peaks, protecting downstream equipment and preventing clipping.

## Finite streams

After the final input block, call `drain()` until it reports completion. At the default 1x setting, the
limiter emits exactly its active lookahead delay plus the ISP output delay
(`3 × detector delay` when ISP is enabled), retaining the final program samples.
Detector history and release time do not add audio support. `tail_length()`
reports this finite bound in output-rate frames after initialization and
`Unknown` before initialization; latency and ordinary processing are unchanged.

Each drain call emits at most 256 frames and accepts any nonempty destination
containing whole channel frames. The output slice determines capacity;
`context.num_frames` is ignored for drain. The sample rate must match
initialization. Invalid rate, shape, or capacity leaves stream history and EOS
state unchanged. Empty-stream drain is a no-op, and a zero-delay stream completes
without emitting samples.

Once valid drain begins for a nonempty stream, reset or reinitialize before new
input or parameter changes. Resending identical parameter values is allowed. Existing structural-parameter
rebuild requirements still apply.
Existing envelopes and smoothers continue under zero input with their accepted
targets; draining does not freeze gain reduction or invent new program samples.
Ordinary hosts that keep calling `process()` with silence retain their existing
parameter behavior. Process, drain, scalar tail queries, and reset do not allocate
or free memory on valid calls.

## Prepared audio oversampling

`oversampling` is a structural choice index: **0 = 1x (default), 1 = 2x,
2 = 4x**. Old presets and the positional constructor retain the exact native
1x path. Select the factor before initialization; changing it afterward needs
a graph rebuild. The 2x/4x paths support 1–32 channels and prepare all storage
on initialization. The limiter handles its own oversampling, so hosts must not
add another preferred-oversampling wrapper.

The prepared wet path upsamples, limits at the higher rate, downsamples, and
applies a native-rate final peak guard. This final guard is required because
resampling can create new output peaks. Dry audio is delayed by the full reported
latency and mixed once after protection. The ceiling guarantee applies to fully
wet output; dry mix can exceed it. ISP mode still requires hard mode, 100% wet,
and adequate input lookahead. It uses the existing finite Hann-sinc reconstruction
criterion; this is not a universal guarantee for every reconstruction filter.

Let `L = floor(lookahead_ms * 0.001 * native_rate)` using the existing f32
quantization. The high-rate core uses exactly `factor * L` frames. Current
prepared latency is **512 + L** native frames in sample mode and
**512 + L + 4D** in ISP mode, where `D` is 6 below 96 kHz, 12 below 192 kHz,
and 0 at higher rates. Query `latency_samples()` after initialization. A selected
but uninitialized prepared path reports latency 0 as an unprepared sentinel,
unknown tail, no drain bound, and rejects processing.

Controls accepted before source frame `s` govern the high-rate wet core starting
at processing frame `factor * s`; a fixed 256-frame queue retains those controls
while input is buffered. This is a processing-clock convention, not alignment
to a filtered impulse peak. Final-guard controls and mix use the native output
clock. Transport position changes do not alter these local control clocks.

At EOS, the current control snapshot applies to synthetic padding only; already
accepted source controls keep their original positions. `begin_drain()` is
bounded and idempotent and may process at most two existing FFT chunks. The
remaining finite wet tail, guard delay, and dry delay are delivered by calls of
at most 256 native frames. Zero-frame incomplete progress advances no dry,
guard, or display clock. Call bounds are available after preparation. The
conservative tail bound includes transform support and is longer than latency.
Reset restores current controls and clears all prepared histories. Failed
initialization leaves the previous prepared stream and metadata intact.

### Meter convention

At 2x/4x, gain reduction reports a **conservative indication of actual limiting
gains**, including both finite downsampler contributors, aligned final-guard
gains, and effective dry/wet mix. Contributors can spread one core gain change
over two native 256-frame blocks. Per-channel association is retained before
selecting the maximum reduction over the existing 100 ms display interval.
It is not an output/input amplitude ratio: filter loss or phase cancellation
alone does not activate `is_limiting`, and a fully dry path reports zero limiting.
Zero-input gain labels are unity. Input peak remains on the native input clock;
true-peak telemetry measures the final emitted output when enabled. Native 1x
telemetry is unchanged.

### Output meters

Both native 1x and 2x/4x publish `output_peak_db` (maximum absolute final
sample per 100 ms interval) and per-channel `output_isp_dbtp` (final-output
BS.1770 true peak when true-peak metering is enabled, else -120.0). The legacy
`peak_db`/`isp_dbtp` fields are preserved: native `isp_dbtp` reports input
peaks while the oversampled path reports output peaks there. New consumers
prefer `output_isp_dbtp` for a consistent final-output reading. Output meters
observe emitted audio only; they never change DSP, latency, or existing fields.

Oversampling reduces some nonlinear aliases at additional CPU cost; improvement
is frequency- and mode-dependent. The final protector itself operates at the
native rate, and no uniform rejection or monotonic 2x-to-4x improvement is claimed.
