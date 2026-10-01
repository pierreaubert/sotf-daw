# 0.6.0 (unreleased)

## Changes

- Correct CLAP surround capability advertisement for Ambisonics and Crossover; keep the reserved legacy BandSplit VST3 output bus silent when active.
- Reuse prepared Convolution resources when reactivating unchanged geometry and configuration, allowing reset after the original IR file has been removed.
- Persist Convolution IR resources in CLAP/VST3 state and validate inactive restoration transactionally; reject active resource restoration before filesystem access and retain committed state after failed preparation.
- Cover native Convolution state callbacks with independent full-waveform true-stereo, latency, tail, rollback and retry checks.
- Add Convolution editor background IR preparation, stale-candidate rejection and retry handling; validate generated-plugin restart and geometry changes.
- Support embedded CLAP editor show/hide and verify IR selection, host restart,
  refusal/retry and true-stereo audio through the actual Linux editor. Preserve
  the latest automated mix value while an editor resource change is pending.
  Verify the embedded VST3 editor through exported wrapper callbacks on Linux,
  including refused reload, retry, process automation and restored IR audio.
  Packaged plugin and other platform validation remain in progress.
- Expose fixed native schemas for higher-order Ambisonics, Crossover topology, DynamicEQ shelves and BandSplit phase compensation.
- Support 11 named Crossover layouts from mono through 9.1.6, with fixed VST3
  output buses and packed CLAP outputs; correct VST3 mono and quad speaker masks
  and verify loaded routing against independent waveform references.
- Add deferred DynamicEQ structural restart and retry handling for CLAP and VST3, including Linux host-runloop dispatch and saved-state restoration.
- Correct optional auxiliary output buffer handling and preserve the active graph on refused preparation.
- Keep scalar control synchronization allocation-free and use plugin-aware synchronization for dormant Crossover controls.

# 0.5.4 (unreleased)

## Fixes

- Fixed VST3/CLAP wrapper compilation: convert `ParameterId` to `String` when
  building `BridgedParamInfo`.
- Documented plugin instance ownership and render-thread allocation guarantees
  in README.

# 0.5.3

## New

- Added missing plugins in docs and AU/Clap/VST3 bridges
- Added missing new-ish plugin to AU plugins repo
- Added an AAE plugin (experimental)

## Changes

- Long overdue split of denoiser into denoiser+declick+hiss-reducer+speach-denoiser
- Listening + bug hunting session on plugins (with missing test files)
- Listening + bug hunting session on plugins
- SOTA plugin improvements: shared DSP components + plugin upgrades
- Next iteration on UI and testing for plugins this time with native look&feel
- AU plugins are working and I can load them but without a proper UI
