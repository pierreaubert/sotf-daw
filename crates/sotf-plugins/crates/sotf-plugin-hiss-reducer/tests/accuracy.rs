//! Independent hiss-reducer accuracy tests (A1/A2).
//!
//! Suppression and wanted-signal loss are measured separately with
//! independent f64 oracles. Bounds are fixed here: SNR improvement above
//! 2 dB (retained), tone loss below 1 dB (retained), hiss-only suppression
//! above 2 dB spectrally, time-domain transient peaks within 1 dB,
//! spectral unity reconstruction within 2e-5 (retained), and time-domain
//! zero-strength bit-exactness.

use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};

const SR: u32 = 48_000;
const LATENCY: usize = 1024;

fn ctx(frames: usize) -> ProcessContext<'static> {
    ProcessContext::new(SR, frames)
}

fn ctx_at(rate: u32, frames: usize) -> ProcessContext<'static> {
    ProcessContext::new(rate, frames)
}

fn lcg(state: &mut u32) -> f32 {
    *state = state
        .wrapping_mul(1_664_525)
        .wrapping_add(1_013_904_223);
    (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

fn spectral_plugin(strength: f32) -> HissReducerPlugin {
    let mut plugin = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: true,
            strength,
            ..HissReducerPluginParams::default()
        },
    );
    plugin.initialize(SR).unwrap();
    plugin
}

fn time_domain_plugin(strength: f32) -> HissReducerPlugin {
    let mut plugin = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            strength,
            ..HissReducerPluginParams::default()
        },
    );
    plugin.initialize(SR).unwrap();
    plugin
}

fn render_partitioned(
    plugin: &mut HissReducerPlugin,
    input: &[f32],
    partitions: &[usize],
) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    let mut part = 0;
    while offset < input.len() {
        let count = partitions[part % partitions.len()].min(input.len() - offset);
        let mut block = input[offset..offset + count].to_vec();
        plugin
            .process_in_place(&mut block, &ctx(count))
            .unwrap();
        output.extend(block);
        offset += count;
        part += 1;
    }
    output
}

/// Independent f64 one-pole band-power oracle (high selects the residual).
fn oracle_band_power(input: &[f32], cutoff_hz: f64, sample_rate: f64, high: bool) -> f64 {
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * cutoff_hz / sample_rate).exp();
    let mut low = 0.0;
    let mut sum = 0.0;
    for &sample in input {
        let dry = sample as f64;
        low = alpha * dry + (1.0 - alpha) * low;
        let component = if high { dry - low } else { low };
        sum += component * component;
    }
    sum / input.len() as f64
}

fn power_db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

#[test]
fn spectral_hiss_only_suppression_and_tone_only_loss_are_separate() {
    let frames = SR as usize * 2;
    let mut state = 0x51ab_0001u32;
    let mut previous = 0.0f32;
    let mut hiss = Vec::with_capacity(frames);
    for _ in 0..frames {
        let white = lcg(&mut state);
        let high_pass = 0.035 * (white - previous);
        previous = white;
        hiss.push(high_pass);
    }
    let tone: Vec<f32> = (0..frames)
        .map(|i| 0.12 * (2.0 * std::f32::consts::PI * 750.0 * i as f32 / SR as f32).sin())
        .collect();

    // Hiss-only: the reducer must suppress stationary high-band noise.
    let mut suppressor = spectral_plugin(0.85);
    let suppressed = render_partitioned(&mut suppressor, &hiss, &[1, 64, 511, 73, 997]);
    let start = SR as usize + LATENCY;
    let input_power: f64 = hiss[start - LATENCY..]
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum();
    let output_power: f64 = suppressed[start..]
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum();
    let suppression = power_db(output_power / input_power);
    assert!(
        suppression < -2.0,
        "hiss-only suppression too weak: {suppression:.2} dB"
    );

    // Tone-only: a below-cutoff wanted tone must pass with < 1 dB loss.
    let mut preserver = spectral_plugin(0.85);
    let preserved = render_partitioned(&mut preserver, &tone, &[1, 64, 511, 73, 997]);
    let tone_in: f64 = tone[start - LATENCY..]
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum();
    let tone_out: f64 = preserved[start..]
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum();
    let loss = power_db(tone_out / tone_in);
    assert!(
        loss.abs() < 1.0,
        "wanted tone changed by {loss:.2} dB"
    );
}

