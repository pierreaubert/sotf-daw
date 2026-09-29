# AUD118 — small FFT geometry and default velvet-noise decorrelation

Resolved by the narrow geometry correction; see [executed verification](upmixer-small-fft.md).
The original reproduction below is retained as historical evidence.

Reproduced while extending the AUD115 accepted-frame/reference-ring matrix, before changing decorrelation code. The evidence log is `/tmp/sotf-spatial-autogain-private.log` (the first matrix run, which failed one test).

The actual public construction/initialization route was:

```rust
let params: UpmixerPluginParams = serde_json::from_value(serde_json::json!({
    "fft_size": 64,
    "speaker_config": "5.1",
    "binaural_preview": false,
    "enable_hr_direct": false,
    "auto_gain_enabled": false,
    "auto_gain_max_db": 12.0,
    "auto_gain_smoothing_ms": 100.0,
    "safety_cap_db": -1.0
})).unwrap();
let mut plugin = UpmixerPlugin::from_params(params);
plugin.initialize(44_100).unwrap();
```

The first configuration in the loop panicked before any process call:

```
sotf-plugin-upmixer/src/decorrelation.rs:377:31:
min > max. min = 128, max = 32
```

Source: `generate_velvet_noise_filter_with_seed` computes `seq_len.clamp(128, self.core.fft_size / 2)`. `initialize` calls `generate_decorrelation_filters`; 5.1 has non-front/non-LFE channels needing the seeded velvet filter. The public constructor accepts a power-of-two FFT64, while this decorrelation helper requires an upper bound at least128 (N>=256). No checked configuration error is returned.

A distinct existing neutral FFT64 fixture succeeds with decorrelation bypass and stereo identity routing; FFT64 itself is usable for that route. The AUD115 production patch does not modify FFT or decorrelation geometry. Its ordinary prepared matrix was changed to 256/512/1024/2048/4096/8192, retaining neutral FFT64 coverage separately. Both now pass.

Suggested follow-up: review all minimum/maximum geometry assumptions before choosing either a documented public minimum or a correctly bounded shorter decorrelator. Preserve valid small stereo/bypassed cases. No decorrelation edits or new repro builds were made here; parent owns the separate scope decision.
