# AUD130 — Upmixer HR/main contribution timing

## Status

The callback-dependent sub-512 HR/main arrival defect is reproduced and the
bounded source-tag correction is implemented. Astra approved this design for
main FFT sizes below512: use a common512-frame latency, spend output frames
only from accepted-input credits, tag HR samples and prepared gain by source
frame, and stop if a nonzero-gain frame is not ready. The fixed-gain callback
matrix, long-callback/EOS oracle, AutoGain/layout matrix, HR-toggle reset test,
and allocator gate are the acceptance evidence. Astra accepted this bounded
N<512 implementation. The separate above-512 timing finding remains
unresolved and outside this change.

The N>=512 route retains its existing callback scheduling and latency. The
measured timing gap above512 and the earlier phase residual failure remain
outside this correction. The off/on partial-input issue is now handled by
clearing queued HR data and its matching pending main gains when resuming; the
test verifies that pre-toggle partial input does not reappear after EOS.

The scoped proposal and reviewed source-clock derivation are
[upmixer-hr-timing.md](proposals/upmixer-hr-timing.md). AUD129 remains the
accepted minimum-FFT geometry and HR ring-capacity change.

## Intended behavior from source/contracts

The Upmixer usage note says the main and high-resolution paths share their
startup boundary. `mix_hr_output` documents lockstep mixing. The host latency
contract measures delay in the emitted stream, including leading silence.
The neutral main route peaks at its reported FFT-size latency. HR audio and its
scheduled gain should refer to the same input-time sample when mixed.

## Pre-correction scheduling behavior

Before the correction, `prepare_hr_output_gains` wrote the gain for each main hop at
`output.next_add_position`. `mix_hr_output` drains available HR samples and
selects gain by the current main output read index. For N>512, current source
also delays HR input by `N / 2 - 256` frames. Measurements below show why those
clocks need review.

For N<512, the main ring's H=N/2 input prefix and H-frame startup discard leave
its read cursor at the source-frame-0 boundary. The HR path similarly prefixes
256 input frames and discards the first 256 negative-time output frames. Both
rings therefore intend to expose source frame0 at their post-discard read
cursors. The cursors advance independently, but neither carries an absolute
source-frame tag. `process_stream` drains main frames and advances its cursor;
`mix_hr_output` then takes only the available HR count and puts the HR FIFO
head at the beginning of that just-drained main chunk. If HR is short, main
frames with no HR are already emitted while the HR read cursor advances by
fewer frames. Later HR samples can then be paired with later main samples.

## Pre-correction impulse and callback-partition measurements

The original fixed-gain impulse test used one-frame callbacks and a nonzero
left/right impulse, ran finite input, and drained EOS. The neutral main peak
was N. HR peaks were frame511 for N=2..256, frame512 at N=512, and later for
N>512:

| N | HR first / peak | Main peak | One-frame apparent offset |
|---:|---:|---:|---:|
| 2 | 511 / 511 | 2 | +509 |
| 32 | 511 / 511 | 32 | +479 |
| 64 | 511 / 511 | 64 | +447 |
| 128 | 511 / 511 | 128 | +383 |
| 256 | 511 / 511 | 256 | +255 |
| 512 | 512 / 512 | 512 | 0 |
| 1024 | 1025 / 1280 | 1024 | +256 |
| 2048 | 2561 / 2816 | 2048 | +768 |

The one-frame settled-tone phases agree with these one-frame impulse offsets.
For the exact frequencies, residuals and phase values, see the proposal's
tables and `/tmp/sotf-aud130-phase-pass8.log`. The second-tone cross-check for
N=2..256 corroborates the one-frame `511 - N` phase result.

A new unit fixture prepares the same constant HR mix-gain target for every main
hop, then compares the real stream path using `[1]`, `[17, 137, 256]`, and
`[512]` callbacks. It subtracts a matched main-only render to isolate the HR
contribution. All contributions are finite and nonzero, with identical peak
amplitudes across partitions, but their arrival differs:

