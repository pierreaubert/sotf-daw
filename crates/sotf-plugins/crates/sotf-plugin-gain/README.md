# sotf-plugin-gain

SOTF Gain plugin for volume control.

Simple SIMD-optimized gain plugin with per-channel support for independent level adjustment.

Gain multiplies samples without clipping or automatic headroom normalization.
Direct, adapter and compiled routes preserve IEEE NaN/Inf arithmetic. The f64
adapter fallback processes at f32 precision. Gain does not define endpoint PCM
saturation; that belongs to the renderer/export stage.

Moving controls advance once per real audio frame, including the first callback.
Zero-frame calls leave ramp state unchanged. Ramps finish at their requested
endpoint when f32 rounding stops further progress, then use the settled SIMD path.
