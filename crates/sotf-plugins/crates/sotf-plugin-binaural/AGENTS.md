# sotf-plugin-binaural

HRTF-based binaural decoder plugin rendering multichannel surround to headphone-compatible stereo using overlap-add convolution.

## Architecture

```
src/
  lib.rs          -- BinauralDecoderPlugin (Plugin), BinauralState, main processing
  config.rs       -- BinauralDecoderParams, configuration types
  error.rs        -- BinauralError type
  filter.rs       -- Filter utilities (diffuse field EQ, LFE lowpass)
  hrtf.rs         -- HRTF loading and interpolation from SOFA files
  hrtf_database.rs -- HRTF database management and selection
  room.rs         -- Room model with reflections (RoomModel, Reflection, ReflectionHrtf)
  params.rs       -- Centralized parameter specs
```

**Algorithm pipeline:**
1. Multichannel input split into zero-padded hop partitions
2. Forward FFT per active input channel
3. Per-channel HRTF convolution in frequency domain (complex multiply-accumulate)
4. LFE channels processed through lowpass filter and mixed at configurable gain
5. Optional diffuse field equalization
6. Optional source-owned broadband-ILD room reflections via image source method
7. Sum left/right HRTF outputs across all channels
8. Inverse FFT + overlap-add
9. Output as stereo (2 channels)

**Key types:**

- `BinauralDecoderPlugin` -- Main plugin implementing `Plugin`. N input channels -> 2 output channels.
- `BinauralState` -- Immutable frequency-domain HRTF filters and diffuse field EQ; published through a prepared shared mutex with callback `try_lock` and an audio-owned snapshot.
- `BinauralDecoderParams` -- Config: HRTF path, speaker config, externalization, near-field, room model.
- `RoomModel` -- Room reflections using image source method with per-reflection HRTF.

## Key Public API

- `BinauralDecoderPlugin::new(input_channels, params) -> Result<Self, BinauralError>` (`lib.rs`)
- `BinauralDecoderPlugin::from_params(params) -> Result<Self, BinauralError>` (`config.rs`)
- Implements `Plugin` trait: N input channels -> 2 output channels
- `RoomModel`, `Reflection`, `ReflectionHrtf` re-exported for room simulation

**Parameters:** `externalization` (0-1, controls HRTF strength), `near_field_strength`, HRTF path, speaker config, room model settings.

## Testing

```bash
cargo test -p sotf-plugin-binaural
```

Benchmarks:
```bash
cargo bench -p sotf-plugin-binaural --bench binaural-decoder-benchmark
```

## Important Notes

- HRTF data loaded from SOFA files via `SofaFile` from sotf-host. The `hrtf_database.rs` module manages a database of available HRTFs.
- Speaker configuration is auto-detected from input channel count. HRTF filters are selected based on speaker positions from `SpeakerConfig`.
- HRTF publication never waits on the audio thread. Construction prepares native mutex resources; contention retains the current snapshot. Interrupted and completed fades transfer ownership to the background reclaimer; full retirement queues postpone adoption without destroying filters in the callback.
- Output accumulator uses a power-of-2 ring buffer with bitmask wrapping (`output_accumulator_mask`).
- LFE channels are identified from the speaker config, processed through a lowpass filter, and mixed into both L/R outputs.
- SIMD operations: `complex_mul_add_simd` for HRTF convolution, `window_mul_simd` for analysis window application.
- Room reflections retain input/source ownership and use HRTF-derived broadband
  ILD. Full reflection ITD/pinna convolution is not claimed.
- Optional RTPGHI (Real-Time Phase Gradient Heap Integration) processor from `math-dsp` for phase reconstruction.
- The `sofa-reader` dev dependency is used only in benchmarks.

## Head-tracking clock

- Advance head smoothers from accepted source samples; send requests at completed source-hop boundaries, preserving partial-hop automation timing.
- Empty callbacks and synthetic EOF input must not advance orientation. Reset discards the pending input-hop phase.
- `src/lib/head_clock_tests.rs` uses a deterministic request sink. It proves scheduling partition invariance, not asynchronous worker latency or stale-result rejection.

## Asynchronous worker epochs

- The atomic mailbox carries the latest pose; the bounded tuple channel supplies wakeups only. A full wakeup channel still accepts the latest desired pose.
- Publish completed generations monotonically within the current epoch. New desired poses do not cancel every in-flight computation, which would starve continuous motion.
- Reset and resource/worker changes invalidate epochs. Reject old epochs at both publication and audio adoption. Workers own immutable SOFA/config snapshots.
- EOF retains the currently audible filter transition and excludes later publications. Process/drain clock mismatches must not mutate audio, controls or the drain budget.
- `head_worker_tests.rs` has deterministic gated-worker tests; actual head-turn audio/latency remains separate validation.

## Angular and spatial-cue invariants

- Use `AngularSmoother` for head components and `angular_delta` for request thresholds. Keep canonical requested targets while smoothing the short component arc; do not replace it with raw scalar-angle interpolation.
- Synchronous HRTF preparation must use the current smoothed pose, matching background preparation. The physical basis is forward/left/up, with positive pitch up and positive roll raising the left ear.
- `head_cue_tests.rs` uses an independent head basis and known real-SOFA impulse cues. Held-pose tests settle a supported 10 ms crossfade before measuring transfer; they do not validate continuous-motion latency or cutoff audibility.

## Retargeting fades

- Interrupted linear fades use the preallocated effective previous transfer, with diffuse-field EQ already included. Do not apply the previous owner's EQ a second time.
- Prepare a candidate transfer before retirement; commit it only after retirement succeeds. Backpressure must preserve the audible transfer and owners.
- Spectral fades coalesce pending publications until completion to preserve nonlinear phase history. Include the remaining fade duration when assessing tracking latency.

## Dataset storage

- Retain shared immutable `Arc<SofaFile>` ownership across completed poses; never deep-clone the measurement dataset per pose.
- Validate prepared-cache storage before reload/initialize commits. The 256 MiB cache-payload limit is separate from source IR storage and total process memory.
- Source HRIR length does not prove interpolated support. The non-grid support diagnostic records an open aliasing defect; held-pose tests do not close it.
