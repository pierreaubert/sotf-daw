# FIR EQ finite-stream correction (AUD-078)

Both phase modes use a prepared NUPC FIR, plus an aligned dry delay. Immediate
drain completion discarded the final program and convolution response. All
three initial regressions failed (`/tmp/sotf-fir-eq-drain-red.log`).

For L actual coefficients, the response ends after L − 1 + 32 zero-input
frames. The 32 frames are NUPC's emitted startup delay, not the linear FIR's
group delay. The dry path lies within that bound in both phase modes. Drain
uses the unchanged sample kernel in at most 256-frame calls, operating directly
on the caller's output prefix. It adds no scratch or callback allocation.

The first valid nonempty drain freezes control updates and prevents later input
until reset. Empty streams remain reusable; complete drains are idempotent.
Capacity, rate and dirty-state checks precede output/state mutation. Reset
clears convolver, dry and EOS histories. Initialization after EOS rearms the
stream; a rate change clears old-rate history. Ordinary same-rate initialization
retains its existing history-preserving behavior. Constructor preparation remains
sufficient for existing direct callers.

Verification:

- 96 configurations cover every FIR length (1,024–8,192), both phases, two
  rates, mono/three-channel processing and dry/mixed/wet settings. Output
  matches independent f64 direct convolution of coefficient data within 2e-6;
  dry timing is predicted analytically. The earlier analytic-response tests
  independently validate filter design rather than just its streaming kernel.
- Exact zero-continuation comparison includes a pending mix ramp. Capacity
  errors, partial frames, wrong rate, suffix canaries, repeated completion,
  reset/reinitialization and both initialization compatibility cases pass.
- 32 cold-thread configurations measure zero allocations and frees during
  first process or first drain, subsequent drain and reset. Normal callback
  size is 17,003 frames; drain uses 17-frame output destinations.
- The full package suite passed 70 tests; the additional initialization
  compatibility regression and three related initialization tests then passed.
  Thus 71 current tests have been verified. All-target/all-feature Clippy with
  warnings denied passes. Logs: `/tmp/sotf-fir-eq-drain-full.log`,
  `/tmp/sotf-fir-eq-drain-lifecycle.log`, `/tmp/sotf-fir-eq-drain-clippy.log`.

## Real chain integration

The facade's `finite_chain` target builds actual factory-created Gate → FIR EQ
→ Limiter chains, then uses `DawHost::process` and `DawHost::drain`. Across 96
waveform runs, complete output equals an independently delayed input exactly,
including the final marker and conservative zero padding. The matrix covers
48/96 kHz, 1/2/6 channels, both FIR phases, each individual host bypass, one-frame
versus whole-input callbacks and reset replay. The per-stage finite response
counts add exactly; upstream tails pass through downstream state before its own
drain. Log: `/tmp/sotf-finite-chain-test.log`.

These changes follow the seventh workspace checkpoint. They do not address
standard IIR EQ's internal oversampler or define truncation for recursive filters.
