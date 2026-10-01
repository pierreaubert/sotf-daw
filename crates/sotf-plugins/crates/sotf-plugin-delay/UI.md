# Delay — UI Specification

## Layout Mode
custom (simple)

## Menu Bar

| Position | Element | Behavior |
|----------|---------|----------|
| Left | "Delay" label | Plugin name |
| Right | Preset picker | Standard preset dropdown |
| Right | T S X | Toggle bypass / Solo / Close |

## Config (Left Column)

Not used — no setup parameters.

## Main (Center, always visible)

Delay and feedback sliders with the dry/wet mix knob.

| Parameter | engine_key | Control | Shortcut | Notes |
|-----------|-----------|---------|----------|-------|
| Delay Time | delay_ms | Vertical slider with ticks | d | 0 to the instance maximum; 5000 ms for the standard effect |
| Feedback | feedback | Vertical slider with ticks | f | -95 to 95% |
| Mix | mix | Knob | m | 0 to 100%; dry/wet blend |

Slider height: 180px

## Output (Right Column)

Not used — the mix knob lives in Main.

## Diagnostic

Not used.

## Tabs

`Modulation & diffusion` tab with LFO rate (`lfo_rate_hz`, 0-20 Hz) and depth
(`lfo_depth_ms`, 0-10 ms) knobs, allpass coefficient (`allpass_coeff`, 0-0.99)
knob with live smoothing, allpass feedback (`allpass_feedback`) toggle
crossfading over 20 ms, and the structural `pitch_preserving` toggle.

## Responsive Behavior
- **Compact:** Sliders shortened to 120px, labels abbreviated
- **Wide:** Sliders at full 180px height with tick marks and value readout

## ParamCategory Mapping

| Parameter | engine_key | Category | Group |
|-----------|-----------|----------|-------|
| Delay Time | delay_ms | Primary | General |
| Feedback | feedback | Primary | General |
| Mix | mix | Primary | General |
| LFO Rate | lfo_rate_hz | Secondary | Modulation |
| LFO Depth | lfo_depth_ms | Secondary | Modulation |
| Allpass Coeff | allpass_coeff | Primary | General |
| Allpass Feedback | allpass_feedback | Primary | General |
| Pitch Preserving | pitch_preserving | Secondary | Modulation |
