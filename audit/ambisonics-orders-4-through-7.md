# AUD-133: Ambisonics orders 4 through 7

Status: the bounded AUD133 implementation was accepted by Astra on
2026-09-29. The coordinated workspace gate, focused engine/bridge Clippy and
affected AIFF/capacity reruns pass. This report does not close the wider spatial
audit or claim complete higher-order parity across all plugin routes.

## Scope and remaining routes

This batch extends the decoder's ACN/SN3D named-layout path to orders 4–7:
25, 36, 49, and 64 input channels. It preserves the existing order 1–3
coefficient tables, channel ordering, matrix and dual-band behavior, structural
parameter rules, and AUD-051 finite-tail contract. The physical output remains
limited to 16 channels; sparse named layouts do not reproduce all 25–64 input
modes.

The engine now admits 64 input channels while retaining a 16-channel output
limit. The integration route decodes an actual 64-channel AIFF, feeds it
through `EmbeddedAudioEngine` and the order-7 plugin, and compares the full
16-channel waveform to a channel-specific reference. A 65-channel AIFF can be
decoded by Symphonia but is rejected by engine admission. Service PCM admission
is separately tested with literal 64-channel accepted and 65-channel rejected
cases, including the decoded `AudioSpec` and samples. No 64-channel WAV claim is
made: the current Symphonia RIFF/WAVE channel mapper rejects standard layouts
above 26 channels.

The player graph regression edits the order through the production model
reconciliation path, serializes and rebuilds the canvas connections, and checks
that channel 63 reaches the decoder and the target output stays at 16 ports.
This is a model/canvas integration test, not a mounted UI click test. Native
plugin bridge width coverage, FFI 64-to-16 routing, NIH's fixed 4-to-6 wrapper,
hidden structural controls, and custom user-authored layouts remain open; see
[`current-feature-route-coverage.md`](current-feature-route-coverage.md).

## Numerical and lifecycle evidence

The production basis supports ACN indices 0–63 with the existing SN3D
normalization. The numerical tests use test-owned differentiated Rodrigues
polynomials rather than the production recurrence, explicit degree/order
enumeration, and generic directions, axes, and poles. The physical solve is
checked against a test-owned one-sided Jacobi SVD, including convergence,
retained-triplet reconstruction, orthogonality, rotated near-cutoff modes, and
rectangular null-space cases. No standalone antipodal-parity assertion is
claimed.

AllRAD uses the accepted deterministic grid sizes 256/384/512/512 for orders
4–7. The independent quadrature checks normalized Gram condition, full virtual
rank, and solve residual; a separate f64 SVD reports rank and condition of the
active physical output rows. These diagnostics remain distinct from the
existing `DecodeQuality` fields, whose AllRAD rank/condition describe the
virtual grid. The same-grid coefficient reference independently checks the
solve and physical VBAP composition; it does not claim an independent VBAP
implementation.

The order-7 plugin test covers all 64 basis channels for both algorithms,
including channel 63, dense finite and non-finite blocks, single/dual-band
processing, irregular partitions, reset, exact frame counts, and the committed
AUD-051 tail/EOS behavior. The pre-edit lower-order fixture captured 192
combinations of orders 1–3, named layouts, algorithms, max-rE states,
dual-band modes, and callback partitions. The post-edit output arrays match
those captured lower-order outputs bit-for-bit. The clean source baseline and
captured arrays are retained at:

`crates/sotf-plugins/target/audit-artifacts/aud133-preedit-9302797/`

The committed independent baseline test is included in the package gate; the
capture helper itself remains ignored and is not counted as a passing test.

## Engine capacity and memory bounds

The fixed maximum engine block remains 8192 frames. At 64 inputs and 16
outputs, the enlarged global decoder-side vector payloads account for about
36 MiB, versus 9 MiB at the former 16-input bound. The six processing-side
buffers account for about 24 MiB versus 6 MiB, for a combined prepared-vector
bound of about 60 MiB versus 15 MiB (+45 MiB). HAL staging adds approximately
2 MiB versus 0.5 MiB at those widths. These figures cover the named prepared
vector payloads, not total engine RSS, allocator overhead, or all engine
objects.

The capacity tests create `ProcessingState` at a narrow width, then reuse its
prepared current/previous/recycle vectors at 8192 frames × 64 inputs while
checking capacity and pointer stability. This proves these vectors do not grow
on that path; it is not a whole-callback allocator/deallocator guard.

## Performance

Criterion was run on an AMD Ryzen Threadripper PRO 3995WX (64 cores, SMT on,
boost enabled; reported CPU scaling was 54%). `9.1.6` in the benchmark names is
the target speaker layout. These measurements are host- and fixture-specific;
they are not worst-case execution-time bounds.

The 100-sample order-7 run used 3 seconds warm-up and 5 seconds measurement.
Criterion's displayed center estimates were approximately:

