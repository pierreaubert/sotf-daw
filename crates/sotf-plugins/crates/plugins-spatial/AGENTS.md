# plugins-spatial

Shared spatial DSP helpers used by spatial-audio SOTF plugins (upmixer, downmix, ambisonics, binaural, beamformer, …).

## Modules

- `lib.rs` — interleaved-buffer validation helpers (`validate_interleaved_io`, `InterleavedBufferSizes`, `checked_interleaved_samples`). Centralises plugin-side I/O size checks so each spatial plugin reports identical errors.
- `nupc.rs` — non-uniform partitioned convolution primitives shared by spatial reverb / convolution paths.

## Testing

```bash
cargo check -p plugins-spatial && cargo clippy -p plugins-spatial
cargo test -p plugins-spatial
```

## Important Notes

- Pure DSP: no `Plugin` trait impls — host integration lives in the per-plugin crates.
- Processing and reset must be real-time-safe: no allocation, freeing or locks.
  Kernel/engine construction allocates prepared storage off the audio thread.
- Keep dependency surface tight (`sotf-host`, `rustfft` and workspace-existing `realfft`) so spatial plugins can pull this in without dragging the whole plugin tree.

- NUPC transforms real blocks with N=2B and stores bins0..N/2 inclusive. Do not
  duplicate negative-frequency spectra or double interior bins for convolution.
  Preserve one 1/N inverse normalization, absolute partition offsets, overlap-add,
  direct-head timing and callback-thread execution. Default engine phases stay zero.
- Every engine owns a prepared real N-sample workspace and half-spectrum FDL,
  input spectrum and sum. Scratch uses the larger forward/inverse requirement.
  The inverse endpoints are real. Shared kernels contain immutable half spectra
  and share both transform plans; processing/reset must not allocate or free.
- Keep independent f64 convolution, DC/Nyquist, non-power-of-two block, direct-head,
  complete-tail and partition/reset references. FFT changes may alter roundoff;
  do not impose historical bit identity across different FFT algorithms.

- Preparation coalesces only adjacent same-block, same-FFT groups with contiguous
  IR coverage. Concatenate their partition spectra into one FDL: older input
  spectra account for the later group's delay. Keep the public/admission plan
  unchanged and retain the first group's output delay.
- Preserve the explicitly unmerged delayed-group reference, independent f64
  convolution, structural tap-count checks, complete tails and reset/partition
  invariants. Coalescing changes floating-point accumulation order; it does not
  add a worker or distribute the remaining FFT bursts over callbacks.

- Direct-head history remains one N-sample ring with causal-order shared taps.
  Split at the current write position and reverse each contiguous history slice
  against its corresponding taps. Four independent sums remove the per-tap wrap
  branch; wrap the write position once per input sample. No history duplication
  or callback allocation is needed. Floating-point accumulation order may change.
- Preserve the f64 wrap/odd/lane-boundary reference and full tail/reset tests.
  `direct_head_paired_callback_cost_abba` is an explicitly executed isolated wall
  timing diagnostic, ignored by default to avoid CI timing gates. Keep raw ABBA
  trials, symmetric output touching and the old scalar control. Profiling the
  head alone does not replace complete Plugin/native deadline validation.

- `NupcKernel::instantiate_with_phase(index, count)` prepares a fractional initial
  phase for each level; require count>0 and index<count. Widen the phase product
  before division so valid large usize fractions cannot overflow.
- Input zero-prehistory fill and output-queue cursor must have the same phase.
  Store it and restore both on reset. Keep the existing output-delay length and
  absolute tap placement: this staggers boundaries without adding signal delay.
- `instantiate()` is phase zero. Per-block FFT/MAC work and steady rate stay the
  same, though one shifted boundary can add a transform in a finite interval.
  Preserve independent f64 dense/sparse full-tail, extreme-fraction and exact
  reset/partition replay tests; roundoff across phases need not be bit-identical.

- Convolution opts into phases for ordinary NUPC only. Its direct-head engines
  retain phase zero: the full-matrix all-engine trial increased head deadline
  misses. The shared API still supports head phases numerically; that does not
  establish their maximum-load performance.

- Direct-head kernel preparation rounds the tail's base down to a power of two
  within min(requested cap, actual head length). Keep every head tap and use the
  existing absolute-offset output delay (257 taps -> B=256, initial delay=1).
  An empty tail needs no positive block cap. No-head construction and the public
  explicit partition planner keep their supplied base.
- Geometry can gain a level near a boundary. Preserve all waveform stimuli and
  numerical gates in the old unmerged reference; structural coalescing coverage
  describes the selected head grid. Keep Convolution's conservative admission
  arithmetic unchanged and verify materialized retained storage at its original
  boundaries. Do not infer capacity from powers-of-two selection alone.

- The proposed standalone final-tail crop is archived, not production behavior.
  Its waveform checks passed but its historical timing guard failed. Preserve
  the failed data and require controlled paired profiling before revisiting.
  A smaller late level must not split a previously shared transform pair, and
  largest-block metadata must remain the maximum if sizes become nonmonotonic.

- The 2026-10-24 final-tail retry passed isolated ABBA kernel timing but failed
  the original full plugin head-miss guard again (1,365 versus 690). Production
  DSP remains restored; preserve both failed full runs and the passing paired
  screen without interpreting kernel throughput as deadline acceptance.
- `final_tail_paired_callback_cost_abba` is an explicitly executed, ignored-by-
  default diagnostic. Keep both frozen constructors independent of production,
  seven selected geometries, two unchanged controls, full measured output checks,
  equal output touching and the untrimmed four-period horizon. Preserve raw
  durations and the constructor/source verifier; do not make timing a CI gate.
