# Proposal: align Upmixer high-resolution and main paths

## Reserved issue and scope

AUD130 tracks HR/main contribution timing. The original finding covered the
sub-512 main FFT route. New measurements also found behavior above 512; that
part is recorded separately below and remains outside this change. Astra
reviewed and approved the bounded sub-512 source-tag design documented below;
that design is implemented and accepted. The historical candidate notes remain
to explain why the approved scheduler does not use a fixed main delay.

The Upmixer usage note says the main and HR paths share their startup boundary.
`mix_hr_output` describes synchronized lockstep mixing, and the host latency
contract measures delay in emitted output frames, including startup silence.
The main-only path currently peaks at its declared FFT-size latency.

## Pre-correction source clocks

- The HR transform is fixed at 512 frames.
- For main FFT sizes above 512, the HR input path currently inserts
  `N / 2 - 256` frames before HR analysis.
- `prepare_hr_output_gains` schedules one main hop at
  `output.next_add_position`. `mix_hr_output` drains only available HR frames,
  but selects each gain from the current main output read position. The HR
  read clock can therefore refer to an earlier source frame than the gain
  selected by the current main clock.
- The neutral main impulse peaks at emitted frame `N`.

For N<512, the main input ring is prefixed by H=N/2 frames. After a main FFT
block is overlap-added, the first H negative-time output frames are discarded.
The remaining main output ring therefore begins at source frame 0; its read
cursor advances only when main frames are drained. The HR input is prefixed by
256 frames and the first 256 negative-time HR output frames are discarded.
Each HR block contributes 256 ready frames after that discard, so the HR read
cursor is also intended to begin at source frame 0 and advance by the number
of HR frames actually drained. These are independent cursors; neither stores
an absolute source-frame tag.

In `process_stream`, main output is drained first and its read cursor advances
by `frames_to_drain`. `mix_hr_output` then takes only
`min(frames_to_drain, hr_output_accumulator_fill)` HR frames. It writes the
first available HR frame at the beginning of the just-drained main chunk and
does not reserve the main frame position corresponding to an HR frame that was
not ready. The two cursors can therefore diverge after an underfilled drain;
later HR frames are mixed against later main frames. The controlled-gain
partition matrix reproduces the resulting callback-dependent stream output.

## Pre-correction behavior and callback-partition evidence

The phase tests use the complex projection
`sum(y[n] * exp(-i * omega * n))`; relative phase is
`arg(HR * conj(main))`. A positive path delay has phase `-omega * delay`.
Both fitted-tone residuals were required to remain at or below 1%; that ceiling
was not relaxed.

The earlier fixed-gain impulse table used one-frame callbacks. It records that
specific stream schedule; it does not establish a partition-independent HR
latency. A new test-only gain hook prepares the same constant mix-gain target
for every main hop, then renders one impulse using one-frame, irregular
`[17, 137, 256]`, and 512-frame callbacks. The mix gain is held constant while
the real input processing, HR readiness, and main drain run normally.

The controlled-gain HR contribution still changes with callback partition:

| Main FFT N | `[1]` first / peak | `[17,137,256]` first / peak | `[512]` first / peak | Max sample delta vs `[1]` |
|---:|---:|---:|---:|---:|
| 2 | 511 / 511 | 512 / 512 | 512 / 512 | 0.015218040 |
| 32 | 511 / 511 | 512 / 512 | 512 / 512 | 0.060872160 |
| 64 | 511 / 511 | 512 / 512 | 512 / 512 | 0.086086228 |
| 128 | 511 / 511 | 512 / 512 | 512 / 512 | 0.121744320 |
| 256 | 511 / 511 | 427 / 427 | 512 / 512 | 0.172172457 |
| 512 | 512 / 512 | 512 / 512 | 512 / 512 | 0 |
| 1024 | 1025 / 1280 | 1025 / 1280 | 1025 / 1280 | 0 |
| 2048 | 2561 / 2816 | 2561 / 2816 | 2561 / 2816 | 0 |

For every size and partition, the HR contribution is finite and has the same
nonzero peak amplitude. Inputs were 4,096 frames except N=2,048, which used
8,192; emitted frame counts did not change across partitions. At N=2..128,
the irregular and 512-frame runs move the contribution one frame later than
the `[1]` run. At N=256, the irregular run peaks 84 frames earlier than the
`[1]` run. This shows that HR readiness and draining affect observed arrival
even when mix gain is held constant. The one-frame arrival table below must
not be treated as an intrinsic or callback-stable path delay.

Under one-frame callbacks with fixed, nonzero HR gain, the HR impulse and
neutral main impulse peaks were:

