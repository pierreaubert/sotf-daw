# AUD137 proposal: ABCompare child and alignment drain composition

**Status:** design accepted; implementation candidate complete and awaiting independent implementation review.

**Scope:** `sotf-plugin-ab-compare` only. The first implementation target is
same-rate child audio and the plugin's existing A/B/dry alignment rings. This
does not authorize generic unequal-rate host queues or manager changes.

## Source finding and reproduced case

`ABComparePlugin::process` advances both nested `DawHost`s, then applies the
prepared A, B, and dry `DelayLine`s before mixing. Its `Plugin` implementation
ends at `latency_samples`; it inherits the trait's `drain`, which returns
`COMPLETE` with zero frames. `PathConfig` accepts public `Plugin`, `Rack`, and
`Graph` paths, and the built-in ABCompare factory can construct a no-feedback
Delay child. Consequently the parent reports EOS completion without asking a
child to emit the accepted program still held in that path.

The public reproducer uses stereo at 48 kHz, `path_a = Plugin(delay)` with
`channel_delays_ms = [1, 1]`, zero feedback and LFO, full delayed mix,
`path_b = None`, outer mix `-1` (pure A), and AutoGain disabled. The configured
1 ms delay is 48 frames. Delay preparation rounds its ring to 64 frames
(`ceil(48) + 4`, next power of two), and the child drain contract therefore
returns 64 continuation frames. Eight accepted frames contain an impulse at
left frame 0 and a 0.5 marker at right frame 7. The independent routing oracle
is an exact 48-frame shift: its total stream is 72 frames, with those markers
at output frames 48 and 55. ABCompare reports latency 0 for this child, so no
outer latency shift is involved in this first case.

The executed public regression returns only the 8 ordinary process frames
(16 interleaved samples) and no drain frames; the expected vector has 72
frames (144 samples). The expected omitted suffix is asserted to contain a
sample above 0.25. This demonstrates a nonzero accepted-program loss, not just
a frame-count discrepancy.

The separate ordinary-output control processes 128 dense stereo frames over
callback partitions `[1, 7, 13, 31, 76]`. It matches the independent exact
48-frame delayed-input vector sample-for-sample; its saved output peak is
0.75. This preserves a meaningful pre-edit ordinary-process control while
keeping the original 8-frame EOS capture intact.

## Accepted behavior

1. Implement the `Plugin` drain hooks on ABCompare. After successful drain
   preparation, drain both nested hosts using their own declared per-call
   capacities and completion results. Apply the same-rate ABCompare alignment
   and dry delay rings to the continuing child streams, including zero input
   for a child already complete, until all emitted child frames and pending
   alignment-ring samples have been composed.
2. Keep unmatched child output in bounded, preallocated ABCompare-owned
   staging storage when the children return different frame counts. Preserve
   each path's independent emitted-frame cursor; do not pair a newly returned
   frame from one child with an earlier frame from the other. Treat a child
   that has reported complete as zero for later output times. Use checked
   geometry/capacity arithmetic and reject inadequate drain destinations
   before advancing either child or any alignment state.
3. Run returned A/B/dry frames through the existing mix semantics: selected
   path or potentiometer mix, difference mode, phase inversion, bypass
   transition, and AutoGain. Prefer a shared mixer kernel if needed to keep
   process and drain arithmetic aligned; in that case the saved ordinary
   output controls below become explicit byte-replay gates.
4. Use an explicit outer lifecycle: accepting, draining, complete, and
   reset-required after a partial child-operation failure. Once drain begins,
   reject process calls and all parameter changes until reset. Keep the
   completed state terminal too: a fresh nonempty input stream requires
   `reset()`, because `DawHost::process` only clears its own drain cursor and
   does not reset terminal child DSP. Zero-frame and malformed process calls
   do not change lifecycle state. Reset clears drain cursors, child completion
   flags, staging queues, outer delay lines, band-mask history, and restores
   accepting state.
5. Preflight-reject `begin_drain` while the recursive band mask is active or
   if any accepted samples since reset were processed with it active. A rejected
   preflight changes neither parent nor child drain state. Remembering prior
   active use is necessary: turning the mask off does not remove its recursive
   filter response. After such a rejection, callers may continue processing,
   but cannot drain that stream; explicit `reset()` starts a new stream and
   discards the old pending programme/filter history. A successful drain also
   freezes parameters, including child paths, mix, and mask settings. If one
   child has advanced and the other child operation fails, fail closed: mark
   reset-required, reject further process/drain calls, and require `reset()`
   rather than retrying with duplicated or misaligned samples.