| Operation, order 7 | Mode matching | AllRAD |
|---|---:|---:|
| Matrix build | 119.2 µs | 13.255 ms |
| Construct + initialize | 245.4 µs | 26.514 ms |
| Process, 512 frames, single-band | 396.5 µs | 396.6 µs |
| Process, 512 frames, dual-band | 1.044 ms | 1.048 ms |

A separate instrumented 2048-call callback sample recorded p50/p95/p99/max of
398/405/409/430 µs for mode-matching single-band; 1.040/1.053/1.070/1.219 ms
for mode-matching dual-band; 398/403/408/609 µs for AllRAD single-band; and
1.059/1.069/1.093/1.209 ms for AllRAD dual-band. These observed maxima include
measurement and scheduler variation; they are not WCET bounds.

Lower-order comparisons used matching 10-sample, 1-second warm-up and
1-second measurement runs before and after the change. Direct comparison of
the center estimates in those two logs gives these ranges:

| Stage | Order 1 range | Order 3 range |
|---|---:|---:|
| Matrix build | −2.93% to +5.82% | −1.09% to +2.26% |
| Construct / initialize | +0.80% to +3.45% | −0.94% to +0.67% |
| 512-frame processing | −3.23% to +2.11% | −1.03% to +1.74% |

This is not a universal unchanged-cost result. In particular, direct pre-edit
to paired-run center estimates show AllRAD matrix build without max-rE rising
5.82% at order 1 and 2.26% at order 3. The paired run's printed Criterion
`change` compares against the immediately preceding same-target 100-sample
candidate run, not the preserved pre-edit run; those separate values are
+8.14% and +2.36%, respectively. The logs also show outliers, especially in
the 10-sample process groups. Treat these as noisy point estimates rather than
causal or statistically settled regressions.

Exact Criterion logs:

- Pre-edit: `/tmp/sotf-aud133-pre-edit-criterion.log`, SHA-256
  `7c0ff5a0bf64e1784b59cd41f556f8299305a17c802442bea8dcc2046882272c`.
- All-order implementation: `/tmp/sotf-aud133-post-edit-criterion.log`,
  SHA-256 `c45fbadce373562c6676eca797b9d2495e64c816de3c8615869cd211ae5248a2`.
- Paired lower orders: `/tmp/sotf-aud133-paired-lower-order.log`, SHA-256
  `93a3e52ce040828d83020ef670b1be88bfbfd0a661d69f4cdfc48da4cb62c2b7`.

The pre-edit and post-edit runs use the same benchmark fixture and target
layout; the pre-edit source is the clean `9302797` baseline, while the paired
run is the candidate source. The paired post-edit run uses the same flags and
package/lock snapshot as its logged candidate run; candidate measurements are
not represented as a full historical whole-workspace comparison.

## Commands and current gate status

Focused passing gates include. The DAW commands below are reproducible from
the workspace root with this shared target configuration (MIDI/IAMF remain
excluded from broader audit gates):

```sh
export CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target
export TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp
export CARGO_NET_OFFLINE=true
```

- `cargo test --offline --locked -p sotf-plugin-ambisonics` — 59 library, 21 public integration, 2 tail, and 1 independent baseline test passed; the separate capture helper is ignored. Log `/tmp/sotf-aud133-ambi-package-final.log`.
- `cargo clippy --offline --locked -p sotf-plugin-ambisonics --all-targets -- -D warnings` — passed; log `/tmp/sotf-aud133-ambi-clippy-final.log`.
- Focused engine configuration and prepared-capacity tests passed (one each): `/tmp/sotf-aud133-engine-config-focused.log`, `/tmp/sotf-aud133-decoder-capacity-focused.log`, and `/tmp/sotf-aud133-processing-capacity-focused.log`.
- `cargo test --offline --locked -p sotf-engine --test order7_aiff_engine_route` — both AIFF64 waveform and AIFF65 engine-admission tests passed; log `/tmp/sotf-aud133-aiff-route-focused.log`.
- `cargo test --offline --locked -p sotf-engine --lib test_create_decoder_from_source_service_stream` — literal 64-channel service PCM accepted and decoded spec/samples verified; literal 65-channel service PCM rejected. Passed 1/1; log `/tmp/sotf-aud133-postbroad-service64.log`, SHA-256 `1f027c14b2bb0f16c19b64128eccc87f18ba67bcaeceb9680b3cb6b5b4fdbe01`.
- `cargo test --offline --locked -p sotf-plugins --lib ambisonics_catalog` — both factory tests passed, including explicit order/width mismatch rejection; log `/tmp/sotf-aud133-postbroad-factory.log`.
- `cargo test --offline --locked -p sotf-plugins --test all_plugins_channel_count_support catalog_default_channel_output_contracts_hold` — passed 1/1; log `/tmp/sotf-aud133-postbroad-catalog-defaults.log`.
- `cargo test --offline --locked -p sotf-plugins --test render_plan_snapshots ambisonics::all_profiles` — passed 1/1 after updating only Ambisonics order-range snapshots from 3 to 7; log `/tmp/sotf-aud133-postbroad-ambi-snapshot-final.log`.
- The GPUI component integration test selected and passed 1/1 under sibling resolver lock `475d5890fddf40c8ca8361c6455057c1f3701c713c5844f2cdfda5a53303fb4e`: `aud133_order7_graph::order_edit_reconciles_64_input_ports_through_canvas_roundtrip`. Log `/tmp/sotf-aud133-graph-test2.log`, source/lock manifests match aggregate `1fb47df44c82aedbaeacc4f1d9ff80d6f1d6a96855ef8b0b12fa0442a3619e53`. At the time of that historical run, the original sibling lock was restored and verified at `2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f`. Root has since retained the reviewed `475d5890…` lock in the sibling worktree; the current sibling lock bytes match the tested resolver.

