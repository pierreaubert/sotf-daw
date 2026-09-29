# AUD010: prepared limiter-local audio oversampling

2026-09-28. Implementation checkpoint; final aggregate verification is owned by
root. MIDI/IAMF, host APIs, engine managers, and branch queues are excluded.

## Implemented contracts

The public owner retains every legacy parameter index and native DSP route.
Appended structural index **10**, `oversampling`, has one numeric choice
representation throughout Params, LimiterPluginParams, JSON and scalar access:
**0=1x (default), 1=2x, 2=4x**. The old constructor remains 1x. Changed factors
reject after initialization; same-value resends remain valid. No host-level
preferred oversampling is requested.

New private `oversampled_core.rs` and `oversampled_path.rs` compose:

1. Existing prepared generic FFT oversampling with a private fully wet kernel.
2. Exactly `factor * L` high-rate lookahead, where native `L` uses the unchanged
   f32 millisecond quantization. The core uses input true-peak detection when
   requested or required by ISP; its output ISP stage is disabled.
3. A native-rate final hard protector after downsampling. Sample mode has zero
   guard delay; ISP mode has native detector lookahead D and output delay 3D.
   This guard uses ordinary release, independently of the wet core's optional
   program-dependent release.
4. One aligned original dry signal, mixed once after final protection.

Prepared latency is currently **512 + L + G** native frames, with G=0 in sample
mode and G=4D in ISP (D=6 below 96 kHz, 12 below 192 kHz, and 0 thereafter). Actual wrapper
metadata supplies the resampling part. New paths accept 1–32 channels and checked
oversampled rates. All recoverable preparation errors precede live path/clock/
metadata changes. Selected unprepared paths report latency0 as an unprepared
sentinel, `Unknown` tail, no call bound, structural drain capacity 256, and reject
processing. Prepared paths are FFT compilation boundaries; native metadata is
unchanged.

### Accepted controls and stream clocks

A fixed 256-slot Copy timeline appends snapshots only when source frames are
accepted. A setter before frame s governs the high-rate kernel at processing
frame F*s. Subdivision cannot execute more than one normal core chunk before the
queued controls are consumed. Final protection and mix use the native output
clock. Copied transport metadata advances sample/PPQ offsets without rebuilding
the host musical origin; local accepted/processed clocks remain independent of
transport seeks. There is no source-impulse-peak alignment claim through the FIR.

At EOS, current canonical controls apply starting with the first synthetic
processing frame. Pending real-input snapshots are retained unchanged. Prepared
wrapper setup executes at most two existing chunks; the child starts its own
finite drain afterward. The public path emits finite wet support, then any
remaining guard and dry support. Zero-output progress does not advance those
output clocks. Every generated sample is delivered once. Full-capacity call
bounds include child setup/cache work and conservatively add guard/dry phases;
queries do not mutate state. Invalid rate/shape/capacity is rejected before EOS
or audio mutation. Partial DSP errors require reset. Empty EOS is a no-op;
nonempty EOS freezes controls until reset or successful reinitialization.

New-factor bulk rejection is preflighted before the existing ordered apply loop:
a permanent red showed mix could otherwise change before the structural factor
rejected. Changed initialized factors and invalid type/range now retain controls
and exact continuation. This deliberately does not change legacy-only bulk
restoration semantics.

### Gain telemetry

Native 1x output/telemetry remains exact. New-path private observers record actual
nonzero-sample gain/clamp ratios in f64 before narrowing; zero-input labels are
unity. Each high-rate output unit supplies per-channel minimum gain. The pinned
synchronous downsampler uses only the current unit and previous overlap; its
startup delivery shift is256 native frames. The prepared eight-entry descriptor
ring therefore retains those two contributors without a reference audio branch.

Core contributor labels are delayed by G. Final guard first-stage labels are
delayed by the actual ISP ring J=3D, then multiplied by that channel's current
output-guard label. The outer indicator uses `(1-mix)+mix*gain`; channel association
is preserved before taking the maximum reduction across the existing 100 ms
publication interval. This is a bounded conservative limiting-gain indication,
not an output/input amplitude ratio. Filter loss and phase cancellation alone do
not activate the meter. True-peak telemetry measures final emitted output on the
native clock, while input peak stays associated with accepted native input.

## Independent verification

| Evidence | Matrix and result |
|---|---|
| Native preservation |96 captured configurations ×2 epochs; exact f32 audio, all telemetry fields/cadence, latency, EOS and count hashes remain identical to pre-extraction baseline |
| Output ceilings |840 burst configurations; unchanged sample0.00001 dB and finite independent Hann-sinc ISP0.1 dB bounds; no tolerance relaxed |
| Accepted automation |1,024 factor/mode/event-phase configurations, absolute F*s lookup oracle independent of pending-ring implementation; output-clock guard/mix; EOS residual phases0/1/255; ≤2e−6 full-waveform error |
| Backend support |1,536 high-rate impulse phases injected after upsampling; actual downsampler output has only current/prior-unit support with exact later zeros |
| Contributor capacity and quotas |1,536 factor/phase/lookahead configurations incl20 ms at 192 kHz; observed descriptor high-water3/8, maximum full-capacity bound/actual calls2.200 |
| Exact dry clock |2,048 factor/rate/residual-phase configurations with irregular input/drain partitions; original samples appear exactly at declared integer delay |
| Nonlinear EOS |128 factor/rate/channel/mode/phase configurations; tail is bit-identical across drain capacities, explicit frozen zero continuation differs≤2e−6; conservative tail bounds hold |
| Ordinary callback/reset parity |48 nonlinear configurations with whole/1/127/257/mixed9217 callbacks, bit-identical output;108 impulse configurations ×2 reset/partition epochs;24 independent final dry/half-wet mixes |
| Lifecycle |Malformed/zero capacity, wrong rate, failed reinit before and during partial drain, same-value writes, rejected changed writes/new input, changed-control reset with pending snapshots, fresh48→192k reinit equivalence |
| Heap ownership |512 cold prepared cases, every phase and per-frame changed controls;1/2/6/32channels,48/192 kHz and sample/ISP; process, begin, partial/final drain, queries and reset measured0 allocations and0 frees. Eight representative cases also cross first ordinary and next EOS telemetry publications inside measurement |
| Private gain math |Core contributor support, scalar extreme finite ratio/silence labels, exact transport offsets, p/r delay coincidence and separate channels, G delay/mix indication, phase-cancellation negative control |

