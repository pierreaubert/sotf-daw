# sotf-plugin-de-esser

De-esser — sibilance reduction for audio.

## What It Does

Reduces harsh sibilant sounds ("s", "sh", "ch") in vocals and other audio. Applies frequency-selective dynamic compression to the sibilance range (typically 4-10 kHz) without affecting the rest of the spectrum.

## Features

- **Frequency-selective processing**: Targets only the sibilance band
- **Dynamic compression**: Reduces sibilance only when it exceeds the threshold
- **Range control**: Limits attenuation to 0–60 dB
- **Stereo linking**: Blends independent channel reduction with a common gain
- **Transparent operation**: Preserves the natural character of the audio

## DSP contract

- `Wideband` applies detector gain reduction to the complete signal.
- `Split-Band` uses an LR4 crossover and applies reduction only to its high
  output. Mix controls reduction depth against the phase-matched low+high sum;
  when gain reduction is zero, every Mix value produces identical output.
- Range bounds the gain reduction envelope in dB. Zero disables reduction;
  in Split-Band mode the crossover's all-pass phase response remains active.
- Stereo Link interpolates smoothed reduction in dB toward the strongest
  channel. At 100%, every channel receives the same reduction. In multichannel
  instances, linking spans all channels. At 0%, detectors act independently.
- Range and Stereo Link automation use 5 ms smoothing per sample. During a
  range change, the reduction limit follows that smoothed control value.
- Older presets default to 60 dB Range and 0% Stereo Link. Existing parameter
  indices are preserved; the two controls are appended at indices 8 and 9.
- Frequency is the detector center. Q defines a symmetric octave bandwidth
  once; the highpass and lowpass edge sections use fixed Butterworth pole Q.
- Frequency, Q, and Mode are structural controls. Hosts rebuild the plugin when
  they change; Threshold, Ratio, Attack, Release, Mix, Range, and Stereo Link
  are realtime-safe.
- Processing has no algorithmic latency, overwrites exactly the active frames,
  sanitizes non-finite input, and allocates nothing after initialization.
- Gain-reduction meters publish from elapsed samples at approximately 30 Hz.

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
