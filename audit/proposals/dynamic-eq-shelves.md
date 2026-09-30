# AUD139 implementation proposal: Dynamic EQ shelving bands

Status: **Astra accepted the bounded design; implementation and route work are
in progress**. The accepted proposal SHA-256 was
`a8d1973f757d689a8c71e3697e44606ff87baaee0cfcf9e9d2ea5a0d4ff85531`; the
implementation must also meet the clarifications in
`audit/reviews/AUD139-astra.md`.

The goal is to add usable dynamic low-shelf and high-shelf bands to the current
Peak-only Dynamic EQ. This proposal fixes their DSP law, detector behavior,
serialization, parameter identity, and route acceptance tests. It does not
claim a match to any proprietary plugin.

## Source basis and current behavior

The current source was inspected on 2026-09-30. The pre-edit production source
copy is preserved under
`crates/sotf-plugins/target/audit-baselines/aud139-pre-edit/source/`; its file
list and hashes are in `source-manifest.sha256`. The issue report
`audit/dynamic-eq-shape-gap.md` records the original gap and source hashes.
Sibling UI/controller findings are in `audit/dynamic-eq-ui-route-findings.md`.

The Dynamic EQ currently creates a fixed-gain Peak biquad per channel, reads a
filtered detector from the input signal, and multiplies the difference between
filtered and dry samples by a modulation proportion. The proportion maps the
existing dynamics core's positive gain-reduction amount to the band target's
signed dB gain. Linked mode takes the channel maximum before one shared
envelope; unlinked mode has per-channel envelopes. The output mix remains a
separate 5 ms-smoothed global control.

The runtime and host serializable band structs are separate. Both contain
frequency, Q, target gain, per-band threshold/ratio, active, and solo. The
shared `BAND_PARAMS` template has seven band fields. The FFI expands that
template after eight globals for eight bands, so its current Dynamic EQ index
range is 0–63. The engine's generic Dynamic EQ accessor only exposes the eight
global controls. The GPUI band controller uses a separate sparse ID range
`100 + 10 * band + local_index`; its current next/previous parameter logic
walks a contiguous count and cannot reach those band IDs. Existing GPUI
Structural changes already have receipt handling that must be retained.

The NIH Dynamic EQ export exists for CLAP and VST3. NIH builds its parameters
from bridge metadata and currently hides non-realtime fields. A shape field
that merely exists in a shared spec therefore would not prove a visible native
control or a successful live-instance structural reconstruction.

## Proposed DSP contract

### Shape, frequency, target gain, and slope

Add three stable shapes: `peak`, `low_shelf`, and `high_shelf`. Missing shape
in old state means `peak`. Existing Q remains the Peak bandwidth control,
unchanged at 0.1–10.0. For shelves Q is dormant and preserved in the state;
it must not be silently reused as shelf slope. Add a separate shelf slope `S`
with range 0.1–1.0, default 1.0, and 0.01 control step. `S = 1` is the
steepest monotonic shelf in the selected RBJ cookbook family; lower `S` gives a
broader transition. The implementation must check monotonic magnitude over
the entire supported frequency and gain range instead of assuming this from
the UI label.

The existing `frequency` is the peak center for Peak and the shelf's
midpoint-gain frequency `f0` for a shelf. Keep the current constructor
validation `20 Hz <= f0 <= min(0.475 * sample_rate, 20 kHz)`. The target gain
remains signed `G` in −24…+24 dB. At DC/low-frequency plateau, Low Shelf is
`G` dB and High Shelf is 0 dB; at Nyquist/high-frequency plateau the roles
reverse. The static shelf magnitude at `f0` is `G/2` dB.

Build each shelf from the RBJ/W3C Audio EQ Cookbook equations with
`A = 10^(G/40)`, `w0 = 2*pi*f0/Fs`,
`alpha = sin(w0)/2 * sqrt((A + 1/A) * (1/S - 1) + 2)`, and
`beta = 2 * sqrt(A) * alpha`. Use the cookbook Low Shelf or High Shelf
coefficients, normalize all coefficients by `a0`, and reject non-finite or
unstable coefficients during control-thread preparation. This coefficient
builder must be implemented independently of the reference evaluator used by
the tests. The `math-audio` shelf constructor currently implements only its
fixed slope and ignores its Q argument for shelves; do not pass `S` as Q and
pretend that makes it adjustable.

For an unambiguous implementation, the unnormalized cookbook coefficients
are:

```text
c = cos(w0)
Low Shelf:
  b0 = A*((A+1) - (A-1)*c + beta)
  b1 = 2*A*((A-1) - (A+1)*c)
  b2 = A*((A+1) - (A-1)*c - beta)
  a0 =     (A+1) + (A-1)*c + beta
  a1 = -2*((A-1) + (A+1)*c)
  a2 =     (A+1) + (A-1)*c - beta
High Shelf:
  b0 = A*((A+1) + (A-1)*c + beta)
  b1 = -2*A*((A-1) + (A+1)*c)
  b2 = A*((A+1) + (A-1)*c - beta)
  a0 =     (A+1) - (A-1)*c + beta
  a1 =  2*((A-1) - (A+1)*c)
  a2 =     (A+1) - (A-1)*c - beta
```

Divide `b0,b1,b2,a1,a2` by `a0` and store normalized `a0=1`.

Reference: [W3C Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/).
Its shelf equations define `S=1` as the steepest monotonic setting. The
proposed 0.1 minimum is an explicit product range to keep the transition
bounded and well conditioned; it is not a claim that the cookbook mandates
that minimum.

The verification oracle is separate from the production digital coefficient
builder. Tests derive the analog low/high shelf prototype in the s-domain,
prewarp its corner with `Omega = 2*Fs*tan(pi*f0/Fs)`, then apply the bilinear
substitution `s = 2*Fs*(1-z^-1)/(1+z^-1)`. Derive the prototype pole/zero
ratio from the requested octave slope using the cookbook's S-to-Q relation,
rather than transcribing the digital coefficient routine into the test.
Compare complex response and poles at the same matrix of rate, corner, gain,
and slope values; independently check DC, Nyquist, and midpoint values.
Normalize near-zero complex errors by a documented response floor, check full
vectors for finiteness before reductions, and evaluate static monotonicity
over a dense log-frequency grid. The implementation's direct digital
coefficient path and the independent analog/prewarped reference must agree
within the predeclared relative-complex tolerance of 1e-6.

### Shape-specific detector

Peak retains the existing second-order high-pass/low-pass detector cascade
using the current Q-derived bandpass edges and Butterworth sections. A Low
Shelf uses one second-order Butterworth low-pass detector at `f0`; a High
Shelf uses one second-order Butterworth high-pass detector at `f0`. Each is
−3 dB at its corner. These detector filters read the original dry input, not
the equalized output or another band's output. The audio path remains serial:
each audible band's EQ processes the current sample after preceding audible
bands, so `x[n]` in the per-band blend is that band's serial input, not
necessarily the original host sample. The per-band detectors still read the
original dry sample. The existing global wet/dry control is applied after the
serial product. Their order and response are not dynamically changed by `S`.

Keep the existing global/per-band threshold and ratio overrides, soft knee,
attack/release, active/solo rules, and linked/unlinked core behavior. A shelf
uses the same existing `DynamicsCore` compression law; this feature does not
add a second dynamics law or a new detector-link mode. Tests must cover the
two shelf detector pass/reject regions, distinct stereo levels, both link
modes, band overrides, solo, and inactive bands.

The multi-band sample reference processes each participating EQ serially,
while each band's detector/envelope reference consumes that band's filtered
view of the original dry-input vector. Check the full-vector output for
mixed Peak/Low/High active bands, with different per-band input levels,
active/solo combinations, and linked/unlinked stereo. For held proportions,
the expected pre-mix transfer is the ordered product of participating
per-band blends; then apply the global wet/dry mix once. In changing-envelope
tests, use an independent sample recurrence and compare the entire output,
not just a curve or telemetry.

Preserve the existing Peak solo-selection behavior. In particular, current
DSP computes `any_solo` from solo flags even on inactive bands, then only
processes active solo bands when any such flag is set. An inactive solo can
therefore suppress active non-solo audio bands. The combined graph must show
the curve for that actual audible selection (possibly the flat response in
this corner); do not silently change legacy Peak samples while correcting
curve truthfulness.

Public-path dynamics tests must independently derive detector impulse/step
response and selected threshold/ratio/knee plus attack/release trajectories.
Cover partial activation, passband sustain and rejected-band input, linked
stereo where the louder channel alternates, per-band overrides, and dry input
isolation from a preceding audible band. Compare whole-vector output under
absolute event time across callback partitions. Include the inherited
`abs(target_gain_db) < 0.01` bypass boundary at values just below, equal to,
and just above ±0.01 dB. Test dry-to-wet transition, reset after populated
recursive state against fresh preparation, and a zero allocation/deallocation
guard after preparation. The natural recursive tail remains Unknown.