| N | `[1]` first / peak | `[17,137,256]` first / peak | `[512]` first / peak | Max sample delta vs `[1]` |
|---:|---:|---:|---:|---:|
| 2 | 511 / 511 | 512 / 512 | 512 / 512 | 0.015218040 |
| 32 | 511 / 511 | 512 / 512 | 512 / 512 | 0.060872160 |
| 64 | 511 / 511 | 512 / 512 | 512 / 512 | 0.086086228 |
| 128 | 511 / 511 | 512 / 512 | 512 / 512 | 0.121744320 |
| 256 | 511 / 511 | 427 / 427 | 512 / 512 | 0.172172457 |
| 512 | 512 / 512 | 512 / 512 | 512 / 512 | 0 |
| 1024 | 1025 / 1280 | 1025 / 1280 | 1025 / 1280 | 0 |
| 2048 | 2561 / 2816 | 2561 / 2816 | 2561 / 2816 | 0 |

The irregular and 512-frame runs move the N=2..128 contribution one frame
later than `[1]`. For N=256, the irregular run peaks 84 frames earlier than
`[1]`, which differs from the one-frame peak at511 and the 512-frame peak at
512. This remains with constant scheduled mix gain, so readiness/drain
scheduling affects observed arrival. The earlier attribution to gain-envelope
variation alone is incomplete. It is not valid to treat the one-frame HR
arrival as an intrinsic or callback-stable path latency.

## Pre-correction gain clock and lifecycle evidence

The main input ring starts with a prefix of H=N/2 frames, then discards the
first H negative-time output frames. Source frame0 therefore maps to ring slot
H; N startup-padding frames make it emitted frame N. At emitted frame t, the
ring slot is `(H + t - N) & mask`. The source-tag fixture calls the actual gain
scheduler and mixer with a synthetic HR sample tagged as source frame0. For
the one-frame N<512 arrival at frame511, the source gain is at H and the
current mixer reads `(H + 511 - N) & mask`. At N=64, that means source slot32
with value `0.000007629` and current slot223 with value `0.000976562`. The
fixture verifies the mapping and mixer lookup; it does not prove the arrival
is frame511 for other callback partitions or exercise the transient detector
end to end.

The pre-correction lifecycle fixture feeds 700 active 6-kHz frames, disables HR for 256
one-frame callbacks, then resumes with silence. At N=64, `hr_input_buffer_fill`
retains 367 partial frames with peak amplitude0.5. Compared with an otherwise
identical control that clears only this partial input, resumed output first
differs at frame145 and peaks at delta0.125066265 at frame326. The real HR
enable envelope ramps during the test. EOS emitted 2,368 frames for 1,980
accepted frames, including a 388-frame tail. This reproduced stale-resume
behavior before the correction. The implemented resume policy clears partial
input, queued HR tags, and pending gains; the post-correction test compares
output to a cleared-state control through EOS.

## Rejected candidate simulations and separate limits

The earlier test-only sub-512 candidate delayed main audio and its gain by
`511 - N`, aligning one-frame impulse peaks at 511 and the corresponding
one-frame phase result through N=512. The callback-partition matrix now shows
that the HR result itself can arrive at 427 or 512 for N=256. The candidate
latency `max(N, 511)` is therefore only a hypothesis; no production delay or
queue design is accepted from the one-frame results.

For N=1024 and2048, the current HR peak offset matches the existing HR input
delay length. Clearing that delay in a test-only impulse simulation aligns
the peaks at N. However, the paired candidate tone test at N=1024 produced a
3.0389396% HR residual, above the predeclared 1% ceiling; no phase claim can be
made for that run, and the loop stopped before N=2048. Do not remove the
above-512 HR delay in production until source-time and gain pairing are
reviewed. The proposal records this as outside the original sub-512 scope.

The full proposal also records failed 57.75-at-N1024 and initial N2048 tone
choices. The residual ceiling was not relaxed.

A separate test-only trial raises startup padding to512 and enlarges the main
and gain rings to four times512 frames, retaining the current HR FIFO drain.
Main peaks land at512 for all tested sizes/partitions, but HR peaks for
N=2..128 are512 with `[1]`/`[512]` and564 with `[17,137,256]`. N=256 and512
remain at512. Thus padding/capacity alone is insufficient; the untagged
source-time scheduling must also be corrected.

