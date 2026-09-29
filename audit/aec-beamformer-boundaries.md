# AEC / Beamformer startup and finite-stream audit

Initial read-only investigation, before the authorized implementation below. Date: 2026-09-28. Scope: `sotf-plugin-aec`, `sotf-plugin-beamformer`; no production edits, no MIDI/IAMF, no host graph changes.

## Executed evidence

A standalone Rust probe links the existing built production crates without modifying the repository or running Cargo:

- source: `/tmp/sotf-aec-beamformer-boundary-probe.rs`
- executable: `/tmp/sotf-aec-beamformer-boundary-probe`
- results: `/tmp/sotf-aec-beamformer-boundary-probe.log`
- linked artifacts: `/tmp/sotf-aec-beamformer-probe-linked-artifacts.txt`
- inspected source hashes/timestamps and library timestamps: `/tmp/sotf-aec-beamformer-probe-source-manifest.json`

The probe compiled with `rustc --edition 2024 -O` and exited successfully. Its assertions deliberately establish the existing defects, rather than asserting a proposed correction. Both crate directories remain clean. The TokenSave index was recently updated but rebuilding; live source inspection confirmed the relevant paths. The math-audio index was stale by approximately three hours, so the actual window-generation source was read directly without syncing or modifying that sibling.

### AEC: proven missing final frames; startup identity passes

60 cases: rates44.1/48/96k × input lengths1/255/256/257/997 × callback sizes1/17/256/1000. Reference channel is identically zero, post-filter is disabled, microphone input is a deterministic nonzero sequence. Thus the independent expected output is exactly the microphone sequence delayed256 frames; adaptive foreground coefficients remain zero.

Every case reports `drain_output_frames_max() == 0`, and `drain()` returns `{ frames: 0, complete: true }`. Continuing the same production plugin with256 explicit zero frames recovers all missing delayed samples, bit-exactly. EOS therefore loses the final `min(input_length, 256)` programme frames in this identity configuration; a shorter-than256-frame clip returns only startup silence. Startup itself is correct and callback-partition invariant in these cases. At48k the missing fixed delay is5.333ms.

Source: `src/lib/aec_plugin.rs:300-416` consumes the prefilled output ring before accumulating each256-frame input block; neither drain method is overridden. The shared Plugin default reports immediate completion (`sotf-host/src/plugin.rs`, drain contract/default).

### Beamformer MVDR / Superdirective: proven startup attenuation and missing drain

144 cases: algorithmsMVDR/Superdirective × rates44.1/48/96k × impulse offsets0/1/64/128/255/256/512/1023 × callback sizes1/127/1024. Two identical microphone channels are steered broadside. Both algorithms' distortionless constraint gives the independent identity response, delayed by their declared512 frames.

The expected impulse amplitude is0.5. Actual amplitudes, identical across the checked rates/callbacks:

| Input offset | Emitted amplitude at offset+512 | Expected |
| ---: | ---: | ---: |
| 0 | 0.000000000 | 0.5 |
| 1 | 0.000018820 | 0.5 |
| 64 | 0.073223300 | 0.5 |
| 128 | 0.249999985 | 0.5 |
| 255 | 0.499981165 | 0.5 |
| 256 and later | 0.500000000 | 0.5 |

These values match `0.5 * (0.5 - 0.5*cos(2*pi*n/512))` during the first256 frames. The first input sample is irrecoverably multiplied by zero. Longer zero continuation cannot restore it. At offset128 the onset is attenuated6.02dB; the startup ramp lasts5.333ms at48k, in addition to the declared10.667ms delay.

All cases also report zero drain capacity/immediate COMPLETE, while explicit zero continuation emits the retained late impulse. The broadside identity response needs the final512 delayed frames; general frequency-dependent weights can have a longer frame support.

Source: `src/lib/beamformer_plugin.rs:83-115,290-301,347-431`. Reset initializes `input_fill=0`; the first analysis window covers input0..511 with a periodic sqrt-Hann window. There is no analysis frame starting at-256 to supply the complementary first-half overlap. A scalar normalization cannot recover sample0 because it was never observed through a nonzero window. The window source is `math-audio/crates/math-dsp/src/stft/generate.rs:24`.

Primary reference: SciPy's STFT documentation describes zero boundary extension to reconstruct the first sample when the window begins at zero, and requires nonzero overlap-add coverage and end padding. This is the same boundary condition implicated by the independently measured Hann envelope: https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.stft.html . The probe does not invoke SciPy or reuse the production window generator for its numerical oracle.

### GSC: no broadside startup defect, but steering history is lost at EOS

Three broadside identity controls (callbacks1/17/513) preserve every source sample bit-exactly with zero reported delay. This branch should not inherit the STFT startup correction.