6. Keep `ABComparePlugin::tail_length()` as `Unknown`. The host wrapper cannot
   infer a finite response for arbitrary nested children, and the optional
   band-mask filters are recursive. Completion means the declared child and
   alignment drain protocol ended for the accepted stream; it is not a proof
   of finite mathematical support. Nested children keep their own drain
   policies, and an unknown child response is never relabeled finite.

The initial exact EOS oracle keeps the default full-range band mask inactive
and AutoGain disabled to isolate finite child and alignment composition. Before
production edits, exact input, parameter JSON, and ordinary-output arrays were
captured for pure A, pure B, 50/50 mix, difference, phase inversion, bypass,
and AutoGain. Each non-AutoGain control uses 512 frames and callback partitions
`[1, 31, 128, 7, 345]`; the AutoGain control uses 9,600 frames split into
20 callbacks of 480 frames. Exact replay of all seven cases passes. Inspection
of the saved AutoGain array shows its output/input median ratio falling from
about 1.99526 in frames 0–999 to 1.02418 in frames 8,600–9,599, confirming
that gain movement is exercised. This is a pre-edit compatibility capture,
not an independent loudness-algorithm oracle. Any shared mixer extraction must
pass exact array replay for these controls.

## Acceptance evidence planned before production

- Preserve the public built-in Delay Plugin red case and dense ordinary-output
  control, plus exact source copies and f32le arrays described below.
- Add a public test factory fixture for two finite child responses with
  different latencies and tails. Compare the complete output against an
  independently computed f64 direct-convolution reference. Exercise `Plugin`,
  `Rack`, and `Graph` child configuration routes.
- Check process callback partitions including one-frame, irregular and large
  callbacks. Check drain destination partitions and repeated zero-output
  progress, both children completing on different calls, the final marker,
  and terminal completion.
- Verify fresh-stream lifecycle with a terminal-locking child fixture: process
  after completion fails without mutation until explicit reset; reset matches a
  freshly constructed plugin. Verify rejected preflight leaves children and
  rings untouched, while a child-operation failure after the other child has
  advanced enters reset-required state. Verify path/mix/mask changes fail after
  accepted drain begins.
- Exercise pure A, pure B, intermediate mix, difference, phase inversion,
  bypass and an AutoGain-enabled zero-continuation comparison. Test reset and
  new-input reuse after completion, invalid process blocks, drain-capacity
  preflight, and complete-state behavior.
- Guard `process`, drain preparation, every drain call and completion with the
  existing no-allocation test mechanism. Include both short and longer tails
  and distinct A/B latency compensation.
- Keep recursive/band-mask response `Unknown`. If the mask has been active on
  accepted samples, this version rejects drain and requires explicit reset to
  discard that stream; it does not silently truncate recursive state. Do not
  broaden into unequal-rate branch queues, host drain protocol, or manager
  state changes.
- Before any shared mixer extraction, save and later byte-replay ordinary
  process controls for pure A/B, 50/50 mix, difference, phase inversion,
  bypass, and an AutoGain-enabled 200 ms run. These seven controls were
  captured and replayed against the pre-edit source; the exact results and
  commands are recorded below.

## Pre-edit snapshot and exact evidence

Source snapshot commit: `93027970f412ce47c0cd2b8e4b7b1a5b0e5f0261`.
The source copies and SHA manifest are under
`crates/sotf-plugins/target/audit-artifacts/aud137/pre-edit-src/`:

| Saved file | SHA-256 |
|---|---|
| `abcompare_plugin.rs` | `c67487204bef00f7a01ca1a27946accf7badda07cfc1ca15c4abaf5acff74cf1` |
| `factory.rs` | `b938f2b8e97ef3194bf8c46c43ceda62dd7dda7ddf10ebffc3cd9e67973d7ada` |
| `config.rs` | `a2ac96936d3498376484623a464f3dd861316bf0ad5e2934525a04917f22e9b4` |
| `delay_line.rs` | `55d89c3a4a32b69519a29c1da1bc399b41c40eab752c3efd39abcb53d9e458e1` |
| `delay_plugin.rs` | `36a222b583467ff45195eaeddd8a213a615e783c905bdaa6da95247ad1e6a237` |
| `plugin_trait.rs` | `c6c77cf2e8a812534f74c8d0816e824862cd67a7894b1c4044436729fd29f346` |
| `daw_host.rs` | `3669edc4a69c5f7bfb894a3fdbd6dc8ca6242425b3dbaa8d9d69da31537bf40e` |
| `Cargo.lock` | `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5` |