#[test]
fn spectral_mixed_snr_improvement_is_retained() {
    // Same construction as the existing SNR regression: the > 2 dB bound
    // must keep passing alongside the new separated cases.
    let frames = SR as usize * 2;
    let mut state = 0x1234_5678u32;
    let mut clean = Vec::with_capacity(frames);
    let mut noisy = Vec::with_capacity(frames);
    let mut previous = 0.0f32;
    for i in 0..frames {
        let white = lcg(&mut state);
        let high_pass = 0.035 * (white - previous);
        previous = white;
        let tone = 0.12 * (2.0 * std::f32::consts::PI * 750.0 * i as f32 / SR as f32).sin();
        clean.push(tone);
        noisy.push(tone + high_pass);
    }
    let mut plugin = spectral_plugin(0.85);
    let output = render_partitioned(&mut plugin, &noisy, &[1, 64, 511, 73, 997]);
    let start = SR as usize / 2 + LATENCY;
    let mut input_error = 0.0f64;
    let mut output_error = 0.0f64;
    let mut clean_power = 0.0f64;
    for i in start..frames {
        let reference = f64::from(clean[i - LATENCY]);
        input_error += (f64::from(noisy[i - LATENCY]) - reference).powi(2);
        output_error += (f64::from(output[i]) - reference).powi(2);
        clean_power += reference.powi(2);
    }
    let input_snr = power_db(clean_power / input_error);
    let output_snr = power_db(clean_power / output_error);
    assert!(
        output_snr > input_snr + 2.0,
        "SNR did not improve enough: input={input_snr:.2} dB output={output_snr:.2} dB"
    );
}

#[test]
fn time_domain_suppression_tone_and_transient_separation() {
    // Quiet stationary hiss (well under the -30 dBFS threshold) plus a
    // low wanted tone whose one-pole high-band leakage stays under the
    // threshold too, so the detector engages on the hiss alone.
    let frames = SR as usize * 3;
    let mut state = 0x7e5f_0001u32;
    let mut mixed = Vec::with_capacity(frames);
    for i in 0..frames {
        let hiss = 0.02 * lcg(&mut state);
        let tone = 0.12 * (2.0 * std::f32::consts::PI * 300.0 * i as f32 / SR as f32).sin();
        mixed.push(tone + hiss);
    }
    let mut plugin = time_domain_plugin(0.8);
    let output = render_partitioned(&mut plugin, &mixed, &[4096]);

    let steady = &mixed[SR as usize * 2..];
    let steady_out = &output[SR as usize * 2..];
    let hiss_change = power_db(
        oracle_band_power(steady_out, 4000.0, f64::from(SR), true)
            / oracle_band_power(steady, 4000.0, f64::from(SR), true),
    );
    assert!(
        hiss_change < -3.0,
        "time-domain hiss suppression too weak: {hiss_change:.2} dB"
    );
    let tone_change = power_db(
        oracle_band_power(steady_out, 4000.0, f64::from(SR), false)
            / oracle_band_power(steady, 4000.0, f64::from(SR), false),
    );
    assert!(
        tone_change.abs() < 0.5,
        "wanted tone changed by {tone_change:.2} dB"
    );

    // Sparse impulses on silence never engage the steady detector (the
    // 2400-sample spacing beats persistence plus envelope recovery), so
    // peaks must survive within 1 dB in the zero-latency path.
    let mut impulses = vec![0.0; SR as usize];
    for sample in impulses.iter_mut().step_by(2400) {
        *sample = 0.08;
    }
    let mut transient_plugin = time_domain_plugin(0.8);
    let transient_out = render_partitioned(&mut transient_plugin, &impulses, &[997, 63]);
    for base in (0..SR as usize).step_by(2400) {
        let window = &transient_out[base.saturating_sub(4)..(base + 5).min(transient_out.len())];
        let peak = window.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let loss = 20.0 * (peak / 0.08).log10();
        let lost = -loss;
        assert!(
            loss > -1.0,
            "impulse at {base} lost {lost:.2} dB (peak {peak})"
        );
    }

    // Impulses over a loud tone (high-band energy above the threshold keeps
    // the detector off) are preserved as well.
    let mut loud = Vec::with_capacity(SR as usize);
    for i in 0..SR as usize {
        let tone = 0.5 * (2.0 * std::f32::consts::PI * 750.0 * i as f32 / SR as f32).sin();
        loud.push(if i % 2400 == 0 { tone + 0.08 } else { tone });
    }
    let mut loud_plugin = time_domain_plugin(0.8);
    let loud_out = render_partitioned(&mut loud_plugin, &loud, &[997, 63]);
    for base in (0..SR as usize).step_by(2400).skip(4) {
        let expected = loud[base].abs();
        let window = &loud_out[base - 4..base + 5];
        let peak = window.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let loss = 20.0 * (peak / expected).log10();
        let lost = -loss;
        assert!(
            loss > -1.0,
            "loud impulse at {base} lost {lost:.2} dB (peak {peak} vs {expected})"
        );
    }
}

