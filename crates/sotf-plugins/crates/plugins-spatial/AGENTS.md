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
- All public functions are real-time-safe; do not introduce allocations or locks here.
- Keep dependency surface tight (`sotf-host`, `rustfft` and workspace-existing `realfft`) so spatial plugins can pull this in without dragging the whole plugin tree.

- NUPC transforms real blocks with N=2B and stores bins0..N/2 inclusive. Do not
  duplicate negative-frequency spectra or double interior bins for convolution.
  Preserve one 1/N inverse normalization, absolute partition offsets, overlap-add,
  direct-head timing and the original callback-thread schedule.
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
