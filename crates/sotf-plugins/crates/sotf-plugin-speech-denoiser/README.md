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
untouched. The first valid finite drain freezes input and changed `enabled`
controls until reset; same-value snapshots remain accepted. Empty streams do
not freeze. Drain, metadata queries and reset allocate and free no memory.

The disabled tail is finite because the output selects pure dry before the
960-frame continuation ends; the neural model remains warm internally. Enabled
wet audio still reports unknown tail metadata and returns immediate native drain
completion. Its recursive high-pass/model response needs a separate render policy.

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
