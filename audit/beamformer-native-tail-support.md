# Beamformer native finite-tail metadata — AUD-051 / AUD-073 follow-up

2026-09-28. Frozen and green after the AUD-097/098 numerical checkpoint and twelfth aggregate. This follow-up changes metadata only; no DSP, parameter, geometry, stream scheduling, host, engine or dependency changes. Parent owns AUDIT.md.

## Implementation

`Plugin::tail_length()` reads the prepared active algorithm:

- MVDR / Superdirective: `Finite(1024)` output-rate frames.
- GSC: `Finite(ceil(max prepared steering delay)+31)`.
- Zero sample rate: `Unknown`.

Constructors already prepare every algorithm, so a valid constructed instance reports the bound without an extra initialized flag. It stays constant during process, partial/complete drain and reset. Reinitialization recalculates GSC delay lengths at the new rate; the getter reads the resulting prepared support. All live controls remain structural and rejected. No mutable metadata cache, allocation, lock or worker read is introduced.

For GSC, the finite delay interpolation and 32-tap reference history are the only audio memory. Retained weights remain finite; zero history therefore gives exact zero independently of continuing adaptation.

For spectral processing, with N=512 and last analysis origin `a=256*floor((S-1)/256)`, final synthesis ends at `a+2*N`. The actual remaining response is `2*N-r`, r=1..256. The static 1024 bound includes physical delay once and covers every phase. Continuing covariance updates alter only finite coefficients/fallback. Fresh zero spectra generate zero synthesis; reading and clearing each OLA cell prevents earlier NaN residue from circulating after its final synthesis endpoint.

The metadata makes no claim that finite extreme input always produces finite audio or preserves adaptive quality. Those separate AUD-101 defects are documented below and in README.

## Files for this follow-up

- `src/lib/beamformer_plugin.rs`: TailLength import and one getter.
- `src/lib/tests.rs`: private test registration.
- New `src/lib/tests/tail_metadata.rs`: prove ordinary continuation actually changes learned state.
- New `tests/tail_metadata.rs`: geometry/lifecycle bounds, all-phase ordinary-zero support, selected overflow support.
- `tests/stream_boundaries.rs`: query metadata within the existing cold drain/reset allocation harness.
- README / CHANGELOG: metadata contract and separate numerical limitation.
- `audit/proposals/beamformer-native-tail.md`: copied approved proposal.

All source paths above are under `crates/sotf-plugins/crates/sotf-plugin-beamformer/` unless prefixed with `audit/`. Existing numerical/startup/drain worktree changes are preserved.

## Verification

Four new tests:

1. Native bound follows independent plane-wave geometry for two/eight microphones, broadside/negative steering, construction, 44.1/192 kHz reinitialization, process, rejected live structural write, partial/complete drain and reset. Zero-rate instance reports Unknown and becomes finite after clocked initialization.
2. Every one of 256 terminal hop phases for all three algorithms continues ordinary zero processing with adaptation enabled; all output beyond the advertised bound is exactly zero. Repeated irregular callbacks are17/257/1/73 frames.
3. Sixteen spectral extreme-input cases verify that actual reused FFT/OLA state becomes exactly zero beyond 1024 frames. The test does not require transient NaNs to persist and will remain valid when AUD-101 is fixed.
4. Private learned-state snapshots show ordinary zero continuation changes MVDR covariance and GSC coefficients while retained audio is being consumed. GSC coefficients stop changing after its finite reference history is zero. This is independent of frozen `drain()`.

Existing cold tests cover all algorithms, first/reset drain, small/large source lengths, partial capacities and repeated metadata queries: **zero allocations and zero deallocations**.

Executed gates:

- Red getter regression: Unknown vs expected Finite(1024), `target/audit-beamformer-tail-metadata-red.log`.
- Full all-feature suite: **80 passed**, zero failed/ignored (50 unit + 3 GSC reference + 15 integration + 2 numerical recovery + 7 boundaries + 3 tail-metadata tests). `target/audit-beamformer-tail-metadata-full.log`.
- Strict all-target/all-feature Clippy: clean. `target/audit-beamformer-tail-metadata-clippy.log`.
- Five changed Rust files pass rustfmt check; scoped diff check is clean.

No additional release QA was needed: the preceding numerical checkpoint passed release QA, and this follow-up adds only the bounded read-only getter. No outstanding builds or source edits.

Root independently reviewed the scalar getter, prepared support proof,
independent geometry calculation, all-hop ordinary continuation and overflow
residue tests. No blocker was found in this metadata scope. Covariance recovery
remains explicitly outside the declaration's claim.

## Independent exploratory evidence

Before implementation, `/tmp/sotf-beamformer-tail-probe/ordinary_support.rs` linked existing post-fix workspace rlibs and ran 192 cases: each algorithm 64 cases (two/eight microphones, 48/192 kHz, 0/37 degree steering, four phases, coherent/opposed f32::MAX bursts after ordinary training). All were exactly zero beyond the proposed bound plus a further 8192 frames and after reset. No drain calls supplied the reference.

- MVDR / Superdirective transient nonfinite outputs summed across cases: 76638 / 76882. Latest nonzero/nonfinite tail index:1006, inside bound1024.
- GSC nonfinite outputs:0. Latest tail index1210, inside prepared bound1211.

Probe and log: `/tmp/sotf-beamformer-tail-probe/ordinary_support.rs`, `ordinary_support.log`. Binary: `target/audit-tmp/beamformer-ordinary-support`.

## AUD-101 remains open: covariance poisoning loses adaptation

Source and public probes establish a further consequence of finite-input covariance overflow. Opposed finite spectra around1e20 cancel in the look-direction projection, pass the noise gate, and overflow the covariance outer products. Multiplication by alpha does not clear Inf/NaN covariance. The validated finite fallback remains active indefinitely.

Direct public MVDR core: after one opposed finite frame and **4096** later small-noise updates, poisoned weights remain `[0.5,0.5]`; healthy weights become `[0.004950495,0.9950495]`.

Full public plugin: opposed finite broadband1e20 burst, then8192 silent frames, then65536 frames of one-sided1.5kHz interference:

- No nonfinite audio during the poisoning burst in this fixture.
- Exact zero after the finite support during silence.
- Later poisoned output RMS: **0.17677665863504555**.
- Healthy output RMS: **0.0017502646786324754**.
- Adaptive rejection lost: **40.08642384425446 dB**.
- Poisoned output matches an independent delayed half-left signal within **1.1920928955078125e-7**.
- Reset restores RMS **0.0017502761430928623**.

Probe/log: `/tmp/sotf-beamformer-tail-probe/covariance_recovery.rs`, `covariance_recovery.log`. No production DSP correction was made under this metadata approval. The exact-zero support contract remains valid despite the loss of later adaptation; do not describe this as a numerical recovery fix.