## Implemented bounded sub-512 correction

The reviewed correction applies only when main N<512. It reports and emits a
stable512-frame latency whether HR is enabled or disabled. The output loop
uses accepted-input credits for this route: a callback cannot spend its whole
startup pad before its input frames have reached the analysis path. After the
pad, source frame t is emitted at output-clock frame512+t.

The readiness derivation follows the prepared windows. Main starts with an
H=N/2 zero prefix and discards the negative-time output hop, so its retained
ring begins at source frame0. HR starts with a 256-frame prefix; its 512-frame
window advances by256. The first HR block only produces negative-time samples;
the second block makes source frames0..255 ready after512 accepted frames.
More generally, source frame t is ready after t+512 accepted frames. The
output-credit clock reaches frame512+t only after that many inputs have been
accepted, independent of host callback partition.

The main gain scheduler carries the source position of each prepared hop. HR
ring slots carry signed-window-derived source tags, with discarded startup
frames invalidated. The sub-512 mixer consumes HR audio only when its tag
matches the main source frame and uses that main frame's prepared gain. Future
HR tags remain queued. Before draining, a nonzero gain without a matching
ready HR tag returns an explicit error, so the route cannot silently lose or
mis-pair HR audio.

On HR re-enable, the plugin clears the partial HR input window, queued HR
samples/tags, and pending main HR gains. The first fresh HR hop before the
restart point is discarded, and newly prepared gains stay zero until the
restart source frame. This prevents a partial pre-toggle HR window or a newly
prepared gain for an older main block from entering the resumed stream.
AutoGain's causal reference ring and the finite drain tail use the same512-frame
latency. Main and HR rings are allocated for four times `max(N, 512)` frames.
The output-credit branch, tags, and resume threshold are restricted to N<512;
the N>=512 callback path remains as it was before this correction.

## Final regression evidence

The final source-tag partition test uses constructor-prepared storage and
latency, with no manual ring or padding overrides. For N=2,32,64,128,256 and
`[1]`, `[17,137,256]`, and `[512]` callbacks, the nonzero HR contribution's
first and peak frame are512 in every partition and its maximum sample delta
from `[1]` is0.

For N>=512, a test-only control reconstructs the prior prepared-gain FIFO
mixer. It compares every output sample against the production route for
N=512,1024,2048 with `[1]`, `[17,137,256]`, and `[512]` callbacks, including a
varying-level input and a nonzero HR contribution. The output arrays are
exactly equal. This is a mixer-only control: it shares the current plugin's
analysis, gain preparation, processing, and drain code. There is no archived
whole-plugin pre-change binary or output golden, so this comparison does not
establish historical whole-plugin byte identity. FNV-1a digests of the
control's little-endian `f32` bit patterns are:

| Main FFT N | `[1]` | `[17,137,256]` | `[512]` | Emitted frames |
|---:|---:|---:|---:|---:|
| 512 | `e269028747bc440e` | `5e7bc45a1e3a99cc` | `36225e9d23b5b4a6` | 4,864 |
| 1024 | `3059904f04d14ab7` | `bc165f6816c793ee` | `67e6fab16b9aa406` | 5,632 |
| 2048 | `f8b76f04a8936cd7` | `71edb4fda4d6605c` | `62461006be4945aa` | 11,264 |

A single 96,017-frame N=2 callback preserves the full neutral identity stream,
its512-frame leading latency, and its final finite tail; the exact drained
length is96,530 frames. The surround/height route matrix covers N=2 and256,
5.1 and7.1.4 layouts, HR on/off, and AutoGain on/off for 6,000 input frames.
It verifies finite output, exact process/EOS frame counts, and the delayed
AutoGain reference position. The HR-toggle test proves that a retained partial
pre-toggle window has no effect after re-enable and EOS; its exact output is
identical to a control that clears that partial window before re-enable.

The package's public small-FFT tests cover factory and direct-constructor
bounds, N=2..128 processing across layouts/rates, HR and AutoGain toggles,
reset, parameter-driven layout changes, and zero allocations on callback and
reset. The independent AutoGain oracle also compares output against exact
512-frame delayed identity at N=2/256 after warming while disabled and enabling
AutoGain, while checking the delayed-reference ring against accepted source
samples. N=64/128 accepted geometry remains intact; only its reported/output
latency is shared at512 while N<512.

