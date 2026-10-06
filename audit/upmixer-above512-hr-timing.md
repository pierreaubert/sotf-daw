# AUD-132: above-512 Upmixer HR/main timing

## Status

The bounded source-tag retiming implementation is accepted by Astra-medium.
Focused and full Upmixer package tests, strict all-target Clippy, package
formatting, and the coordinated offline locked workspace gate pass. The fixed
HR input delay is retained.
N=512 continues on its prior path, while accepted AUD-129 minimum geometry and
AUD-130 N<512 source-tag behavior remain covered. This report does not claim
high-resolution feature quality or CPU performance.

## Captured pre-edit baseline

Before production edits, the full Upmixer crate and lock manifest was saved at
`/tmp/sotf-aud132-source-baseline.sha256` (aggregate SHA256
`7f56892a35e7c43e237aa735dde14a2594fec381d983f8041e5cfd94dc4ba9d6`). A
complete source copy is retained under
`crates/sotf-plugins/target/audit-artifacts/aud132-preedit/source-snapshot/`.
The package-plus-lock manifest for that copy is
`/tmp/sotf-aud132-preproduction-source.sha256` (aggregate
`117647951e071c90d8221630d256ac803ddae905518c50fef9ad0494a5de8af0`). The
captured production files are `upmixer_plugin.rs`
(`c8076236e553f2bf76f14dcb0edd2843e3f0edf28045b71c6fea827c37afedb6`) and
`hr_processing.rs`
(`c59dc614967621371983780d4a4fcb4de5ee94854c271d6f550aba76c268baaf`).
Cargo.lock at that capture was `3bfd0812c9b551ef4f62d306a2d447f5ec175f6a9fc69907b06a5d15827c9c0a`.

Pre-edit vectors were produced through the actual plugin stream path with
neutral stereo routing and deterministic test-only gain scheduling. Full
output and main-only stereo arrays are retained beneath
`crates/sotf-plugins/target/audit-artifacts/aud132-preedit/`. The original
capture log is `/tmp/sotf-aud132-preedit-render-capture4.log` (SHA256
`9650582bac82f6689591629e99545980272ecb0fe4fd78bd4c6a4a9dd98ccf32`).

The pre-edit N=2, 256, and 512 outputs have complete digests. The post-edit
control test reproduces them exactly on x86. See the
[cross-architecture control audit](upmixer-retained-control-portability.md)
for recovered full-vector fixtures and the strict ARM64 numerical comparison:

| N | HR disabled FNV-1a | HR enabled FNV-1a | Emitted frames, off/on |
|---:|---:|---:|---:|
| 2 | `02ddc52702cb7e9f` | `5605bd821b4857ee` | 4609 / 4864 |
| 256 | `b106d5e545f3b14f` | `7aaff37f8e9d2070` | 4736 / 4864 |
| 512 | `b6b9e4eaa0d6739b` | `941ff0e437b3e7f3` | 4864 / 4864 |

Pre-edit above-512 impulse outputs:

| N | D | Main peak | HR peak before | Emitted frames | Full-output SHA256 |
|---:|---:|---:|---:|---:|---|
| 1024 | 256 | 1024 | 1280 | 9728 | `96b08b1f27cbe589734db42ab395038f67a16c317cb274d8e4e46aee209f1f05` |
| 2048 | 768 | 2048 | 2816 | 11264 | `7b5a02ac090ff629e0802382f7dd4b67845d79ae8bc00f1f726770aaa48ae3d2` |

## Implementation and measured behavior

For N>512, HR samples now retain source-frame tags through startup discard and
mix only against the matching main source frame and prepared gain. The
existing D-frame HR input delay remains. Re-enable clears its delay history,
partial input, tag/sample queues, and pending gains before restarting the tag
clock. N=512 remains on the legacy path. The explicit missing-tag error stays
in force.

The focused live impulse matrix moves the HR arrival to the main source latency:

| N | D | HR peak before → after | Callback partitions | Result |
|---:|---:|---:|---|---|
| 1024 | 256 | 1280 → 1024 | one-frame, irregular, 512, whole | exact saved full/main retime at whole stream; partition delta ≤3e-8 |
| 2048 | 768 | 2816 → 2048 | one-frame, irregular, 512, whole | exact saved full/main retime at whole stream; partition delta ≤3e-8 |
| 4096 | 1792 | 5888 → 4096 | irregular, 512, whole | full-vector same-run legacy retime delta 0 |
| 8192 | 3840 | 12032 → 8192 | irregular, 512, whole | full retimed vector delta 0 at reduced input; cap inactive |