| Main FFT N | HR first / peak | Main peak | Current HR offset | Accepted / emitted frames |
|---:|---:|---:|---:|---:|
| 2 | 511 / 511 | 2 | +509 | 4096 / 4354 |
| 32 | 511 / 511 | 32 | +479 | 4096 / 4384 |
| 64 | 511 / 511 | 64 | +447 | 4096 / 4416 |
| 128 | 511 / 511 | 128 | +383 | 4096 / 4480 |
| 256 | 511 / 511 | 256 | +255 | 4096 / 4608 |
| 512 | 512 / 512 | 512 | 0 | 4096 / 4864 |
| 1024 | 1025 / 1280 | 1024 | +256 | 4096 / 5632 |
| 2048 | 2561 / 2816 | 2048 | +768 | 8192 / 11264 |

The one-frame settled-tone phase measurements match those one-frame impulse
offsets. For N=2..512 the tone used `k=57.75` relative to the 512-frame HR FFT
(5,414.0625 Hz), with a 2,048-frame window. N=1024 used `k=57.5`
(5,390.625 Hz), 2,048 frames. N=2048 used `k=57.125` (5,355.46875 Hz),
4,096 frames, exactly 457 cycles. Measured HR-minus-main phases were:

| N | Measured phase (rad) | One-frame apparent HR delay relative to main |
|---:|---:|---:|
| 2 | -2.586134422 | 509 frames |
| 32 | -0.174873806 | 479 frames |
| 64 | -2.629243061 | 447 frames |
| 128 | -1.254796278 | 383 frames |
| 256 | +1.494097291 | 255 frames |
| 512 | +0.000000013 | 0 frames |
| 1024 | +1.570796282 | 256 frames |
| 2048 | +1.963495397 | 768 frames |

The two-tone cross-check for N=2..256 agrees with the one-frame `511 - N`
phase result.
The first phase test initially used a 57.75-bin tone at N=1024 and measured a
2.315% residual, so that result was excluded. At N=2048, the earlier 57.5 and
57.25 choices produced 1.4775% and 2.8985% residuals; the 57.125 longer-window
measurement produced 0.0019% and was eligible.

A separate nonstationary stream probe uses two impulses plus low-level signal
and compares `[1, 17, 137, 256]` and `[512]` callback partitions. Its N=256
HR first/peak frame is 429 for the irregular partition and 512 for fixed 512
callbacks. The fixed-gain impulse test puts both at 511. This partition-sensitive
arrival was initially attributed to gain/envelope variation. The controlled-
gain matrix above shows that callback sensitivity also remains when the mix
gain is held constant.

The main input ring starts with a prefix of H=N/2 frames, and the first H
negative-time output frames are discarded. Source frame 0 therefore maps to
ring slot H; the N-frame startup padding makes it emitted output frame N. At
emitted frame t, the corresponding ring slot is `(H + t - N) & mask`. The
synthetic source-tag fixture invokes the real gain scheduler and HR mixer with
an HR sample tagged as source frame 0. For the one-frame N<512 arrival at 511,
the source gain is at slot H, while `mix_hr_output` currently reads slot
`(H + 511 - N) & mask`. For N=64, that is source slot 32 (`0.000007629`) and
frame-511 lookup slot 223 (`0.000976562`). This validates the ring-index
derivation and mixer lookup. It does not establish a frame-511 arrival under
other callback partitions or exercise the transient detector end to end.

The off/on lifecycle characterization is a separate sub-finding. At N=64,
after 700 active 6-kHz frames and 256 one-frame callbacks with HR disabled,
367 partial HR input frames remain. Re-enabling HR with silence changes output
relative to an otherwise identical control that clears only those partial
frames: first difference at resumed frame 145, peak delta 0.125066265 at frame
326. The real HR enable envelope ramps; transient gain is held measurable in
the fixture. EOS emitted 2,368 frames for 1,980 accepted frames, including a
388-frame tail. This reproduced pre-correction stale-history behavior. The
implemented resume policy clears the partial HR window, queued tagged samples,
and pending main gains; a post-correction test compares output with a cleared-
state control through EOS.

## Sub-512 delay hypothesis, not approved

The one-frame fixed-gain impulse and phase evidence motivated a test-only
candidate that delays main OLA audio and its scheduled gain by `D = 511 - N`
frames. The controlled-gain matrix now shows that the HR contribution is not
partition-stable for N<512: at N=256 it peaks at 427 under irregular callbacks
and at 511/512 under the other partitions. Delaying the main stream to 511
cannot be assumed to align that callback-dependent HR output. The candidate
and `max(N, 511)` latency are hypotheses only; they are not approved for
production.

Any alignment design must map audio and gain to the same source-time frame,
and account for HR frames not being available when the main path is ready. At
N=64, the prefix/discard mapping identifies the matching main gain at ring slot
H=32; the current one-frame run at output frame 511 reads slot 223. This
source-slot derivation is separate from the callback-dependent arrival. Before
selecting a delay or queue policy, a design must establish stable source-time
placement across callback partitions and prove that unavailable HR frames are
neither mixed against the wrong main sample nor discarded.

