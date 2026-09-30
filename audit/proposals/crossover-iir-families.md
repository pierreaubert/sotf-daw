# AUD142: selectable crossover families and complete configuration routes

Status: **bounded design accepted by Astra with explicit implementation
requirements; pure DSP and public Crossover core implemented and verified**.
This checkpoint does not close the FFI, actual engine, NIH native, packaged
host or mounted UI routes. See `crossover-aud142-route-design.md` for source
findings and decisions on native capacity/identity, bus layouts, state
precedence, frequency admission, automation bounds and pre-edit baselines.
The pure-core evidence and qualified throughput data are collected in
`audit/artifacts/aud142-post-core/pure-core-results-r1.md`. Prepared
2026-09-30 against current source. Existing local issue AUD142 in `AUDIT.md`
owns this work.

## Comparison and source findings

The [miniDSP Flex documentation](https://docs.minidsp.com/product-manuals/flex-dl/dsp-reference/crossover.html)
offers Butterworth orders 1–8, LR12/24/48 and second-order Bessel. These establish
concrete missing choices. This proposal defines SOTF's response conventions;
it does not claim to reproduce undocumented vendor coefficients.

Current `CrossoverKind` and the app-facing `CROSSOVER_TYPES` expose only LR24
and LinearPhase. The plugin owns f64 LR24 banks behind its f32 boundary,
including separate two-way, multiway and per-channel paths. AUD141 corrected
the LR24 multiway branch product. Existing constructors support extra split
frequencies and per-channel modes, but typed `PluginSettings::Crossover` omits
them and its converter hardcodes `extra_frequencies: []`. Engine type accessors
hardcode 0/1 and silently fall back to LR24. The exact current NIH and FFI
parameter routes differ from that app schema; the supporting route note records
those source boundaries and the new native IDs.

## Stable names and choices

Keep the app-facing static parameter keys and indices 0=`type`,
1=`frequency`, 2=`mode`, 3=`fir_taps`. Keep choice 0=LR24 and choice
1=LinearPhase; append LR12, LR48, BW6, BW12, BW18, BW24, BW30, BW36, BW42,
BW48, Bessel12. Do not insert options before old choices. Preserve the string
aliases accepted by `CrossoverKind::parse`. Centralize conversions from the
canonical 13-entry list; invalid numeric choices and unknown strings must fail
before committing a new configuration.

The runtime plugin IDs remain `type`, `frequency`, `mode`, `frequency_2`,
`frequency_3`, `fir_taps`, and zero-based `channel_frequency_N` /
`channel_mode_N`. Current `plugins-nih` and FFI code do not publish the static
Crossover schema: NIH drops runtime string parameters and the FFI returns an
empty Crossover `ParamSpec` list. Therefore the currently published native
Crossover control is `frequency`; there is no old native two-choice `type`
address to enlarge. The new native enum uses the new stable ID `family`, while
JSON/app/FFI configuration continues to store the canonical string under
`type`. Preserve the existing native `frequency` ID. See the route note for the
full native and FFI metadata map and state migration rules.

## Frequency and phase contracts

The following are proposed design choices to be reviewed, not measured SOTF
results. Let `u = j*tan(pi*f/fs)/tan(pi*fc/fs)` for a stationary digital
frequency response. `fc` is in Hz; digital frequency is `2*pi*f/fs` radians
per sample. Use bilinear prewarping independently for each split.

- **Butterworth N=1..8:** unit-DC analog lowpass prototype with left-half-plane
  unit-circle poles `exp(j*pi*(2*k+N+1)/(2*N))`, k=0..N-1. Highpass follows
  the lowpass-to-highpass substitution `u -> 1/u`, with unity positive gain
  at the high-frequency endpoint. Both magnitudes at fc are `1/sqrt(2)`.
  Expose the canonical filter polarity; do not silently add inversion based
  on output selection. Even-order LP+HP sums can peak or cancel and must be
  presented truthfully. External routing/polarity remains under user control.
- **Linkwitz–Riley 2N:** square the Butterworth-N LP and HP transfer functions.
  LR12 uses N=1 with the emitted high band negated; LR24 uses N=2 and LR48
  uses N=4 with positive high-band polarity. Both branch magnitudes at fc
  are 0.5, and their signed sum has unity magnitude. LR12's high-band sign
  is the same in high-only, both, multiway and per-channel outputs, and is
  documented in the choice help. Preserve the exact existing LR24 path.
- **Bessel12:** choose magnitude normalization, with both LP and HP at
  `1/sqrt(2)` magnitude at fc. Let `a=sqrt(3*(sqrt(5)-1)/2)` and
  `L_norm(u)=3/((a*u)^2+3*a*u+3)`. Define `H(u)=L_norm(1/u)`.
  Both endpoint gains are positive. There is no unity-sum
  promise. The [SciPy Bessel documentation](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.bessel.html)
  distinguishes magnitude, phase and delay normalization and explains that
  bilinear conversion does not preserve analog maximally flat group delay
  over the entire digital band. UI/docs must not claim digital linear phase.

Implementation should use bounded first/second-order sections with f64
coefficients and state. Keep setup and storage allocation outside processing.
Do not copy a production coefficient generator into the accuracy oracle.

## Two-way, multiway and per-channel behavior

Two-way outputs are the declared LP/HP responses with band-major packing for
`both`. Preserve low/high aliases and existing FIR behavior.

For LR families, retain the AUD141 decomposition with the signed HP above:
`B_b = product(H_i, i<b) * L_b * product(L_i+H_i, i>b)` and final
`B_m = product(H_i, i<m)`. The stationary sum is the product of all-pass sums.
Each literal compensation pair needs independent persistent state. Automation
does not permit commuting time-varying filters or claiming instantaneous
unity recombination.

For Butterworth/Bessel, use conventional serial splitting:
`B_b = product(H_i, i<b) * L_b`; `B_m = product(H_i, i<m)`.
There is no artificial all-pass compensation. Low-only selects the first
emitted band; high-only selects the last; both emits all bands. Document the
family-specific sum behavior and measure every branch and the actual sum.
Family changes are explicit structural changes, not hidden compatibility
migrations of existing LR24/FIR presets.

Extend per-channel operation to each new IIR family, retaining existing
lowpass/highpass/mute/passthrough semantics and equal input/output width.
Reject LinearPhase in per-channel topology because FIR per-channel routing is
outside this design; preserve the previous valid configuration on rejection.
The route note fixes channel negotiation, active versus dormant topology
precedence, cutoff/sample-rate admission, and structural versus
scalar updates. Keep source cutoffs ordered without reassigning parameter IDs
during scalar automation; crossing/invalid updates must preserve live state.
Preserve the 16-sample persistent coefficient clock and existing smoothing for
old LR24 behavior; use the same 20 ms log smoothing and persistent clock for
new IIR scalar cutoffs.

## Complete settings, host and visible control route

Add serde-defaulted typed `extra_frequencies`, `channel_frequencies_hz`,
`channel_modes` and `topology`, forwarding the active topology to the public
constructor while preserving inactive values in serialized settings. Missing
topology in legacy data is inferred only from an unambiguous legacy per-channel
payload. Preserve the legacy public fallback for omitted/short per-channel
modes; a new typed explicit mode vector must match the active width exactly.
Unknown modes, ambiguous legacy topology and explicit invalid data fail before
the current live configuration is changed.
Import of the old NIH state containing only `frequency` means LR24, bands
topology, two retained bands and lowpass output; preserve its frequency and
reset any newer populated native family/topology/mode to those defaults. Keep
this native full-state migration separate from partial FFI merge semantics.

Expose a 13-choice family selector, ordered two/three/four-way cutoffs and a
per-channel editor. Only `Both` has band-major output width
`input_width * active_band_count`; lowpass, highpass and per-channel output
retain `input_width`. The native package remains stereo input: CLAP has 2→2,
2→4, 2→6 and 2→8 layouts; VST3 has one stereo input and four optional stereo
output buses. The route note specifies which topology/mode is valid for each
layout and rejects width mismatches. Engine/plugin channel width is negotiated
from the graph input independently of band count. FFI fixed-width replacements
reject any input/output-width change before replacing a populated live
instance. Treat `mode` as structural, including low/high to Both transitions
that change output width.

Family, band count, routing mode, topology, FIR taps and input width are
structural changes that build and validate a prepared candidate. Active IIR
scalar cutoff edits keep their scalar route; per-channel edits remain
structural. Verify real mounted selection and keyboard navigation, actual
queued update types, and applied engine audio. AU execution remains a named
platform-specific gate.

## Baselines and acceptance gates

1. The four AUD141 vector pairs and seven AUD142 pre-edit public vectors were
   preserved before DSP edits. The corrected multiway-LR24 `Both` and FIR
   process-plus-EOF vectors are included. Release-profile setup and callback
   baselines are preserved separately. Their target-only origin and
   clean-checkout limits are recorded in the route note and baseline document.
2. Build the independent response oracle from analog pole products and the
   stated prewarp. Check endpoints, fc magnitudes, slope trends and signed
   complex response. Check LR sums against unity; compare BW/Bessel sums with
   their actual reference instead of requiring unity. Validate the Bessel
   normalization and LR12 polarity independently before production work.
3. Exercise every new family/order at 44.1/48/96 kHz, selected low/central/high
   legal cutoffs, close/wide multiway splits and distinct channel signals.
   Cover accepted/rejected sample-rate changes and near-Nyquist limits without
   deriving expected values from a silently clamped production cutoff.
4. For public waveform measurements, use the slowest pole-radius decay as a
   minimum settling estimate only; extend it for repeated-pole factors and
   residue magnitude using a conservative transient bound or independent
   convergence evidence. Use coherent windows or least-squares projection,
   and check every sample finite. Initial proposed absolute complex error
   <=0.002 and settled
   input-normalized RMS error <=0.002 apply to every branch and actual sum.
   Near-null outputs use absolute input-normalized error. Record maximum
   observed errors and do not relax bounds to bless an incorrect reference.
5. Cover first/repeated processing, populated repeated reset, reinitialization,
   invalid updates and identical sample-timed automation across irregular
   partitions. For new IIR families, require finite output peak <=8 times the
   input peak plus 1e-6 and input-peak-normalized maximum partition difference
   <=2e-5; the route note gives the exact vectors and denominator. Preserve the
   stricter existing AUD141 LR24 check (absolute max partition difference
   <=1e-6 and fixture peak <=2.0). Check allocations and deallocations on the
   complete tested callback/reset paths. IIR tail remains Unknown; preserve
   accepted FIR finite drain and latency.
6. Exercise public constructors, facade, typed settings/converter, real
   Crossover -> BandMerge engine audio, FFI state/presets and native setup.
   Restore old state and each new choice into fresh and populated instances.
   Failed candidate replacement must preserve settings and stateful audio,
   with a positive control proving the continuation oracle can detect reset.
7. Execute real mounted selector/topology/per-channel controls and disk preset
   reconstruction, plus rebuilt packaged CLAP/VST3 and consuming-host routes.
   Pass targeted accuracy/replay/lifecycle tests, strict lint, then the combined
   integration gate at a coordinated source snapshot. Astra reviews actual
   implementation and evidence; this brief closes no feature by itself.

## Ownership and sequencing

The pure numerical core checkpoint is complete. Implement the remaining
AUD142 FFI and actual engine routes next, coordinating shared parameter-map
and factory files with their current owners; then add Crossover's NIH schema,
native buses, and mounted UI. Use only the accepted route design and the
captured metadata as compatibility constraints. Preserve MIDI/IAMF exclusions,
concurrent changes, and the existing prohibition on broad generic host
queue/manager rewrites. The complete audit still includes custom crossovers
and other feature/quality gaps beyond these named families.