The N=1024 and N=2048 corrected full output arrays match the saved pre-edit
main plus HR arrays retimed by D exactly at whole stream (maximum delta 0).
The initial N=8192 full-vector comparison showed a roughly 0.2066 mismatch,
but that fixture did not establish that the safety cap stayed inactive. The
corrected N=8192 comparison uses a 0.005 impulse, enables the 3 dB cap, checks
all three combined output peaks against the cap, and requires the final cap
scale to remain exactly 1.0. Across irregular, 512-frame, and whole-stream
callbacks, the tagged output equals the main-only output plus the legacy HR
contribution shifted by D with maximum delta 0. The maximum tagged/legacy/main
peaks are 0.023416590 / 0.019479090 / 0.003937500 against cap 1.412494183;
each route emits 20,480 frames total, including EOS. This confirms
the old full-vector retiming relation at N=8192 under a controlled linear
range and resolves the earlier comparison gap. The independent no-drain
comparison also remains: tagged queue fill 4096, legacy fill 7936, tag-zero
offset 0, and 4096 compared raw HR samples at offset D have maximum delta 0.

For the exact HR-bin tone k=57 and the second noninteger tone, all projections
use the same absolute emitted-frame windows and the fixed 1% residual bound.
Projected magnitudes are nonzero. Exact-bin residuals are 6.47e-6 at N=1024
and 0.00640–0.00672 at N=2048. Noninteger-bin residuals are 1.2e-7 at
N=1024 and 0.00636–0.00669 at N=2048. The maximum phase error across tested
callback partitions is 5.34e-6 rad.

The nonstationary gain fixture exercises one-frame, irregular, 512-frame, and
whole-stream callback partitions. Tagged output compared with its independent
source-time gain oracle has maximum error 5.96e-8 at N=1024 and 8.94e-8 at
N=2048. Compared with legacy drain-time gain pairing, the fixture differs by
0.244 and 0.366 respectively, showing the test detects clock mispairing.

Active AutoGain on/off tests at N=1024 and N=2048 process exactly 24,000
accepted frames, check the delayed reference positions across every callback,
and confirm a nonzero HR contribution. Resume tests clear populated delay and
partial-hop history before further processing and EOS. Reconfiguration tests
cover 512→1024 and 1024→512 with the expected latency, tail, and impulse peak.

## Rejected candidate

Removing the HR input delay moved the N=2048 impulse, but failed the declared
tone criterion: measured residual 0.014197 (1.4197%) exceeds the unchanged
0.01 ceiling. The production delay remains. The regression is retained as an
ignored/manual rejected-candidate test with its fixed assertion; an ignored
test is not counted as a pass.

The explicit rerun reproduces the rejected candidate's N=2048 residual
`0.0141972 > 0.01`; log
`/tmp/sotf-aud132-rejected-candidate.log` (SHA256
`e5b0d5fceb477288ff0b0b9758848a306575e6d7ac2a47fe6958ce116527c0e2`).

## Fixed-gain phase fixture and vector oracle

The first complete package run used a 0.5-amplitude left-only tone. Once HR and
main were aligned, their combined peak entered the final safety cap. The test
subtracts a separately rendered main-only reference, so this nonlinear cap
made the inferred HR-only residual invalid: it reported 3.0389% at N=1024 and
4.9763% at N=2048. The same test-only fixed-gain fixture through the
pre-correction mixer measured residuals 0 and 0.0019%, respectively, because
its HR/main phase offset kept the sum below the cap. This failure is retained
in `/tmp/sotf-aud132-package-test.log` (SHA256
`4e01c0cecf5d9b7c44e497e215b15df64ac4d482f70f69ef05f063107f332c59`); it is
not passing evidence.