Independent public telemetry adds 512 residual-phase configurations × 2 reset
epochs, inspecting 2,048 publication checkpoints through live mix and EOS. Twelve
frequency/mix/factor cases measure actual filter attenuation independently;
0.49 × rate loses 106.381037–106.381274 dB while gain reduction stays exactly zero.
Ninety-six positive DC/reset cases retain the original **0.01 dB** tolerance;
maximum residual error is **0.006903506 dB** after accounting independently for
the finite Hann-sinc detector's DC phase maximum. The initial ideal T/A trial's
0.0143956 dB deviation was a missing 0.0078527 dB detector calibration in that
test expectation; no DSP change or tolerance relaxation was needed.

See the [independent public telemetry review](limiter-oversampling-telemetry-review.md),
[accuracy matrix](limiter-oversampling-accuracy.md), and
[native/factory propagation report](limiter-oversampling-propagation.md).

### Alias limits

The independent coherent24-render matrix retains >10dB (2x) and >20dB (4x)
third-harmonic folded-bin improvement for sample-mode11/17 kHz fixtures. It does
not assert universal or monotonic improvement. For example sample17kHz is
−56.951/−94.802/−83.396dBc at1x/2x/4x, and ISP7kHz is
−127.546/−102.794/−96.017dBc. The native final protector itself can generate
nonlinear products. Full values and methodology are in the independent accuracy
report; no proprietary equivalence or universal reconstruction guarantee is made.

## Measured release CPU cost

Isolated public API probe `/tmp/sotf-limiter-phase2-cpu.rs`, existing release rlibs,
48 kHz stereo,1 ms lookahead,2 seconds of deterministic signal per trial, five
trials/median. Fixed controls use256-frame callbacks. Maximum-density cases
change threshold/release/link at every accepted native frame with1-frame calls.
Each value is elapsed/audio duration as a percentage of one core on this machine.

| Mode | Controls |1x|2x|4x|
|---|---|---:|---:|---:|
| Sample | Fixed |0.284%|1.466%|2.225%|
| Sample | Every frame |0.957%|2.613%|3.435%|
| ISP | Fixed |0.692%|2.497%|2.820%|
| ISP | Every frame |1.472%|3.901%|4.234%|

These are local measurements, not portable deadline guarantees. Compared with
native, the new path costs extra FFT work, a second guard, aligned telemetry and
prepared control scheduling. Construction allocates the fixed control/descriptor
storage and rate/channel-dependent DSP rings; callbacks do not grow or free it.

## Commands and artifacts

Final source is frozen. **150 tests passed**, none failed or ignored, under
`cargo test -p sotf-plugin-limiter --all-features`. Strict
`cargo clippy -p sotf-plugin-limiter --all-targets --all-features -- -D warnings`
passed. Release accuracy/absolute-automation oracles passed **8/8**, preserving
all numerical bounds under release optimization. Existing release `qa-limiter`
passes **4/4**. Formatting and scoped diff checks are clean.

The final full count includes all three independent public telemetry tests,
the latest typed/range/changed-factor bulk rejection regression, and changed-
control reset plus partial-drain reinitialization checks.

The final integration gate added one constructor wire-form regression and the
shared choice deserializer, bringing limiter coverage to **151 tests**. Valid
labels normalize to the existing integer serialization; no DSP changed.
Strict limiter lint passes after this addition. The complete workspace passes
**5,841 tests across 324 binaries**, with 10 skipped and MIDI/IAMF excluded.
See [engine and toolbar wiring](limiter-oversampling-wiring.md) and the fourteenth
checkpoint in `AUDIT.md` for the two integration failures found and corrected.

- `/tmp/sotf-limiter-oversampling-native.log`
- `/tmp/sotf-limiter-oversampling-full-final.log`
- `/tmp/sotf-limiter-oversampling-automation.log`
- `/tmp/sotf-limiter-oversampling-stream.log`
- `/tmp/sotf-limiter-oversampling-bound.log`
- `/tmp/sotf-limiter-oversampling-telemetry.log`
- `/tmp/sotf-limiter-oversampling-bulk-red.log`
- `/tmp/sotf-limiter-oversampling-cold-bulk-final.log`
- `/tmp/sotf-limiter-oversampling-reset-final.log`
- `/tmp/sotf-limiter-oversampling-clippy-final.log`
- `/tmp/sotf-limiter-oversampling-release-oracles.log`
- `/tmp/sotf-limiter-oversampling-release-qa.log`
- `/tmp/sotf-limiter-phase2-cpu.log`
- `audit/limiter-oversampling-accuracy.md`
- `audit/limiter-oversampling-telemetry-review.md`
- `audit/limiter-oversampling-propagation.md`

Owned changes are limiter production/types/params/tests/docs and the accepted
integration/telemetry proposals. Root and the integration agent separately own
engine/bridge/FFI/NIH propagation; no shared host changes were needed. Source
history and original prototype reports remain intact.
