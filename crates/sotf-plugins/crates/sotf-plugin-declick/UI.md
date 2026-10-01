# Declick UI

The Repair group exposes the legacy parameters plus mode controls:

| Control | Type | Range/default | Behavior |
| --- | --- | --- | --- |
| Enabled | Toggle | On | 5 ms crossfade between delayed dry and repaired audio; detector stays warm. |
| Sensitivity | Knob | 1–100 / 10 | Lower values repair more candidates; automation is smoothed over 5 ms. |
| Link Channels | Toggle | On | Share detection decisions in adjacent channel pairs while interpolating each channel separately. |
| Mode | Selector | Random / Periodic | Periodic repetition tracking with phase prediction; structural, clears detector history. |
| Repair Width | Knob | 0–8 samples / 0 | Symmetric repair extension; structural, adds equal latency on new-mode paths. |

The Multiband group exposes band detection controls:

| Control | Type | Range/default | Behavior |
| --- | --- | --- | --- |
| Bands | Selector | Fullband / 2-band / 3-band | Complementary splits with a fullband supervisor gate; structural. |
| Crossover | Knob | 80–12000 Hz / 4000 Hz | 2-band edge, or 3-band geometric center; structural; neutral in Fullband. |
| Frequency Skew | Knob | −1–1 / 0 | Bias detection toward high (+) or low (−) bands; smoothed via per-band targets; neutral in Fullband. |

The Monitor group exposes residual audition:

| Control | Type | Range/default | Behavior |
| --- | --- | --- | --- |
| Audition Residual | Toggle | Off | 5 ms crossfade to the aligned residual (removed clicks) instead of repaired audio. |

The host should display the reported latency (eight samples by default, plus
repair width on new-mode paths) in both enabled and disabled states. There
is no dynamic visualization or analyzer output.
