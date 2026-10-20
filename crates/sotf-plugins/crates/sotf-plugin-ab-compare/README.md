# sotf-plugin-ab-compare

SOTF AB Compare plugin for A/B comparison.

Enables fair comparison between two audio processing chains with automatic loudness matching.
Supports a single plugin, full rack, or arbitrary graph per path. Path/topology and band-mask
changes are applied through an outer-host structural rebuild so realtime processing remains bounded.

The built-in fallback factory selects one band for `compressor`, matching the
canonical and bridge factories even if the saved parameters contain a different
`num_bands`. Select `multiband_compressor` to honor multiband topology. Earlier
fallback paths labeled `compressor` used multiband defaults; change their type
to `multiband_compressor` to preserve that processing. This correction can change
the audio of those saved fallback paths. An injected external factory retains
full control of construction, including its own identity and parameter policy.

Loudness matching updates on a fixed 50 ms sample clock (`floor(sample_rate / 20)`
frames per interval), independently of callback size. Each update uses only audio
already rendered and changes the gain target for subsequent samples. Large callbacks
retain every update; no future measurement is applied to the beginning of a callback.

The loudness meter needs its first 100 ms energy block before a valid correction is
available. It initially averages the available history, growing to the full 400 ms
momentary or 3 s short-term window. Gain smoothing adds settling time after each target
change. This is adaptation time, with no additional audio delay: reported latency
remains the maximum of the two processing paths. Reset clears the measurement history
and restarts this clock. Parameter indices, defaults, and correction limits are unchanged.
