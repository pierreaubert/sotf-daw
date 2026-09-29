# AUD-095: continuous MultibandExpander soft-knee state boundary

## Implemented scope

The reviewed proposal is `audit/proposals/multiband-expander-soft-knee.md`. Root approved both spectral and time-domain paths after the eleventh aggregate passed. The production change is one scalar `opening_threshold(T,K)` helper plus eight state-comparison substitutions and two scalar uses in `src/lib/multiband_expander_plugin.rs`. Exact isolated delta: `/tmp/sotf-mbe-soft-knee-production.patch`.

- Helper near line 674 returns `T` for `K<0.1`, otherwise `T+K/2`.
- Spectral state logic near lines 798–832 uses the resulting unity edge for Open/Hold/Closing and the same hysteresis offset below it.
- Time-domain state logic derives that boundary from the current per-sample threshold/per-band knee near line 1689, then uses it near lines 1752–1775.
- The centered attenuation function, envelope coefficients, hold-counter timing, bin normalization, source scheduling, drains and buffer storage are unchanged. Global IDs/defaults, per-band overrides and serialized settings are preserved. README/CHANGELOG explicitly document the audible preset correction.

Only the MBE crate's source, new `tests/soft_knee.rs`, README and CHANGELOG were changed for this implementation. No host/native/wiring, MIDI or IAMF changes. AUDIT.md remains root-owned.

## Red evidence and independent oracles

The isolated spectral probe first measured a **4.179154 dB settled gain jump for a 0.02 dB input change** around the knee center, with a **4.144154 dB** upper-half gain error. It derives the periodic-Hann fundamental gain independently as `(2g(A)+g(A/2))/3`; standard f64 logarithms/powers and a cosine projection supply the oracle, not production FFT or fast math. Artifacts `/tmp/sotf-mbe-soft-knee-probe.rs`, `/tmp/sotf-mbe-soft-knee-red.log`.

An independent peer's public time-domain DC probe confirmed a **4.5164 dB** jump where the centered curve predicts only **0.030001 dB**. Its 432-case matrix and provenance are `/tmp/sotf-mbe-time-knee-review.md`, `/tmp/sotf-mbe-time-knee-probe.rs`, `/tmp/sotf-mbe-time-knee-probe.log`. The maximum ordinary fast-math error was 0.015118824 dB.

Before production edits, both permanent full-curve tests failed:

- Spectral: input −23.99 dBFS, actual gain −2.993686 dB versus independent −7.138753 dB.
- Time-domain: input −23.99 dBFS, actual gain 0 dB versus independent −4.485012 dB.
- Log `/tmp/sotf-mbe-soft-knee-permanent-red.log`: 0 passed, 2 failed.

## New regression coverage

Six focused tests pass:

1. Coherent spectral tone at lower edge, center, upper edge and close brackets around center/neighbor-bin crossings, initially open/closed histories, exact one-frame versus irregular callback equality, and a separate continuity assertion.
2. **1056 public DC configurations**: 44.1/48/96 kHz, 1/2 channels, linked/unlinked, knees 0/0.099/0.1/12, two initial histories and 11 levels. Both stereo channels independently checked.
3. **72 hysteresis/hold configurations**: both modes, three rates, hold 0/7 ms, levels below/inside/above the corrected dead band, both histories. Intentional history dependence between close/open edges is retained.
4. Hold timing: exact time-domain first-closing sample after the existing entering-Hold sample plus rounded hold count; spectral first changed synthesis-window boundary derived from rounded hold hops and negative startup origins. Covers 44.1/48/96 kHz and hard/soft knees.
5. Global and per-band threshold/knee automation at a fixed source offset: two rates, mono/stereo, spectral/time modes, three callback partitions including oversized calls. All corresponding output streams are bit-identical; settled post-change gain matches the independent curve.
6. Ratio-one, zero range, dry mix, inactive/bypassed-band neutral delay oracles, and exact knee-zero versus knee-0.099 waveform equality with positive hold/hysteresis.

The **new** full-curve tolerance is 0.03 dB, justified by the independently measured <=0.01512 dB approximation floor. Existing 0.01 dB fixtures were preserved verbatim. No assertions were relaxed to hide the multi-dB defect.

## Verified outcome

Re-running the original isolated spectral probe against the corrected public crate yields a gain change of **0.033063 dB** across the same 0.02 dB input bracket, versus **0.032849 dB** predicted. At −23.99 dBFS the gain error falls to **−0.001937 dB**. Maximum absolute error across its nine reported levels is **0.013820 dB**. Open/closed settled histories and bit-identical callback partitions still pass. Log `/tmp/sotf-mbe-soft-knee-green-probe.log`, build provenance `/tmp/sotf-mbe-soft-knee-green-build.txt`.

- Focused new suite: **6 passed**, `/tmp/sotf-mbe-soft-knee-expanded.log`.
- Full `cargo test -p sotf-plugin-multiband-expander --all-features`: **152 passed, 0 failed, 0 ignored**. Log `/tmp/sotf-mbe-soft-knee-full.log`.
- Existing finite-stream/cold suites passed, including **36 fresh-thread native/2x/4x fixtures**, two epochs, **0 allocations and 0 deallocations**. The change adds no storage or allocation path.
- Strict `cargo clippy -p sotf-plugin-multiband-expander --all-targets --all-features -- -D warnings`: clean. Log `/tmp/sotf-mbe-soft-knee-clippy.log`.
- Release QA: **all 4 checks passed** (expansion accuracy −60.91 dB against −60 dB expectation within unchanged 2 dB allowance, latency, zero allocations, performance). Log `/tmp/sotf-mbe-soft-knee-release-qa.log`.
- Scoped rustfmt and `git diff --check`: clean.

Root reviewed the isolated production delta and both processing paths; a separate
agent reviewed the boundary logic, DC/Hann equations, hold timing, automation and
tolerance calibration. Neither review found a remaining blocker. Existing nonzero-knee presets intentionally become more attenuating near threshold and their hold/hysteresis triggers move to the knee's unity edge. Hard-knee results and timing remain unchanged; no migration rewrite or hidden legacy behavior is introduced.
