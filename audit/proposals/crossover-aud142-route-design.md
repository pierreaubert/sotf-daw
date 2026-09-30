# AUD142 concrete route and baseline decisions

Status: **bounded design accepted by Astra; pre-edit captures are complete and
the pure DSP/public Crossover core is implemented and verified**. The FFI,
actual engine, NIH native, packaged host and mounted UI routes remain open.
Prepared 2026-09-30 from the current checkout. The accepted filter
mathematics is in `crossover-iir-families.md`. This note pins route choices,
pre-edit evidence and the implementation boundary; it does not claim full
feature acceptance.

## Current route facts

- `sotf-plugin-crossover/src/params.rs` has app-facing parameter order
  `type`, `frequency`, `mode`, `fir_taps`, with only LR24 and LinearPhase in
  its choice labels. `PluginSettings::Crossover` stores only type/frequency/
  output/taps, and `convert_crossover()` writes `extra_frequencies: []`.
- `CrossoverPlugin::parameters()` has configuration-dependent runtime order:
  `type`, `frequency`, `mode`, then `frequency_2`/`frequency_3` for active
  extra splits, then `fir_taps` for FIR. Per-channel mode keeps `type`, replaces
  the global frequency/mode fields with alternating zero-based
  `channel_frequency_N` and `channel_mode_N` fields. These IDs and their
  existing per-instance ordering are the runtime/FFI compatibility surface.
- `plugins-nih::wrapper::get_param_specs()` has no Crossover arm. Its fallback
  introspects the default plugin, and `bridged_info_from_parameter()` returns
  `None` for `ParameterValue::String`; the current native Crossover therefore
  publishes only `frequency` from its default LR24 instance. It does not
  publish a native `type` choice or `mode` choice today. Its generated layout
  is fixed at two input and two output channels (`channels: 2`).
- `plugins-ffi::parameter_map::get_param_specs()` also has no Crossover arm.
  FFI uses runtime plugin parameters and canonical string config values; it
  does not currently expose a numeric Crossover family enumeration. Do not
  add a fixed FFI schema arm that replaces the configuration-dependent runtime
  order. Before changing either route, capture representative NIH/CLAP/VST3
  parameter metadata and FFI parameter lists for LR24 two-way, LR24 four-way,
  FIR, and per-channel instances.

## Width, identities, and host buses

Channel count is the input width negotiated by the engine/plugin graph. It is
not inferred from the number of crossover bands. For engine and FFI
per-channel construction, support widths 1 through `EngineConfig::MAX_INPUT_CHANNELS`
(currently 64); require exactly one cutoff and one active mode per input
channel. Per-channel output width is the same input width. Lowpass and
highpass band selection also retain `input_width`, regardless of dormant band
count. Only `Both` has band-major output width `input_width * band_count`,
computed with checked multiplication and reported to the graph. Reject
unsupported/overflowing widths; never truncate to stereo. The native package
remains explicitly stereo because its current generated plugin is fixed to
two input channels.

Keep engine/plugin runtime IDs exactly as currently named:

| Runtime ID | Meaning |
| --- | --- |
| `type` | Canonical family string |
| `frequency` | First split |
| `mode` | Lowpass, highpass, or both |
| `frequency_2`, `frequency_3` | Second/third ordered splits |
| `fir_taps` | FIR tap count |
| `channel_frequency_N`, `channel_mode_N` | Zero-based per-channel values |

The app-facing static schema keeps the current four keys/order and appends the
new family labels after choice indices 0 and 1. Keep JSON and FFI state under
the canonical string field `type`; existing string aliases remain supported.
For the native wrapper, add an explicit native schema rather than trying to
bridge runtime strings as numeric controls. Preserve its one existing host
parameter ID, `frequency`. Append these native IDs in this fixed order:

