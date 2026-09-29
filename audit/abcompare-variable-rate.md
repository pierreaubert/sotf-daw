# AUD087 — A/B comparison with a variable-rate nested path

This read-only follow-up used TokenSave then current source inspection. No A/B
production source or repository tests changed.

The authoritative facade factory supplies its full plugin factory to A/B path
construction. A resampler can therefore be installed in a nested path, including
a path whose output rate differs from the comparison plugin's rate. The comparison
process allocates equal input/output frame regions for both paths, ignores the
actual frame counts returned by each nested host, and mixes the complete input
frame region. Its public output rate remains the original rate.

A public probe uses stereo constant 0.25 input, pure path A, disabled AutoGain,
and a 64-frame nested resampler chunk. Both 48→24 and 48→96 kHz paths construct
and initialize successfully and report a public rate of 48 kHz.

For the 48→24 kHz path, after 4096 source frames:

| Callback frames | Silent frames in final callback | Maximum sample |
| --- | --- | --- |
| 1 | 0/1 | 0.250003 |
| 64 | 32/64 | 0.250003 |
| 137 | 41/137 | 0.250003 |

The repeated silent portion of settled constant audio independently proves that
the comparison is reading unwritten path output. The 96 kHz constant probe alone
does not quantify its nonconstant waveform error; no stronger executed claim is
made for that direction.

The probe initially used the bridge factory, which rejects this nested resampler
through its limited built-in factory. The executed results above instead use the
authoritative facade factory and its full nested factory, matching current
`factory/create.rs`. Source, log and exact linked-artifact command are
`/tmp/sotf-abcompare-rate-probe.rs`, `/tmp/sotf-abcompare-rate-probe.log`, and
`/tmp/sotf-abcompare-rate-probe-build.txt`.

Proper support needs both paths in a common output clock, retained actual output
frames, latency alignment and a defined policy for bounded chunk waiting. Checking
only the final path rate is insufficient: an up/down pair can end in the original
clock while still returning bursts. Any corrective scope must preserve the
existing sample-based loudness/mix timeline and nested finite tails. The earlier
host queue proposal remains unapplied; this finding does not authorize bypassing
that separate approval block.