### Exact partially activated shelf response

Keep one shelf filter at the full target `G`; do not update coefficients on
the audio thread. For existing gain reduction `r >= 0`, define the desired
signed plateau gain

```text
g = sign(G) * min(r, abs(G))
p = clamp((10^(g/20) - 1) / (10^(G/20) - 1), 0, 1)
y[n] = x[n] + p[n] * (shelf_G(x)[n] - x[n])
```

For `abs(G) < 0.01 dB`, `p = 0`, matching the existing Peak zero-gain
convention. With a held proportion `p`, the steady-state complex transfer is
`H_p = 1 + p * (H_G - 1)`. The low/high asymptote reached by that blend is
exactly `g` dB. The transition region is the transfer-function blend shown
above; it is generally **not** an RBJ shelf redesigned at gain `g`, and its
midpoint is generally not `g/2` dB. That distinction is part of the user
contract, not a test-only implementation detail.

For a +12 dB target at a 1 kHz midpoint, commanding a +6 dB plateau gives
`p = 0.3338605754`. The proposed blend has +1.5289746841 dB at the midpoint,
while a shelf redesigned at +6 dB has +3 dB there. A −12 dB target at −6 dB
plateau gives `p = 0.6661394246`, with −4.4710253159 dB blended midpoint
versus −3 dB from a redesigned shelf. Both the defined blend and that
distinction are acceptance-test cases.

The graph shows the static, full-target curve built from `shape`, `f0`, `S`,
and `G`, labeled as the target curve. The gain-reduction meter remains a
separate live display. If a live held-gain curve is ever added, it must plot
`H_p` above. Do not draw the current gain as though it were a newly designed
RBJ filter. During changing `p[n]`, this is a time-varying sample blend, not a
time-invariant filter with an instantaneous coefficient; validate the causal
sample recurrence, transient response, and callback-partition behavior.

## State and parameter identity

### Rust, bridge, engine, presets

Add serde-defaulted `shape = peak` and `shelf_slope = 1.0` to both
`DynEqBandParams` and host `BandParams`; retain every existing field/key and
the prior meaning of a missing shape. Update constructors, conversions,
validation, defaults, `current_values`, parameter schema, state and preset
roundtrips, UI settings converters, and all public struct literals. Reject
invalid enum values, non-finite slope, and slope outside 0.1–1.0 before any
replacement is committed. Shape and slope are structural controls. The
in-place scalar setter must return the existing structural-change error
without mutating the live instance; control-thread transactional state/preset
replacement must consume the new values before constructing the replacement.

Retain the existing string IDs for every control. Add `band_{i}_shape` and
`band_{i}_shelf_slope` for `i = 0..7`. Preserve the seven entries in
`BAND_PARAMS` and `NUM_BAND_PARAMS`; code using the legacy template stride
must continue to resolve the old fields to the same IDs.

### C ABI and Audio Unit numeric addresses

Before production edits, capture the actual `DynamicEQ` C ABI handle's full
64-entry index-to-ID and metadata table plus actual saved state/preset bytes.
The current first 64 indexes must remain byte-for-byte equivalent in meaning:
8 globals followed by eight bands × seven legacy controls. Append shape and
slope descriptors after index 63 (indices 64–79, two per band) through an
explicit mapping; never insert them in the old band template. Add lookup and
roundtrip tests for all sixteen appended entries, choice discriminants, C ABI
parameter setting rejection for structural changes, and state/preset restore
into a fresh handle.

Represent the choice as stable discrete values `0=Peak`, `1=Low Shelf`,
`2=High Shelf`, with a two-step range. Do not enlarge or reorder the existing
`#[repr(C)] ParameterInfo`. Add an additive C ABI function
`plugin_get_parameter_choice_label(handle, parameter_index, choice_index)`
that returns the handle-independent static label pointer for a valid choice
and null for a non-choice or invalid index. Exercise valid and invalid queries
in the in-process FFI tests and wire the AU display to it. Preserve all prior
numeric AU addresses, and replace any seven-slot arithmetic used to locate
new controls with explicit ID-to-address lookup. This adds a function without
changing any existing ABI struct or address.

