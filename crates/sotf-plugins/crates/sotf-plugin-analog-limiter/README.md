# sotf-plugin-analog-limiter

Analog limiter — a mastering limiter with an analog coloration stage.

## What It Does

The proven limiter core (ceiling, release, lookahead, soft knee, true peak,
mix) runs first; the output then passes through one of six `math-analog`
coloration models. With color at 0% it behaves exactly like the clean
limiter for signals below the ceiling (hot signals agree within the
dB-conversion epsilon; see contract); raising drive and color adds
tape/console-style finishing character staged at 0 VU = −18 dBFS.

## Features

- **Full limiter core**: no fork — composition over `sotf-plugin-limiter`
- **Opinionated subset**: threshold, release, lookahead, soft knee, true
  peak, mix; advanced detector options stay at core defaults
- **Six analog models**: fail-closed selection, shared drive/color/character/trim
- **Core lookahead latency reported**; color and the final sample clamp add none

## Consumer notes

- `analog_model` is structural: apply a model change by reconstructing the
  plugin or restoring a saved snapshot. Live `set_parameter` on an
  initialized instance refuses and keeps prior state and history.
- FFI callers enumerate the six models by 0-based index (`analog_model`
  scalar range 0–5); the family exposes no per-model label strings.
- Primitive scalar validation/setters reuse cached storage. Valid numeric/Boolean
  bulk updates and canonical same-model snapshots require no new allocation;
  consuming a caller-owned map can free its storage. Model replacement and owned
  metadata queries remain control-thread operations.
- Batch validation includes live structural restrictions before changing color.
  A rejected lookahead/model/bad-value batch preserves controls, audio history
  and the proved zero-color finite-tail epoch.
- Meter snapshots come from the wrapped core before coloration and the final
  sample clamp. Use the final clean ISP stage's output meters for the protected
  chain's emitted levels.

## Output ceiling contract

`threshold` is the final emitted sample-peak ceiling when `mix` is fully wet
(100%): after the limiter core and the analog color stage, a zero-latency
per-channel clamp limits emitted samples to the threshold. Analog drive, color,
character, and trim cannot push fully wet output past the ceiling.

- A dry blend (`mix` below 100%) can exceed the ceiling, exactly like the clean
  limiter core.
- `true_peak` enables ITU-R BS.1770-compatible inter-sample peak detection in
  the core detector. It is detection only: with no output ISP correction
  stage, no strict output true-peak guarantee is claimed before or after
  color.
- Signals already below the ceiling pass the guard bit-exactly (it never
  writes in-range samples, so it cannot blanket-attenuate). With color at 0%,
  below-ceiling output is bit-identical to the clean limiter; hot signals
  agree within the dB-conversion epsilon (2e-6 relative), the only divergence
  between the precise guard ceiling and the core's fast clamp ceiling.
- Automation: the guard tracks the threshold/mix targets immediately, while
  the core detector threshold and blend smooth one-pole over 5 ms. A downward
  threshold step therefore flat-tops at the new target from the first sample
  (stricter than the core-only transient, which the core automation contract
  allows to exceed it briefly), and a fade to fully wet clamps dry overshoot
  during the transition.
- Linking applies to the core detector gain (pinned at 100%, shared across
  channels). The color stage is per-channel, so colored lanes may diverge
  across channels by design while sharing one gain history.

## Protected render chain

Near-Nyquist color automation can produce reconstructed peaks more than 5 dB
above the sample ceiling in the stress fixtures. A clamp alone does not establish
an output true-peak bound. Oversampling can reduce nonlinear aliases, but the
downsampler can create new native-rate peaks after the high-rate clamp.

Use a final clean Limiter with ISP mode **after** coloration and downsampling.
The tested library chain is:

