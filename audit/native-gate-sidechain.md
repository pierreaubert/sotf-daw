# Native Gate sidechain and bus boundaries (AUD-068/AUD-072)

## Routing and compatibility

Gate exposes stereo main input/output plus an optional stereo key bus. CLAP
preserves its original stereo configuration ID0 and appends keys as ID1. VST3
uses the key-bus layout initially so hosts can discover it; the original
stereo-only layout remains negotiable. Main widths, plugin identities,
parameter IDs, and existing CLAP configuration IDs remain unchanged.

A default Vst3Plugin::default_audio_io_layout() hook preserves the previous
first-layout behavior for existing implementations. Gate explicitly selects its
key-bus layout. Only the VST3 initial-layout consumer uses this hook; subsequent
negotiation and activation continue using the selected supported layout.

Saved `sidechain_external=true` activates a four-input/two-output DSP only when
the selected layout has the key bus. Internal detection continues using two DSP
inputs and ignores any connected keys. Routing changes remain structural and
require host reactivation. The existing native structural-state guard remains.

The native callback uses the activated DSP's actual input count and prepared
interleaving storage. It emits only the stereo program. Direct malformed wrapper
buffers fail before advancing detector history. At the CLAP/VST3 boundary,
unconnected or missing keys are replaced with prepared silence by NIH.

## Native defects corrected

The private NIH source had three relevant defects:

1. CLAP and VST3 auxiliary loops allowed `index == count`, which could access a
   port beyond the declared prefix. VST3 additionally compared auxiliary input
   indices with the output count. The loops now stop at the appropriate count.
2. VST3 layout negotiation started auxiliary validation at main's index instead
   of the next port. Stereo main plus a requested mono key could incorrectly
   satisfy the stereo-key layout. Both input/output offsets now follow main.
3. The shared input-buffer manager resized only supplied key channels. A short
   supplied-key callback followed by a longer missing-key callback could expose
   short channel slices with a larger frame count. All prepared channels now
   resize to the current length before copying or clearing; capacity is retained.

These changes are limited to bus bounds, channel-count validation and prepared
slice lengths. Provenance remains in the vendored NIH README.

## Executed evidence

The initial wrapper regression failed because only the stereo layout existed
(`/tmp/sotf-native-gate-key-red.log`). After adding the layout, it reached the
separately corrected AUD-070 adapter output-width error. The adapter's shape
error allocated under the native guard and aborted that intermediate test; it
was not counted as a pass.

The native wrapper library then passed 90 tests with the Gate feature, followed
by six focused final Gate sidechain tests after the last boundary corrections:

- Three modes, linked/independent detectors, distinct stereo keys, signed analytic
  plateau gains, irregular callbacks and an 8,192-frame callback.
- Invalid key dimensions/oversized callbacks preserve detector history; reset
  replays the same waveform; internal detection matches the stereo-only layout.
- Actual CLAP extension discovery, configuration selection, activation and audio.
- Actual VST3 default bus discovery, wrong key-width rejection, legacy stereo-only
  negotiation, activation and independent-key audio.
- Actual CLAP and VST3 short supplied keys (17 frames) followed by longer missing
  keys (257 frames), plus CLAP's partially supplied stereo key.

Missing-port fixtures retain physically valid loud sentinel ports outside the
declared prefix. Thus a count error has a deterministic incorrect audio result
without requiring an invalid-memory read. Native process calls and wrapper
callbacks retain allocation guards. Host/DSP tests separately count both
allocations and deallocations; these native tests do not add a separate free
counter.

Final compatibility verification passes all90 NIH library tests plus two
synthetic native auxiliary-output tests, with no failures or ignores. These
checks include stable CLAP configuration IDs, the VST3 initial-layout override,
both input/output count boundaries, wrong-width negotiation, and recovery after
missing ports. All-target Gate-feature Clippy passes with warnings denied for the
workspace package; the vendor retains its preexisting unused-import warning.

Logs: `/tmp/sotf-native-gate-compatibility-tests.log`,
`/tmp/sotf-native-gate-compatibility-clippy.log`, and
`/tmp/sotf-native-gate-boundaries-final.log`. No macOS Audio Unit or physical
DAW/device execution is claimed.