| Native order | ID | Type and meaning |
| ---: | --- | --- |
| existing | `frequency` | Float; primary cutoff |
| 1 | `family` | Choice 0..12 in the canonical family order |
| 2 | `mode` | Choice: lowpass, highpass, both; structural because Both changes output width |
| 3 | `fir_taps` | Integer; FIR-only value |
| 4 | `topology` | Choice: bands, per_channel |
| 5 | `band_count` | Choice: 2, 3, 4; retained while low/high output is selected |
| 6 | `frequency_2` | Float; second cutoff |
| 7 | `frequency_3` | Float; third cutoff |
| 8 | `channel_frequency_0` | Float; native input channel 0 cutoff |
| 9 | `channel_mode_0` | Choice: lowpass, highpass, mute, passthrough |
| 10 | `channel_frequency_1` | Float; native input channel 1 cutoff |
| 11 | `channel_mode_1` | Choice: lowpass, highpass, mute, passthrough |

These are new native identities except `frequency`; in particular, `family` is
new because current NIH source exposes no native `type` choice. Its choice
order is LR24, LinearPhase, LR12, LR48, BW6, BW12, BW18, BW24, BW30, BW36,
BW42, BW48, Bessel12. Map native `family` to the canonical JSON `type` string
inside the Crossover configuration adapter. Keep `family`, `topology`, and
`band_count` structural; keep active IIR cutoffs real-time where the filter
path supports smoothing; keep FIR frequency/taps structural. Do not reuse the
app parameter positions as NIH parameter IDs.

The CLAP configurations are:

| Input | Output | Valid native route |
| ---: | ---: | --- |
| 2 | 2 | Bands low/high, or per-channel |
| 2 | 4 | Bands + Both + 2 bands |
| 2 | 6 | Bands + Both + 3 bands |
| 2 | 8 | Bands + Both + 4 bands |

The 4/6/8-channel output is band-major and has no speaker-layout labels. For
low/high and per-channel operation, the plugin packs the selected result to the
stereo output. Reject initialization when mode/topology/band count does not
match the selected CLAP width.

The VST3 configuration is one stereo input bus plus a maximum of four stereo
output buses. For `Both`, activate a contiguous prefix of 2, 3, or 4 buses
matching `band_count`, with slots named `Band 1` through `Band 4`. For
lowpass, highpass, or per-channel, activate only the main bus and label it
truthfully for the selected result (`Low Only`, `High Only`, or `Per-channel`),
not `Band 1`. Require every active bus buffer; legally absent inactive bus
storage is accepted. Silence valid provided inactive buffers when appropriate
without dereferencing absent storage. Index buffers by their actual bus slot;
never compact sparse host slots. Reject non-prefix activation, a band count
that conflicts with active bus geometry, or a declared DSP/output width
mismatch. This follows the existing BandSplit bus capacity while giving
Crossover its own mode/topology checks.

## Full state and partial updates

Add serialized `topology` (`bands` or `per_channel`), `extra_frequencies`,
`channel_frequencies_hz`, and `channel_modes` to typed Crossover settings.
Preserve every valid dormant value when topology changes. The active topology
alone controls audio and width:

- `bands`: primary `frequency`, `extra_frequencies`, and `mode` are active;
  channel arrays remain stored and inactive.
- `per_channel`: family, channel cutoffs, and channel modes are active; global
  cutoffs, band count, and output mode remain stored and inactive.
- If legacy state omits `topology`, infer `per_channel` only when it has
  non-empty channel cutoffs and no extra cutoffs. Otherwise infer `bands`.
  If both non-empty channel and extra cutoff arrays occur with no topology,
  reject as ambiguous. New state always writes topology explicitly.
- In active per-channel mode, frequency count must equal negotiated input
  width. Explicitly supplied `channel_modes` must have the same length and
  every mode must parse. For legacy public `CrossoverPluginParams`, retain its
  existing empty/short-mode fallback to scalar `output`; reject fallback
  values such as `both` that are not per-channel modes. In the new typed
  settings route, use an optional mode vector so omitted legacy data can use
  that fallback while explicit empty/short data is rejected.
- Inactive arrays are serialized and validated for finite values and
  recognized enum strings, but need not fit the current host width or
  sample-rate ceiling. Apply the new UI's 20–20,000 Hz bounds to newly supplied
  values. Do not retroactively reject an old value that was legal through an
  existing public LR24/FIR API; if made active on that legacy route, retain its
  legacy admission behavior. New-family values must pass the new-family bounds
  before commit.