`plugins-bridge` must translate both shapes and slope in construction and
state replacement. The engine's global-only accessor is not sufficient for
per-band edits: update the existing band-edit configuration/converter route
and prove the applied plugin is rebuilt from the selected band state. Keep
existing structural update acknowledgments and rollback behavior.

### NIH CLAP/VST3

Keep stable string IDs and add readable native controls for all three shapes
and the slope. Since the wrapper currently hides structural fields, merely
adding those fields to `DynamicParams::from_infos` is insufficient. Follow the
existing structural fingerprint/reinitialize route so a user can select a
shape in an active instance and the replacement constructor receives its
shape/slope before activation. Test actual CLAP and VST3 callbacks with the
packaged `SotfDynamicEQ` plugin: read the parameter, select both shelf shapes,
render to demonstrate the processed response changes, save/reload state, and
verify a fresh native instance restores the same shape/slope. Do not claim
AU runtime execution unless a macOS AU host test is run.

## GPUI controller and mounted control route

Keep the existing UI IDs for locals 0–6. Assign local 7 to shape and local 8
to shelf slope, inside the existing ten-slot per-band range. In Peak mode show
Q and hide slope; in Low/High Shelf show slope and hide Q while preserving the
stored Q. Make the shape picker labels `Peak`, `Low Shelf`, `High Shelf`; make
slope display the slope value, not a Q or dB label. Keep the graph on the full
target curve described above.

Fix the current sparse-band navigation mismatch at the controller route:
construct the selectable ID list from the eight globals and actual visible
field IDs for the active `num_bands`, not a contiguous count from zero. Skip
nonexistent bands and shape-hidden controls (Q in shelf mode, slope in Peak
mode). Keep the Active control selectable when a band is inactive so keyboard
navigation can reactivate it; keep Solo selectable whenever the UI exposes
it. Next/previous must visit every such visible control and wrap in both
directions. Clamp or move selection to the nearest valid ID after a band
count/shape change. Test band-count changes, shape-dependent Q/slope
selection, and value adjustment through the real `PluginController`,
including deactivate-then-navigate-to-Active-then-reactivate.

For mounted UI acceptance, click the actual painted shape control, observe
the new band setting and a Structural update receipt, then save/load a disk
preset and construct a fresh graph/handle. Verify the acknowledged structural
state and applied processing through the existing route. A settings-only or
source-text check is not mounted-control evidence.

## Compatibility and preparation behavior

- Old JSON without a shape remains Peak and reproduces existing Peak audio.
- Existing seven per-band spec slots, string IDs, and all first 64 C ABI/AU
  parameter addresses retain their meanings.
- Peak coefficient construction, Peak detector filter design, and Peak
  modulation behavior remain unchanged. The proposed tests compare captured
  Peak audio before/after as exact f32 bytes for deterministic no-build-change
  cases and separately verify tolerances for platforms where floating point
  differs.
- Validate and stage the entire parameter/state replacement on the
  control thread. Invalid state, invalid sample rate/frequency/slope, or
  coefficient preparation failure leaves the live plugin, persisted config,
  active controls, and prior rendered history unchanged. No allocation,
  filter rebuild, locking, or state replacement occurs in `process`.
- Shape/slope/frequency/target-gain and current active/solo controls retain the
  existing structural update class; the setter cannot half-apply them.
- Reset/rebuild clears biquad and detector/envelope histories by the existing
  reset contract. Dynamic EQ's natural IIR tail remains recursive/unknown; no
  finite tail or EOF truncation claim is made.

## Captured pre-edit evidence

The source archive contains 17 preserved workspace files, including the
Dynamic EQ DSP/parameter files, bridge factory/state converters, FFI parameter
map/factory/API, and locked workspace manifests. It is under
`crates/sotf-plugins/target/audit-baselines/aud139-pre-edit/source/`, with
`source-manifest.sha256` at the archive root. `sha256sum -c` verified every
entry. Manifest SHA-256:
`4a31b8acebc843cf9a81495763711e8e0f8976046eed3decc5a3cd7057e24965`.

The ignored manual Peak capture ran with `--offline --locked` under the
process-held DAW Cargo flock:

```text
cargo test --offline --locked -p sotf-plugin-dynamic-eq --test aud139_peak_baseline \
  capture_aud139_pre_edit_peak_audio_and_serialized_parameter_baseline \
  -- --ignored --nocapture
```

