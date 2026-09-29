# AUD-070 implemented checkpoint

## Corrected contracts

Both host in-place adapter classes now expose correctly sized public output buffers. The raw in-place core still receives all interleaved input lanes, including external keys. Adapter-owned scratch compacts only program lanes into output. Gate is the only production plugin opting into the new `supports_bounded_subdivision` trait contract; synthetic tests exercise both traits. The opt-in guarantees exact full consumption, signal-state/audio partition equivalence, context independence except rate/framecount, and no valid-block failure after lifecycle/rate validation. Unsupported asymmetric processors fail explicitly during initialization.

Adapters allocate checked 256-frame f32/native-f64 work storage during initialization. That size bounds internal chunks, not public callbacks. Full public shape and finite-sample validation precedes all inner calls. Initialization/rate/layout checks precede output publication. Native f64 stays double precision; opted-in f32 fallback uses prepared f32 storage and never calls the allocating trait default. Reset keeps capacity. Direct in-place fallback retains every input-stride lane. Short/excess inner frame returns produce explicit errors before that chunk is published; rollback from a misbehaving opted-in DSP that already mutated earlier chunks is not claimed.

The host's two old sidechain input-width-output exceptions (f32/f64) were removed. Both call sites now supply output_frames*declared_output_channels, with unchanged graph scheduling and queues. Existing host f32/f64 sidechain waveform tests pass. No blocked branch-queue integration occurred.

C construction now uses requested output/program width for Gate and verifies both actual buses. A stereo external-key request is 4 inputs/2 outputs. Input order is program L,R then key L,R; output contains only program L,R. Canonical state restoration preserves this layout through both C state APIs. Mismatched external/internal bus shapes reject. The C callback preparation setting remains unchanged: default4096 frames, configurable to65536 with `_sotf_max_callback_frames`; tests explicitly negotiate17003 for a large valid C callback.

## Files

- New `sotf-host/src/plugin/bounded_in_place.rs`: prepared scratch, checked lifecycle, bounded conversion/compaction.
- Both in-place traits/adapters plus plugin module registration; dynamics' independently added parametric drain hooks are preserved.
- Gate adds only the subdivision opt-in marker (subsequent native Gate drain work is dynamics-owned AUD071).
- Host daw_host.rs changes only the two obsolete output-width workarounds for this task; earlier unrelated host changes remain intact.
- C Gate constructor mapping, bridge/C README route documentation, synthetic host fixtures and integration tests.
- NIH auxiliary-bus integration belongs to root under AUD068, not this patch.

## Independent verification

- `target/audit-bounded-host-suite.log`:610 host tests pass,0fail/0ignored across15 suites, including all503 library tests at this checkpoint. Includes six new bounded-adapter tests plus existing precision and actual host sidechain regressions.
- `target/audit-bounded-bridge-ffi-suite.log`:116 bridge/C tests pass,0fail/0ignored across6 suites (64 C unit tests). Includes restored saved bridge external-key positive regression, raw-unsplit Gate waveform comparison across all3 modes×linked/unlinked×RMS/peak×lookahead0/3.7ms, blocks1/17/255/256/257/8192/17003, reset and reinitialize, and live threshold changes at public boundaries.
- Synthetic plain/parametric/nested adapter matrix verifies f32, f64 fallback and native f64 values below f32 resolution; input-lane sentinels and independent recurrence oracle; late invalid samples, dimension overflow, wrong rate and uninitialized transactionality; short/excess frame counts; direct in-place key retention.
- Cold fresh-thread callback tests for synthetic adapters and real Gate report zero allocations and zero deallocations, including17003-frame calls and reset. Invalid error formatting is outside the successful realtime-path guarantee.
- `target/audit-bounded-adapters-clippy.log`: all-target host/bridge/C Clippy exits0 with -Dwarnings (previous full focused pass also included Gate).
- `git diff --check` clean.

The exact original red bridge fixture and log remain preserved beside this report. The restored live fixture preserves its stereo-output assertion and analytic signed-gain bounds; its only style change uses `as_chunks` to satisfy current Clippy. Initial full-host failure showed its obsolete wide-buffer workaround causing isolation fallback; the source fix restores the original independent expected waveform in both precisions. Initial large C call was rejected by the existing4096 configured capacity; negotiating the already-supported callback bound made the test valid without changing that C contract.

AUD068 native auxiliary routing has separate root-owned actual CLAP validation. General metadata/event-aware subdivision and max-block preparation APIs remain outside this narrow opt-in implementation. Symmetric non-opted-in legacy f64 fallback behavior is unchanged in scope; no claim of universal allocation-free fallback follows.
