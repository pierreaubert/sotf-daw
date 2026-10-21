# plugins-nih

VST3/CLAP plugin wrappers for SOTF audio plugins via nih-plug.

## Architecture

```
lib.rs      -- Feature-gated plugin definitions: one mod per plugin, each using sotf_nih_plugin! macro + nih_export_clap!/nih_export_vst3!
              Also defines PluginBridgeWrapper for parameter sync.
params.rs   -- DynamicParams: runtime-built nih-plug Params struct from BridgedParamInfo
wrapper.rs  -- sotf_nih_plugin! macro: generates complete nih-plug Plugin/Vst3Plugin/ClapPlugin impl from SOTF metadata
```

Build model: one `cdylib` per plugin, selected by feature flag. Each build produces a `.dylib` exporting both VST3 and CLAP entry points.

## Key Public API

- `sotf_nih_plugin!` macro -- Generates a full nih-plug plugin struct from: plugin_type, name, clap_id, vst3_class_id, channels
- `DynamicParams` -- Builds nih-plug `Params` at runtime from `BridgedParamInfo` (supports Float, Int, Bool)
- `PluginBridgeWrapper` -- Syncs nih-plug parameters to the underlying SOTF `Plugin` instance

## Building

```bash
# Build a single plugin (one feature = one dylib)
cargo build --release -p plugins-nih --features eq --no-default-features

# Build all 29 plugins (via Justfile)
just build-nih-plugins
```

## Testing

```bash
cargo check -p plugins-nih --features eq
```

## Supported Plugins (29 features)

eq, compressor, limiter, gate, gain, delay, expander, crossfeed, saturation, denoiser, downmix, mono-to-stereo, stereo-imager, transient-shaper, de-esser, dynamic-eq, multiband-compressor, multiband-expander, convolution, fletcher-munson, loudness-compensation, channel-mute-solo, upmixer, xtc, binaural, matrix, pnd, ab-compare, crossover

## Important Notes

- Only one feature should be active per build -- each feature gates a different `mod plugin` block
- The macro handles interleave/deinterleave buffer conversion between nih-plug (planar) and SOTF (interleaved)
- Parameters are built dynamically at runtime from ParamSpec or fallback to Plugin::parameters()
- Uses `plugins-bridge::create_plugin()` to instantiate the underlying SOTF plugin
- The `assert_process_allocs` nih-plug feature is enabled to catch allocations in the audio thread
- All plugins default to 2 channels (stereo)

## Upmixer snapshot lifecycle

- Upmixer state snapshots are inactive/control-thread only. Both `DynamicParams::validate_state` and the macro's `state_restore_allows_audio_thread` gate are required: NIH can otherwise rebuild active FFT state and change latency before host restart.
- Reject all active snapshots before mutation, including identical/scalar/sparse states; ordinary scalar automation stays live. Retry by deactivating, loading and reactivating.
- `native_upmixer_clap_tests` uses the exported CLAP entry/factory and real state/audio callbacks. Generated wrapper-context tests alone do not validate native state loading.
- The macOS `upmixer_vst3_host` example loads the actual dylib and invokes exported factory/COM callbacks on the OS main thread. Keep all COM references inside the loaded library's lifetime. Only C-compatible pointers, planar audio and serialized bytes cross this boundary.
- During VST3 processing, controller-only writes are not audio automation: send `IParameterChanges`/`IParamValueQueue` events through `ProcessData`. The harness compares those events against direct DSP updates.
- Pluginval strictness 5 and the native executable have different scopes. External DAW PDC, CLAP artifact loading and other-platform validation still need independent evidence.
