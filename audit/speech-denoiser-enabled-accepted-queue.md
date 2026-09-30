# AUD136: enabled SpeechDenoiser accepted-program drain

**Status:** Implementation follows Astra's accepted cutoff design. Focused
package tests, the pre-edit array replay, and strict package/backend Clippy
pass. Final Astra implementation review is pending.

## Behavior

The enabled denoiser reports `TailLength::Unknown`, because RNNoise and the
high-pass response can continue recursively. At EOF, the plugin now feeds 960
zero frames through its existing backend and emits those 960 output frames in
calls of at most 480 frames. This flushes the fixed accepted-program queue and
any final partial model block. After the last output sample, it resets the
backend and discards residual recursive state under this explicit render
cutoff. It does not claim that the natural response has finite support.

The wrapper keeps terminal drain state until reset or successful
reinitialization, rejects new positive-frame input and changed `enabled`
settings, and preserves the last published analyzer snapshot. A zero-frame
process and repeated complete drain are state-neutral. Empty EOF does not freeze
the plugin, and a zero-capacity attempt on an unfinished nonempty stream fails
before changing state. Disabled EOF behavior and its `Finite(960)` semantics
remain unchanged.

The only host change is the `PluginDrainResult.complete` field documentation:
completion certifies that the plugin's declared drain policy has no later
output; for an `Unknown` response, the policy can explicitly cut off residual
state without making the tail finite. There is no host behavior or queue change.

## Changed files

- `crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/src/lib.rs`
- `crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/tests/finite_stream.rs`
- `crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/tests/timing.rs`
- `crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/tests/host_finite_stream.rs`
- `crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/README.md`
- `crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/CHANGELOG.md`
- `crates/sotf-plugins/crates/sotf-host/src/plugin.rs` (documentation only)

`plugins-denoiser/src/rnnoise.rs` is unchanged; EOF uses its existing
allocation-free `reset()`.

## Pre-edit evidence

The pre-edit source copies and manifest are retained in
`crates/sotf-plugins/target/audit-artifacts/aud136/pre-edit/source`. The source
manifest is
`crates/sotf-plugins/target/audit-artifacts/aud136/pre-edit/pre-edit-source.sha256`
(SHA-256 `002b0398f71dfedb57a853528b2e9582e9b681f2de97cb420a537b62173ba6a4`).
It contains the original plugin/backend/host sources and the relevant test and
manifest files; `Cargo.lock` SHA-256 was
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.

Before production edits, an ignored capture stored a 1,513-frame voiced mono
input and two 2,473-frame f32le outputs under
`crates/sotf-plugins/target/audit-artifacts/aud136/pre-edit/audio`:

| Artifact | SHA-256 |
| --- | --- |
| `input_mono.f32le` | `f9d289b128c03b3d2a4ce8e6701a6e283c6a969db6a81fc16184d17f62040bad` |
| `enabled_ordinary_process_zero_continuation.f32le` | `6b0b1c5c8fd26c2d4114a22619085affb9c98c6406536581eeab488b08113e07` |
| `disabled_process_and_drain.f32le` | `00ed1602cb69e77eed28af50bd93fa0e64c9aa9adb94e37105f298d87dd76b32` |

The capture log is `/tmp/sotf-aud136-preedit-audio-capture.log`, SHA-256
`cb7815b6e17de111a6bb52543a59ce9be7906dc36e14059485f7d2db51531d76`.

## Reproducer and test oracles

The original tightened reproducer,
`enabled_partial_eof_preserves_accepted_program_like_zero_continuation`, failed
before the production change. It submits 1,513 mono frames, checks that the
reference's omitted 960-frame suffix exceeds a `1e-5` peak floor, then compares
the full drain result against a separately initialized enabled plugin given
the same input plus 960 ordinary zero frames. The old drain returned 1,513
samples instead of the required 2,473. The terminal red log is
`/tmp/sotf-aud136-red-suffix-eof.log`, SHA-256
`4f0956e315f63906869b9cd7ef9aafc07a42f96f3a71d97bcc985a03255b1b0f`.

This twin is the same RNNoise implementation through a separate ordinary
process/zero-continuation route. It verifies accepted-program alignment,
partition timing, and output accounting; it is not an independent RNNoise
algorithm or speech-quality oracle. The `1e-5` suffix condition is an asserted
floor, not a logged peak measurement.

