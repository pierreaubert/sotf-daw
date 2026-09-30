# AUD140 proposal: channel-changing serial host EOF

**Status:** bounded design accepted by Astra; implementation is complete and
awaiting independent implementation review.

## Confirmed behavior

`DawHost::drain` currently calls `is_topologically_linear_chain`. That helper
requires each node's input and output channel counts to match. The same helper
also gates `can_process_f32_linear_chain`, so removing the width check from the
shared helper would change ordinary processing fast-path eligibility.

The repository now has an explicit `Plugin::guarantees_identity_frame_geometry`
contract. It defaults to false; sample-size probes are not treated as proof.
The host query requires each active node to opt in and to preserve its
negotiated sample rate. `AmbisonicsDecoderPlugin` now opts in. Its `process`
implementation validates interleaved channel geometry and returns
`context.num_frames` in both single-band and dual-band branches, and its
`output_sample_rate` returns the input rate. This is evidence for a frame-count
opt-in, not evidence that a recursive dual-band response has finite support.
Single-band reports `TailLength::Finite(0)`; dual-band reports `Unknown`.

## Proposed boundary

Add a drain-only admission check for a single serial audio route. Keep
`is_topologically_linear_chain` and all ordinary process selection unchanged.
The new check should admit a channel-changing route only when all of these
conditions hold:

1. The graph contains exactly the serial `chain_nodes` route, with one input,
   one output, one ordinary audio edge between adjacent nodes, no channel maps,
   and no destination offsets.
2. After bypassed nodes are removed from the active route, the host input width
   equals the first active node input width, each adjacent output/input width
   matches, and the final width equals the host output width. A bypassed node
   must itself preserve width. Keep the existing public refusal to bypass a
   width-changing node.
3. Every active node negotiates the host's single sample rate and explicitly
   guarantees identity frame geometry. Do not infer this property by probing a
   few input sizes. Ambisonics opts in only after checking its supported
   process modes and rates; keep frame geometry separate from tail metadata.
4. For this first width-changing route, every active node reports a finite
   `TailLength`. This admits the tested FIR producer and single-band
   Ambisonics transform. Keep unknown or recursive tails out of the new path;
   this does not change existing equal-width drain behavior.
5. Before calling any plugin's `begin_drain` or `drain`, validate the caller's
   whole-frame output geometry and `drain_output_frames_max()` capacity, every
   checked frame/channel multiplication, downstream frame geometry, and the
   prepared capacity of all intermediate scratch. If scratch is insufficient,
   provision it during `build()` or reject before child state advances. Do not
   grow staging buffers in `drain()`.

The drain walk should preserve the existing causal order: obtain one producer
tail chunk, pass its exact frame count through every downstream active node
using the adjacent channel widths, then advance to the next tail producer only
after the earlier stage completes. All admitted nodes preserve frame count, so
no resampling, frame probing, or queue between rates is needed. The host's
returned samples remain interleaved in the final node's output layout.

Invalid topology, width discontinuity, rate change, missing geometry
capability, unknown tail under this bounded width-changing policy, or
insufficient prepared/output capacity must be rejected before any plugin drain
state advances. The current host contract permits queued graph and parameter
updates to apply before a capacity error; do not claim those queues are
transactional here. Existing plugin failure behavior after a child has already
advanced remains unchanged and should be reported as a separate failure mode.

This proposal covers strict serial paths only. It does not admit branching or
sidechain graphs, explicit channel maps, unequal-rate nodes, terminal sinks, or
manager-owned queues. It does not change ordinary f32 fast-path eligibility.

## Required public evidence

- Keep a zero-tail actual Ambisonics host route at order 7 (`64 -> 16`,
  `9.1.6`) and require a zero-frame completion receipt with an empty output
  slice. Include adjacent width cases (`4 -> 6`, `16 -> 12`) as completed
  no-tail routes.
- Keep the finite producer route (`64 -> 64 -> 16`) with two final frames,
  exact one-frame-per-call capacity, an explicit completion receipt, a unique
  final-frame marker, and a full-vector comparison. The producer oracle is an
  independent f64 implementation of `y[n] = x[n] + 0.5 x[n-1] + 0.25
  x[n-2]`; the downstream Ambisonics mapping uses a separately constructed
  production decoder. Cite AUD133 for its independent decoder-accuracy
  evidence; do not describe the composed oracle as an independent Ambisonics
  implementation.
- Preserve the public bypass refusal and byte-identical ordinary output before
  and after the refusal. Preserve a pre-edit rejected-drain capture that proves
  zero `begin_drain` and `drain` calls, then legally remove the downstream
  decoder and compare the still-pending producer tail against the complete f64
  reference. This checks retained audio, not only counters.
- Retain ordinary process captures for `4 -> 6`, `16 -> 12`, `64 -> 16`, and
  the finite producer's `64 -> 16` process route. These are wrapper-versus-
  direct-plugin compatibility controls, not independent decoder-accuracy
  oracles.
- Verify invalid adjacent widths, a node without the explicit geometry
  capability, unequal negotiated rates, undersized output, and misaligned
  output all fail before any `begin_drain`/`drain` call. Use exact declared
  capacity on successful paths and check terminal completion rather than only
  comparing emitted samples.

Before future host edits, preserve exact production source copies, the test
source, `Cargo.lock`, ordinary output vectors, refusal metadata, and recovered
producer tail. Capture/replay helpers remain manually ignored in routine test
runs and must be invoked with `SOTF_AUDIT_BASELINE_DIR` set.