Three endfire cases (44.1/48/96k) choose microphone spacing for a seven-sample compensation delay. A single0.5 sample on the delayed microphone yields no initial output. `drain()` immediately completes with zero frames, but explicit zero continuation recovers0.24999988 at absolute frame7, matching the first delay-and-sum response0.25. Adaptive weights are initially zero, so this first delayed response is independently predictable.

A separate public GSC-core fixture uses identical3.5-frame delays and zero learning rate: a0.5 common impulse produces exactly0.25 at frames3 and4 and zero elsewhere. This validates the independent fractional-delay convention and shows no startup coefficient loss in GSC.

Source: `gsc.rs:113-202` retains microphone delay lines and32-tap blocking-reference histories. These remain unflushed because the plugin uses the default drain.

## Recommended scope and expected behavior

Priority1: fix the Beamformer spectral startup boundary. Priority2: add finite-stream drain for AEC and all Beamformer branches, with an explicit adaptation policy. These are separate auditable changes within the two plugin crates; no graph scheduling rewrite is needed.

User-facing result: recordings/renders retain their first onset and last buffered speech samples; stop/reset does not replay earlier state; silence padding used to finish a recording does not retrain the echo canceller or noise canceller. Existing declared latencies remain256(AEC),512(MVDR/Superdirective), and the configured GSC delay. Ordinary streaming learning and algorithm selection remain unchanged.

### Spectral Beamformer startup proposal

N=512, H=256, declared latency L=512. On construction/reset, prefill H leading zero samples and set `input_fill=H`. The first frame represents input[-H..H-1] and is computed after H actual input samples.

Its synthesis frame starts at output L-H=H. Discard its first H synthesis samples (the negative source-time portion), then place its latter H samples at output L..L+H-1. The next frame covers input[0..N-1], writes at output L, and supplies the complementary overlap. This preserves the existing L-frame leading latency while restoring complete window coverage, including sample0. The first-frame flag can use/replace the currently unused `stft_filled` field. Do not merely change the advertised latency or normalize the zero endpoint.

For MVDR, keep covariance learning disabled for the synthetic-prefix frame, but compute initial weights if dirty using the existing initial covariance. Start ordinary covariance learning on fully real analysis frames. Superdirective has fixed coefficients. General steered/spatial fixtures must verify that negative-time synthesis is intentionally clipped and that the first emitted full timeline remains aligned atL.

### Finite zero-continuation support

These are conservative structural support bounds, not measured amplitude-decay estimates. They must be proved by learned-weight and fixed-weight oracles before production claims.

**AEC:** B=256; P=ceil(echo_tail_samples/B), L_filter=P*B. Track whether any real input was accepted and its frame phase r=((S-1)%B)+1, where S is the input-frame count since reset. The last real block is q=floor((S-1)/B). Because overlap-save retains previous reference, the final nonzero reference spectrum can enter at block q+1 and remain in the P-entry FDL through q+P. Process that block completely, then emit its queued B samples. A safe exact scheduling horizon for that worst case is `(P+2)*B-r` additional output frames; the phase-independent upper bound is `(P+2)*B`. This includes the output queue once and avoids assuming that the last meaningful time-domain error tap is exactly at the nominal unpadded echo length.

The bound also contains the partial final mic block and its within-block post-filter IFFT response. Once the reference FDL is zero and microphone input is zero, the post-filter multiplies zero error spectra and cannot create new audio, even if gain envelopes retain state. Use B-frame maximum drain steps with preallocated zero input/output scratch or a shared internal sample/block helper; do not reset before emitting the tail.

**MVDR/Superdirective:** the final analysis frame touching real input starts at a=floor((S-1)/H)*H. Full synthesis support ends exclusively at a+L+N. Therefore a conservative scheduling horizon is `L+N-r`, with r=((S-1)%H)+1, i.e.768..1023 zero-input output frames for N=L=512,H=256. This retains the full synthesis frame for frequency-dependent weights, rather than stopping after only the512-frame identity delay. Hop-sized drain calls can complete all relevant analysis windows and emit the remaining OLA buffer. A phase-independent upper bound isL+N. Empty input drains immediately.

**GSC:** with maximum fractional compensation delay D (rounded upward) and adaptive FIR length F=32, zero input has bounded support through D+F-1 frames. Delay interpolation and the blocking-reference FIR are feedforward. Freeze adaptive coefficients during drain, shift both delay/history lines normally, and emit at most this bound. Broadside zero-weight history may be silent much earlier; no amplitude scanning is needed to claim the conservative bound.

### Explicit adaptation freeze policy

