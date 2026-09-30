# AUD139: dynamic EQ shelf bands

Status: **AUD139 shelf core accepted; FFI state/descriptor route implemented and verified; engine and consuming-host routes remain in progress**.

Astra accepted the bounded shelf design and core. The current FFI stage preserves
the 64 legacy parameter addresses, appends 16 shape/slope descriptors, rejects
direct scalar structural writes, and transactionally rebuilds on state restore.
The captured legacy Peak preset now restores active bands as Peak while partial
state loads retain omitted live values. The latest `plugins-ffi --lib` gate
passed 78 tests with one manual capture ignored; scoped FFI all-target Clippy
passed with `--no-deps`. The public choice-label declaration is present in both
checked-in headers, which currently have identical SHA-256
`fe3e2512f366a2dc5aa742fe5474fe0e295669d07ed72b108a71c485ccc0d843`; this
gate skipped header generation/synchronization.

The actual engine offline-render route, native/consuming-host behavior,
reachable mounted controls, and matched CPU comparison remain open. This file
preserves design history and does not claim those later routes are complete.

## Comparison basis

Current [FabFilter Pro-Q 4 dynamic EQ documentation](https://www.fabfilter.com/help/pro-q/using/dynamic-eq)
describes dynamic bell and shelving bands, with detection normally following
the band's affected frequency range. Its controls also describe independent
trigger filtering and flat-tilt dynamics. These are feature comparison
dimensions, not a specification of the proprietary algorithm or a claim that
SOTF must reproduce its sound. Source checked 2026-09-29.

## Historical pre-implementation source findings (captured 2026-09-29)

The following table records the source state before AUD139 shelf implementation.
Treat it as historical evidence; paths and behavior may have changed since it
was captured.

| Layer | Evidence |
| --- | --- |
| DSP band state | `sotf-plugin-dynamic-eq/src/lib/dyn_eq_band_params.rs:8` has frequency, Q, target gain, threshold/ratio overrides, active and solo; no shape field. |
| Serializable host band state | `sotf-plugin-dynamic-eq/src/params.rs:170` separately defines `BandParams`, also without shape. Both representations need deliberate compatibility handling. |
| Filter construction | `src/lib/dyn_eq_band.rs:74,165` hardcodes `BiquadFilterType::Peak` in initial construction and rebuilding. |
| Existing math shelf behavior | The DAW manifest patches `math-iir-fir` to the sibling checkout. In `math-iir-fir/src/iir/biquad.rs:303-377`, standard `Lowshelf`/`Highshelf` use `beta = sqrt(2*A)` and never use the Q-dependent `alpha`. They implement a fixed shelf slope. Passing the current band Q to these constructors would not make Q affect the shelf. |
| Detection | `DynEqBand::apply_sidechain_bp` always cascades highpass and lowpass filters around the band's frequency/Q interval. Merely replacing the audio filter with a shelf would retain peak-style trigger behavior. |
| Dynamic gain | Audio uses a fixed full-gain filter and a dry/wet blend. `modulation_proportion` derives the blend from desired versus full target amplitudes. A new shape must define where this produces the intended gain and test the complete complex response. |
| Parameters | `params.rs:80-124` contains seven band parameters and a fixed `NUM_BAND_PARAMS = 7`. Shape is absent from the shared parameter template and UI state. Adding it requires examining index/ID consumers rather than assuming stride changes are compatible. |
| Runtime band descriptors | `src/lib/dynamic_eq_plugin.rs:388-405` constructs `band_{i}_{engine_key}` IDs from `BAND_PARAMS` but handles only float and bool types; other types are skipped. A new choice/int shape needs an explicit descriptor and value path as well as stable IDs. |
| Engine configuration | `sotf-engine/src/plugins/plugin_config_converter/dynamics.rs:367` passes the host `bands` state into the dynamic-EQ configuration. |
| Bridge factory | `plugins-bridge/src/factory.rs:122` deserializes `DynamicEqPluginParams` and constructs the real plugin. A serde-only field would not establish an audible route. |
| Packaged native exports | `plugins-nih/src/lib.rs:213-217` exports `SotfDynamicEQ` through both CLAP and VST3 under feature `dynamic-eq`, using plugin type `DynamicEQ`, CLAP ID `org.spinorama.sotf.dynamic-eq` and VST3 class ID `SotfDynEq0000001`. The route exists; it needs real build/load/state evidence for new shapes. |
| Native structural visibility | `plugins-nih/src/params.rs:54-143` creates dynamic native parameters from bridge information but hides non-realtime fields. Current band frequency/Q/gain/active/solo specifications are structural. Adding a structural shape spec alone therefore does not prove an accessible native control or successful active-instance reconstruction. |

Short DAW paths above are under `crates/sotf-plugins/crates/` except the engine
path, which is under `crates/`. The math shelf implementation is in the sibling
`../math-audio/crates/` tree. Findings are from current source, not a new
executed numerical regression.

## Independent response reference prepared for the design

These are analytical checks for the proposed feature, not executed SOTF shelf
tests. The math graph was one commit behind; the coefficient findings above
were checked against current source directly. Current full-file SHA-256:

- `dyn_eq_band.rs`: `6e59f6ad79eebb086fafc2ce07a3d92266248a11696c969dac307acc4c203df3`.
- `dynamic_eq_plugin.rs`: `8e4389fa7a8fdc961727b01513a8e723649db1c30ddb8b722ce2ca2450975498`.
- Sibling `iir/biquad.rs`: `51eac7d93dee8e9f213fbfc235272be5dcb398b772bece02b889f37b0bba760a`.

Use the analog prototypes and prewarped bilinear substitution in the
[W3C Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/) to build a
reference independently of the production coefficient builder. For DTFT
`H(exp(jΩ))`, `Ω = 2πf/Fs`, define `A = 10^(G/40)` and
`s = j*tan(πf/Fs)/tan(πf0/Fs)`. Then:

```text
L(s) = A * (s² + sqrt(A)*s/Q + A) / (A*s² + sqrt(A)*s/Q + 1)
U(s) = A * (A*s² + sqrt(A)*s/Q + 1) / (s² + sqrt(A)*s/Q + A)
```

Here `f0` is the shelf midpoint; evaluate DC and Nyquist using limits. The
existing standard shelf builder corresponds to `Q = 1/sqrt(2)` in this
reference. If the new design promises adjustable Q/slope, it needs a builder
that implements that promise and a matching displayed response. Do not label
the fixed-slope constructor as Q-controlled.

### Partial activation is a separate response contract

The current peak implementation uses a fixed full-gain filter with a varying
sample blend. For a *held* blend `p`, the derived transfer is
`Hmix = 1 + p*(HG - 1)`, where
`p = (10^(g/20) - 1)/(10^(G/20) - 1)` and `g` has the same sign as `G`.
At a shelf's affected plateau this produces exactly `g` dB. It generally does
not produce the response of a shelf redesigned at `g` dB through the
transition region. The zero/small-gain convention must remain explicit.

An independent Python complex-arithmetic probe evaluated both shelf shapes at
48 kHz with a 1 kHz midpoint and `Q = 1/sqrt(2)`:

| Full target G | Desired plateau g | Blend p | Blended midpoint gain | Shelf redesigned at g |
|---|---|---|---|---|
| +12 dB | +6 dB | 0.3338605754 | +1.5289746841 dB | +3 dB |
| −12 dB | −6 dB | 0.6661394246 | −4.4710253159 dB | −3 dB |

Low-shelf blend phases at the midpoint are respectively −26.48348° and
+26.48348°; high-shelf phases have the opposite signs. These differences
require an explicit design choice and truthful UI curves; a plateau-only test
would miss them. Existing peak compatibility must be measured independently
of whichever shelf behavior is selected.

The probe also checked unity gain, conjugate symmetry, and reciprocal static
boost/cut responses at Q values 0.1, 1/sqrt(2), 1 and 10, across DC, Nyquist
and five interior frequencies. Maximum static reciprocal error was
`5.55155e-16`; this is reference arithmetic consistency, not measured plugin
accuracy. Results: `/tmp/sotf-aud139-shelf-oracle-probe.json`, SHA-256
`96c48b0374ac6b6f7acb330f7fe59ed1e4df1491d69f0e8cc484703efc91ca3f`.

For changing `p[n]`, the whole processor is time-varying. Validate its causal
time-domain recurrence and modulation artifacts; a held-response plot is not
a transient oracle. The detector currently reads the original dry input for
every band, linked detection takes the channel maximum before the shared
envelope, and unlinked detection has per-channel envelopes. Preserve or
deliberately document these semantics. Test partial and full activation, both
gain signs, shape-specific detector rejection, distinct-channel triggers,
irregular partitions, and both low-rate and near-Nyquist boundaries.

## Accepted implementation constraints (retained from design review)

### Confirmed native parameter identity constraint

The numeric-ID concern is concrete for the AU/FFI route. In
`plugins-ffi/src/parameter_map.rs:145-172`, globals are emitted first, followed
by band-major expansion of `BAND_PARAMS` for eight bands (`:429-461`). The
current Dynamic EQ layout therefore has eight global entries followed by
seven entries for each band. `plugins-au/GenericAU/GenericRustAudioUnit.swift:1087-1122`
uses the FFI enumeration position directly as `AUParameterAddress(i)`;
`plugins-au/DynamicEQAudioUnit/DynamicEQAudioUnit.swift:6-7` selects this
generic implementation for Dynamic EQ.

Consequently, appending shape inside the seven-entry band template would
change existing AU addresses. For example, current address 15 denotes
`band_1_frequency`; with eight entries per band, address 15 would denote the
new `band_0_shape`. This is a source-derived mapping consequence, not an
executed AU host session. Existing automation must retain its old meaning.
Capture the actual FFI address-to-ID list before implementation and verify it
afterward. One possible design is to retain all 64 legacy entries and append
the eight shape entries afterward, with explicit lookup translation; the
implementation design must settle this rather than silently shifting IDs.
The AU UI adapter also derives band offsets from the template length
(`plugins-ffi/src/au_host.rs:81-89,159`), so it needs consistent mapping if the
flattened representation ceases to be a uniform band stride.

NIH CLAP/VST3 use a different identity mechanism. `DynamicParams::param_map`
returns the original string IDs (`plugins-nih/src/params.rs:275-288`), and
the CLAP wrapper and VST3 inner wrapper pass those strings to
`hash_param_id`. That helper (`nih-plug/src/wrapper/util.rs:40-50`) uses the
same stable 31-based string hash with the high bit cleared. Preserving
`band_{i}_{engine_key}` strings preserves those computed IDs; still capture
the actual exposed maps and check collisions/new control visibility. Do not
conflate native numeric IDs with enumeration order or per-kind vector slots.

The engine's generic Dynamic EQ accessor exposes only its eight global
parameters (`sotf-engine/src/plugin_param_accessors.rs:577-585`), and
`PluginParamDef` likewise maps indices 0 through 7. Per-band state therefore
needs its own existing band-edit path followed through to the actual applied
plugin; extending the global accessor alone would not wire shelf controls.

Paths in this subsection are relative to `crates/sotf-plugins/crates/`, except
`sotf-engine` under `crates/` and `nih-plug` under `crates/3rdparties/`.

### Design choices

1. Define low/high shelf frequency, Q/slope and signed target-gain semantics,
   including valid ranges at low sample rates. State the detector response for
   each shape, solo meaning and linked/unlinked behavior.
2. Define the dynamic transfer for partial activation. The existing blend can
   be verified as a blend of transfer functions; it must not be described as
   a continuously redesigned biquad unless the implementation does that.
3. Preserve old presets as peak bands, stable public parameter keys and any
   externally persisted numeric IDs. Resolve both band-state representations,
   engine accessors, real bridge/FFI/native routes and UI graph/control mapping.
   The existing NIH macro exports a real Dynamic EQ plugin; use its exact
   stable IDs and exercise its persisted structural configuration, not merely
   a new parameter's presence in the shared specification.
4. Follow the existing structural-update contract for shape/frequency/Q/gain
   changes. Preparation, activation, failure rollback and reset must be
   explicit; rendering must not allocate or rebuild heap storage unexpectedly.

## Evidence still needed for the full feature

- Preserve meaningful old peak-band audio before edits: positive/negative
  target gains, inactive and solo bands, linked/unlinked channels, irregular
  callback partitions and restored presets.
- Independently derive shelf response references rather than compare the
  production coefficient builder with itself. Cover passband endpoints,
  transition gain/phase, high-Q cases, positive/negative targets and rates near
  the allowed frequency boundary.
- Verify detector pass/reject behavior and steady/transient dynamics using
  independent level/ratio/knee and timing references. Test partial activation,
  not only neutral and fully triggered endpoints.
- Exercise the full configured engine/plugin chain and bridge state roundtrip,
  declared parameter mapping, automation/reconfiguration, reset and the cold
  and repeated realtime callback paths. Prove native routes only where actual
  packaged plugins are built and loaded.
- Reach the new shape through real host controls and preset restoration;
  compare the displayed response with the applied setup.
- The bounded core design was accepted by Astra at medium review. Subsequent
  route stages still require their own executed evidence and review.

Tilt, spectral dynamics, per-band channel/M/S selection, phase modes and broader
quality evidence remain part of the full audit. Implementing shelves alone will
not establish full dynamic-EQ or whole-workspace feature parity.
