# sotf-plugin-denoiser

SOTF broadband spectral denoiser using IMCRA/MCRA noise estimation and Wiener filtering.

It provides 2048-point quality and 512-point low-latency STFT modes, decision-directed SNR,
captured noise profiles, psychoacoustic masking, spectral subtraction, formant and transient
protection, an editable three-knot frequency-dependent reduction curve, aligned residual
audition with click-free switching, and optional dual-resolution analysis. Reported latency
is one FFT (2048 or 512 samples); processing is preallocated and allocation-free after
construction.

Spatial mode processes coherent channel pairs. Stereo and 3.0 use the front L/R pair. Standard
5.1 adds side L/R, and standard 7.1 adds side and rear L/R. Centre, LFE, and any unmatched channel
remain on the ordinary per-channel denoising path.

Serialized percentage-like values are normalized fractions (`0.70` means 70%); UI scaling is
presentation only. Noise-profile learning lasts approximately one second at every sample rate and
in both FFT modes.

See `USAGE.md` for parameters and examples and `UI.md` for the compiled layout contract.

## Finite streams and startup

With FFT size `N` and hop `H=N/2`, startup includes the zero-padded window at
origin `-H`. Its negative synthesis prefix is discarded, and public latency
remains `N` frames. At unity gain, every source sample is reconstructed, including
the first sample. Note detection consumes source slices through each large FFT
boundary, so its decisions are independent of callback size. The optional small
FFT computes gains and does not add a separate audio delay or tail.

For `T>0` source frames, `drain` emits
`2N + floor((T-1)/H)*H - T` frames of zero-input continuation. Native tail metadata
reports the phase-independent maximum `2N-1`. Positive channel-aligned drain
capacities of any size are accepted; a prepared one-hop cache preserves internal
processing order and leaves unused destination samples untouched. Empty streams
complete without output.

The first successful drain closes the stream. Nonempty input and parameter
changes then require reset or reinitialization. Invalid rates/capacities consume
no history, completion is stable, and reset/reinitialization restores the startup
scheduler. Process contexts must match the initialized sample rate.

Explicit noise-profile capture is frozen at the accepted end of the stream:
padding cannot advance its count, finish capture, or replace the stored profile.
Ordinary adaptive noise and gain updates continue to render retained audio. A
partial capture keeps its owned storage and active flag until reset; reset clears
that incomplete capture while preserving the existing profile and its settings.
