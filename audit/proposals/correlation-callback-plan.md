# AUD106 — standalone correlation ingestion proposal

2026-09-28. Read-only: no edits, tests or new probes. Existing evidence is
`/tmp/sotf-correlation-capacity-probe.rs` and `.log` supplied by root.

## Confirmed accepted-audio loss

`crates/sotf-plugins/crates/sotf-host/src/analyzer_channel_correlation/channel_correlation_plugin.rs`
creates a ring of 96000 **samples** at line 38. `process` (line 115) copies audio,
pushes the entire callback into that ring, drops overflow samples, and only then
reads its own consumer. Both endpoints and the monitor are owned by the same
plugin, used synchronously on the same callback; no worker/producer protocol
needs preservation.

The probe compares the public plugin with a direct monitor and proves exact
passthrough despite incomplete/wrong analysis:

- 7 channels: accepts 28000 frames over two callbacks, counts 21714;
  matrix maximum difference 1.003736973.
- 32 channels: accepts 28000 frames, counts 6000.
- 2 channels: these callback sizes remain within capacity and match exactly.

At 7 channels, 96000 mod 7 = 2: the retained partial frame is completed using
samples after a discarded gap. Subsequent channels are rotated relative to the
intended frame, so this is more than shortened history.

## Smallest ingestion correction

Remove the producer/consumer fields, rtrb imports, ring construction, push/read
loops and overflow log from this wrapper only. On each valid enabled callback:

1. Validate the complete block before mutating output, monitor or cache.
2. Copy the validated input to output (unchanged passthrough).
3. Call `monitor.add_frames(input)` once.
4. Update the existing cache once using `monitor.update_correlation_data`.
5. Return exactly the accepted context frame count.

Disabled processing still passes audio through and freezes analysis. Compiled
AnalyzerTap already delegates to ordinary `process`; it needs no separate DSP.
No callback-size cap or replacement queue is needed: work remains O(frames ×
channels²), and the monitor owns bounded per-channel/pair state. Do not remove
rtrb from the host manifest: other analyzers use it.

Validation is currently absent: `copy_from_slice` can panic on unequal lengths,
context frame count can disagree with equal input/output lengths, and nonfinite
samples poison monitor state. Use checked frames×channels and exact declared
input/output lengths plus a whole-input finite scan before copying, as required
by Plugin::process. A context-rate check against the prepared rate prevents the
monitor silently using the wrong decay clock. Zero-frame valid calls should be
state/output no-ops. Constructor zero-channel and initialize zero-rate behavior
should be decided explicitly; do not invent a maximum channel count or silently
change the direct monitor's public arbitrary-slice contract.

Reset clears monitor history and partial carry. Initialize replaces its monitor
at the new rate; it currently leaves old published cache data until processing,
so require a defined cold cache after successful reinitialize if lifecycle is
included. Existing valid passthrough, parameter ID `enabled`, diagonal identity,
EW Pearson equations and reset decay/sample-count conventions remain unchanged.

## Separate source-proven split-frame allocation concern

`channel_correlation_monitor.rs:54` reserves only channels−1 floats in
`partial_frame`. At lines 96–98, completing a split frame extends that Vec to
exactly channels before copying it into existing `frame_scratch`. Therefore the
first partial-frame completion exceeds reserved capacity. Existing
`split_call_preserves_correlation` proves numbers, but does not count allocation.
This concern is separate from ring loss, and it also applies to direct users.

Minimal correction: reserve channels floats instead. Persistent carry remains
at most channels−1; the extra slot covers the transient completed frame. No new
DSP or extra per-callback storage. A distinct fresh-thread allocator test must
capture the current red, then verify first split completion and reset/reuse have
zero allocations/frees; cover every split offset for 2/7/32/40 channels, including
several sub-frame calls before completion. Compare exact state/matrix with
aligned ingestion. No allocation measurement has been run in this investigation.

## Cache/reset findings: do not fold into a blanket realtime claim

The wrapper calls `RealTimeCache::new(CorrelationData::new(n))`. Its clone shares
the nested matrix Arc between prepared slots; first matrix update must allocate
through `CorrelationData::update_matrix_with` fallback. Reset also assigns a
fresh `CorrelationData::new(n)` inside the audio cache closure. Both are visible
source paths, not measured evidence here.

Existing `RealTimeCache::new_pair`/`new_triplet` can prepare independent matrix
slots, and reset can fill identity/count zero in existing unique storage. This
is a local follow-up, not a reason to redesign publication. However, an external
reader can clone the public nested matrix Arc and release its outer snapshot;
the current `update_matrix_with` may then allocate even with an available outer
cache slot. Include retained-outer and retained-inner reader tests before making
an unconditional zero-allocation cache claim. Avoid silently weakening reader
lifetime guarantees or final-dropping snapshots on the callback. The ingestion
fix can be verified separately from this publication concern.

## Focused regression plan after approval

- Preserve public red unchanged: 20000 then 8000 frames at 2/7/32 channels,
  direct-monitor exact matrix/sample-count comparison and exact passthrough.
- Add independent analytic anti/correlated pairs and a final block marker; use
  irregular partitions around 96000/channels, and callbacks larger than that
  old capacity. Both ordinary and compiled AnalyzerTap must agree.
- Malformed late nonfinite sample, dimensions and count overflow leave output
  sentinels and future valid analysis unchanged; disabled and zero-frame paths.
- Reset/reinitialize, retained UI snapshots, and distinct cold allocator guards
  for aligned ingestion, split-monitor ingestion and publication/reset.
- Focused host correlation tests and strict Clippy; parent aggregate follows.

The standalone wrapper is publicly exported by the facade but explicitly not
registered in the engine factory; no engine registration work is proposed.
No host DAG queues, manager protocol, MIDI or IAMF changes.