#[test]
fn zero_strength_is_exact_and_spectral_unity_reconstructs() {
    // Time-domain strength 0 is bit-exact across rates, layouts, and blocks.
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            let mut plugin = HissReducerPlugin::from_params(
                channels,
                HissReducerPluginParams {
                    strength: 0.0,
                    ..HissReducerPluginParams::default()
                },
            );
            plugin.initialize(rate).unwrap();
            let frames = 8192;
            let input: Vec<f32> = (0..frames * channels)
                .map(|i| ((i * 7919 % 1021) as f32 / 510.5 - 1.0) * 0.2)
                .collect();
            for blocks in [&[frames][..], &[1, 63, 997, 5]] {
                let mut output = input.clone();
                let mut pos = 0;
                let mut call = 0;
                while pos < frames {
                    let count = blocks[call % blocks.len()].min(frames - pos);
                    plugin
                        .process_in_place(
                            &mut output[pos * channels..(pos + count) * channels],
                            &ctx_at(rate, count),
                        )
                        .unwrap();
                    pos += count;
                    call += 1;
                }
                assert_eq!(
                    output, input,
                    "rate={rate} channels={channels} blocks={blocks:?}"
                );
            }
        }
    }

    // Spectral strength 0 reconstructs broadband input within 2e-5.
    let input: Vec<f32> = (0..8192)
        .map(|i| ((i * 3571 % 2053) as f32 / 1026.5 - 1.0) * 0.2)
        .collect();
    let mut plugin = spectral_plugin(0.0);
    let output = render_partitioned(&mut plugin, &input, &[1, 64, 511, 997]);
    let mut worst = 0.0f32;
    for i in LATENCY..input.len() {
        worst = worst.max((output[i] - input[i - LATENCY]).abs());
    }
    assert!(worst < 2.0e-5, "spectral unity error {worst}");
}

#[test]
fn strength_law_is_monotonic_and_rate_consistent() {
    fn attenuation_db(rate: u32, cutoff_hz: f32, strength: f32) -> f64 {
        let mut plugin = HissReducerPlugin::from_params(
            1,
            HissReducerPluginParams {
                frequency_hz: cutoff_hz,
                strength,
                ..HissReducerPluginParams::default()
            },
        );
        plugin.initialize(rate).unwrap();
        let frames = rate as usize * 3;
        let mut state = 0x9a1e_0000u32.wrapping_add(rate);
        let input: Vec<f32> = (0..frames).map(|_| 0.02 * lcg(&mut state)).collect();
        let mut output = input.clone();
        let mut pos = 0;
        while pos < frames {
            let count = 4096.min(frames - pos);
            plugin
                .process_in_place(
                    &mut output[pos..pos + count],
                    &ProcessContext::new(rate, count),
                )
                .unwrap();
            pos += count;
        }
        let skip = rate as usize * 2;
        power_db(
            oracle_band_power(
                &output[skip..],
                f64::from(cutoff_hz),
                f64::from(rate),
                true,
            ) / oracle_band_power(
                &input[skip..],
                f64::from(cutoff_hz),
                f64::from(rate),
                true,
            ),
        )
    }

    // Expansion law: higher strength attenuates stationary hiss more.
    let none = attenuation_db(SR, 4000.0, 0.0);
    let half = attenuation_db(SR, 4000.0, 0.5);
    let full = attenuation_db(SR, 4000.0, 1.0);
    assert!(none.abs() < 1e-6, "strength 0 must be exact, got {none} dB");
    assert!(half < -2.0, "strength 0.5 too weak: {half:.2} dB");
    assert!(
        full < half - 1.0,
        "strength law not separated: half={half:.2} full={full:.2}"
    );

    // Rate-derived envelopes: same normalized cutoff keeps attenuation
    // within 1 dB across rates (192 kHz uses the 16 kHz registry maximum,
    // the same normalized 1/12 rate fraction as 4 kHz at 48 kHz).
    let a44 = attenuation_db(44_100, 3675.0, 0.8);
    let a48 = attenuation_db(48_000, 4000.0, 0.8);
    let a96 = attenuation_db(96_000, 8000.0, 0.8);
    let a192 = attenuation_db(192_000, 16_000.0, 0.8);
    assert!(
        (a44 - a48).abs() < 1.0 && (a96 - a48).abs() < 1.0 && (a192 - a48).abs() < 1.0,
        "rate spread too wide: 44.1k={a44:.2} 48k={a48:.2} 96k={a96:.2} 192k={a192:.2}"
    );
}
