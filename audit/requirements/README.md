# Plugin requirements for manual parallel execution

Snapshot: 2026-10-01. These assignments preserve the full feature-parity, whole-chain and independent-accuracy objective. MIDI and IAMF are excluded.

**51 requirement files** cover all **45 plugin-prefixed workspace crates**, broadband Compressor/Expander, host analyzers, standalone correlation and external hosting. All **50 canonical factory catalog entries** are mapped; FletcherMunson is covered as the Loudness Compensation compatibility route. Analog Common is included as shared support.

## Start here

1. Read [COMMON.md](COMMON.md) and [CHECKPOINT.md](CHECKPOINT.md), then choose one plugin below.
2. Claim an owner/worktree in the table. Shared-source conflicts are listed in [SHARED.md](SHARED.md); separate git worktrees do not remove semantic merge conflicts.
3. Use the task prompt below. Implement known gaps first; audit-only assignments must establish current requirements and correct real gaps, not invent optional features or claim parity from smoke tests.
4. Review each result in a separate Muse session, return fixes to its Muse implementation owner, then integrate agreed shared changes and the combined gate.

**Current continuation:** the sibling GPUI EQ receipt route now compiles and passes all 38 mounted scenarios; player controls and receipt merging pass 11 tests each. Native EQ passes 147 active tests, and FFI passes 133 active tests. Independent Muse reviews accept the baseline repairs, including the actual facade factory test. #1 implementation is active for measured Hiss profiles and converter-inclusive EQ response/chart integration; FIR numerical investigation remains open. These focused gates do not close full loaded-plugin, feature-accuracy or platform acceptance. See CHECKPOINT.md and the current continuation evidence before running broad checks.

## Suggested parallel groups

- Independent core lanes: Gain, Delay, Matrix, Dither, Transient Shaper, Stereo Imager, Mono to Stereo; these mainly need audit/accuracy completion rather than known large feature additions.
- Feature implementation lanes: De-esser, Declick, Denoiser, Hiss Reducer, Speech Denoiser, Ambisonics custom layouts, Dynamic EQ and Linear-phase EQ. Coordinate shared dependencies listed below.
- Keep Compressor with Multiband Compressor, and Expander with Multiband Expander: each pair is one implementation.
- One shared analog owner; one owner for common native/FFI/engine/UI integration files. Spatial SOFA/geometry and restoration shared libraries also require coordination.
- EQ continuation is a dedicated integration lane; avoid concurrent generic native-wrapper or rack receipt edits.
- HAL native validation needs a macOS lane. Corpus/listening tasks can run independently once data access is established.

## Copyable task prompt

```text
Implement audit/requirements/<plugin>.md and its COMMON.md contract.
Use current source as authority; preserve accepted fixes and old audio/state fixtures.
Claim your files in the requirements index and coordinate SHARED.md dependencies.
Use Muse muse-spark-1.3-contributor at max effort for implementation and a separate Muse session for independent review.
Fix review findings until the feature, accuracy, lifecycle and actual chain gates pass.
Do not edit MIDI/IAMF, bump versions again, weaken tolerances, or infer success from skipped fixtures.
Report exact commands, raw results, quantitative errors, shared patches and unresolved requirements.
```

## Assignments

