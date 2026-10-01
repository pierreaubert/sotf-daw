# sotf-plugin-speech-denoiser

SOTF speech denoiser plugin — RNNoise voice denoising.

Wraps the `RnnoiseBackend` block (which itself wraps `nnnoiseless`) from `plugins-denoiser` in the SOTF host plugin trait.

The model runs at 48 kHz and uses 480-sample internal quanta, but the plugin
accepts arbitrary host callback sizes and reports a constant 960-frame (20 ms)
latency: 480 frames from model analysis/synthesis and 480 from the streaming
queue. The first model output frame is preserved; it can contain pre-ringing,
so only the initial 480-frame queue is guaranteed to be silent. Every successful
callback returns the requested frame count, including startup and partial model
frames.

Only mono and stereo layouts are supported. Stereo forms one polarity-aware,
energy-normalized detector signal, then applies the model's same 22 smoothed,
bounded spectral suppression gains to both original channels. This avoids
anti-phase cancellation and independent channel decisions while preserving the
original stereo relationships. Wider layouts are rejected rather than
receiving undefined independent spatial processing.

Bypass keeps the same 960-frame latency, continues advancing the neural model,
and crossfades between delayed wet and dry paths over 480 samples. Non-finite
input is replaced with silence and finite input is clamped to the model domain
`[-1, 1]` before persistent state is updated.

Enable automation selects delayed audio on the output clock. Empty callbacks
do not latch an initial bypass state or advance a fade. Reset clears audio/model
history while retaining the selected enabled setting.

The corrected latency replaces the earlier 480-frame report. Enabled wet audio
is unchanged; dry audio is delayed another 480 frames to align it with wet, and
bypass transition waveforms change accordingly. Existing sessions may realign
relative to other tracks when the host recalculates latency compensation.
When disabled at EOF, native drain returns exactly 960 continuation frames for
nonempty input, including any unfinished bypass fade and the complete delayed
dry signal. Each call returns at most 480 frames and leaves unused output space
untouched. The first valid nonempty drain freezes input and changed `enabled`
controls until reset; same-value snapshots remain accepted. Empty streams do
not freeze. Drain, metadata queries and reset allocate and free no memory.

At enabled EOF, native drain emits the same 960 frames that ordinary processing
would produce from 960 zero input frames. This releases the final accepted
partial model block and fixed processing queue. The plugin then resets the
backend and discards any remaining recursive model/high-pass response. Enabled
`TailLength` remains `Unknown`: this is an explicit render cutoff, not a claim
that the natural response ends after 960 frames. The last published analyzer
snapshot is retained. After completion, new input and changed `enabled` values
are rejected until reset or successful reinitialization; empty EOF remains
unfrozen.

The disabled tail is finite because the output selects pure dry before the
960-frame continuation ends; the neural model remains warm internally until
reset. Both enabled and disabled drain have a maximum 480-frame output call.

`get_data()` exposes a fixed-size `SpeechDenoiserData` snapshot containing the
22 model gains after RNNoise's inter-frame release smoothing, the bounded VAD
probability, and a completed-model-frame generation. The realtime cache is
preallocated and skips a publication under prolonged reader contention rather
than allocating on the audio thread. These values are model decisions, not
quality scores or gains inferred from broadband output/input RMS.

RNNoise FFT plans/tables and all large FFT, pitch, feature, synthesis, and RNN
workspaces are prepared or allocated during initialization. Processing, reset,
and live `enabled` changes do not allocate. The Audio Unit rejects sample rates
other than 48 kHz and layouts other than matching mono or stereo during format
negotiation.

Suppression `strength` (0..1, default 1.0) blends latency-aligned wet and dry
audio per sample: `out = dry + s * (wet - dry)`, where `dry` is the sanitized
input delayed by exactly the 960-frame signal latency. Strength slews toward
its target over 480 frames (10 ms), matching the bypass crossfade; the 0.0
and 1.0 endpoints emit dry and wet bit-exactly, so default and disabled audio
are unchanged and latency stays a constant 960 frames. The blend applies only
when enabled; the disabled path replays backend audio exactly as before.
Strength automation is realtime-safe and allocation-free; rejected values
(NaN, infinite, out of range, wrong type) retain the accepted target and
audio history.

`model` names the bundled inference model (`RNNoise Full`, index 0). It is a
structural parameter: unknown identities are rejected transactionally with
the running model continuing unchanged, same-value writes are no-ops, and a
changed identity on a live instance requires a host graph rebuild from
serialized configuration so weight preparation never runs on the audio
thread. The registry is append-only; future models add labels without
renumbering index 0.

Saved state is schema v2. V1 state carrying only `enabled` loads with
strength 1.0 and the bundled model, reproducing v1 audio bit-exactly.
Unknown fields and malformed values are still rejected. Strength and model
changes freeze once nonempty EOF drain work begins, exactly like `enabled`;
same-value snapshots remain accepted until reset or reinitialization.
