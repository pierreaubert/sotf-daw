# AUD-131 implementation and validation record

Updated 2026-09-29. Scoped implementation and source review are accepted by
Astra. The first offline workspace run completed with one AUD-132 Upmixer
candidate failure, preserved below as historical evidence. A later coordinated
run after that candidate was reclassified passed the workspace gate and
supersedes the earlier qualification for the final AUD131/AUD132 snapshot.

## Behavior implemented

The shared SOFA loader now accepts finite fractional and signed `Data.Delay`
values in its existing `[I,R]` and `[M,R]` forms. Missing, zero, and integer
delays retain the exact integer-copy path. For fractional inputs it decomposes
each delay into `q = floor(d)` and `f = d - q`, chooses the common causal offset
`max(0, 32 - floor(minimum_delay))`, and computes integer prefixes from those
components. It does not add the offset to the original floating delay or form
`32 + f` to locate the response. The 65-tap Hann-windowed sinc is normalized
before storing f32 coefficients; its sinc uses the analytic limit near zero.
Checked sizing and the existing 256 MiB materialization limit remain in force.

The loader reports the common shift in source-rate samples and seconds.
Binaural carries it through source-rate resampling, and XTC exposes the active
value after filter preparation. Failed consumer replacement keeps the prior
active filters and diagnostic. No host/engine scheduler latency protocol or
audio callback algorithm changed. Negative-delay rebasing is explicit SOTF
causalization behavior; it is not a claim that every SOFA convention requires
or permits negative values.

## Evidence

- Frozen pre-edit loader and complete Binaural/XTC output arrays were captured
  before production edits. The recaptured integer-only loader output (138
  f32s) and five consumer arrays (640 f32s each) compare byte-for-byte with
  `diff -r /tmp/sotf-aud131-preedit/arrays /tmp/sotf-aud131-postedit/arrays`.
  The pre-edit array hashes are in `/tmp/sotf-aud131-preedit-arrays.sha256`;
  the comparison exited zero.
- The independent public frequency-domain oracle checks DC separately,
  magnitude within 0.03 dB, and unwrapped phase-delay error within 0.001
  samples through 0.45 cycles/sample. The oracle measures the materialized
  response against analytic delay rather than reusing production coefficients.
- Boundary coverage includes the smallest positive and negative subnormal
  f64 values and the representable values immediately adjacent to +1 and -1.
  It requires finite materialized samples and checks DC, magnitude, and
  phase-delay against the analytic response. This catches zero-over-zero sinc
  evaluation and the old rounded `delay + offset` one-sample error.
- Binaural public tests cover 44.1-to-48 kHz and 48-to-96 kHz conversion,
  complete EOS response, reset, callback partitions and failed replacement.
  XTC tests cover the analytic plant and plant/filter cascade, FFT support
  boundaries, and transactional replacement.

Executed focused gates on the final AUD-131 source:

- `CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target cargo test --locked --offline -p sotf-plugins --test sofa_delay` — **16 passed, 1 ignored**. Log: `/tmp/sotf-aud131-public-sofa-boundaries-final2.log`.
- `CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target cargo clippy --locked --offline -p sotf-host -p sotf-plugin-binaural -p sotf-plugin-xtc -p sotf-plugins --all-targets -- -D warnings` — passed. Log: `/tmp/sotf-aud131-clippy-boundaries-final3.log`.
- `rustfmt --edition 2024 --check` on the eight AUD-131 Rust paths — passed. Log: `/tmp/sotf-aud131-rustfmt-scoped-final.log`. A workspace-wide `cargo fmt --all -- --check` also reports formatting differences in dirty third-party rubato files and the parallel Upmixer test file; those files were not changed by this track.

The final AUD-131 source-and-lock manifest is `/tmp/sotf-aud131-source-final.sha256`, aggregate SHA-256 `37df45680a7704c83861d0969a033fb60f8ff313b3b597cab0e887105d7575d9`.

## Workspace gate qualification

The required offline command was run on a frozen tree, excluding MIDI and
IAMF:

```sh
CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp \
CARGO_NET_OFFLINE=true cargo nextest run --offline --workspace \
  --exclude sotf-midi --exclude sotf-iamf --no-fail-fast --status-level fail
```

The earlier AUD131-only source snapshot completed **6,064 passed, 1 failed,
14 skipped across 359 binaries in 271.106 seconds**. Its sole failure was the
parallel AUD-132 test
`sotf-plugin-upmixer::above_512_hr_delay_baseline_and_source_aligned_candidate_tone`
at N=2048, where the source-aligned candidate has a 0.0141971849 HR tone
residual against a 0.01 assertion. This was a known rejected candidate that
was still active in the suite at the time; it is now retained as explicit
rejected-candidate evidence. This historical failure is outside the SOFA
source paths and has been superseded by the following green gate. Full log:
`/tmp/sotf-aud131-workspace-final.log`.

The pre/post workspace manifests are `/tmp/sotf-aud131-workspace-start.sha256`
and `/tmp/sotf-aud131-workspace-end.sha256`; both have aggregate SHA-256
`021d0b339aa31701394bd4223ee73dfecd7dd22759a7f857377c9c8b3143a1c4`. The
DAW `Cargo.lock` stayed at SHA-256
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.

The subsequent coordinated offline/locked workspace gate ran on the final
AUD131/AUD132 source snapshot, with MIDI and IAMF excluded. It completed
**6,071 passed, 0 failed, 15 skipped across 359 binaries in 269.739 seconds**.
The final tree start/end manifests are
`/tmp/sotf-aud132-final-workspace-start.sha256` and
`/tmp/sotf-aud132-final-workspace-end.sha256`; both aggregate to
`1e4bb4097b37cc238911b211606d76f8e3c4f071de6683b5a388e9c0a61d6aeb` and have
an empty diff. The DAW Cargo.lock remained at SHA-256
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`. Full
log: `/tmp/sotf-aud132-aud131-workspace-final.log`. This scoped AUD131
acceptance still does not complete the broader spatial or plugin audit.

## Limits and remaining work

The accuracy claim is bounded to 0.45 cycles/sample; response quality nearer
Nyquist is not established. No CPU benchmark or official EBU corpus run was
performed in this batch. AUD-128 use/access remains unresolved, so no corpus
was fetched. The rejected AUD-132 candidate remains separately identified as
a failed candidate and is not passing evidence.