Full-state restoration is an atomic configuration operation: parse all fields,
validate topology, width, cutoff ordering, mode/family, and bus geometry, build
and initialize a detached candidate, then replace the live plugin. On failure,
keep settings and stateful audio unchanged. Treat old NIH state containing only
`frequency` as LR24, bands topology, two retained bands, and lowpass output;
preserve the saved frequency. Restoring that old state over an instance with
newer populated family/topology/mode values replaces them with those explicit
legacy defaults. Do not confuse old full-state import with partial FFI state:
partial FFI updates merge omitted fields from the current configuration under
their separate merge contract. For modern complete native state, saved
topology, mode, band count, and selected bus geometry must agree. A deliberate
control-side negotiation/replacement path is allowed, but bus width must never
silently overwrite saved complete state. Preserve the old `frequency`
parameter's range, normalized representation, and saved value. Test old
frequency-only restore over a populated modern instance, plus conflicting
typed, bus, and opaque state; every rejected restore must preserve stateful
audio. When both an old alias and canonical field are present, the canonical
field wins; unknown choice indices/strings are errors.

Partial scalar updates are distinct from full-state migration. `frequency`,
`frequency_2`, and `frequency_3` retain their IDs and update only their own
cutoff. Require strict ordering against current neighbors; a crossing, NaN,
infinity, or out-of-range value returns an error without changing the smoother
target, coefficients, or delay state. A coordinated ordered shift is applied
through one complete settings/candidate replacement, never a sequence of
temporarily crossing scalar writes. Family, topology, band count, FIR taps,
input width, and per-channel arrays remain structural. FFI replacement of a
fixed-width handle must reject any change to input/output width before
swapping the candidate. Reject LinearPhase when per-channel topology is active,
before replacing any existing route; FIR per-channel behavior is out of scope.

## Cutoff, sample-rate, and automation bounds

For every active new-family split, admit only
`20 Hz <= fc <= 20,000 Hz` and `fc < 0.495 * sample_rate`; require strict
ascending order and no duplicate split. Apply the same bound to active
per-channel cutoffs. At 44.1/48/96 kHz the nominal upper bound is 20 kHz; at
lower rates the 0.495-sample-rate limit applies. Reject zero sample rate,
nonfinite/out-of-range cutoffs, or a sample-rate reinitialization that would
make any active cutoff illegal. Validate before mutating sample rate, smoother,
coefficients, or recursive state. New-family construction and reinitialization
reject; they never silently clamp. Keep the existing LR24 and FIR acceptance
behavior on their legacy paths.

Use f64 coefficients and states in bounded first/second-order sections. New
families use the same 20 ms logarithmic scalar smoothing and persistent
16-sample coefficient-update phase as LR24. The clock phase survives callback
boundaries. Family/topology/band-count changes rebuild; per-channel cutoff and
mode values remain structural and are not automated in this implementation.
Do not promise instantaneous LTI/all-pass behavior during a coefficient ramp.

For each automation partition comparison, render the same input and schedule
absolute parameter event frames across all partitions. Assert every output
finite before computing bounds. Declare:

- `max(abs(output)) <= 8 * max(abs(input)) + 1e-6` over all output channels
  and frames during transitions, including silence/reset controls.
- `max(abs(output_a - output_b)) / max(max(abs(input)), 1e-12) <= 2e-5`
  across the full rendered vectors. Keep the stricter existing LR24 fixture
  gate (`max absolute difference <= 1e-6`, fixture output peak <= 2.0).
- `<= 0.002` absolute complex transfer error per branch and actual sum, and
  `<= 0.002` settled RMS residual normalized to nonzero input RMS. Near-null
  outputs use input-RMS normalization, never branch-output normalization.

