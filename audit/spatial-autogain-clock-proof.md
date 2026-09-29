# AUD115: AAE and Upmixer AutoGain caller timing

Date: 2026-09-28. Public factory probe after AUD110. No AAE/Upmixer or multichannel-helper production changes were made for this proof.

## Measured defect

Both plugins call `MultichannelAutoGain::measure_input` on the whole incoming callback, then `measure_and_apply` on its whole output. The helper derives the new output measurement before advancing gain over that same output. This lets later input affect earlier output and ties gain updates to callback partitioning.

Public bridge construction uses otherwise default AAE/Upmixer settings, 48 kHz, enabled/disabled AutoGain, 100 ms smoothing and 12 dB maximum gain. The source lasts six seconds plus 17 frames, with changing stereo levels and independent 1 kHz/3.7 kHz components. All outputs are finite and every successful process returns the requested frame count.

| Plugin | AutoGain | 137 vs 512 frames, max absolute sample error | 137 vs 8192 frames |
| --- | --- | ---: | ---: |
| AAE | off | 0 | 0 |
| AAE | on | 0.00013354793 | 0.0028697848 |
| Upmixer | off | 0 | 0 |
| Upmixer | on | 0.001336515 | 0.017378747 |

For causality, two identical instances receive identical warm history (109 callbacks of 4096 frames). Their next 8192-frame inputs match through frame4095 and differ only in the remaining suffix. Earlier output differs by **4.1787513e-5** for AAE and **0.00024104957** for Upmixer with AutoGain enabled. Both disabled earlier-output controls are bit exact. The first enabled difference appears at interleaved output sample30 in each case.

These controls isolate the measured discrepancy to enabled AutoGain for this fixed configuration; they do not claim every dynamic Upmixer parameter is independently callback invariant. No new numeric tolerance or corrected-output claim is inferred from these defect magnitudes.

## Reproducibility

- Source: `/tmp/sotf-spatial-autogain-clock-probe.rs`.
- Output: `/tmp/sotf-spatial-autogain-clock-probe.log`.
- Matched Cargo artifact list: `/tmp/sotf-spatial-autogain-build.jsonl`; build log with the same prefix.
- Standalone compile log: `/tmp/sotf-spatial-autogain-clock-compile.log`.
- SHA256 manifest for source, executable and matched bridge/host libraries: `/tmp/sotf-spatial-autogain-clock-manifest.json`.
- Executable: `target/audit-tmp/sotf-spatial-autogain-clock-probe`.

Next: review a causal continuous aligned input/output clock, preserve the existing stereo-fold measurement objective and raw DSP callback chronology, and verify EOS, lifecycle and cold allocation behavior. AAE, Upmixer and the public helper need explicit compatibility boundaries; broader host queues and engine transitions remain separate.