The latest coordinated workspace gate, after focused Ambisonics and
convolution corrections, ran 6,100 tests across 362 binaries: 6,100 passed,
0 failed, 19 skipped, in 272.300 seconds. Its log is
`/tmp/sotf-aud133-134-workspace-rerun.log`, SHA-256
`4c417bab556373409374329b1de38b30abab67cdbafc36908037eb23bb4dd449`. The
2,676-file start/end manifests match exactly at aggregate SHA-256
`85dcb633fc9d01db143f4bf5c598b565775ea28c2c4ad49b1a4dd92b6ba6d91c`; the
DAW lock is `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.

The immediately preceding non-green gate ran 6,100 tests: 6,093 passed,
7 failed, 19 skipped. Its log is `/tmp/sotf-aud133-134-workspace.log` (SHA-256
`3184dabce32c53e8716185a4556747f9aedc92567dd1b715b9de73034a263c5e`). Five
failures were in this Ambisonics/64-channel batch and now pass focused tests;
the two AUD-134 failures were corrected on the parallel track. Its matching
source/lock manifests aggregate to
`aa9444f069da77e0c8a9a362a24f1c69ebc59c2eb076eea4a6e1a181b41669aa`, apart
from two generated `.snap.new` files in the raw end manifest. The green rerun
supersedes that historical result.

After the broad gate, three test-only strict-Clippy findings were fixed. The
AIFF route rerun passed 2/2 (`/tmp/sotf-aud133-postlint-fix-aiff.log`, SHA-256
`65a418caf80a8a56082701282fc45949e27ac01bd41597cdc3b4605ed2552772`); the
prepared-buffer capacity test passed 1/1 (`/tmp/sotf-aud133-postlint-fix-capacity.log`,
SHA-256 `a5e49a292d3884b50407d0f0c5d740f257505bf83bc4fa73a4fe32b18bda8490`).
The changed scratch-preparation test also passed 1/1
(`/tmp/sotf-aud133-postlint-fix-scratch.log`, SHA-256
`6ab4bed1c4770404303f225e7b0f34f93b4db7c32575587a7306a1480132b947`).
`cargo clippy --offline --locked -p sotf-engine -p plugins-bridge --all-targets
-- -D warnings` passed (`/tmp/sotf-aud133-postlint-fix-clippy.log`, SHA-256
`fabcf3092709263303a15f62441120e7d43c1056055877d594b90d51ffb7c48e`). These
changes affect only test iteration/setup; the two-file+lock start/end manifest
is byte-identical at `/tmp/sotf-aud133-postlint-fix-start.sha256` and
`/tmp/sotf-aud133-postlint-fix-end.sha256`.

The scoped file manifest before the final test-only Clippy cleanup (including
the 64/65 service-stream fixture) is
`/tmp/sotf-aud133-focused-final2-manifest.sha256`, SHA-256
`4d3282f9ae97a267350ed0dfd1ad6bedc7cc3c80056aab4091869a3d1a406dc8`. The
post-cleanup two-file+lock start/end manifests match; exact hashes are at
`/tmp/sotf-aud133-postlint-fix-end.sha256`. The root's current whole-tree
manifest is `/tmp/sotf-aud133-134-postlint-current.sha256`, aggregate SHA-256
`d408e22055188bbec6031e416c71ac3240f3fa86ac20ab2e1254012465155bfc`; it
differs from the green workspace snapshot only in the two test fixtures.
Cargo.lock remains `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.

## Full-audit limits

This batch covers named-layout decoder orders 4–7 and bounded 64-input engine
admission. It does not establish arbitrary-layout quality, equivalence to an
external decoder, standard 64-channel WAV support, mounted graph UI behavior,
or native plugin-route compatibility. The current feature-route inventory and
follow-up issues remain authoritative. The official EBU corpus is not acquired
or used. MIDI and IAMF remain excluded.