## Review artifacts and tests

- Fixed-gain HR impulse, all sizes: `/tmp/sotf-aud130-fixed-impulse.log`.
- Current settled-tone phase: `/tmp/sotf-aud130-phase-pass8.log`.
- Sub-512 second-tone check: `/tmp/sotf-aud130-second-tone.log`.
- Partition-sensitive current arrival and the ineligible earlier phase:
  `/tmp/sotf-aud130-characterization-pass4.log`.
- Corrected gain-ring fixture: `/tmp/sotf-aud130-gain-ring-corrected.log`.
  The earlier `/tmp/sotf-aud130-gain-clock.log` used an incorrect source ring
  index and is superseded.
- Prepared constant-gain partition matrix:
  `/tmp/sotf-aud130-prepared-arrivals.log`, SHA256
  `9ef754d2564ef989df615e180aa83b54cb719599466e5b2734bffe269f622ba6`.
  The earlier run with the incorrect invariance assertion failed at N=2 and is
  retained at `/tmp/sotf-aud130-prepared-partitions-pass1.log`, SHA256
  `8d508408b8d7af236bfcb8c2cc94fefc2636d9a7aba4339da6d37454419a65f8`.
- HR toggle/EOS lifecycle: `/tmp/sotf-aud130-toggle-final.log`.
- N>=512 pre-correction prepared-gain equivalence and sample digests:
  `/tmp/sotf-aud130-prepared-baseline-digests.log`, SHA256
  `7b3f00226ce8deaf4bde2349ef3b44fb7088c7f0721118f59dca525563cfe5db`.
- Test-only impulse candidate: `/tmp/sotf-aud130-candidate.log`.
- Candidate tone phase failure at N=1024:
  `/tmp/sotf-aud130-candidate-phase.log`.
Final package gates:

- `cargo test --offline -p sotf-plugin-upmixer`: 175 passed, 2 ignored, 0
  failed across unit, integration, quality, AutoGain, small-FFT, and allocator
  targets. The ignored tests retain the rejected static-delay candidate's
  failed impulse and tone evidence. Log
  `/tmp/sotf-aud130-final-accepted-package.log`, SHA256
  `3fb5a5b2415e170c8d2262cc437a902026307cdb067454075f7c135faa788453`.
- `cargo clippy --offline -p sotf-plugin-upmixer --all-targets -- -D warnings`
  passed. Log `/tmp/sotf-aud130-final-accepted-clippy.log`, SHA256
  `05589137b05d7f8f143413b3d08483665064dbfb308cda21a1d4716e057d8c02`.
- `cargo fmt --manifest-path crates/sotf-plugins/crates/sotf-plugin-upmixer/Cargo.toml -- --check`
  passed.
- The before/after manifests for every Upmixer crate file and `Cargo.lock`
  match byte-for-byte. Manifest files
  `/tmp/sotf-aud130-final-accepted-start.sha256` and
  `/tmp/sotf-aud130-final-accepted-end.sha256` both have SHA256
  `f5b180b05ff15981b20ea2e5ad306bf0001daafc34dba5258bc74a6394eb45f9`.

Successful focused runs captured pre/post manifests of every file in the
Upmixer crate and `Cargo.lock`; each paired manifest is identical. The
prepared-arrival manifest-file SHA256 is
`2b8690e3fecaac7b7761d3f27d572ceb7c1e7a7ad50aa0499197bce8fd1d7788` at both
start and end. The failed candidate-tone run also has identical start/end
manifests. Logs are under
`/tmp`; builds used
`CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target`
and `TMPDIR` under that target's `audit-tmp` directory.

AUD130 changed Upmixer production and test code in the bounded N<512 scope.
The shared ledger belongs to the metering owner. MIDI and IAMF remain excluded.
Astra accepted the bounded implementation; the full package gates above were
run against matching source start/end manifests. Above-512 timing remains a
separate unresolved finding. Only files under the Upmixer crate and the two
AUD130 documents changed on this track.
