# Proposal: define and verify Upmixer's minimum FFT size

## Reserved issue

AUD129 — reject or normalize the zero-hop Upmixer transform geometry and verify
every smaller power-of-two size already admitted by its API.

## Source evidence

- `UpmixerPlugin::new` currently checks `fft_size.is_power_of_two()` but has no
  lower bound. In Rust, one is a power of two, so the constructor accepts 1.
- `UpmixerPlugin::from_params` rounds the configured size with
  `next_power_of_two()`. Configured zero and one both become one; non-powers
  round upward. The default is 2048 and low-latency mode selects 1024.
- Construction sets `hop_size = fft_size / 2`; one therefore has hop zero.
- `process_stream` repeatedly processes full frames and advances the input
  buffer by `hop_size * 2`. At size one, that shift is zero, so a full-buffer
  processing iteration does not consume input and can loop indefinitely.
- Initialization validates sample rate but not transform geometry. Runtime
  low-latency reconfiguration uses fixed sizes 1024 and 2048.
- AUD118 repaired initialization for sizes 64 and 128. Public behavior for
  sizes 2, 4, 8, 16, and 32 has not been verified.
- The HR output accumulator currently uses four times the main FFT size, but
  each 512-point HR block advances 256 frames. At main FFT32, the 128-frame
  ring cannot hold one HR hop; a 269-frame HR-enabled callback triggers the
  existing overflow guard.

## Proposed behavior

1. Define two as the minimum transform geometry that makes forward progress
   with a positive 50%-overlap hop. For the periodic sqrt-Hann window, N=2 is
   `[0, 1]`; adjacent hop-one squared windows sum to one. This bound says
   nothing about useful spatial-frequency resolution or full-quality upmixing
   at N=2.
2. Keep `UpmixerPlugin::new`'s existing power-of-two precondition and make its
   minimum explicit in the assertion and constructor docs.
3. Preserve the parameter factory's documented round-up behavior, while
   normalizing configured sizes below two to two before calling
   `next_power_of_two`. Thus zero and one map to the smallest progressing
   geometry; existing sizes at or above two keep their current geometry.
4. Verify direct `new(0)` and `new(1)` reject invalid geometry. Through
   `from_params`, verify 0, 1, and 2 map to N=2, 3 rounds to N=4, and
   `low_latency=true` still selects N=1024. Do not process the defective
   size-one instance because its source loop has no progress condition.
5. Exercise 2, 4, 8, 16, and 32 through public initialization, irregular
   processing, reset, and fresh-instance comparison across representative
   rates and speaker layouts. Preserve AUD118's 64/128 coverage.
6. Extend the internal neutral-route oracle to every initial window phase at
   sizes 2 through 32. The identity route still runs the FFT, inverse FFT,
   windowing, and overlap-add path. At N=2, cover both impulse phases, DC,
   Nyquist, and deterministic dense input. Compare the complete output to the
   independently delayed input with a `2e-6` absolute error limit, and verify
   exact callback frame counts and finite-stream EOS output.
7. Exercise both public decorrelation modes and the low-size route with HR
   direct and AutoGain both enabled and disabled, using surround layouts with
   height channels. Process beyond `max(sample_rate / 10, 512)` so enabled
   AutoGain crosses a refresh and HR completes multiple blocks. Add an
   end-to-end neutral stream comparison with matched main-path state,
   irregular callback partitions, finite EOS, and HR-only output difference.
   Record any fixed 512-point HR arrival offset as a separate limitation; do
   not claim full feature-quality parity at N=2.
8. Add fresh-thread callback allocation/deallocation checks for the small
   supported sizes, covering first processing and processing after reset.
9. Size the HR output ring for `4 * max(main_fft_size, hr_fft_size)` in
   construction, speaker-layout changes, and FFT resize. This keeps every HR
   hop bounded when the main transform is shorter than the fixed 512-point HR
   transform.

## Scope and acceptance evidence

Production edits remain within `crates/sotf-plugins/crates/sotf-plugin-upmixer`.
The dedicated report will record source hashes, independent-oracle expectations,
exact commands and outcomes, CPU/allocation limitations, and Astra's review.
No host metering, shared issue ledger, or implementation-plan files are part
of this batch.

Acceptance requires the public factory and constructor bounds above, passing
accuracy/lifecycle/realtime tests at all sizes 2–32, an HR-enabled route after
layout reconfiguration, unchanged 64/128 behavior, the neutral block and
end-to-end stream timing probes with any alignment limitation tracked
separately, and Astra-medium review of source, tests, and executed evidence.
Preserve the existing causal AutoGain and EOS fixes.