This static-delay candidate's impulse and tone phase results used one-frame
callbacks and do not cover callback-dependent readiness. The candidate was
rejected. It did not alter the production path; the later source-tag design in
the next section was reviewed and implemented with allocation-free storage,
reset/bypass, causal AutoGain history, stable latency while HR is toggled,
exact emitted counts, and EOS drain.

A separate test-only trial raises startup padding to 512 and enlarges the main
and gain rings to four times 512 frames, while retaining the current HR FIFO
drain. Main impulse peaks then land at512 for every tested size and partition,
but HR does not: for N=2..128, `[1]` and `[512]` peak at512 while
`[17,137,256]` peaks at564; N=256 and512 peak at512 for all three. This
padding/capacity change alone therefore does not correct the defect. The trial
records the need to fix HR source-time scheduling as well; it is not a
production candidate.

## Bounded sub-512 design approved and implemented

Limit the correction to main N<512, where the HR input delay is empty. Use a
fixed common reported/output latency of512 frames for these FFT sizes, whether
HR is enabled or disabled, so toggling HR cannot move the main timeline.
Preserve N>=512 behavior in this batch. The main output/gain storage holds the
extra latency without overwriting pending WOLA frames.

The output loop spends frames only after corresponding input has been
accepted. The post-discard HR output is tagged with its source-frame position;
the main gain scheduler carries the source position for each prepared hop.
The mixer consumes HR only when its tag matches the main source frame and
prepared gain, retaining future HR tags. If a nonzero gain has no matching
ready HR frame, processing returns an error instead of emitting mispaired
audio. The 512-frame output clock reaches source frame `t` only after at least
`t + 512` input frames are accepted.

The source-position origins follow each startup prefix and discard: main source
frame 0 is at the post-discard main read boundary, while HR source frames become
available after the 512-frame analysis window. Tests cover callback partitions
`[1]`, `[17,137,256]`, `[512]`, and a multi-second callback; impulse arrival,
transient gain pairing, exact process/EOS counts, reset, bypass, and heap-free
callbacks. The causal AutoGain reference and finite drain tail use the shared
512-frame latency. HR re-enable clears partial HR input, queued HR tags and
pending main HR gains; the resume test confirms pre-toggle partial input is
not replayed after EOS. N>=512 remains on the previous prepared-gain FIFO
mixer path. Astra approved this bounded design before the production change.
The N>=512 prepared-gain mixer is compared sample-for-sample against an
isolated copy of the prior mixer for N=512,1024,2048 and all three callback
partitions; its output digests are in the AUD130 report. This is a mixer-only
control sharing the current plugin's analysis, gain preparation, processing,
and drain code; it is not an archived whole-plugin pre-change baseline, and
does not establish historical whole-plugin byte identity. Timing above 512
remains unresolved and outside this change.

## Additional finding above 512; not approved for this proposal

For N=1024 and 2048, current fixed-gain peaks trail the main path by exactly
the current HR input delay `(N / 2 - 256)`. Clearing that delay in a
test-only impulse experiment moved peaks to N. However, the corresponding
test-only tone experiment at N=1024 exceeded the unchanged 1% residual limit:
HR residual was 3.0389396%, so its phase was ineligible. The experiment stopped
at N=1024; there is no candidate phase result for N=2048. The change above 512
therefore needs a separate source-clock derivation and gain-pairing review
before it can be adopted or included in a production fix.

## Evidence and review artifacts

- Fixed-gain impulse, all tested sizes:
  `/tmp/sotf-aud130-fixed-impulse.log`
  SHA256 `ca1fafcfc2cfb0b3c0372ce8fdbcae541bea27c14fb3efca69aec1d090fd7eb0`.
- Current fixed-gain phase and first two-tone cross-check:
  `/tmp/sotf-aud130-phase-pass8.log` (SHA256
  `df1afa8a43953776937a69aa9e7ad962c0db0b44a7c18e0483116bf914e2ba46`) and
  `/tmp/sotf-aud130-second-tone.log` (SHA256
  `5c7d3c64c6fec94fee80dae7a7bedae5c3d587590078c7d7853d39c11a3271cb`).
- Partitioned current arrival:
  `/tmp/sotf-aud130-characterization-pass4.log` (SHA256
  `69ee6b902ac657983ec7527aad0b7d843130b7d55cbfc32757e1edd89a768d46`). Its
  phase test failed the 1% ceiling at N=2048 with 1.4775%; the longer-window
  eligible phase run is recorded separately above.
