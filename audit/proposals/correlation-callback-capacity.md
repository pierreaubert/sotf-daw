# AUD106 — Standalone correlation callback capacity

2026-09-28. Public reproduction only. No production change or permanent test.

The standalone `ChannelCorrelationPlugin` synchronously enqueues a complete
callback into a fixed 96,000-sample ring before reading that same ring into its
monitor. Excess samples are discarded while the plugin reports every input
frame accepted and copies all input to audio output. The ring is not a thread
handoff: both endpoints and the monitor are used within the same callback.

The public probe `/tmp/sotf-correlation-capacity-probe.rs` runs 20,000-frame and
8,000-frame callbacks at 48 kHz. It compares the actual plugin cache to a direct
monitor fed all the same interleaved audio. The exact accepted frame count is
also an independent oracle. Audio passthrough remains exact in every case.

| Channels | Accepted frames | Analyzed frames | Maximum matrix error |
| --- | ---: | ---: | ---: |
| 2 | 20,000 | 20,000 | 0 |
| 2 | 28,000 | 28,000 | 0 |
| 7 | 20,000 | 13,714 | 0.000677308 |
| 7 | 28,000 | 21,714 | 1.003736973 |
| 32 | 20,000 | 3,000 | 0.004579433 |
| 32 | 28,000 | 6,000 | 0.000458658 |

At seven channels the truncated first callback also leaves an incomplete frame.
The next callback completes it with the wrong channel samples, corrupting the
subsequent frame alignment. This failure is specific to the standalone wrapper;
the embedded output monitor does not use this ring. The standalone plugin is
currently unregistered in the engine factory.

Proposed narrow correction: feed complete accepted input directly into the
existing synchronous monitor and retain the current cache publication policy.
Review callback validation, reset and compiled dispatch before implementation.
Separately inspect the monitor's partial-frame storage, whose initial capacity
is one sample smaller than the full stitched frame; do not conflate that
potential allocation with the reproduced sample-loss defect.

Log: `/tmp/sotf-correlation-capacity-probe.log`. Build provenance:
`/tmp/sotf-correlation-capacity-probe-build.json`. The probe links the existing
local host library and writes its executable only in `target/audit-tmp`.
