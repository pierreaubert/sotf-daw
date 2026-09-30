# AUD142: selectable IIR crossover families and slopes

Status: **IIR families/core implemented; complete route delivery remains open**.
The original source/product comparison below is from 2026-09-29. The current
implementation plan records the later numerical, compatibility and CPU gates,
runtime metadata checks, and active FFI/engine/native/UI work.

The current [miniDSP Flex crossover documentation](https://docs.minidsp.com/product-manuals/flex-dl/dsp-reference/crossover.html)
offers Butterworth slopes from 6 through 48 dB/octave in 6 dB steps,
Linkwitz–Riley 12/24/48 dB/octave, and second-order Bessel. These are concrete
comparison dimensions for selectable crossover filtering, not a claim that
SOTF must reproduce a vendor's implementation or entire hardware product.

Current SOTF source has only `CrossoverKind::{Lr24, LinearPhase}`.
`CrossoverKind::parse` accepts LR24/LR4 and FIR/LinearPhase aliases and rejects
other families/orders. `params::CROSSOVER_TYPES` has the two entries LR24 and
LinearPhase; its visible layout repeats those choices. The DSP owns concrete
`PreciseLr4` banks, which wrap f64 LR24 coefficients/state behind the f32 host
boundary. Per-channel operation likewise uses LR24 specifically.

The earlier audit recorded missing IIR slope choices. Current source confirms
that this remains unimplemented; FIR tap selection does not supply these IIR
responses. AUD141's discovered multiway recombination mismatch takes priority
over extending this topology.

Before implementation, define each family's cutoff normalization, order,
polarity, phase and multiway sum semantics. For example, LR12 needs a polarity
convention to obtain an all-pass sum; a Butterworth pair must not inherit an
unproven unity-sum claim. See the primary
[Linkwitz filter derivation](https://www.linkwitzlab.com/crossovers.htm).

Preserve old parameter identities and serialized choices: LR24 is currently
choice index 0 and LinearPhase index 1. Do not insert new choices ahead of the
existing FIR entry. New families must reach configuration/serde, shared
descriptors, engine and bridge/FFI/native routes and actual visible controls.
Cover two-way, multiway and per-channel use explicitly. Frequency automation,
sample-rate limits, preparation/reset, f64 state precision, callback allocation
and recursive-tail reporting need deliberate contracts.

Acceptance requires independent pole/prototype-based complex responses and
time-domain references across rates, cutoff boundaries and every supported
order. Preserve old LR24/FIR vectors and accepted FIR EOF evidence. Test real
split/recombine routing, presets and mounted selection; a new enum entry alone
does not implement the feature.

## Existing DSP controls missing from typed application settings

The current DSP `CrossoverPluginParams` also supports `extra_frequencies`
for three/four-way operation and `channel_frequencies_hz`/`channel_modes`
for per-channel operation. Engine `PluginSettings::Crossover`
(`plugin_settings.rs:1218–1231`) contains only type, scalar frequency, output
and FIR taps, and `convert_crossover` (`plugin_config_converter/spatial.rs:391`)
has the same limited fields. The typed application route therefore cannot
carry the existing DSP multiway/per-channel configuration. Include these
configuration/control/width routes in the queued crossover feature work;
AUD141's bounded DSP correction and explicit public host chain do not by
themselves add mounted multiway controls or persistence. No UI execution is
claimed by this source finding.

Current source hashes (under `crates/sotf-plugins/crates/sotf-plugin-crossover/`):

- `src/crossover_kind.rs`: `ecc4e33b59324c7ffe37bc0af4aa763941ca80fc805ad712cbdadabd98ffcda1`.
- `src/params.rs`: `68b6890c9bcc57fb558313146b7c37a5c91a28b9bca81d4973adf276d798bbd9`.

No implementation or executed unsupported-family regression is claimed here.
The full audit also retains custom crossover controls and other unverified
quality/route requirements; this comparison is not whole-product parity.

## Executed public FFI state-route baseline, 2026-09-30

Root's independent `tools/aud142_crossover_state_probe.py` calls exported C
functions through ctypes against the immutable library preceding Crossover's
new restore path (the sealed AUD144 r3 library, SHA-256
`7311b8fa7044084096bbdb219f56656673d7feef86bd8c58943a4147c36e71a8`).
The probe compares successful imports with freshly constructed configured
instances and rejected imports with populated untouched twins; complete
finite f32 output vectors, saved states and input documents are retained.
These are state/routing references using production DSP, not an independent
filter-mathematics proof.

The actual command exits 1: 19 of 30 cases fail. Seventeen valid filter-family,
FIR-alias and same-topology multiway/per-channel imports fail. Two rejected
topology cases instead return success: adding `frequency_2` or per-channel IDs
to an existing two-way handle leaves saved settings unchanged but reconstructs
the DSP and changes its full continuation. The populated-versus-cold control
has a maximum residual of approximately 0.28668, establishing sensitivity to
lost recursive history. Eleven other cases pass, including the unchanged LR24
control and the remaining malformed-state refusals.

The frozen 177-entry packet is `artifacts/aud142-public-state-probe-pre-route-r1/`;
checksum index SHA-256 is
`bb2375ea2a61302eb0bfbdf077f88d3c2af0640d98e44e4384cbac1bbbb23c22`.
Its receipt identifies the command, actual exit status, library/probe hashes
and provenance limits. This baseline does not judge concurrently edited Rust.
Luna's focused new FFI state suite now reports 6/6; the exact unchanged external
probe must run against the next immutable corrected library before that route
checkpoint is ready for Astra review.


### Corrected public FFI state and preset checkpoint, 2026-09-30

Root independently reran the unchanged 30-case public C ABI state probe against
the sealed corrected r2 library, SHA-256
`8424bfe9375ca5d621754d330ef1b9acc662392f0d0897fce7bda43bdefcaa73`: all
30 pass. A separate eight-case probe first caught full two-way preset import
into a four-way target returning success and resetting populated history, even
though output widths matched. The r2 preset-only topology guard fixes that
defect: all eight unchanged cases pass, including partial raw merge, matching
full import, topology refusals preserving complete continuation, and FIR/IIR
family transitions. The corresponding packets are
`artifacts/aud142-public-state-probe-corrected-r2/` and
`artifacts/aud142-full-preset-probe-corrected-r2/`.

Full FFI passes 91 tests with one manual utility ignored. A subsequent fixture
positive assertion passes in the seven-case focused gate; strict all-target
Clippy passes. The r2 build manifest contains 342 selected source files whose
start/end hashes match, rather than a complete workspace closure. Generated
header writes were skipped. Complete output comparisons use fresh production
DSP to validate routing; independent filter mathematics is covered separately
by the pure-core numerical packet. Astra medium accepted this bounded FFI
checkpoint in `reviews/AUD142-ffi-astra.md`. Actual
engine, native wrapper and mounted UI delivery remain open.