For settling, treat `ceil(ln(1e-5) / ln(rho_max))` as a minimum decay estimate
for a bare exponential only; it does not account for residue magnitude or
polynomial factors from repeated poles. Extend it using a conservative
transient bound that accounts for multiplicity/residues, or independently
verify convergence before choosing the measurement window. Then continue
through an integer number of coherent cycles for the probe tone. Compare
complex responses with coherent least-squares projection and report maxima by
family, rate, cutoff, branch, and sum. The bounds are acceptance targets, not
measured results; do not raise them after observing a failure.

## Existing AUD141 and AUD142 pre-edit evidence

AUD141's four audio baseline pairs originated in
`crates/sotf-plugins/target/audit-baselines/aud141-preedit/` and were copied
byte-exactly to `audit/artifacts/aud142-preedit/aud141/` before DSP edits.
Root's recorded control check is `/tmp/sotf-aud141-root-control-check.json`
(SHA-256 `87143a00197fc7b45ba7d92dedb0e8c65ed4e410431666439f0e058349d17982`);
the replay log and output hashes are retained under
`audit/artifacts/aud142-preedit/logs/`. Current replay is included in the
passed package run. These fixtures originated in the target directory; a
clean-checkout replay has not been established. The ignored replay test reads
them from `SOTF_AUDIT_BASELINE_DIR`.

| Preserved output vector | SHA-256 |
| --- | --- |
| `two-way-lr24-stereo-output-f32le.bin` | `d09256fe721d0e3c872ada6b2e301c897cfcc34def707a4ed9aa32fdd5cc71dc` |
| `per-channel-lr24-stereo-output-f32le.bin` | `b708f735e0a77786db98505420358d3c7ecdfbc9e06d2099eddf7f9767e11c15` |
| `four-way-fir-stereo-output-f32le.bin` | `4c4c6d11cbc47a77a2137c4c0337a4941a3bb9c789b17a17d034fc916ca90c6b` |
| `four-way-lr24-final-high-stereo-output-f32le.bin` | `88f989fa5649f6d17132b4dc92b4f5705fd997b9da27a2e257231a8f194d12bb` |

All four use the same saved input vector, SHA-256
`10832c567a2298b403c7a41f5adc929fb574d4e4db8e0bb55e7bf77b6da406c0`.
The preserved pre-edit `crossover_plugin.rs` SHA-256 is
`ed8bc6a21aac4f95ef4a1d113ee5503b23de5101e084d364c2ccb0fdeaa8a121`.
The source and replay provenance is described in
`audit/crossover-multiway-recombination.md` and `audit/reviews/AUD141-astra.md`.

Before DSP edits, seven public Crossover cases were captured under
`audit/artifacts/aud142-preedit/crossover/`: two-, three- and four-way LR24
`Both` (every output band), LR24 Low, LR24 High, LR24 per-channel, and
four-way FIR process plus its complete EOF drain. The FIR vector uses 1,025
taps and three split stages. `tests/aud142_preedit_baselines.rs` replays these
vectors bit-exactly; the scoped package run passed the AUD141 and AUD142
replays after the pure-core edits. See
`audit/crossover-aud142-pre-edit-baselines.md` for byte counts and case
details. The source archive proves source preservation, not a clean-checkout
reconstruction of all transitive dependencies.

The optimized pre-edit benchmark is preserved at
`audit/artifacts/aud142-preedit/criterion-r3/` with 30 raw samples, estimates,
and summary for the exact 31 legacy cases. A paired post-core run used the
same archived executable and the current binary with unchanged legacy case
IDs and settings. Raw current/archived outputs, per-case sample CVs, exact
commands, environment and qualified results are in
`audit/artifacts/aud142-post-core/`; it adds separate absolute setup and
processing results for LR12, LR48, Bessel12 and representative BW6/BW42/BW48
routes. These are throughput samples, not WCET or feature-performance
acceptance.

Pre-edit NIH, CLAP/VST3 and FFI enumeration/state captures are summarized in
`audit/crossover-aud142-pre-edit-baselines.md` with their execution logs and
hashes. The native routes have no current `type` choice parameter to extend:
NIH filtering publishes only `frequency`, and FFI currently exposes the
dynamic runtime list. Keep stable native/FFI identifiers and append behavior
as specified above when implementing the remaining route work.