```rust
use sotf_host::oversampling::AutoOversampledPlugin;
use sotf_plugins::{DawHost, create_plugin};

fn protected_chain(rate: u32) -> Result<DawHost, String> {
    let mut host = DawHost::new(2, rate);
    host.set_max_block_frames(4096)?;
    let color = create_plugin("analog_limiter", &serde_json::json!({
        "threshold": -12.0, "release": 10.0, "lookahead": 0.37,
        "true_peak": true, "analog_model": "Harmonics",
        "analog_drive": 24.0, "analog_color": 1.0,
        "analog_character": 0.7, "analog_trim": 6.0
    }), 2, rate)?;
    host.add_plugin(Box::new(
        AutoOversampledPlugin::new_with_max_frames(color, 4, 4096)?
    ))?;
    host.add_plugin(create_plugin("limiter", &serde_json::json!({
        "threshold_db": -12.0, "release_ms": 10.0, "lookahead_ms": 0.37,
        "isp_mode": true, "true_peak": true, "soft": false,
        "mix": 1.0, "oversampling": 0
    }), 2, rate)?)?;
    host.build()?;
    Ok(host)
}
```

The final ISP stage owns a real output correction delay and protects the complete
emitted waveform using the clean limiter's finite Hann-sinc criterion: a 4x grid
below 96 kHz, 2x below 192 kHz, and sample peaks at 192 kHz and above. It requires
hard/full-wet operation and adequate lookahead; its threshold edits use 5 ms
smoothing. Query the complete host latency after building. Keep any further
gain, effects, mixing or conversion before this final protection stage.
The Analog Limiter's True Peak control retains its input-detection meaning.

## Finite streams

Call `initialize` before processing and repeatedly call `drain` after the last
input block. Drain writes only its returned output frames, up to 256 per call.
Invalid rate, incomplete channel frames or insufficient capacity for a pending
tail leave drain history untouched and can be retried. A supported nonempty EOS
freezes controls and new input until `reset` or reinitialization; identical
parameter snapshots remain accepted. Empty-stream drain is a no-op. Reset
retains parameter targets and clears EOS and audio history.

Exact finite drain is supported when analog color has remained zero throughout
the epoch established by initialization/reset. Core drain output passes through
the same prepared color stage, and the finite bound is the core lookahead.
Input true-peak detection does not add a post-color ISP output delay.
Any nonzero color target invalidates this proof until reset/reinitialization,
even if the target is changed back to zero without processing. A zero target
alone cannot prove that a previous color fade has settled. Replacing a model
preserves a proved zero-color epoch and the core audio history.

Color models contain recursive filters or nonlinear history. Outside a proved
zero-color epoch, the response is `TailLength::Infinite` and legacy zero-frame
COMPLETE drain behavior remains; automatic recursive completion is unresolved (AUD073).
The change does not recover recursive colored tails. Callers needing a chosen
tail duration can continue ordinary zero-input processing before EOS.

`drain_output_frames_max` is a per-call capacity; `tail_length` is a bound from
the last input and does not count down. Queries are allocation-free. The generic
in-place adapter can copy input to output before an inner `process` error;
processing errors do not promise destination preservation. Drain capacity
errors and direct/compiled EOS preflight retain their transactional guarantees.

The protected chain does not convert recursive coloration into exact finite
support. Choose an explicit zero-input render duration before ending colored
renders, and evaluate the cutoff for the chosen model/material. Tests use a
declared 1024-frame continuation to exercise the cutoff and resampler/final-ISP
flush; that duration is a test budget, not a recommendation or a decay guarantee.
`COMPLETE` from the legacy colored drain must not be read as proof of silence.

## Audit validation

```bash
cargo test -p sotf-plugin-analog-limiter
cargo test -p sotf-plugins --test audit_analog_limiter_host
```

An independent centered f64 sinc oracle characterizes 360 near-Nyquist color/
random-phase cases at 44.1/48/96/192 kHz, checking the exact sample ceiling and
the reconstruction operator's derived L1 bound. This characterization deliberately
demonstrates reconstructed overshoot; it does not turn the native clamp into ISP
protection. Actual protected host chains cover all six models, native/2x/4x
processing, live color/drive changes, callback partitions and EOF. Final samples
meet the -12 dBFS fixture ceiling within 0.00001 dB; the rate-dependent ISP
criterion meets it within 0.1 dB. No arbitrary DAC/ideal-sinc guarantee is claimed.

Zero-color host EOF is compared with a fresh host's ordinary zero continuation,
including reset and finite/infinite metadata transitions. Native impulse delay
is exact; projected fractional oversampling delay rounds within one native frame.
Queued scalar controls are allocation/deallocation guarded in the actual host
callback. The existing native ceiling/audio contract and parameter wire fields
are preserved; no measured-hardware model validation is inferred.
