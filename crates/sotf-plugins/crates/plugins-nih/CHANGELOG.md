# 0.6.0 (unreleased)

## Changes

- Expose fixed native schemas for higher-order Ambisonics, Crossover topology, DynamicEQ shelves and BandSplit phase compensation.
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