The expanded finite-stream suite covers all final 480-frame accumulator
residues (1 through 480) in mono and stereo; one-frame, 17-frame, irregular,
480-frame, oversized, and mixed callback partitions; varied drain capacities;
enabled/bypass transitions; empty and unfinished zero-capacity behavior;
rate/geometry preflight rejection; repeated completion; reset versus a fresh
instance; terminal input/control rejection; and zero-frame state neutrality.
The process and drain full vectors are compared exactly with ordinary
zero-continuation execution. Enabled `TailLength` stays `Unknown`; the disabled
AUD083 finite path remains covered by its existing 960-frame waveform tests.

An enabled heap test counts thread-local allocator allocations and frees across
the accepted process call, both drain frames, the final backend reset, a
zero-frame process, and repeated terminal drain. The test passes with zero
allocations and zero deallocations for mono and stereo. It also checks that the
final analyzer frame remains published.

## Host endpoint

`tests/host_finite_stream.rs` uses a real `DawHost` chain with
`ParametricInPlacePluginAdapter<SpeechDenoiserPlugin>`. At 48 kHz, process plus
host drain is compared sample-for-sample with a second host processing the same
input followed by 960 zero frames. It verifies accepted input plus 960 output
frames and repeated host completion. A direct 44.1 kHz chain is required to
reject initialization; no automatic rate-conversion behavior is claimed.

## Verification

Using `CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target`
and `TMPDIR` below that target:

```text
cargo test --offline --locked -p sotf-plugin-speech-denoiser --tests -- --nocapture
```

Passed 46 tests, 0 failed, 2 ignored. The ignored tests are the pre-edit capture
and the explicit baseline replay, so the latter was run separately:

```text
env CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp SOTF_AUDIT_BASELINE_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-artifacts/aud136/pre-edit/audio cargo test --offline --locked -p sotf-plugin-speech-denoiser --test finite_stream replay_aud136_pre_edit_audio_baselines_bit_exact -- --ignored --exact --nocapture
```

The replay passed 1/1 and compared the encoded sample bytes exactly for both
the enabled ordinary process path and the disabled process-plus-drain path.
Log: `/tmp/sotf-aud136-preedit-array-replay.log`, SHA-256
`eac417940815ae64d0347abfbc458af28915169674c8fe65a48d61efb67016a4`.

The final package test log is `/tmp/sotf-aud136-speech-plugin-final-tests.log`,
SHA-256 `277c14999119f890c6c0b03fd38d8745592ebdfce08d72af3bb2506bfc512bef`.
It contains 46 passes, 0 failures, and the two intentional ignored manual
tests. The baseline replay was run separately against the final test helper and
sample snapshot; `/tmp/sotf-aud136-preedit-array-replay-final.log` SHA-256 is
`ae0ae78efdb733a33da8f7881812ca2ddf645574a6462832eb490a50b1c68aaf`.

Strict all-target lint passed for both the plugin and its backend:

```text
cargo clippy --offline --locked -p sotf-plugin-speech-denoiser --all-targets -- -D warnings
cargo clippy --offline --locked -p plugins-denoiser --all-targets -- -D warnings
```

Logs: `/tmp/sotf-aud136-speech-clippy-final.log` (SHA-256
`894bf92e109fa1fca028816ffc94650c50d13381b3b4ffc7a5a6a6219bf8287b`) and
`/tmp/sotf-aud136-denoiser-backend-clippy-final.log` (SHA-256
`7585532064236b009843fc4447ec94852e5cf875fa2f72d770a8805755e1e0ee`).
`cargo fmt --check --package sotf-plugin-speech-denoiser` and scoped
`git diff --check` passed.

The selected final source/lock manifests are
`/tmp/sotf-aud136-final-start.sha256` and
`/tmp/sotf-aud136-final-end.sha256`. They are byte-identical; the manifest file
SHA-256 is `c7aca58a5882162c229d38e8b8b282b2b131c3501e18486410a1655093e786e9`.
The manifest covers the SpeechDenoiser plugin, all three relevant integration
test files, its README and changelog, the host completion comment, the unchanged
RNNoise backend, and `Cargo.lock` (SHA-256
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`). It is a
selected source manifest, not a whole-workspace digest. Astra implementation
review remains pending.

## Limits

The plugin supports mono/stereo at 48 kHz; direct 44.1 kHz initialization
rejects. The enabled recursive RNNoise/high-pass response remains unknown and
is intentionally cut off after the fixed 960-frame continuation. The same
implementation's ordinary process path is the audio/timing reference; this
batch makes no model-quality, independent-reference, or corpus-level claim.
The 960-frame figure is not generalized to other sample rates.