The phase fixture now uses 0.1 input amplitude, pins HR gain to a constant
test schedule, and asserts the combined peak stays below 0.5. It retains the
same 1% residual ceiling. Corrected HR residuals are `0` at N=1024 and
`0.000019` at N=2048, with relative phase within about `1.3e-8` rad of zero.
A samplewise mono-left comparison of tagged HR output against the
pre-correction HR output shifted by D has maximum error `2.98e-8` at both
sizes. A separate no-drain accumulator comparison matches raw HR samples
exactly (maximum delta 0) for 512 frames at N=1024 and 1024 frames at N=2048.
The corrected phase log is `/tmp/sotf-aud132-phase-lowlevel.log` (SHA256
`2b1661437df8756f77e55ee0b99235699a452c61db24ad3d651310e71cb4976d`); the
final shifted-vector/raw-ring log is `/tmp/sotf-aud132-rawtone2.log` (SHA256
`2595be3c1587edb1d5a7bb6c375339d04895c409cb3ee143242d6d51e22210f1`).

## Current verification and limits

`cargo test --offline --locked -p sotf-plugin-upmixer` passed 184 package tests,
with 0 failures and 3 intentionally ignored tests. This includes the heap
activity callback tests. Strict all-target Clippy passed with warnings denied;
`cargo fmt --package sotf-plugin-upmixer -- --check` passed. Test log
`/tmp/sotf-aud132-final2-package-test.log` has SHA256
`d02c79599c89a2d605406d6f1988541be77093cdaa8a5655947a3e1742c3f543`, and
Clippy log `/tmp/sotf-aud132-final2-clippy.log` has SHA256
`095e0ad06e5f9c5b3cf709fd2f8793950590020836fdacbda5f702fbad1b9fa7`.

The Upmixer package-plus-lock start and end manifests are
`/tmp/sotf-aud132-final2-start.sha256` and
`/tmp/sotf-aud132-final2-end.sha256`; they match at aggregate SHA256
`0cc6e67e47820317d2be69e210c96dd608ad981465a998a3f686e6b2eef8800c`.
Cargo.lock is `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.

After the N=8192 test edit, strict all-target Clippy passed at
`/tmp/sotf-aud132-final-clippy.log` (SHA256
`a95ab8d362057ec9662fcabd3839989d4d9bf484bd3e6018c3ab080ed37096ed`). The
coordinated offline locked workspace nextest, excluding MIDI and IAMF, passed
6071 tests with 0 failures, 15 skipped, and 2 slow across 359 binaries. Log
`/tmp/sotf-aud132-aud131-workspace-final.log` has SHA256
`1e0614b64e8ca15361b916ef10ed348e9d6794f75a785759b6f93b8b9de2c52a`. Full
tree start/end manifests `/tmp/sotf-aud132-final-workspace-start.sha256` and
`/tmp/sotf-aud132-final-workspace-end.sha256` match at aggregate SHA256
`1e4bb4097b37cc238911b211606d76f8e3c4f071de6683b5a388e9c0a61d6aeb`;
Cargo.lock SHA256 is
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
The final gate-result and acceptance notes in this report were appended after
the workspace run. That was documentation-only: the post-gate package-plus-
lock manifest `/tmp/sotf-aud132-postgate-package.sha256` still matches the
tested capfix package manifest at aggregate SHA256
`b3c4d7d54063e203edf613d32c4ad6e47a3dc4426171f8c171f423a770671ad7`.

The follow-up N=8192 capped full-vector run also passes. Its package-plus-lock
manifests `/tmp/sotf-aud132-n8192-capfix-start.sha256` and
`/tmp/sotf-aud132-n8192-capfix-end.sha256` match at aggregate SHA256
`b3c4d7d54063e203edf613d32c4ad6e47a3dc4426171f8c171f423a770671ad7`.
The focused log `/tmp/sotf-aud132-n8192-capfix-test.log` has SHA256
`0315e048e1934aaa25de1b3c63e34899a464c4274f0cd1a8cef7fd4b7af448d5`.
Astra-medium accepted the scoped AUD-132 implementation after reviewing the
updated report, focused evidence, strict Clippy, and broad workspace result.

This evidence covers neutral stereo routing and the tested HR direct
contribution. It does not prove general high-resolution spatial quality or
CPU cost; the N=8192 vector oracle is a same-run reconstructed control, not an
archived historical whole-plugin output. MIDI, IAMF, host metering, and
shared-ledger changes are outside this track.
