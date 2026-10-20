# sotf-plugin-declick

SOTF Declick detects short impulsive corruption from robust context and
reconstructs it by interpolation. It uses eight samples of fixed lookahead
(0.167 ms at 48 kHz), reports that latency to the host, and keeps the dry path
latency-matched while bypassed.

The detector compares each candidate with pre/post medians and median absolute
deviation, while rejecting persistent steps and high-variation programme.
Adjacent channel pairs share the detection decision by default so stereo and
surround images remain coherent; interpolation remains channel-specific.

Parameters:

- `enabled`: smoothly crossfade between delayed dry and repaired audio.
- `sensitivity` (1–100): lower values repair more candidates; changes are
  smoothed over 5 ms.
- `link_channels`: link decisions in adjacent channel pairs; disable for fully
  independent channels.
- `mode` (Random/Periodic): periodic repetition tracking with phase
  prediction; falls back to random-style detection without a lock.
- `bands` (Fullband/2-band/3-band): complementary multiband detection with a
  fullband supervisor gate; fullband preserves legacy behavior.
- `crossover_hz` (80–12000): band split frequency.
- `frequency_skew` (−1–1): bias detection toward high (+) or low (−) bands.
- `repair_width` (0–8 samples): symmetric repair extension; adds equal
  latency on new-mode paths (legacy path stays at eight samples).
- `audition_residual`: output the aligned residual (removed clicks) instead
  of repaired audio.

Default settings (random mode, fullband, zero width) use the legacy path
bit-exactly: eight samples of latency and the accepted detector behavior.
New-mode paths add only the configured repair width to that latency.

The callback is allocation-free, lock-free, frame-major, and accepts arbitrary
block sizes. Non-finite input is replaced locally with the last finite sample.
See `USAGE.md` and `UI.md` for contracts and controls.

## Finite streams

After a nonempty input stream, `drain` returns exactly `latency_samples`
frames of zero-input continuation (eight by default, plus repair width on
new-mode paths), including repaired or bypassed audio still held in lookahead.
It accepts any positive destination capacity aligned to the channel count and
leaves unused destination samples untouched. An empty stream completes without
output. The declared tail equals the reported latency.

The first successful drain after nonempty input closes the stream: subsequent nonempty input and
parameter changes require `reset` or successful reinitialization. Invalid drain
capacities and sample rates leave the stream untouched. Completion is stable,
and both draining and reset reuse prepared storage without allocating or freeing.
An empty-stream drain is a reusable no-output query, as pinned by the lifecycle
tests; it does not finalize a stream that has accepted no input.

## Checked-in clean-reference quality benchmark

`tests/audit_quality.rs` runs without external data. It keeps unmodified clean
references for a recorded cymbal attack, a bowed violin attack and controlled
synthetic percussion, then adds fixed boundary clicks, clustered clicks and
low-amplitude irregular crackle to separate copies. No quiet-slot selection or
parameter tuning is used. Source credits, licenses, transforms and hashes are in
[`tests/fixtures/recorded`](tests/fixtures/recorded/README.md).

The 108-cell matrix covers Random/Periodic, full/2/3-band, sensitivities 2/10 and
three corruption families at 48 kHz. Clicks include frame zero, callback-crossing
widths 1–6, dense clusters and the last input frames. Every render includes the
declared EOF continuation, exact latency/length checks, bit-identical regular
versus irregular callbacks and allocation/deallocation guards.

The report defines detection operationally: an aligned left sample changed by
more than 1e-4. Strict per-frame precision/recall compares that mask with the
known injected-sample mask; interpolated clean neighbors count against strict
precision. Precision is undefined when nothing is repaired. Repair error is
measured against the held-out clean reference, not only against another plugin
render. Damage outside ±16-frame repair guards, uncorrupted linked-right damage
and the entirely clean control render are reported separately.

The conservative default sensitivity 10 meets the existing 0.05 peak clean-damage
bound on these references (observed maximum 2.99e-8). A separate aggressive
isolated-click gate prevents an inert detector from passing on preservation
alone. Aggressive sensitivity 2 can damage the clean cymbal by 0.2212 peak;
default sensitivity 10 preserves it but synthetic-percussion recall spans
0–0.214 across the tested corruption/mode cells. There is no universal
percussion-safe setting or guaranteed crackle/cluster removal established by
this benchmark. These half-second excerpts and the specific synthetic signal
are conditional regression evidence, not music-population certification.
The 2.0 frame-zero click remains unrepaired in every tested cell. Measured EOF
event peak error spans 0.0097–1.5; callback-crossing event peak error reaches
2.0001. Separate event errors are retained alongside aggregate scores.

Per-cell precision/recall, injected-frame error improvement and damage are in
`audit/artifacts/plugin-audit-20261018/declick-corpus.json` at the workspace root.
The legacy and owned classical interpolation paths remain unchanged. Optional
transient-discriminator/constrained-inpainting research stays separate.

The older `corpus_quality.rs` piano/rock tests remain explicitly ignored by
default and require `SOTF_TEST_DATA_ROOT` when selected. They fail on missing or
invalid configured files; the checked-in benchmark does not claim to replace
that external release corpus, which was unavailable for this audit run.
Four additional ignored unit probes are the external-corpus slot diagnostic
and three deferred R48 estimator accuracy characterizations. They remain
unverified here; see `audit/RELEASE-DEFERRED.md` at the workspace root.
