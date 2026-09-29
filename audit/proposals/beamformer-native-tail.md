# Beamformer native finite-tail metadata proposal

2026-09-28, read-only. AUD-097/098 numerical checkpoint is frozen and the twelfth workspace gate passed. No metadata or repository source changes made by this proposal. Parent owns AEC and AUDIT.md.

## Minimal change

Add only `Plugin::tail_length()` in `src/lib/beamformer_plugin.rs`, importing the existing host `TailLength` enum:

- Prepared MVDR or Superdirective: `Finite(1024)` output-rate frames.
- Prepared GSC: `Finite(self.gsc.finite_support_frames())`, i.e. the actual prepared `ceil(max steering delay)+31`.
- Conservatively return `Unknown` for zero sample rate. There is no need to add a separate initialized flag: `new` and `from_params` already prepare every active algorithm, and ordinary processing is currently allowed before an additional initialize call.

Use the prepared GSC helper, not a second calculation from requested geometry. It scans at most eight retained steering delays, reads scalar state, and allocates nothing. No metadata countdown or mutable cache is needed. The result stays stable during ordinary processing, drain and reset; successful reinitialization reconstructs the delay lengths for the new clock before the getter reports the new GSC bound. All exposed controls are structural and rejected by live setters. There is no asynchronous coefficient worker or supported live algorithm transition.

## Ordinary-processing support proof

### GSC

The delay interpolation consumes at most `ceil(max delay)` retained input frames. Its blocking projection is memoryless. Each adaptive reference has 32 FIR history samples, contributing another 31 frames after the last delayed reference. The AUD-097 coefficient commits admit only finite values; after the reference history clears, all learned coefficients multiply exact zero. The power regularizer stays positive and the zero-input update adds zero to existing finite weights. Target-presence state does not inject an audio term. The final output is finite even if earlier intermediate cancellation exceeds the sample range. Therefore ordinary adaptation cannot extend audio beyond `D+31`.

### MVDR / Superdirective

For N=512, H=256, let S be the number of accepted input frames and `a=H*floor((S-1)/H)`. The last analysis frame that can contain source audio starts at a. Its synthesis occupies `[a+N, a+2*N)`, so the remaining response after input becomes zero is `2*N-r`, where `r=S-a` is1..256. The static conservative bound1024 covers this phase-dependent horizon, including physical delay once.

Continuing MVDR covariance adaptation changes only coefficients. Every all-zero analysis frame has exact zero input spectra; the noise gate accepts that frame, and the updated AUD-098 normalization installs finite candidates or the finite steered fallback. Superdirective coefficients are fixed by the validated array and regularized finite matrix. Beamforming is a dot product of current spectra; neither covariance nor weights are an additive signal source.

Finite extreme input can overflow the f32 FFT and temporarily produce NaN synthesis. This does not make the support infinite: every analysis fills the full input workspace, every beamforming pass overwrites the frequency output, and zero input replaces the finite overlap history. The OLA reader returns each cell and immediately sets that cell to zero. Zero synthesis does not copy or spread an old NaN to other cells. The last contaminated synthesis window therefore has the same endpoint `a+2*N` as any other nonzero window. The prepared real FFT helper is stateless across calls apart from fully written buffers and work scratch; the isolated probe checks actual reuse after overflow.

**Scope limit:** this proves eventual exact zero, not finite output throughout the transient or recovery of learned covariance quality. Spectral transient overflow is separately tracked as AUD-101. A poisoned MVDR covariance could continue selecting the finite fallback forever and still satisfy this audio-tail bound; its subsequent adaptive performance needs a separate probe and review.

## Executed isolated evidence

No Cargo invocation or repository test/source change. `/tmp/sotf-beamformer-tail-probe/ordinary_support.rs` was linked against existing post-fix workspace rlibs; binary `target/audit-tmp/beamformer-ordinary-support`, results `/tmp/sotf-beamformer-tail-probe/ordinary_support.log`.

192 cases: each algorithm64 cases (2/8 microphones, 48/192kHz, 0/37degree steering, 50cm spacing, four terminal hop phases, coherent/opposed f32::MAX bursts). Each starts with real ordinary adaptation, accepts the burst using repeated17/997/1/256 callbacks, then processes the proposed bound plus8192 zero frames. Every sample beyond the bound is exactly zero, and reset restores exact silence.

| Algorithm | Transient nonfinite outputs summed across cases | Latest nonzero/nonfinite zero-continuation index | Largest proposed bound |
|---|---|---|---|
| MVDR | 76638 | 1006 | 1024 |
| Superdirective | 76882 | 1006 | 1024 |
| GSC | 0 | 1210 | 1211 |

Existing frozen drain tests additionally verify nonidentity spectral support and final GSC FIR taps. The new ordinary-processing evidence is independent of calling drain and permits adaptation to continue.

## Permanent tests if metadata change is approved

1. Check exact metadata values for each active algorithm, representative valid geometries/clocks, zero-rate Unknown, and scalar getter stability across nonempty process, partial/full drain and reset.
2. Reinitialize at a different sample rate: GSC must follow the new prepared delay; spectral support remains1024. Unsupported structural writes must not change the bound or retained state.
3. Promote the ordinary-zero probe to a focused regression, including FFT-overflow recovery and explicit documentation that transient nonfinite outputs are an open separate issue. Assert exact zeros beyond the bound, with ordinary adaptation enabled.
4. Use a private learned-state fixture to prove adaptation actually changes during ordinary continuation, contrasting the existing frozen-drain policy without using frozen drain as the ordinary output oracle. Cover all hop phases with bounded modest input, and selected overflow cases.
5. Include repeated getter calls in the existing fresh-thread zero-allocation/deallocation harness. No lock, FFT, allocation, cloning, state mutation or worker adoption belongs in the getter.

Run full Beamformer tests and strict all-target/all-feature Clippy after the focused metadata/ordinary-zero regressions pass. Do not change GSC/MVDR DSP or native wrapper policy as part of this metadata patch.

## Current source hooks

- `src/lib/beamformer_plugin.rs:301`: synchronous initialize/rebuild/reset.
- `:329`: ordinary process; `:358`: separate frozen drain; `:410`: latency getter area suitable for tail getter.
- `:494`: OLA read and immediate clear; `:509`: fill/FFT current analysis; `:528`: ordinary covariance update and weight solve; `:567`: synthesis accumulation; `:578`: input overlap shift.
- `src/gsc.rs:123`: prepared finite support helper; `:218`: finite coefficient commit; `:240`: bounded public sample conversion.
- `src/mvdr.rs:109`: ordinary noise decision; `:202`: widened normalization and complete-bin validation; `:233`: finite steered fallback.
- `src/superdirective.rs:71`: fixed regularized diffuse covariance; `:132`: current-spectrum dot product.
- `../math-audio/crates/math-dsp/src/stft/real_fft_processor.rs:66`: prepared forward/inverse FFT calls.