Audio captures are under
`crates/sotf-plugins/target/audit-artifacts/aud137/pre-edit-audio/`.
The 8-frame public red input is `f98f2223ab7d16141432befa0133777a8b249aa187def279f1b354e87b8bbf78`;
the saved pre-edit process/emitted output is
`f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b`;
the drain array is empty (SHA-256
`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`). The
72-frame analytic expected output is
`c03d636e67d273ebc2cc1802441ad01adffcf77d1eb88d4ea1103ac0d6251b42`.
The 128-frame dense process control output is
`b6cd6798f3adeb63c8fbfcb6c8b2f507eeb9fd6d2de378cd814cbfb3e255184e`, equal
to its analytic reference byte-for-byte; input hash is
`f06c0a1a9546c5fb7e37e71c791d88866220a2b24a6c7836da47dc7f8406b7e6`.

Focused command form (all runs used the absolute shared workspace target and
`TMPDIR` below that target; capture commands also set the artifact directory):

```sh
CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp \
SOTF_AUDIT_BASELINE_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-artifacts/aud137/pre-edit-audio \
cargo test --offline --locked -p sotf-plugin-ab-compare --test aud137_finite_stream \
  capture_aud137_pre_edit_finite_delay_baseline -- --ignored --exact --nocapture
```

The capture passed 1/1 (`/tmp/sotf-aud137-preedit-capture.log`, SHA-256
`4594cf0294437977bc238cc6c0676a9d3339825923e76c09dce010c163562302`). The
dense capture passed 1/1 (`/tmp/sotf-aud137-preedit-dense-capture.log`, SHA-256
`3a8e19a902ab12c895fbce774239011b39790f9b5d66419ad2af5c37c2c027c5`), and
the ordinary-process analytic control passed 1/1
(`/tmp/sotf-aud137-preedit-process-control.log`, SHA-256
`621b7bae35a0fd204bc4394daaf24a64d931612502fd962d76b6f756b35bf681`). The
expected-red public drain test exited 101 with actual 16 versus expected 144
interleaved samples (`/tmp/sotf-aud137-public-red-final.log`, SHA-256
`0e43fef081bea3203a459952a37671e90bc8468b22d1698b0a851080f18512da`).

The source-copy manifest is
`crates/sotf-plugins/target/audit-artifacts/aud137/pre-edit-source.sha256`
(SHA-256 `1da15762358fef0c2c623645b6cca39e770dd4e3697ed5fd20ef4093cb84e3ab`);
it records the seven source files above and `Cargo.lock` from the baseline
commit. The complete input/output/control capture manifest is
`crates/sotf-plugins/target/audit-artifacts/aud137/pre-edit-audio/aud137-pre-edit-audio.sha256`
(SHA-256 `5ba91080ae9685be9665ae54f3667cfa2e18f4c62c2d76a43d207683f5adc983`).

The ignored mixer-control capture and byte-replay both passed 1/1 using the
saved pre-edit source. Logs:

- Capture: `/tmp/sotf-aud137-preedit-mixer-controls.log`, SHA-256
  `e5a3711d80aaa240e4095db8785f72625fdca1b93b7c1e41d85cb83a51f410f8`.
- Replay: `/tmp/sotf-aud137-preedit-mixer-controls-replay.log`, SHA-256
  `229f9cb212382ec5c82872eab67b51af5a556f21ab9f0194d89a84af3197ad46`.

Both commands use the absolute shared target, offline locked dependencies, and
the baseline directory environment. Capture command:

```sh
flock /tmp/sotf-daw-audit-cargo.lock env \
  CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp \
  SOTF_AUDIT_BASELINE_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-artifacts/aud137/pre-edit-audio \
  CARGO_NET_OFFLINE=true cargo test --offline --locked -p sotf-plugin-ab-compare \
  --test aud137_finite_stream capture_aud137_pre_edit_mixer_control_vectors \
  -- --ignored --exact --nocapture
```

Replay uses the same command and environment with
`replay_aud137_pre_edit_mixer_control_vectors` in place of
`capture_aud137_pre_edit_mixer_control_vectors`. The audit test source
containing the capture and replay harness hashes to
`c2877e917dc73a2222074a5090c824bf645f1c18a369d0bdf2a8a4be58d776f4`.
Implementation and focused evidence are recorded in
`audit/abcompare-finite-stream.md`; production and test paths are now changed.
The design was accepted before implementation. Independent implementation
review is pending.
