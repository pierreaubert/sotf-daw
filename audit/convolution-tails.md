# AUD-051 Convolution tail contract checkpoint

Scope: sotf-plugin-convolution only. No host, NIH, HAL, MIDI, or IAMF edits in this implementation.

## Implementation

- Prepared IrLoadResult records the maximum channel length after IR resampling, before FFT partition padding. Installation copies this into an audio-owned scalar cache; clear resets the active length to zero.
- `tail_length()` returns Finite(latency + max(length - 1, 0)) for reset/stable state, using actual backend latency and no ArcSwap read. No IR therefore reports its existing inactive dry delay. NUPC's zero UPC partition count is irrelevant.
- An already scheduled async replacement reports Unknown until accepted/failed. Accepted replacement or clear conservatively preserves the maximum prior finite bound, adds 128 frames for its held-last-output fade, and maxes with the new IR's stable bound. The retained history does not shrink during silence. Repeated replacements can over-report; reset clears retained history. initialize does not clear history; rate reload remains Unknown while pending.
- Valid scalar parameter getters/validation and mix/gain setters avoid ParameterSet/schema reconstruction. The cached scalar schema value is updated in place to preserve legacy current_values() correctness. ir_file remains a setup/control query that returns an owned String.

## Red/green and numerical evidence

- `/tmp/sotf-convolution-tail-red.log`: the first independent last-tap test failed with Unknown versus expected Finite(1024).
- `/tmp/sotf-convolution-scalar-red.log`: fresh-thread scalar getter reported **90 allocations** for six lookups before correction.
- `/tmp/sotf-convolution-tail-final.log`: **61 tests pass**: 47 unit, 4 direct/numerical/RT, 10 integration; zero failures/ignored.
- `/tmp/sotf-convolution-tail-clippy-final.log`: targeted all-target Clippy with warnings denied (see final result in log).

New independent support checks:

1. 120 cases = rates44.1/48/96k × lengths1/255/256/257/1023/1024/1025/4097 × UPC/NUPC/direct-head32/128/512. Three distinct input channels use cyclic asymmetric IR channels; input includes an impulse at its final frame. Full emitted output is compared to an f64 direct convolution sum with independently fixed backend latency0/1024. The exact expected final coefficient is retained at the advertised final frame, later output is below1e-5, and the bound does not count down during silence.
2. 18 cases = six source-rate/target-rate/IR-length combinations × UPC/NUPC/direct-head. Exact rational ceiling predicts prepared length. Prepared resampled coefficients are fixture data for an independent time-domain sum, so this checks convolution/support rather than independently revalidating Rubato's SRC quality. Full output matches to1e-5 and no significant output remains beyond the bound.
3. Pending completion is injected deterministically immediately after the old4097-tap IR emits its final tap. All three backend families match the complete analytical128-frame held-output fade, keep Unknown before adoption, retain the old bound+128 after adoption, extend rather than shrink on clear, and return to dry delay after reset.
4. No-IR delay response is exact at0/1024;48→96k reinitialization is Unknown until prepared4106-frame IR adoption and then reports4105 with direct head, including after reset.
5. Real CountingAlloc installed in the integration binary. Fresh-thread tail getter and scalar reads, valid validation, mix/gain setters all pass allocation assertions. This proves these getters/automation paths, not every existing Convolution process error/ArcSwap path.

## Limits

The finite support oracle tolerates existing f32 FFT roundoff (1e-5 absolute). No new decay truncation or amplitude-based tail heuristic is introduced. Tail is a processing-response bound including latency once, not remaining EOS drain capacity. IR path exposure/native state restoration are NIH concerns handled separately. The high-water policy intentionally over-reports after live swaps until reset, avoiding a shorter new bound while earlier input's held output is still relevant.