| Requirement | Package / implementation | Coordination group | Owner / worktree |
|---|---|---|---|
| [aae](aae.md) | `sotf-plugin-aae` | Spatial/shared AutoGain | Unassigned |
| [ab-compare](ab-compare.md) | `sotf-plugin-ab-compare` | Clock/host integration | Muse: `abcompare-variable-rate-r1` (R1/R2 variable-rate; same-rate preserved) |
| [aec](aec.md) | `sotf-plugin-aec` | Spatial/adaptive clock | Unassigned |
| [ambisonics](ambisonics.md) | `sotf-plugin-ambisonics` | Spatial/shared geometry | Muse: `ambisonics` |
| [analog-common](analog-common.md) | `sotf-plugin-analog-common` | Shared analog family | Unassigned |
| [analog-compressor](analog-compressor.md) | `sotf-plugin-analog-compressor` | Shared analog family | Unassigned |
| [analog-eq](analog-eq.md) | `sotf-plugin-analog-eq` | Shared analog family | Unassigned |
| [analog-limiter](analog-limiter.md) | `sotf-plugin-analog-limiter` | Shared analog family | Muse max: analog-limiter-contract-r1 (owned crate + requirements; shared via proposals) |
| [band-merge](band-merge.md) | `sotf-plugin-band-merge` | Split/merge/shared native layouts | Unassigned |
| [band-split](band-split.md) | `sotf-plugin-band-split` | Split/merge/shared native layouts | Unassigned |
| [beamformer](beamformer.md) | `sotf-plugin-beamformer` | Spatial/adaptive | Unassigned |
| [binaural](binaural.md) | `sotf-plugin-binaural` | Spatial/shared SOFA | Unassigned |
| [channel-correlation](channel-correlation.md) | `sotf-host` | Shared host analyzers | Unassigned |
| [channel-mute-solo](channel-mute-solo.md) | `sotf-plugin-channel-mute-solo` | Independent utility | Unassigned |
| [compressor](compressor.md) | `sotf-plugin-multiband-compressor` | Shared compressor implementation | Muse compressor family |
| [convolution](convolution.md) | `sotf-plugin-convolution` | Convolution/shared native resources | Unassigned |
| [crossfeed](crossfeed.md) | `sotf-plugin-crossfeed` | Stereo utilities/shared AutoGain | Muse max: crossfeed-r1 implementer (owned crate + handoff; whole-chain open) |
| [crossover](crossover.md) | `sotf-plugin-crossover` | Split/merge/shared native layouts | Unassigned |
| [de-esser](de-esser.md) | `sotf-plugin-de-esser` | Dynamics | Muse: `de-esser` |
| [declick](declick.md) | `sotf-plugin-declick` | Restoration | Muse: `declick-consumers-r1` (continuation; DSP/engine baseline preserved) |
| [delay](delay.md) | `sotf-plugin-delay` | Independent utility | Muse: `delay` |
| [denoiser](denoiser.md) | `sotf-plugin-denoiser` | Restoration/shared denoiser | Muse max: `denoiser-feature-continuation-r1` (R1/R2 owned implementation; R3 shared patches proposed, gates pending root) |
| [dither](dither.md) | `sotf-plugin-dither` | Independent utility | Muse: `dither` |
| [downmix](downmix.md) | `sotf-plugin-downmix` | Spatial/shared geometry | Unassigned |
| [dynamic-eq](dynamic-eq.md) | `sotf-plugin-dynamic-eq` | EQ integration | Muse: `dynamic-eq` |
| [eq](eq.md) | `sotf-plugin-eq` | EQ integration | Muse max: oversampled response/chart continuation |
| [expander](expander.md) | `sotf-plugin-multiband-expander` | Shared expander implementation | Unassigned |
| [external-plugin](external-plugin.md) | `sotf-host` | Shared native host/engine | Unassigned |
| [gain](gain.md) | `sotf-plugin-gain` | Independent utility | Muse: `gain` |
| [gate](gate.md) | `sotf-plugin-gate` | Dynamics | Unassigned |
| [hal-input](hal-input.md) | `sotf-plugin-hal-input` | HAL/macOS shared transport | Unassigned |
| [hal-output](hal-output.md) | `sotf-plugin-hal-output` | HAL/macOS shared transport | Unassigned |
| [hiss-reducer](hiss-reducer.md) | `sotf-plugin-hiss-reducer` | Restoration/shared denoiser | Muse max: measured-profile continuation |
| [limiter](limiter.md) | `sotf-plugin-limiter` | Dynamics/shared oversampling | Muse limiter |
| [linear-phase-eq](linear-phase-eq.md) | `sotf-plugin-linear-phase-eq` | EQ integration | Muse: `linear-phase-eq` |
| [loudness-compensation](loudness-compensation.md) | `sotf-plugin-loudness-compensation` | Metering/shared AutoGain | Unassigned |
| [loudness-monitor](loudness-monitor.md) | `sotf-host` | Shared host analyzers | Unassigned |
| [matrix](matrix.md) | `sotf-plugin-matrix` | Routing/shared geometry | Unassigned |
| [mono-to-stereo](mono-to-stereo.md) | `sotf-plugin-mono-to-stereo` | Stereo utilities | Unassigned |
| [multiband-compressor](multiband-compressor.md) | `sotf-plugin-multiband-compressor` | Shared compressor implementation | Muse compressor family |
| [multiband-expander](multiband-expander.md) | `sotf-plugin-multiband-expander` | Shared expander implementation | Unassigned |
| [pnd](pnd.md) | `sotf-plugin-pnd` | Spatial/time-frequency | Unassigned |
| [resampler](resampler.md) | `sotf-plugin-resampler` | Clock/host integration | Unassigned |
| [saturation](saturation.md) | `sotf-plugin-saturation` | Dynamics/shared oversampling | Unassigned |
| [spectral-compressor](spectral-compressor.md) | `sotf-plugin-spectral-compressor` | Spectral dynamics/shared DSP | Unassigned |
| [spectrum-analyzer](spectrum-analyzer.md) | `sotf-host` | Shared host analyzers | Unassigned |
| [speech-denoiser](speech-denoiser.md) | `sotf-plugin-speech-denoiser` | Restoration/shared inference | Muse: `speech-denoiser` |
| [stereo-imager](stereo-imager.md) | `sotf-plugin-stereo-imager` | Stereo utilities | Unassigned |
| [transient-shaper](transient-shaper.md) | `sotf-plugin-transient-shaper` | Dynamics | Unassigned |
| [upmixer](upmixer.md) | `sotf-plugin-upmixer` | Spatial/shared geometry/inference | Unassigned |
| [xtc](xtc.md) | `sotf-plugin-xtc` | Spatial/shared SOFA | Unassigned |

## Catalog aliases and additional scope

- `compressor` and `multiband_compressor` share one crate; `expander` and `multiband_expander` share another.
- `fletcher_munson` → [loudness-compensation.md](loudness-compensation.md); retain old preset migration.
- `binaural_decoder` → [binaural.md](binaural.md); `ambisonics_decoder` → [ambisonics.md](ambisonics.md).
- `external` → [external-plugin.md](external-plugin.md).
- [channel-correlation.md](channel-correlation.md) covers the standalone host module even though it is not a canonical factory entry.
- Non-plugin engine, drivers, adapter/support layers and final workspace integration remain assigned through [SHARED.md](SHARED.md); plugin files do not shrink the overall audit to just DSP crates.

## Status rules

Requirement checkboxes start unchecked. Existing implementation notes credit scoped work; they are not blanket completion. Do not read old historical counts or old "pending" rows as the latest state. Update the plugin file with evidence and a clear completed/remaining summary at handoff.