It passed 1/1. Log `/tmp/sotf-aud139-peak-baseline-r2.log`, SHA-256
`4fd352364f67f01235e14b27f92a51ff936f56ece138491410c52f952a8966f2`.
It saved the deterministic 8192-frame stereo input, serialized parameters,
and six 65,536-byte output vectors under
`crates/sotf-plugins/target/audit-baselines/aud139-pre-edit/audio/`: linked
boost/cut, unlinked boost/cut, inactive Peak, and solo Peak. The test asserts
finite output, nontrivial signal, inactive transparency, and distinct
boost/cut and linked/unlinked results. The inactive vector is bit-exact to its
input. Test source SHA-256:
`f5785e8975f51e7b1b3c9a57fbdf8133a07382f0ed48d97f3f1b3d542f31fec5`.

The ignored in-process public C ABI capture also passed 1/1:

```text
cargo test --offline --locked -p plugins-ffi --lib \
  capture_aud139_pre_edit_dynamic_eq_parameter_addresses_and_state \
  -- --ignored --nocapture
```

Log `/tmp/sotf-aud139-ffi-baseline-r2.log`, SHA-256
`c019849acda10958a208e2bf3f5b3343824a11573f5b366f72cf515bf1098f5b`.
It created a real `DynamicEQ` handle through the public C ABI, captured all 64
old index/ID/metadata entries, and saved that handle's actual state and preset
JSON. It verifies index 15 is `band_1_frequency`. This is an in-process C ABI
test, not AU-host execution. Final test source SHA-256:
`ab853908f47061e6dfe1fdc456340bae21a248785529ab7b3beaa0ef79c1ebac`; its
test-only `lib.rs` registration SHA-256 is
`7f9af048ae0eb674e0bbbca44219a7ffb028f3a6c93cadd2b893404071463974`.

The combined audio/C ABI artifact checksum list is
`crates/sotf-plugins/target/audit-baselines/aud139-pre-edit/artifact-sha256.txt`,
SHA-256 `be3b96b841e3a577a4ff2ac5b978a0154cf9ad32c17614ee1873aae72e48c901`.
It contains checksums for all 17 captured input, audio, parameter, state,
preset and capture-manifest files. The initial audio compile attempt stopped
before running and wrote no baseline artifact; final receipts above are the
successful runs.

Those manual artifacts have also been copied into tracked test data:
`crates/sotf-plugins/crates/sotf-plugin-dynamic-eq/tests/data/aud139_pre_edit/audio/`
contains the input, six output vectors, parameter JSON for each case, and
capture manifest. The public C ABI table, state, and preset are in
`crates/sotf-plugins/crates/plugins-ffi/tests/data/aud139_pre_edit/cabi/`.
The per-fixture SHA-256 list is
`crates/sotf-plugins/crates/sotf-plugin-dynamic-eq/tests/data/aud139_pre_edit/durable-fixture-sha256.txt`.
Normal clean-checkout replay tests use these tracked files, not ignored target
artifacts.

### Matched optimized pre-edit callback CPU

Before changing Dynamic EQ production sources, I ran the optimized manual
callback benchmark with the preserved 17-file source archive still matching
the live Dynamic EQ source and the original lock. The release command was:

```text
cargo test --offline --locked --release -p sotf-plugin-dynamic-eq \
  --test aud139_cpu_baseline -- --ignored --nocapture
```

The test creates and initializes the plugin, resets state, generates input
blocks, and allocates its block pool outside each timed interval. It times
only the public `process_in_place` calls. Each trial processes 65,536 frames;
there are two warmup trials and seven measured trials. The matrix uses 48 kHz,
all bands active, linked channel detection, 4 or 8 Peak bands, 2 or 8
channels, target gains alternating +8/−6 dB, threshold −28 dB, ratio 3:1,
3 ms attack, 80 ms release, and 100% mix. Callback sizes are 64, 256, and
1,024 frames. These are sample distributions on the captured x86_64 Linux
host, not worst-case execution times. Construction/reset cost was outside
the timed interval and was not measured.