- AEC: add an internal frozen-processing path to TwoPathAec. Advance the shared reference FFT/FDL and foreground signal computation with adaptation_scale0. Skip background learning/leakage, foreground/background promotion, transfer counters, power/double-talk learning, and other learned-state mutations. Freeze at the first drain call, including the padded final partial block; ordinary process remains adaptive.
- AEC post-filter: simplest reviewable policy is to apply the current stored per-bin gains during drain without learning DTD powers/residual leakage or changing those gain estimates from synthetic padding. Existing user-requested wet/dry mix smoothing can finish as an output envelope. A different policy that continues gain-envelope release needs a separate stored-target definition and oracle; do not silently run all existing adaptation on padding.
- MVDR: continue FFT, beamforming and synthesis with current weights; skip covariance updates/solves on synthetic-suffix frames. If no real frame ever computed weights, solve once from the initial covariance before applying the partial clip. Superdirective needs no learned-state freeze.
- GSC: add a boolean/internal mode to skip only the NLMS update while preserving fixed-beamformer delay, blocking matrix, reference-buffer shifts and output computation. Existing `mu=0` core fixtures can independently validate frozen fixed-weight behavior.

This changes the proposed EOS output from ordinary adaptive processing of an arbitrarily long silent signal. The distinction is intentional: EOF padding is not new microphone evidence. It must be documented and tested, not hidden behind a call to `process()` that continues learning.

### Drain lifecycle / realtime requirements

Validate destination capacity and sample rate before mutation. A too-small destination must leave the next successful drain result unchanged. Bound work/output per call using existing block/hop sizes, with no allocation, coefficient cloning, or unbounded silence loops. Repeated completion returns zero frames. No-input/reset drains produce nothing. Reset clears stream buffers, phase and drain state as well as existing learned state; fresh real input cancels an incomplete EOS mode only under an explicit documented continuation policy. No ordinary `process()` call may unexpectedly drop a pending real prefix.

## Acceptance tests required before implementation is considered complete

1. Preserve the above independent AEC identity and Beamformer startup/direct-delay fixtures at all listed callbacks/rates; compare one-shot and irregular partitions including1-frame callbacks.
2. Beamformer dense identical-mic reconstruction from the very first sample and through the last sample, with initial/final impulses at every hop phase; retain declared latency512.
3. Deterministic learned AEC coefficients with a nonzero last partition/tap: independent direct FIR echo estimate plus microphone input, padded partial block, post-filter disabled. Add a separate known fixed post-filter-gain IFFT oracle for enabled suppression.
4. MVDR/Superdirective frozen frequency weights with nontrivial frequency response and steering: explicit offline frame/window sum through the last supported synthesis sample, not only a unity identity.
5. GSC deterministic nonzero FIR weights and fractional delays: direct interpolation/blocking/FIR sum; exercise the final adaptive tap at nonzero steering. Weight arrays/covariance/promotion counters must be unchanged during drain.
6. Capacity errors transactional, empty drain, repeated COMPLETE, reset/reuse, cold-thread allocation/deallocation counters, and no changes to realtime state from a rejected request.

The executed probe proves the current startup/EOS defects and the simple identity/delay controls. Learned-state support bounds, adaptation-freeze behavior and the proposed startup correction are designs only; production remains unchanged pending review.


## Implemented AUD-054 / AUD-055 — verified checkpoint

Authorized production edits are confined to `sotf-plugin-aec` and
`sotf-plugin-beamformer`; no host, native wrapper, MIDI, IAMF, or HAL changes.

### Startup correction (AUD-054)

Spectral Beamformer construction/reset primes 256 zero samples and places the
first synthesis frame at frame256. That frame covers source times[-256,256).
Its negative-time half is discarded, so source sample0 still lands exactly at
emitted frame512. The complementary squared sqrt-Hann windows now sum to unity
at the startup boundary. Initial MVDR weights are solved once; synthetic-prefix
covariance learning is skipped. GSC startup/latency is unchanged.

This audibly restores previously attenuated/missing startup samples. Existing
parameters, algorithm IDs and declared latency do not change. Source tests
retain the previous failing impulse/dense results in
`/tmp/sotf-beamformer-startup-red.log` and the corrected results in
`/tmp/sotf-beamformer-startup-green.log`.

### Finite-stream drain policy (AUD-055)

`process()` and `drain()` share the same signal-history path. Drain supplies
preallocated zeros while freezing learned state:

- AEC: foreground/background coefficients and leakage, foreground transfer
  decisions, power/DTD history, background adaptation scale, suppressor gains and
  residual-leakage/DTD state. FDL, overlap-save, output FIFO and FFT work buffers
  continue to advance. The reference-FFT diagnostic counter records actual work.
  Existing wet/dry ramps complete toward their pre-EOF target.
