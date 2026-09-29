# AUD081 minimal Downmix startup proposal — not implemented

XTC AUD080 is green; this is the next separately reviewable scope. Only the Downmix crate's primary source, new startup tests and documentation would change. No drain, LFE recursion, parameter schema, public latency, host or native wrapper changes.

## Exact source delta

1. Add one private `discard_synthesis_prefix: bool` field to DownmixPlugin.
2. Consolidate constructor/from_params state initialization through existing `clear_stream_state()` after mode selection. In spectral mode this sets input_fill=H=1024, output_read_position=H, discard_synthesis_prefix=true, startup_delay_remaining=N=2048; arrays stay zero, output_fill and next_add remain0. Simple mode keeps zero prefix/read/delay and false. initialize/reset already use clear_stream_state.
3. Keep current per-frame process scheduler unchanged.
4. Keep every FFT, phase-coherence, LtRt, coefficient, LFE-filter and OLA calculation unchanged. At the end of `process_fft_block`, before advancing next_add:
   - For the one negative-origin window, clear accumulator samples0..H*2, clear discard_synthesis_prefix, and do not increment output_accumulator_fill.
   - For subsequent windows, increment output_accumulator_fill byH as today.
   - Advance next_add byH as today.
5. Preserve declared N=2048 latency and existing mode-change reconstruction contract.

The first input window spans programme[-H,H). Clearing its finalized negative synthesis hop preserves its contributions at nonnegative programme positions; the next window completes all overlaps for programme0 before the first declared output at2048. Clearing skipped cells prevents stale output after circular wrap.

## Focused regression scope

Capture red first/final impulse and dense front-channel matrix oracles before editing. Test phase-coherent and LtRt modes with independently specified front-channel coefficients, every phase of the first1024-frame hop, first/frame1024/window2048 boundaries, rates44.1/48/96/192k, callback1/17/137/N/8193 and mixed large→small callbacks, full ring wrap and reset/reinitialize. Include surrounding output canaries, disabled spectral/simple-mode invariance, existing multichannel coefficient/LtRt quality tests, and cold allocation AND free counting on actual spectral callbacks/reset.

No change to recursive LFE completion. Existing N-frame sample scheduling is already callback invariant; only its missing negative-time window is corrected. A zero-padded process reference is not the sole oracle: neutral paths must equal the independently known delayed programme waveform. Nontrivial LtRt surround validation can reuse the existing independently derived quadrature frequency oracle while preserving its tolerance.
