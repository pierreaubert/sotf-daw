# NIH-plug: pinned native timing and tail fixes

Source: <https://github.com/robbert-vdh/nih-plug>

Original commit: `de421011f41a6d10fc8c7a6084e4f4dee0143683`.

This directory contains the patched `nih_plug` library source. Its unmodified
`nih_plug_derive` package resolves from the exact original Git commit above;
its integration tests live in the DAW workspace's `plugins-nih/tests/` and
exercise the macro against the shipped fork. The original ISC license is
preserved in `LICENSE`; the upstream README is preserved as `UPSTREAM_README.md`.
Upstream GUI packages, tools, examples, and product plugins are not copied.
The SOTF workspace patches the original Git dependency to this copy.

## Automation timing changes (AUD-044)

The original two timing fixes affect `src/wrapper/clap/wrapper.rs`:

1. Check the first event's sample offset before applying it. With sample
   accurate automation enabled, a single parameter event inside a callback must
   split the buffer at that event, just like subsequent events already did.
2. Advance the seconds timeline by the split buffer's sample offset even when
   the host does not provide tempo. Seconds conversion only requires sample rate;
   the musical beat conversion retains its tempo requirement.

No synthetic events or guessed transport positions are added. Parameter IDs,
state formats, and VST3 automation dispatch are unchanged.

## Explicit tail bounds (AUD-051)

The optional `Plugin::tail_length()` getter lets a plugin report its configuration
bound before processing and after reset. `None` preserves upstream behavior based
on the last `ProcessStatus`. Opt-in plugins return `Some` throughout their lifetime;
an unknown or unbounded tail uses `u32::MAX`. The bound includes physical buffering
delay once and uses output-rate frames. CLAP's infinite threshold (`i32::MAX`) is
also the finite upper limit for this shared representation.

`src/wrapper/tail.rs` owns a scalar atomic cache. The CLAP and VST3 wrappers refresh
it after successful initialization, reset, state restoration, and processing.
Deactivation, a new VST3 setup, or failed initialization invalidates an explicit
bound conservatively. Native queries never acquire the plugin mutex. VST3's DSP
initialization occurs at activation, so an earlier setup-time query remains unknown.

CLAP caches the host tail extension during initialization and emits `changed()` on
the audio thread after publishing a changed explicit bound and releasing the DSP
lock. The final notification/status decision follows any queued GUI state restore,
preventing the host from sleeping on the preceding state's zero-tail result.
Explicit finite tails use `CLAP_PROCESS_TAIL`; unknown/infinite tails keep processing.
Legacy plugins keep their existing process-status mapping and notification behavior.

This addition changes `src/plugin.rs`, `src/wrapper.rs`, the new tail cache module,
and the CLAP/VST3 wrapper implementation files. The derive package remains unchanged.

## GUI state retirement (AUD-057)

The original zero-capacity state response could block the audio callback until the
GUI entered its receive call, allocating Crossbeam's thread-local wait context on
the cold path. CLAP and VST3 now return restored states through a preallocated
one-element `ArrayQueue` in `src/wrapper/gui_state_return.rs`. GUI callers serialize
each exchange until the response is consumed and destroyed, then release that
control-only mutex before notifying the host. The audio reply never takes this
mutex, waits for the GUI, or destroys the returned state.

The GUI polls for the accepted request's response with 1 ms sleeps. It retains the
wrapper and exchange lock until receipt, so an accepted request cannot leave a
stale response for a later request. The existing request rendezvous, request
timeout, inactive-state application, and plugin state reinitialization behavior
remain unchanged; this is a guarantee for the reply path, not the entire restore.

## CLAP state stream validation and host notification

`src/wrapper/clap/util.rs` reads the length-prefixed state in 8192-byte chunks
and grows its buffer only after each chunk arrives. A malformed length, short
stream, or allocation failure returns `false` without allocating the declared
length up front. Valid state bytes and the shared JSON format are unchanged.

After a successful CLAP stream restore, `src/wrapper/clap/wrapper.rs` schedules
the existing main-thread `RescanParamValues` task. This tells the host to refresh
parameter values changed by the restore; the GUI notification remains separate.
The native stream tests cover short reads, truncated input, and a forged huge
length. The pinned CLAP validator checks the host rescan and parameter round trip.

## Auxiliary bus boundaries (AUD-072)

CLAP and VST3 auxiliary input/output loops now stop at `index >= declared_count`,
before pointer arithmetic; VST3 input loops use the input count. VST3 arrangement
validation starts auxiliary buses after the
main bus when one exists. The shared buffer manager resizes every prepared key
channel to the current callback length before copying or clearing it, including
absent buses/channels after a shorter callback. Storage capacity is still prepared
at activation; these changes add no callback allocation.

SOTF's actual native Gate tests cover CLAP discovery/selection and missing keys,
17-frame supplied keys followed by 257-frame missing keys or missing channels,
VST3 default bus discovery, incorrect key-width rejection, stereo-only legacy
negotiation, and independent-key audio through both native formats. The CLAP
missing-port fixture retains a physically valid loud sentinel beyond the declared
bus prefix so a count error has a deterministic audio symptom without requiring
an invalid-memory read. The output-bound correction uses the same index rule; separate synthetic native
output tests cover absent auxiliary destinations.

## Format-specific initial layout

The additive `Vst3Plugin::default_audio_io_layout()` getter defaults to the first
supported layout, preserving existing implementations. VST3 construction uses
this getter for initial bus discovery. SOTF Gate chooses its supported key-bus
layout while keeping the shared layout ordering, and therefore existing CLAP
configuration IDs, unchanged. Subsequent VST3 negotiation still searches the
same supported layouts. Native tests verify both CLAP configuration IDs and
VST3 default discovery plus legacy stereo-only negotiation.

## Regression coverage

The native CLAP harness in SOTF `plugins-nih` exercises the actual dispatcher,
including a single event at the first, interior, and last sample; equal-time
event ordering; multiple events; irregular callback sizes; independent Gain
smoothing waveforms; and seconds-only transport across split buffers. All native
process calls retain allocation assertions. Additional native CLAP and VST3 tests
cover setup/activation/reset/restart queries, tail changes, synchronous host queries
inside notifications, queued state restoration, legacy behavior, and actual SOTF
Gain/Delay/Convolution processing.

The shared state-return helper is also compiled directly by the SOTF wrapper test
crate to check cold replies before the GUI receives, GUI-thread destruction,
concurrent callers, and request timeout ownership without adding a public API.
Native VST3 coverage uses its actual editor `GuiContext` for concurrent restores.

Run from the SOTF DAW workspace root:

```sh
cargo test -p plugins-nih --no-default-features --features gain --lib wrapper::transport
```

When updating NIH, compare these changes against upstream and remove the
local patch only once equivalent behavior passes the native regression tests.