- MVDR: covariance and established weights freeze. A stream shorter than the
  first hop may solve initially dirty weights from the existing covariance once,
  without learning from padding.
- GSC: steering delay, blocking histories and FIR convolution continue, while
  NLMS coefficients freeze. Superdirective coefficients remain fixed as usual.

Frozen drain is deliberately distinct from ordinary adaptive processing of
zero input. Numeric reference tests derive direct convolution/window equations;
they do not use adaptive zero-continuation as the expected signal.

Bounds for a nonempty stream, emitted after the last real input:

| Algorithm | Maximum drain frames | Derivation |
| --- | --- | --- |
| AEC | `(P+2)*256-r`, r=1..256 | Complete the partial block, clear all P reference-FDL partitions including previous/current overlap, emit the final queued block and any within-block suppressor spreading. P is prepared partition count. |
| MVDR/Superdirective | `1024-r`, r=1..256 | Last real-containing analysis frame starts at the last hop boundary; emit its full512 synthesis support after declared512 delay. |
| GSC | `ceil(max steering delay)+31` | Fractional interpolation support followed by the32-tap adaptive FIR. |

Bounds may emit trailing zeros. `drain_output_frames_max()` is256 for all these
plugins. Any positive mono output capacity is accepted, capped to256 emitted
frames; unwritten output remains unchanged. Empty streams complete immediately.
Rate mismatch/empty pending-output requests leave state intact. Completion is
idempotent. Once a valid drain begins, positive input and parameter changes
require reset or initialization. Reset restores the startup state and discards
old signal/adaptive history. Native `tail_length()` remains Unknown (unchanged);
this work implements explicit finite-stream drain, not a new native-host tail
advertisement.

### Executed regression evidence

`cargo test -p sotf-plugin-aec -p sotf-plugin-beamformer`: **124 tests pass**
(AEC44unit+9integration+3boundary; Beamformer47unit+15integration+6boundary).
No ignored regressions. Log: `/tmp/sotf-aec-beamformer-drain-final.log`.

Matrices inside these tests:

- Startup:6,912 single-impulse cases (3algorithms ×3rates ×3callbacks ×256hop
  phases), exact fixed-delay/amplitude oracle;72 dense/reset cases
  (3algorithms ×3rates ×4callbacks ×2resets). Error tolerance2e-6.
- AEC54 sparse direct-convolution cases:3rates ×6input phases/lengths ×3paired
  input/drain partitions. Injected foreground IR has nonzero first and final
  prepared-partition taps; final response explicitly checked. Real background
  training and all frozen state are snapshotted before/after drain.
- AEC9 frozen, nonconstant suppressor-gain cases:independent three-tap circular
  time-domain formula for G(k)=0.5+0.25cos(2πk/512), with zero-prefix block edges,
  nonzero final echo tap and partial input. Gains/DTD/leakage remain unchanged.
- Spectral Beamformer180 cases:2algorithms ×3rates ×10lengths ×3paired partitions.
  Known nonidentity weights implement circular taps at0,31,510. Independent f64
  sqrt-Hann analysis/convolution/synthesis sums every output sample; selected
  fixtures exercise the final synthesis sample and exceed identity-delay-only
  flushing. Fixed-weight/covariance snapshots are unchanged.
- GSC90 cases:3rates ×2fractional delays ×5lengths ×3drain capacities. Direct
  interpolation, blocking projection and learned32-tap FIR equation; nonzero
  last learned tap explicitly reaches the final declared drain sample.
- Additional real-input MVDR noise-learning regression verifies covariance
  actually changed before EOS, then covariance/weights/solve count remain fixed.
- Empty streams, repeated completion, rejected capacity/rate before and during
  drain, continued real input after rejected requests, partial/oversized output
  canaries, reset-required input/parameter changes, and fresh-state reset parity.
- Fresh audio-thread first drain and reset/drain:zero allocations **and** zero
  deallocations, both1-frame and997-frame source lengths, all Beamformer
  algorithms and both AEC post-filter states. Startup first callbacks separately
  retain zero-allocation checks.

All333 independent drain numerical cases use an absolute2e-6 waveform tolerance.
`cargo clippy -p sotf-plugin-aec -p sotf-plugin-beamformer --all-targets -- -D warnings`
passes (`/tmp/sotf-aec-beamformer-drain-clippy.log`). Scoped `git diff --check`
passes. README/CHANGELOG in both crates describe boundary and frozen EOF policy.
The original pre-fix last-frame failures are preserved in
`/tmp/sotf-aec-beamformer-drain-red.log` and
`/tmp/sotf-beamformer-drain-red.log`.
