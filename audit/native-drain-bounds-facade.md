# Public native drain-bound verification

## Scope

Only added `crates/sotf-plugins/tests/native_drain_bounds.rs`. No production, host, manifest, MIDI, or IAMF source changes in this follow-up. The facade builds existing transitive dependencies; the tests themselves exercise only the requested eleven DSP families.

## Executed coverage

12 tests pass, comprising 26 factory configurations and 104 processed stream epochs (baseline plus reset with one full-capacity prior call, one-frame prior call, and max-capacity-minus-one prior call), plus two dedicated final-cache EQ cases.

- Delay: feedback-zero finite support and recursive feedback legacy completion.
- Declick: delayed output.
- Denoiser: default, low latency, multiresolution, polyphonic detection.
- Hiss: spectral finite support and classic recursive completion.
- SpectralCompressor: two FFT sizes and partial cached output.
- AEC: microphone/reference two-input, one-output layout.
- Beamformer: all three algorithms, two-input, one-output layout.
- LinearPhaseEQ: both phase modes.
- Crossover: three-way FIR with 257 taps and LR24 recursive completion, expanded output layout.
- EQ: empty identity at 1x/2x/4x and recursive peak filter.
- Convolution: independently written sparse 513-frame WAV, UPC, NUPC, and direct-head NUPC.

Each configuration checks empty completion, acceptance of later input, reset, completed-state quota, idempotent scalar queries, successful full-capacity calls no greater than the saved quota, finite output, untouched suffix canaries, and bit-exact suffix equality after prior partial/full drainage. Adaptive histories are reset and the same ordinary programme/callback sequence is replayed before comparing drainage.

The dedicated EQ regression reaches the final refill, exposes only its first frame, then checks that the quota remains one and all remaining cached frames are returned. This covers `drain_remaining == 0` while the rendered output cache is still nonempty.

These tests verify call accounting and public adapter forwarding. They do not establish new finite-tail eligibility or replace each DSP crate's independent signal/support oracles. Existing recursive unsupported modes are explicitly expected to complete without native tail output. Long-IR Convolution and dynamic Resampler bounds are covered by the spectral agent separately.

## Verification

- `cargo test -p sotf-plugins --no-default-features --test native_drain_bounds`: 12 passed, 0 failed, 0 ignored.
- `cargo clippy -p sotf-plugins --no-default-features --test native_drain_bounds -- -D warnings`: passed.
- Scoped `rustfmt --check` and `git diff --check`: passed.

Logs: `/tmp/sotf-native-drain-bounds-facade.log`, `/tmp/sotf-native-drain-bounds-facade-clippy.log`.

Source is frozen for aggregate verification. No production defects found in this public matrix.
