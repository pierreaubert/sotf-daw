# FIR crossover finite-stream correction (AUD-074)

The public linear-phase crossover previously returned immediate drain
completion, dropping FIR history and inter-band alignment. Three independent
regressions failed before the correction (`/tmp/sotf-crossover-finite-red.log`).

The shared prepared FIR kernel now accepts ordinary input or exact zero
continuation. For S splits of L taps, S × (L − 1) bounds every emitted band:
an earlier band's alignment adds only half the support of each missing split.
Drain returns that conservative support in blocks of at most 256 frames, using
existing scratch. It honors smaller whole-frame output destinations and leaves
the unused suffix unchanged. Native tail metadata uses the same finite bound.
The LR24 recursive path keeps its existing unknown-tail/default-drain behavior;
its termination policy remains an open part of AUD-073.

Empty streams do not latch EOS. The first valid nonempty drain freezes controls
and prevents later input until reset. Invalid capacity or sample-rate calls do
not consume history. Reset and successful FIR reinitialization clear both FIR
and alignment state. Parameter indices, coefficients and signal latency are
unchanged; the extracted normal kernel retains its arithmetic and channel order.

Verification:

- 324 rate/channel/tap/split/mode/length configurations match independent f64
  direct convolution of the designed coefficient data within 2e-6 absolute.
  Summed bands independently reconstruct delayed input through the final marker.
- Drain capacities include 1, 13 and oversized 1,024 frames, with exact total
  counts, destination canaries, error/retry, empty/repeated drain and reset/reinit.
- 18 cold-thread configurations measure zero allocations and zero frees during
  first process or first drain, subsequent drain and reset, including 17,003-frame
  normal callbacks and 1/2/6 channels with one/two/three splits.
- Full package suite: 97 tests pass, none ignored. All-target/all-feature Clippy
  with warnings denied passes. Logs: `/tmp/sotf-crossover-finite-full.log` and
  `/tmp/sotf-crossover-finite-clippy.log`.

These changes follow the seventh workspace checkpoint and are not included in
its 5,512-test count. They do not establish a hardware callback deadline or fix
the separate engine drain call-count limit.
