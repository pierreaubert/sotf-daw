# sotf-plugin-de-esser

De-esser — sibilance reduction for audio.

## What It Does

Reduces harsh sibilant sounds ("s", "sh", "ch") in vocals and other audio. Applies frequency-selective dynamic compression to the sibilance range (typically 4-10 kHz) without affecting the rest of the spectrum.

## Features

- **Frequency-selective processing**: Targets only the sibilance band
- **Dynamic compression**: Reduces sibilance only when it exceeds the threshold
- **Range control**: Limits attenuation to 0–60 dB
- **Stereo linking**: Blends independent channel reduction with a common gain
- **Lookahead**: Delays program audio so reduction anticipates sibilance
- **Linear-phase split**: Optional FIR split bank with explicit latency
- **Mid/Side processing**: Reduces the sum/difference pair on stereo instances
- **External sidechain**: Detects from a key bus instead of the program input
- **Transparent operation**: Preserves the natural character of the audio

## DSP contract

- `Wideband` applies detector gain reduction to the complete signal.
- `Split-Band` uses an LR4 crossover and applies reduction only to its high
  output. Mix controls reduction depth against the phase-matched low+high sum;
  when gain reduction is zero, every Mix value produces identical output.
- `Split Topology` selects the split-band bank: `Minimum-Phase` (LR4,
  zero added latency) or `Linear-Phase` (1025-tap symmetric FIR, 512 samples
  group delay). The control is inert in wideband mode. Mix keeps its
  reduction-depth law against the delayed low+high reference in both banks.
- Range bounds the gain reduction envelope in dB. Zero disables reduction;
  in Split-Band mode the crossover's all-pass phase response remains active.
- Stereo Link interpolates smoothed reduction in dB toward the strongest
  channel. At 100%, every channel receives the same reduction. In multichannel
  instances, linking spans all channels. At 0%, detectors act independently.
  In M/S mode the two linked channels are Mid and Side.
- Range and Stereo Link automation use 5 ms smoothing per sample. During a
  range change, the reduction limit follows that smoothed control value.
- `Lookahead` (0–20 ms, structural) delays program audio while the detector
  runs on undelayed input, so reduction anticipates sibilant onsets. The dry
  path is taken after the delay: Mix 0 reproduces the delayed input exactly.
  Reported latency is the lookahead delay plus the FIR group delay (512
  samples when the linear-phase bank is active, else 0).
- End-of-stream drain emits exactly the retained frames (lookahead delay +
  full FIR support of 1024 frames when the linear-phase bank is active) and
  then completes. Residual LR4/detector/envelope tails are cut at completion;
  `tail_length` is finite except for minimum-phase split mode. Input or
  parameter changes after drain require `reset()`.
- `M/S Mode` (realtime) encodes stereo L/R to Mid/Side before detection and
  processing and decodes after. Other channel counts process discrete
  channels even when enabled. Encode is `(L+R)/2`, `(L-R)/2`; decode is
  `M+S`, `M-S`. Toggling M/S live carries detector/crossover/envelope state
  across domains (multiband precedent), so expect a short transient; hosts
  that need click-free switching should rebuild or crossfade.
- `Ext Sidechain` (structural) doubles the input width: each frame carries
  program channels followed by key channels, and the detector reads the key
  bus only. The external key width must equal the program width
  (no mono-key-into-stereo); a short buffer (missing key) is rejected
  transactionally with no fallback to internal detection. Program writes
  never touch the key region, but non-finite/denormal sanitization may
  normalize key samples. In M/S mode the key follows the same Mid/Side
  domain as the program.
- There is no bypass parameter; bypass is host-level (route around or
  crossfade the plugin). Mix 0 is not a bypass: it reproduces the delayed
  dry path including split-band phase response and reported latency.
- Older presets default to 60 dB Range, 0% Stereo Link, 0 ms Lookahead,
  Minimum-Phase split, L/R processing and internal detection. Existing
  parameter indices 0–9 are preserved; the four audit controls are appended
  at indices 10–13.
- Frequency is the detector center. Q defines a symmetric octave bandwidth
  once; the highpass and lowpass edge sections use fixed Butterworth pole Q.
- Frequency, Q, Mode, Lookahead, Split Topology and Ext Sidechain are
  structural controls. Hosts rebuild the plugin when they change; Threshold,
  Ratio, Attack, Release, Mix, Range, Stereo Link and M/S Mode are
  realtime-safe.
- Processing overwrites exactly the active frames, sanitizes non-finite
  input, and allocates nothing after initialization. Output frame count
  always equals input frame count; latency is compensated by the host from
  the reported value plus the drained tail.
- Gain-reduction meters publish from elapsed samples at approximately 30 Hz.
  In M/S mode the meters report Mid/Side-channel reduction.

## Architecture

```
src/
├── lib.rs                  # crate surface
├── params.rs               # canonical parameter specs and UI layout
└── lib/
    ├── de_esser_plugin.rs  # detector, dynamics, split/wide processing
    ├── de_esser_data.rs    # realtime monitoring snapshot
    ├── types.rs            # strict serialized state
    ├── consts.rs           # DSP constants
    └── tests.rs            # focused DSP/host regressions
```

## Testing

```bash
cargo test -p sotf-plugin-de-esser
cargo run -p sotf-plugin-de-esser --features qa --bin qa-de-esser
```

## License

Part of the SOTF (Sound of the Future) project.