| Channels | Bands | Frames/callback | Median ns/frame | Median ns/callback |
|---:|---:|---:|---:|---:|
| 2 | 4 | 64 | 202.672 | 12,971.016 |
| 2 | 4 | 256 | 203.082 | 51,989.066 |
| 2 | 4 | 1,024 | 202.485 | 207,344.625 |
| 2 | 8 | 64 | 413.590 | 26,469.739 |
| 2 | 8 | 256 | 410.876 | 105,184.363 |
| 2 | 8 | 1,024 | 411.238 | 421,107.828 |
| 8 | 4 | 64 | 353.050 | 22,595.213 |
| 8 | 4 | 256 | 355.218 | 90,935.730 |
| 8 | 4 | 1,024 | 352.532 | 360,993.172 |
| 8 | 8 | 64 | 762.788 | 48,818.449 |
| 8 | 8 | 256 | 767.453 | 196,467.898 |
| 8 | 8 | 1,024 | 764.421 | 782,766.812 |

Log `/tmp/sotf-aud139-cpu-preedit.log`, SHA-256
`7b52d2aafd55ba38dc643a1daf36bf526bb46a4d33f2e8715c383c20d0385591`.
The benchmark source SHA-256 is
`7e48711da3adf514e91d2433180886308ad7607bfbd9be8d9337a9f3a1b4714a`.
The start/end selected source-hash lists match at SHA-256
`1c69c7cb22b6366667a3375931c62d46f7316ea4a28c314587c5e4b3cdd44930`.
The initial list mistakenly addressed `Cargo.lock` relative to the nested
workspace and omitted that one entry; the actual root lock SHA-256 was
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`,
matching the lock in the preserved source archive. The full `rustc -Vv`
output and benchmark source/log are also stored beside the tracked fixture
vectors under `tests/data/aud139_pre_edit/cpu/`.

## Acceptance gates for implementation

1. **Preserved baseline:** run the ignored peak audio/state capture before
   edits. Replay every saved sample after implementation through the same
   public constructor, serialized params and irregular partition sequence.
   Preserve all six vectors, the input, params JSON, capture manifest, C ABI
   enumeration, state JSON and preset JSON with SHA-256 receipts.
2. **Independent static reference:** implement a test-only f64 complex
   evaluator directly from the cookbook equations. Compare production
   coefficients/response at 44.1, 48, and 96 kHz; `f0` at 20 Hz, 1 kHz, and
   max valid; gain at 0, ±6, ±12, ±24 dB; and `S` at 0.1, 0.5, 1.0. Check
   DC/Nyquist endpoints, midpoint, full complex phase/magnitude, poles, and
   monotonic endpoint-bounded magnitude. Gate maximum relative complex error
   at 1e-6, with stated handling near zero response.
3. **Blend law:** independently verify held `p=0, .25, .5, .75, 1` against
   `1 + p*(H_G-1)` for positive and negative boosts/cuts and both shapes.
   Explicitly demonstrate that intermediate response differs from an RBJ
   filter redesigned at `g` for the selected fixture; never use the latter as
   the oracle. Also compare the full time-domain recursive filter output and
   mixed sample output against an independent f64 recurrence with normalized
   peak residual <=1e-5 and RMS residual <=1e-6.
4. **Dynamics and state:** verify each detector's passband, transition and
   rejection, sign, thresholds/ratios/knee, attack/release envelope, linked vs
   unlinked stereo, per-band overrides, inactive/solo, global mix and partial
   activation. Compare irregular partitions 1, 17, 64, 257, 1024, and 8192
   frames against one-block processing under identical absolute event timing.
   Invalid batch/state/preset and direct structural scalar setters must leave
   full live state and history unchanged; valid control-thread replacement
   must restore into a fresh configured instance.
5. **Realtime resources/performance:** after construction, run cold and
   repeated process/reset tests with an allocator that detects both allocation
   and deallocation for supported channel counts and the maximum callback.
   Rerun the same optimized benchmark source/settings/compiler at 64, 256,
   and 1024 frames. Compare post-edit Peak to the captured pre-edit matrix
   above (<=1.10x) and shelf to matched post-edit Peak (<=1.25x), or document
   the measured regression for review.
6. **Reachability:** cover bridge/factory and engine band state replacement,
   all legacy/appended C ABI indices and state/preset calls, the real active
   CLAP and VST3 callback routes, and a mounted GPUI shape click through
   Structural receipt to saved preset and fresh-instance restore. Clearly
   label an in-process C ABI test separately from actual AU host execution.

The implementation report must list exact source manifests and executed log
hashes, report actual observed maxima separately from acceptance thresholds,
and name any platform route not run. Astra accepted this bounded design
before production changes began; full feature acceptance remains pending the
numerical, lifecycle, resource, persistence, and consuming-route gates above.
