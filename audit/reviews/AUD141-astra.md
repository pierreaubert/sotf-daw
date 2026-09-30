# AUD141 Astra design review

Status: **bounded multiway LR24 correction implementation ACCEPTED**.

## Final evidence closure

The last automation finding is closed at test SHA
`7fb35ab37eb50685eba30ef8096bdfa80f1894ebe4afe5230bf4d9ef6013068c`.
Both full vectors now assert equal lengths, every sample finite, and peak at
most the declared fixture ceiling 2.0 before any `f32::max` comparison.
Verified focused 1/1 log SHA
`a02f1581fcc8d49e61bc200aa68c155fa4f490a9f435fc4e8ea4046351c89052`
and completed strict all-target lint log SHA
`8cb4a5c514618b15c8d4c12952a6bb82afe9f43f375339527ae3860bc9c7769f`.
Production is unchanged from the prior reviewed package snapshot. The report
now consolidates the package/host/replay/lint receipts; append this narrow closure
receipt and change its pending-review status without rerunning those gates.

The implementation and executed evidence satisfy the bounded correction.
No CPU, general crossover-family, BandSplit/AUD143, recursive-tail or full-audit
acceptance is implied. No reviewer Cargo or Rust edits.

## Historical implementation review (conditions closed above)

Production SHA `0c4357938dcf6b74141eee65416592b5c6db0b3a61a0efebcf9e8a742df28053`
implements the accepted branch rule directly: highpass before the selected split,
lowpass at it, and low+high at later splits. No new filter state or unrelated
processing branch change was found. The complex f64 matrix oracle normalizes
residual waveform energy to nonzero input RMS, avoiding near-null band division.
Its 0.01 fitted-waveform residual bound is distinct from the unchanged 2e-3
complex transfer bound. The actual 2→6→2 Crossover/BandMerge host test additionally
uses a fully independent expected sinusoid, 2e-3 waveform bound and leakage check.
Mode selection, reset/reinitialize, rejected-update twins, cold/repeated/reset
allocation and deallocation guards, and four saved baseline vectors are meaningful.

**Required small test refinement:** `frequency_automation_uses_absolute_event_frames_across_partitions`
folds differences with `f32::max`, which can ignore NaNs. Before comparing, assert
both complete rendered vectors are finite and satisfy a conservative declared
fixture peak bound. This closes the accepted bounded-waveform automation gate;
no production change or broader numerical threshold relaxation is requested.
Rerun that focused test and relevant lint only if this is the sole source delta.

Verified package/clippy start=end manifest
`cb69595ccab212c041b2f15cca62692464fedcf56643941f8c38c362229bde33`, all current
entries matching. Host-chain start=end manifest
`0426eeaee0b5a9df334a82f695fa8a8c7b7dbd90e2a7c88ef43e63756647746c` differs now
only in unrelated native external-plugin test source. Inspected terminal logs:
package 104 passed / 2 manual ignored, explicit four-array replay 1 passed,
host chain 1 passed, package/facade strict lint completed. No whole-tree shared
snapshot or CPU claim follows. The report still needs these final receipts and
current status in place of its earlier pending-gate text.

No production correctness finding in the reviewed scope. Final acceptance awaits
the small automation assertion and report closure. AUD142/AUD143 and recursive
tail policy remain separate. No reviewer Cargo or Rust edits.

Reviewed proposal SHA
`4817094bae37ae5efa1ef27ef72328643a64d1fa0755632a07f6a2a339d94b46`,
report SHA `5873d5755453d70b9c058b4506265aed8b56331c5e4d83c274ba1a439cc531fb`,
the public analytical regression and unchanged multiway processing loop.
The red log `/tmp/sotf-aud141-public-red.log` contains the reported band-0
complex error 0.16770490 and summed magnitude 0.83229510. Its final status is
one expected failure; later assertions are not passing evidence.

## Topology and oracle

For fixed cutoffs, the proposed branch
`H0…H(b-1) Lb A(b+1)…A(m-1)` and final all-high branch telescope exactly to
`A0…A(m-1)`, with `Ai=Li+Hi`. The implementation can use the existing independent
per-band split states and sum their low/high outputs at later splits. It needs
no extra filter state. This is an all-pass response, not zero-phase identity.
The low endpoint intentionally changes magnitude and phase; the final high
endpoint does not. Separate two-way, per-channel and FIR paths stay outside
the correction.

The f64 oracle independently evaluates analog fourth-order Linkwitz–Riley
prototypes at the prewarped bilinear frequency. Its shared squared second-order
denominator, fourth-power highpass numerator and complex output/input projection
are consistent. Band-by-band complex comparison prevents a summed-only test
from hiding incorrect branch routing. Keep the fixed `2e-3` absolute complex
error bounds for each branch and sum, plus the unity-magnitude bound; report
observed errors separately.

## Required implementation evidence

- Capture actual pre-edit two-way/per-channel/FIR vectors and source/lock
  provenance before production changes, then replay byte-for-byte. Include the
  unchanged multiway final-high endpoint as an additional inexpensive control.
- Exercise three/four bands, close/wide splits, probe points near every cutoff,
  44.1/48/96 kHz and distinct stereo channels. Compare low/high selections with
  their corresponding both-mode branch as well as the analytical reference.
- Make the proposed residual-energy gate numerical before running the expanded
  candidate matrix. Normalize residual to input tone energy (or use an explicit
  near-zero branch floor), so deliberately attenuated bands do not divide by
  vanishing fitted energy. Assert nonzero input projection and coherent windows.
- Use the actual Crossover → BandMerge `2→6→2` host chain, independent complex
  all-pass reference and cross-channel leakage checks. BandMerge's internal
  reconstruction metric is not an input-reference oracle. AUD143 BandSplit is
  separate and is not repaired or accepted by this test.
- Preserve the 16-sample smoother cadence across irregular callback partitions.
  Compare automation runs at the same absolute event times, including reset and
  reinitialization; require finite bounded audio and partition agreement without
  claiming instantaneous LTI/all-pass behavior for moving coefficients.
- Prove rejected parameter updates preserve subsequent waveform/state. Keep
  structural contracts, channel ordering and FIR EOS gates. Guard allocation
  and deallocation on cold/repeated callbacks and reset/post-reset processing.
  Capture pre-edit timing first if reporting a before/after CPU comparison;
  operation-count intuition alone is not measured performance evidence.

These are implementation acceptance criteria within the proposed scope, not
additional production scope. No design blocker remains. Recursive IIR tail
policy, arbitrary spacing quality, generic graph support and the full audit
remain open. No reviewer Cargo or production edits.