- Corrected sub-512 gain-ring fixture:
  `/tmp/sotf-aud130-gain-ring-corrected.log` (SHA256
  `d52556d98794cac46de04f30880bd7c52d75c9000124fc9f7af98fcb7c0a103a`). The
  earlier `/tmp/sotf-aud130-gain-clock.log` (SHA256
  `f53bc872633f2b9d8b6c1fef9f8c9da952e863c903c3a8ccd8483aed5af7c860`)
  assumed the wrong source ring index and is superseded.
- Prepared constant mix gain across callback partitions:
  `/tmp/sotf-aud130-prepared-arrivals.log` (SHA256
  `9ef754d2564ef989df615e180aa83b54cb719599466e5b2734bffe269f622ba6`). The
  earlier run with an invariance assertion failed at N=2 and is retained at
  `/tmp/sotf-aud130-prepared-partitions-pass1.log` (SHA256
  `8d508408b8d7af236bfcb8c2cc94fefc2636d9a7aba4339da6d37454419a65f8`).
- Padding/capacity-only candidate, expected-misalignment characterization:
  `/tmp/sotf-aud130-padding-only-candidate.log` (SHA256
  `ca5ca2242f9368e30eccfe2a260ee204b6c36444fe9d30c93e5310f88a5862ee`). Its
  source manifest-file SHA256 matches at start and end:
  `b7c421379c7278081de6e146407129d9829ecc5058968cb22500bb8e31dbfe5d`. The
  initial assertion run is retained at
  `/tmp/sotf-aud130-common-padding-candidate.log` (SHA256
  `6e1dc7efbe2ca62ea5492882a907e959ab6d1c03d1c200cc821cb64fc42021e1`).
- HR toggle and EOS lifecycle:
  `/tmp/sotf-aud130-toggle-final.log` (SHA256
  `0444cefa050b218f0b675a248ef12adb86d24d4032f852b24de87d9405d7400f`).
- Test-only candidate impulse alignment:
  `/tmp/sotf-aud130-candidate.log` (SHA256
  `e7aac5caad69e387e5e2433a7ad9d046eba699a57771509f33743273f017e60a`).
- Candidate tone experiment, retained as a failed gate:
  `/tmp/sotf-aud130-candidate-phase.log` (SHA256
  `970ab03bdeffbe4c3feb7d31bc9e0421a6a926b9281a263f22f4378e9d3714ba`). It
  passed through N=512, then failed at N=1024 with 3.0389396% HR residual.
- Earlier pre-hook package gate: `cargo test -p sotf-plugin-upmixer --lib`
  passed120/120 tests. Log SHA256
  `9d89ef984fa353f0c5b8f5982f7643f0648479b3fbeb347b17de2eef0328a465`; its
  matching start/end source and lock manifest was
  `8b16cf18e92fe94a8837c546138f40009ef6cb64f16b58af1bdf30c04624edf7`.
- N>=512 complete-output comparison against the reconstructed prior
  prepared-gain mixer and its FNV64 sample digests (mixer-only control; not an
  archived whole-plugin pre-change baseline):
  `/tmp/sotf-aud130-prepared-baseline-digests.log`, SHA256
  `7b3f00226ce8deaf4bde2349ef3b44fb7088c7f0721118f59dca525563cfe5db`.
- Final `cargo test --offline -p sotf-plugin-upmixer`: 175 passed, 2 ignored,
  0 failed. Log `/tmp/sotf-aud130-final-accepted-package.log`, SHA256
  `3fb5a5b2415e170c8d2262cc437a902026307cdb067454075f7c135faa788453`.
- `cargo clippy --offline -p sotf-plugin-upmixer --all-targets -- -D warnings`
  passed. Log `/tmp/sotf-aud130-final-accepted-clippy.log`, SHA256
  `05589137b05d7f8f143413b3d08483665064dbfb308cda21a1d4716e057d8c02`.
- `cargo fmt --manifest-path crates/sotf-plugins/crates/sotf-plugin-upmixer/Cargo.toml -- --check`
  passed. Final package and `Cargo.lock` start/end manifests match; both
  manifest-list files SHA256
  `f5b180b05ff15981b20ea2e5ad306bf0001daafc34dba5258bc74a6394eb45f9`.

Each successful focused run captured the complete Upmixer crate file manifest
and `Cargo.lock` before and after; paired manifests match. The prepared-arrival
manifest-file SHA256 is
`2b8690e3fecaac7b7761d3f27d572ceb7c1e7a7ad50aa0499197bce8fd1d7788` at both
start and end. The candidate tone failure also has matching start/end
manifests. All runs used the dedicated
`crates/sotf-plugins/target` and a `TMPDIR` beneath its `audit-tmp` directory.

AUD130 changed Upmixer production and test code in the bounded N<512 scope.
The full Upmixer package test and strict all-target Clippy pass on the same
source manifest. Astra accepted the bounded implementation. Above-512 timing
remains unresolved and outside this change. MIDI and IAMF remain excluded.
