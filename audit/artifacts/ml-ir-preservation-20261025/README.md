# Optional local-noise-budget preservation for learned IR tails

This checkpoint addresses the rejected tail-mask experiment without changing its
weights or overwriting its final results. It adds an opt-in preservation policy
to the offline IR export CLI. Convolution runtime/defaults remain unchanged.
Measured-response and listening acceptance, and general ML adoption, remain open.

For each short time window/channel, let observed energy be E and the reference
noise budget be B = alpha * prefix_noise_power * window_frames. The linked gain
floor is the largest channel value of `1 - min(0.95, B/E)`. This ensures removed
energy cannot exceed B in exact arithmetic. The exported f32 candidate is checked
again. Zero-noise channels conservatively preserve linked output exactly. This is
a prefix-noise-relative constraint, not an error bound against unknown clean IRs.
The prefix must represent stationary measurement noise. Existing 50 ms early
protection and sampled complex-response checks remain active.

Sixty independently seeded new development cases select window/fraction from a
fixed 3x3 grid. Learned, Wiener and decay methods each select their own policy;
all choose 5 ms and alpha=0.5. Every selected policy qualifies: worst development
SNR change >=-0.25 dB and added late-structure loss <=0.1 dB. Policy bytes/hashes are
saved before generating any of 72 new final or 32 new shift cases. New sources
are disjoint from all 382 old and 60 development sources. Original model and
previous rejected evidence are unchanged.

Final data expand to 16/44.1/48/96 kHz. Six final families include independent
single-slope decays, late reflections, two slopes, ringing, colored and stepped noise.
Shift families add late packets, truncation, nonstationary prefixes and linked
four-channel copies. These are generated IRs, not measured rooms or independent
spatial-path captures. All claims below include exact fallback cases.

| Guarded method | Final accepted / 72 | Final median SNR improvement | Shift accepted / 32 | Shift median improvement |
| --- | ---: | ---: | ---: | ---: |
| Learned | 51 | 1.5248 dB | 21 | 1.9616 dB |
| Wiener | 49 | 1.4850 dB | 21 | 1.9152 dB |
| Decay | 54 | 0.5250 dB | 21 | 1.3504 dB |

All guarded methods have worst reconstruction SNR change 0 dB in both new splits.
The learned method beats the better classical result in only 6/72 final and 6/32
shift cases, so broad learned benefit remains unproven. On the same data the
unprotected learned method loses up to 8.905 dB and unprotected decay loses up to
12.793 dB. The improvement primarily supports protection, not an automatic switch
to ML. All eight nonstationary-prefix cases retain original samples exactly.

`independent-quality-final.json` recomputes all metrics and policy selection,
checks source disjointness, every local channel/window budget, protected/fallback
samples, mask reconstruction and sampled complex response via SciPy full FFTs.
Maximum metric discrepancy is zero, response discrepancy 1.76e-13 and removed
energy/budget ratio 0.94786. With decisions held fixed on known clean IRs, worst
learned full-IR conditional projection loss is 0.00805 dB and late-structure loss
0.02514 dB. These controlled decompositions do not identify unknown clean IRs.

`native-verification-final.json` replays all 234 legacy export arrays identically
when the policy is omitted and all 312 new guarded arrays/decisions identically
with final source. It verifies 36 CLI WAV exports and 72 allocation-guarded native
UPC renders. Valid policy, invalid policy hash and missing model cover four
sample rates, mono/stereo/linked four-channel files and callbacks 193/1024.
Maximum realization error after 1024-sample latency compensation is 5.97e-8.
The unchanged native executable/source hashes match the prior verified build.
This is not true-stereo, measured-room, native-device or DAW deadline acceptance.

The final loader reads model bytes once with a size cap, hashes those bytes and
parses the same bytes. A file-replacement regression proves that replacing the
path between reading and parsing cannot substitute unverified weights. Policy
loading uses the same single-read pattern. `verify-final.json` explains the
final-source replay after this identity fix; initial native evidence stays intact.

Ten Python tests, style and format pass. Command/status/duration records have
matching text logs (trailing blank lines trimmed). `policy-eol-check.json` verifies
raw policy/evidence identity with autocrlf filtering. Source snapshots under
`experiment/evaluation-source` preserve the frozen experiment. All case metrics,
array hashes, selection scores and policy files are committed; float arrays stay
under `/Volumes/home_tmp/tmp/ml-ir-preservation-20261025`. Native temporary WAVs
and outputs stay under the corresponding `ml-ir-preservation-native-*` paths.

Reproduction commands are in `evaluate.json`, `independent-analysis.json`,
`verify-final.json` and the model README. Existing experiment/output directories
are preserved. `source-sha256.json` hashes final runtime/model/verification/docs;
`artifact-sha256.json` hashes all other files here. The policy SHA-256 is
`1c29da25a280f3b82e2fd77e5a6d22c71bb80131e449378e7153ae3594f2d14b`.
The complete 31-plugin/5-program ML audit and Convolution proposal remain active.
